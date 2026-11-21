//! Image-grounded translation; original OCR and ordinary caches stay intact.
use crate::{
    compat::{array, keeps_original, text, truth},
    persistence,
    service::TranslationService,
    structure,
    translate_pipeline::{Observer, Trace, prepare_config, remove, save_results},
    translation, vision_image,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub fn prepare(image: &Path, destination: &Path, ordinary_root: &Path) -> Result<Option<Value>> {
    if destination.exists() {
        return Ok(Some(persistence::parse_page(&std::fs::read(destination)?)?));
    }
    let ordinary = persistence::cache_path(image, ordinary_root)?;
    if !ordinary.exists() {
        return Ok(None);
    }
    let page = persistence::parse_page(&std::fs::read(ordinary)?)?;
    Ok(Some(from_text(page)))
}

pub fn from_text(mut page: Value) -> Value {
    if let Some(blocks) = page["blocks"].as_array_mut() {
        for b in blocks {
            if !truth(&b["edited"]) {
                b["translation"] = json!("");
                b["error"] = json!("");
                remove(b, &["translation_config_override", "quality_warnings", "vision_source"]);
            }
        }
    }
    if let Some(dialogues) = page.get_mut("dialogues").and_then(Value::as_array_mut) {
        for d in dialogues {
            remove(
                d,
                &[
                    "full_translation",
                    "mapping",
                    "mapping_status",
                    "translation_config",
                ],
            );
        }
    }
    remove(&mut page, &["translation_config"]);
    page["model"] = json!("");
    page
}

fn parse_vision_response(content: &str, ids: &[String]) -> Result<Value> {
    let value = translation::strict_json(content)?;
    ensure!(value.as_object().is_some_and(|fields| fields.len() == 2),
        "视觉回复必须包含 translations 和 source_suggestions 两个字段。");
    for name in ["translations", "source_suggestions"] {
        let matches_batch = value[name].as_object().is_some_and(|fields| {
            fields.len() == ids.len() && ids.iter().all(|id| fields.get(id).is_some_and(Value::is_string))
        });
        ensure!(matches_batch, "视觉回复中的 {name} 与本批文字框编号或类型不符。");
    }
    Ok(value)
}

pub async fn translate(
    current_page: &mut Value,
    image: &Path,
    layout_image: Option<&Path>,
    previous_image: Option<&Path>,
    model: &str,
    service: &TranslationService,
    block_id: Option<&str>,
    observer: &mut impl Observer,
) -> Result<()> {
    ensure!(
        service.provider == "omlx",
        "视觉辅助翻译需要 oMLX 和支持图片的模型，请在接口设置中选择。"
    );
    if array(&current_page["blocks"]).is_empty() {
        return Ok(());
    }
    // Planning normalizes dialogue structure and block provenance. Publish the
    // prepared page only after the model check (or a fully cached result).
    let mut prepared = current_page.clone();
    let page = &mut prepared;
    let audit_changed = translation::audit_cache(page, block_id);
    structure::ensure(page, layout_image)?;
    let mut config = json!({"mode":"vision","version":1,"prompt_revision":3,"model":model,
        "enable_thinking":false,"strict_json":true,"max_tokens":translation::VISION_MAX_TOKENS,
        "image_edge":vision_image::IMAGE_EDGE,"batch_size":8,"ocr":structure::ocr_signature(page),
        "provider":service.provider,"base_url":service.base_url});
    // Hash and encode the same snapshot even if the reference file is replaced
    // during translation. A matching cache only needs the hash, never a decode.
    let previous_bytes = previous_image.map(|path| -> Result<Vec<u8>> {
        ensure!(path.is_absolute(), "上一页参照需要原图的完整路径。");
        std::fs::read(path).context("无法读取上一页参照，请检查上一页原图后重试。")
    }).transpose()?;
    if let Some(bytes) = &previous_bytes {
        config["prompt_revision"] = json!(4);
        config["previous_image_sha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
    }
    let page_changed = page["translation_config"] != config;
    let changed = prepare_config(page, &config, page_changed, block_id);
    let mut pending: Vec<_> = (0..array(&page["blocks"]).len())
        .filter(|&i| {
            let b = &page["blocks"][i];
            !truth(&b["edited"])
                && !keeps_original(b)
                && !crate::ocr_review::suppressed(b)
                && b["overlay"] != "skip"
                && b["text_kind"] != "noise"
                && block_id.is_none_or(|id| text(&b["id"]) == id)
                && (!truth(&b["translation"]) || truth(&b["error"]))
        })
        .collect();
    pending.sort_by(|&a, &b| {
        page["blocks"][a]["reading_order"]
            .as_f64()
            .unwrap_or(1e9)
            .total_cmp(&page["blocks"][b]["reading_order"].as_f64().unwrap_or(1e9))
    });
    if pending.is_empty() {
        if changed || audit_changed { structure::sync(page)?; observer.save(page)?; }
        *current_page = prepared;
        return Ok(());
    }
    let previous = previous_bytes.map(|bytes| {
        vision_image::encode_bytes(&bytes).context("无法解码上一页参照，请检查上一页原图后重试。")
    }).transpose()?;
    ensure!(
        service
            .list_models()
            .await?
            .iter()
            .any(|name| name == model),
        "所选视觉模型不在 oMLX 模型列表中，请刷新模型列表。"
    );
    let encoded = vision_image::encode(image)?;
    *current_page = prepared;
    let page = current_page;
    if changed { observer.save(page)?; }
    let count = pending.len().div_ceil(8);
    for (index, indices) in pending.chunks(8).enumerate() {
        observer.progress(&format!("正在结合画面翻译第 {} / {} 组…", index + 1, count))?;
        let started = Instant::now();
        let members: Vec<_> = indices.iter().map(|&i| page["blocks"][i].clone()).collect();
        let ids: Vec<_> = members.iter().map(|b| text(&b["id"])).collect();
        let mut event = json!({"mode":"vision","model":model,"image":image,"batch_ids":ids,
            "provider":service.provider,"base_url":service.base_url});
        let outcome: Result<()> = async {
            let payload = translation::vision_payload(model, page, &members, &encoded.url,
                previous.as_ref().map(|image| image.url.as_str()));
            let response = service
                .request(
                    service.chat_route(),
                    Some(&payload),
                    Duration::from_secs(300),
                )
                .await?;
            let content = service.translation_content(&response)?;
            let sources: Map<_, _> = members
                .iter()
                .map(|b| (text(&b["id"]), b["source"].clone()))
                .collect();
            let sources = Value::Object(sources);
            let obj = parse_vision_response(content, &ids)?;
            let (translated, errors) = translation::parse_rows(&obj, &ids, Some(&sources), true)?;
            for &i in indices {
                let b = &mut page["blocks"][i];
                let id = text(&b["id"]);
                b["translation"] = translated.get(&id).cloned().unwrap_or(json!(""));
                b["error"] = errors.get(&id).cloned().unwrap_or(json!(""));
                let source = b["source"].as_str().unwrap_or("");
                let suggestion = obj["source_suggestions"]
                    .as_object()
                    .and_then(|m| m.get(&id))
                    .and_then(Value::as_str)
                    .filter(|s| {
                        s.chars().count() <= 2000
                            && s.trim_matches(whitespace) != source.trim_matches(whitespace)
                    })
                    .map(|s| s.trim_matches(whitespace))
                    .unwrap_or("");
                b["vision_source"] = json!(suggestion);
                let mut warnings = translation::quality_warnings(&b["source"], &b["translation"]);
                if translation::same_as_source(&b["source"], &b["translation"]) {
                    warnings.push(translation::SHARED_HANZI_WARNING.into());
                }
                b["quality_warnings"] = json!(warnings);
            }
            if let Some(usage) = response.get("usage") {
                if let Some(usage) = usage.as_object() {
                    event["usage"] = Value::Object(
                        usage
                            .iter()
                            .filter(|(_, v)| v.is_number())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    );
                }
            } else {
                event["usage"] = json!({});
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            let reason = format!("视觉翻译失败：{}", service.safe_error(error));
            for &i in indices {
                let b = &mut page["blocks"][i];
                b["translation"] = json!("");
                b["error"] = json!(reason);
                remove(b, &["vision_source"]);
            }
            event["error"] = json!(reason);
        }
        event["wall_seconds"] = json!(started.elapsed().as_secs_f64());
        let mut runs = array(&page["vision_runs"]).to_vec();
        runs.push(event.clone());
        if runs.len() > 50 {
            runs.drain(..runs.len() - 50);
        }
        page["vision_runs"] = json!(runs);
        page["vision_input"] = json!({"width":encoded.width,"height":encoded.height});
        save_results(page, layout_image, observer)?;
        observer.metrics(&event)?;
    }
    Ok(())
}

fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub async fn probe(request: &Value) -> Result<Value> {
    let image = Path::new(
        request["image"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("image required"))?,
    );
    if request["op"] == "prepare" {
        let destination = Path::new(
            request["destination"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("destination required"))?,
        );
        let root = Path::new(
            request["ordinary_root"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("ordinary_root required"))?,
        );
        return Ok(json!(prepare(image, destination, root)?));
    }
    let service = TranslationService::new(
        request["provider"].as_str().unwrap_or("omlx"),
        request["base_url"].as_str(),
        request["key"].as_str(),
    )?;
    let mut page = request["page"].clone();
    let mut trace = Trace::default();
    let result = translate(
        &mut page,
        image,
        Some(image),
        request["previous_image"].as_str().map(Path::new),
        request["model"].as_str().unwrap_or("same-model"),
        &service,
        request["block_id"].as_str(),
        &mut trace,
    )
    .await;
    Ok(
        json!({"page":page,"saves":trace.saves,"progress":trace.progress,"metrics":trace.metrics,"error":result.err().map(|e|service.safe_error(e))}),
    )
}
