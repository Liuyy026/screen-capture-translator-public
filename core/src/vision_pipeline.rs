//! Image-grounded translation; original OCR and ordinary caches stay intact.
use crate::{
    compat::{array, text, truth},
    overlay, persistence,
    service::TranslationService,
    structure,
    translate_pipeline::{Observer, Trace, remove},
    translation, vision_image,
};
use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};
use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, Instant},
};

pub fn prepare(image: &Path, destination: &Path, ordinary_root: &Path) -> Result<Option<Value>> {
    if destination.exists() {
        return Ok(Some(serde_json::from_slice(&std::fs::read(destination)?)?));
    }
    let ordinary = persistence::cache_path(image, ordinary_root)?;
    if !ordinary.exists() {
        return Ok(None);
    }
    let mut page: Value = serde_json::from_slice(&std::fs::read(ordinary)?)?;
    if let Some(blocks) = page["blocks"].as_array_mut() {
        for b in blocks {
            if !truth(&b["edited"]) {
                b["translation"] = json!("");
                b["error"] = json!("");
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
    Ok(Some(page))
}

pub async fn translate(
    page: &mut Value,
    image: &Path,
    model: &str,
    service: &TranslationService,
    block_id: Option<&str>,
    observer: &mut impl Observer,
) -> Result<()> {
    ensure!(
        service.provider == "omlx",
        "视觉辅助翻译需要 oMLX 和支持图片的模型，请在接口设置中选择。"
    );
    if array(&page["blocks"]).is_empty() {
        return Ok(());
    }
    translation::audit_cache(page);
    structure::ensure(page, Some(image))?;
    let config = json!({"mode":"vision","version":1,"prompt_revision":2,"model":model,
        "image_edge":vision_image::IMAGE_EDGE,"batch_size":8,"ocr":structure::ocr_signature(page),
        "provider":service.provider,"base_url":service.base_url});
    if page["translation_config"] != config {
        for b in page["blocks"].as_array_mut().unwrap() {
            if !truth(&b["edited"]) {
                b["translation"] = json!("");
                b["error"] = json!("");
                remove(b, &["vision_source"]);
            }
        }
        if let Some(dialogues) = page["dialogues"].as_array_mut() {
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
        page["translation_config"] = config;
        page["model"] = json!(model);
        observer.save(page)?;
    }
    let mut pending: Vec<_> = (0..array(&page["blocks"]).len())
        .filter(|&i| {
            let b = &page["blocks"][i];
            !truth(&b["edited"])
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
        return Ok(());
    }
    ensure!(
        service
            .list_models()
            .await?
            .iter()
            .any(|name| name == model),
        "所选视觉模型不在 oMLX 模型列表中，请刷新模型列表。"
    );
    let encoded = vision_image::encode(image)?;
    let count = pending.len().div_ceil(8);
    for (index, indices) in pending.chunks(8).enumerate() {
        observer.progress(&format!("正在结合画面翻译第 {} / {} 组…", index + 1, count))?;
        let started = Instant::now();
        let members: Vec<_> = indices.iter().map(|&i| page["blocks"][i].clone()).collect();
        let ids: Vec<_> = members.iter().map(|b| text(&b["id"])).collect();
        let mut event = json!({"mode":"vision","model":model,"image":image,"batch_ids":ids,
            "provider":service.provider,"base_url":service.base_url});
        let outcome: Result<()> = async {
            let payload = translation::vision_payload(service, model, page, &members, &encoded.url);
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
            let (mut translated, mut errors) =
                translation::parse_translations(content, &ids, Some(&sources))?;
            let obj = translation::strict_json(content)?;
            let mut unchanged = HashSet::new();
            for id in &ids {
                let source = sources[id].as_str().unwrap_or("");
                let proposed = obj["translations"]
                    .as_object()
                    .and_then(|m| m.get(id))
                    .and_then(Value::as_str);
                if errors[id] == translation::ECHO_ERROR
                    && proposed.is_some()
                    && source
                        .chars()
                        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
                    && !source
                        .chars()
                        .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))
                {
                    translated[id] = json!(proposed.unwrap().trim_matches(whitespace));
                    errors.as_object_mut().unwrap().shift_remove(id);
                    unchanged.insert(id.clone());
                }
            }
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
                if unchanged.contains(&id) {
                    warnings.push("原文与译文相同，可能为中日共用词，请核对".into());
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
        overlay::ensure(page, Some(image))?;
        structure::sync(page)?;
        observer.save(page)?;
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
