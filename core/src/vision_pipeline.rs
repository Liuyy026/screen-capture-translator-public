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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_request_enforces_schema_for_current_batch_with_thinking_off() {
        let batch = [json!({"id":"7"}), json!({"id":"12"})];
        let page = json!({"blocks":[batch[0], batch[1], {"id":"not-requested"}]});
        let payload = translation::vision_payload("test-model", &page, &batch, "data:", None);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert_eq!(payload["max_tokens"], 8192);
        let schema = &payload["structured_outputs"]["json"];
        assert_eq!(schema["required"], json!(["translations", "source_suggestions"]));
        assert_eq!(schema["additionalProperties"], false);
        for name in ["translations", "source_suggestions"] {
            let fields = &schema["properties"][name];
            assert_eq!(fields["properties"], json!({"7":{"type":"string"}, "12":{"type":"string"}}));
            assert_eq!(fields["required"], json!(["7", "12"]));
            assert_eq!(fields["additionalProperties"], false);
        }
    }

    #[test]
    fn vision_response_rejects_out_of_batch_fields_before_saving_translations() {
        let ids = ["7".to_owned()];
        let good = json!({"translations":{"7":"你好"},"source_suggestions":{"7":""}});
        assert!(parse_vision_response(&good.to_string(), &ids).is_ok());
        let mut reply = good.clone();
        reply["source_suggestions"] = json!({"8":""});
        assert!(parse_vision_response(&reply.to_string(), &ids).is_err());
        reply["source_suggestions"] = json!({"7":null});
        assert!(parse_vision_response(&reply.to_string(), &ids).is_err());
        reply = good;
        reply["unexpected"] = json!(true);
        assert!(parse_vision_response(&reply.to_string(), &ids).is_err());
    }

    #[test]
    fn reference_precedes_the_current_image_and_only_current_ids_are_targets() {
        let block = json!({"id":"a","source":"ありがとう","translation":"private edit"});
        let page = json!({"image":"/private/chapter/page.png","blocks":[block]});
        let payload = translation::vision_payload("test", &page, &[block], "data:current", Some("data:previous"));
        let parts = payload["messages"][1]["content"].as_array().unwrap();
        let targets: Value = serde_json::from_str(parts[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(targets["target_ids"], json!(["a"]));
        assert_eq!(targets["page"][0]["ja"], "ありがとう");
        assert_eq!(parts.len(), 5);
        assert!(parts[1]["text"].as_str().unwrap().contains("上一页"));
        assert_eq!(parts[2]["image_url"]["url"], "data:previous");
        assert!(parts[3]["text"].as_str().unwrap().contains("当前页"));
        assert_eq!(parts[4]["image_url"]["url"], "data:current");
        assert!(!payload.to_string().contains("private"));
    }

    #[tokio::test]
    async fn contextual_cache_uses_reference_bytes_and_failed_changes_preserve_translations() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let previous = folder.path().join("page2.pgm");
        let encoded = b"P5\n1 1\n255\n\x00";
        std::fs::write(&previous, encoded)?;
        let mut page = json!({"width":100,"height":100,"blocks":[
            {"id":"a","source":"ありがとう","translation":"谢谢","box":[10,10,40,40]},
            {"id":"b","source":"えっ","edited":true,"translation":"手改","box":[60,10,90,40]}]});
        let service = TranslationService::new("omlx", Some("http://127.0.0.1:1/v1"), None)?;
        page["translation_config"] = json!({"mode":"vision","version":1,"prompt_revision":4,
            "previous_image_sha256":format!("{:x}", Sha256::digest(encoded)),
            "model":"offline","enable_thinking":false,"strict_json":true,
            "max_tokens":translation::VISION_MAX_TOKENS,"image_edge":vision_image::IMAGE_EDGE,"batch_size":8,
            "ocr":structure::ocr_signature(&page),"provider":service.provider,"base_url":service.base_url});
        let mut trace = Trace::default();
        let current = Path::new("cached-current-image");
        translate(&mut page, current, None, Some(&previous), "offline", &service, None, &mut trace).await?;
        assert!(trace.metrics.is_empty());
        assert_eq!(page["blocks"][0]["translation"], "谢谢");
        let saved = page.clone();
        // Same path, different pixels must require a new translation.
        std::fs::write(&previous, b"P5\n1 1\n255\n\xff")?;
        let error = translate(&mut page, current, None, Some(&previous), "offline", &service, None, &mut trace).await.unwrap_err();
        assert!(error.to_string().contains("无法连接"));
        assert_eq!(page, saved);
        std::fs::write(&previous, b"broken")?;
        let error = translate(&mut page, current, None, Some(&previous), "offline", &service, None, &mut trace).await.unwrap_err();
        assert!(error.to_string().contains("无法解码上一页"));
        std::fs::remove_file(&previous)?;
        let error = translate(&mut page, current, None, Some(&previous), "offline", &service, None, &mut trace).await.unwrap_err();
        assert!(error.to_string().contains("无法读取上一页"));
        assert_eq!(page, saved);
        Ok(())
    }

    #[tokio::test]
    async fn accepted_shared_word_is_complete_on_offline_resume() -> Result<()> {
        let warning = translation::SHARED_HANZI_WARNING;
        let mut page = json!({"width":100,"height":100,"model":"offline","blocks":[
            {"id":"1","source":"当然","translation":"当然","quality_warnings":[warning],
             "error":translation::ECHO_ERROR,"box":[10,10,40,40]}]});
        let service = TranslationService::new("omlx", Some("http://127.0.0.1:1/v1"), None)?;
        page["translation_config"] = json!({"mode":"vision","version":1,"prompt_revision":3,
            "model":"offline","enable_thinking":false,"strict_json":true,
            "max_tokens":translation::VISION_MAX_TOKENS,"image_edge":vision_image::IMAGE_EDGE,"batch_size":8,
            "ocr":structure::ocr_signature(&page),"provider":service.provider,"base_url":service.base_url});
        let mut trace = Trace::default();
        translate(&mut page, Path::new("unused.png"), None, None, "offline", &service, None, &mut trace).await?;
        assert_eq!(page["blocks"][0]["error"], "");
        assert_eq!(page["diagnostics"]["translation"]["translated_blocks"], 1);
        assert!(trace.progress.is_empty() && trace.metrics.is_empty());
        assert_eq!(trace.saves.last().unwrap()["blocks"][0]["error"], "");
        Ok(())
    }

    #[test]
    fn shared_words_respect_mode_and_duplicate_response_ids() -> Result<()> {
        let ids = vec!["a".into(), "b".into()];
        let sources = json!({"a":"当然","b":"そうだ"});
        for reply in [
            json!({"translations":{"a":"当然","b":"そうだ"}}),
            json!({"translations":[{"id":"a","text":"当然"},{"id":"b","text":"そうだ"}]}),
        ] {
            let (vision, errors) = translation::parse_rows(&reply, &ids, Some(&sources), true)?;
            assert_eq!(vision["a"], "当然");
            assert_eq!(errors["b"], translation::ECHO_ERROR);
            let (_, errors) = translation::parse_rows(&reply, &ids, Some(&sources), false)?;
            assert_eq!(errors["a"], translation::ECHO_ERROR);
        }
        let repeated = json!({"translations":[{"id":"a","text":"当然"},{"id":"a","text":"当然"}]});
        let (vision, errors) = translation::parse_rows(&repeated, &ids, Some(&sources), true)?;
        assert!(vision["a"].is_null());
        assert!(errors["a"].as_str().unwrap().contains("重复"));
        Ok(())
    }
}
