//! Native translation orchestration. The page is caller-owned; every completed
//! batch is checkpointed through an observer, suitable for resumable queued work.
use crate::{
    compat::{array, keeps_original, text, truth},
    overlay,
    service::TranslationService,
    structure, translation,
};
use anyhow::{Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

pub struct Options<'a> {
    pub model: &'a str,
    pub unit: &'a str,
    pub batch_size: usize,
    pub block_id: Option<&'a str>,
    pub image: Option<&'a Path>,
}

pub trait Observer {
    fn progress(&mut self, message: &str) -> Result<()>;
    fn save(&mut self, page: &Value) -> Result<()>;
    fn metrics(&mut self, event: &Value) -> Result<()>;
}

pub(crate) fn remove(value: &mut Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            obj.shift_remove(*key);
        }
    }
}

pub(super) fn save_results(
    page: &mut Value,
    image: Option<&Path>,
    observer: &mut impl Observer,
) -> Result<()> {
    if let Some(image) = image {
        if let Err(error) = overlay::ensure(page, Some(image)) {
            remove(page, &["overlay_signature"]);
            if !page["diagnostics"].is_object() {
                page["diagnostics"] = json!({});
            }
            page["diagnostics"]["geometry_error"] = json!(error.to_string());
        }
    }
    structure::sync(page)?;
    observer.save(page)
}

fn has_content(block: &Value) -> bool {
    truth(&block["translation"]) && !truth(&block["error"])
}
fn allowed(block: &Value, block_id: Option<&str>) -> bool {
    !truth(&block["edited"])
        && !keeps_original(block)
        && !crate::ocr_review::suppressed(block)
        && block["overlay"] != "skip"
        && block["text_kind"] != "noise"
        && block_id.is_none_or(|id| text(&block["id"]) == id)
}

// A one-block retry can use a different model from its neighbours. Keep the
// page-wide provenance until a full-page request actually adopts a new config.
pub(super) fn prepare_config(
    page: &mut Value, config: &Value, page_changed: bool, block_id: Option<&str>,
) -> bool {
    let mut reset_ids = HashSet::new();
    let mut changed = false;
    for block in page["blocks"].as_array_mut().expect("validated blocks") {
        if block_id.is_some_and(|id| text(&block["id"]) != id)
            || truth(&block["edited"]) {
            continue;
        }
        if keeps_original(block) || crate::ocr_review::suppressed(block) {
            // Old pages did not record which model produced a hidden translation.
            // Never attribute that text to an inferred model after a bulk switch.
            if block_id.is_none() && page_changed && truth(&block["translation"])
                && block.get("translation_config_override").is_none() {
                block["translation_config_override"] = Value::Null;
            }
            continue;
        }
        let stale = block.get("translation_config_override")
            .map_or(page_changed, |previous| previous != config);
        if stale {
            block["translation"] = json!("");
            block["error"] = json!("");
            remove(block, &["quality_warnings", "vision_source"]);
            reset_ids.insert(text(&block["id"]));
        }
        if block_id.is_some() && page_changed {
            changed |= block.get("translation_config_override") != Some(config);
            block["translation_config_override"] = config.clone();
        } else if block.get("translation_config_override").is_some() {
            remove(block, &["translation_config_override"]);
            changed = true;
        }
    }
    if let Some(dialogues) = page["dialogues"].as_array_mut() {
        for d in dialogues {
            if (block_id.is_none() && page_changed)
                || array(&d["block_ids"]).iter().any(|id| reset_ids.contains(&text(id))) {
                remove(d, &["full_translation", "mapping", "mapping_status", "translation_error",
                    "translation_config", "quality_warnings"]);
            }
        }
    }
    if block_id.is_none() {
        page["model"] = config["model"].clone();
        page["translation_config"] = config.clone();
    }
    changed || !reset_ids.is_empty() || (block_id.is_none() && page_changed)
}

pub async fn translate(
    current_page: &mut Value,
    service: &TranslationService,
    options: &Options<'_>,
    observer: &mut impl Observer,
) -> Result<()> {
    ensure!(
        ["blocks", "dialogues"].contains(&options.unit),
        "未知翻译单位"
    );
    if array(&current_page["blocks"]).is_empty() {
        return Ok(());
    }
    // Planning normalizes dialogue structure and block provenance. Publish the
    // prepared page only after the model check (or a fully cached result).
    let mut prepared = current_page.clone();
    let page = &mut prepared;
    translation::audit_cache(page, options.block_id);
    structure::ensure(page, options.image)?;
    let mut ordered: Vec<usize> = (0..array(&page["blocks"]).len()).collect();
    ordered.sort_by(|&a, &b| {
        page["blocks"][a]["reading_order"]
            .as_f64()
            .unwrap_or(1e9)
            .total_cmp(&page["blocks"][b]["reading_order"].as_f64().unwrap_or(1e9))
    });
    let eligible: Vec<_> = ordered
        .iter()
        .copied()
        .filter(|&i| allowed(&page["blocks"][i], options.block_id))
        .collect();
    if eligible.is_empty() {
        *current_page = prepared;
        return Ok(());
    }
    let batch_size = if options.batch_size == 0 {
        8
    } else {
        options.batch_size
    };
    let config = json!({"model":options.model,"unit":options.unit,"batch_size":batch_size,"protocol":4,
        "ocr":structure::ocr_signature(page),"structure":page["structure"]["signature"],"provider":service.provider,"base_url":service.base_url});
    let legacy = json!({"provider":"ollama","base_url":"http://127.0.0.1:11439"});
    let previous = page
        .get("translation_config")
        .filter(|v| !v.is_null())
        .map(|v| {
            let mut merged = legacy.clone();
            if let Some(obj) = v.as_object() {
                for (k, v) in obj {
                    merged[k] = v.clone();
                }
            }
            merged
        });
    let invalidate = page["model"] != options.model
        || previous.as_ref().is_some_and(|p| p != &config)
        || previous.is_none() && (options.unit == "dialogues" || service.identity() != legacy);
    let changed = prepare_config(page, &config, invalidate, options.block_id);
    if changed { structure::sync(page)?; }
    let pending: Vec<_> = eligible
        .iter()
        .copied()
        .filter(|&i| !has_content(&page["blocks"][i]))
        .collect();
    let by_id: HashMap<_, _> = array(&page["blocks"])
        .iter()
        .enumerate()
        .map(|(i, b)| (text(&b["id"]), i))
        .collect();
    let mut jobs: Vec<(Vec<usize>, Option<usize>)> = vec![];
    let mut handled = HashSet::new();
    if options.unit == "dialogues" && options.block_id.is_none() {
        let eligible_ids: HashSet<_> = eligible
            .iter()
            .map(|&i| text(&page["blocks"][i]["id"]))
            .collect();
        for (i, d) in array(&page["dialogues"]).iter().enumerate() {
            let ids = array(d.get("source_block_ids").unwrap_or(&d["block_ids"]));
            if ids.is_empty() || ids.iter().any(|id| !eligible_ids.contains(&text(id)))
                || array(&d["block_ids"]).iter().any(|id| by_id.get(&text(id))
                    .is_some_and(|&i| keeps_original(&page["blocks"][i]) || crate::ocr_review::suppressed(&page["blocks"][i])))
            {
                continue;
            }
            let members: Vec<_> = ids.iter().map(|id| by_id[&text(id)]).collect();
            handled.extend(array(&d["block_ids"]).iter().map(text));
            if d["translation_mode"] == "dialogue" && truth(&d["translation_complete"]) {
                continue;
            }
            jobs.push((members, Some(i)));
        }
    }
    let pending: Vec<_> = pending
        .into_iter()
        .filter(|&i| !handled.contains(&text(&page["blocks"][i]["id"])))
        .collect();
    jobs.extend(
        pending
            .chunks(batch_size)
            .map(|chunk| (chunk.to_vec(), None)),
    );
    if !jobs.is_empty() {
        ensure!(
            service.list_models().await?.iter().any(|m| m == options.model),
            "所选模型不在服务的本地模型列表中，请刷新模型列表并选择已下载模型"
        );
    }
    *current_page = prepared;
    let page = current_page;
    if changed { save_results(page, options.image, observer)?; }
    for (job_index, (indices, dialogue_index)) in jobs.iter().enumerate() {
        let members: Vec<_> = indices.iter().map(|&i| page["blocks"][i].clone()).collect();
        let dialogue = dialogue_index.map(|i| page["dialogues"][i].clone());
        let (ids, sources, context) = if let Some(d) = &dialogue {
            let id = text(&d["id"]);
            (
                vec![id.clone()],
                json!({id.clone():d["source"]}),
                json!([{"id":id,"ja":d["source"]}]),
            )
        } else {
            let ids: Vec<_> = members.iter().map(|b| text(&b["id"])).collect();
            let sources: Map<_, _> = members
                .iter()
                .map(|b| (text(&b["id"]), b["source"].clone()))
                .collect();
            let positions: Vec<_> = indices
                .iter()
                .map(|i| ordered.iter().position(|v| v == i).unwrap())
                .collect();
            let start = positions.iter().min().unwrap().saturating_sub(4);
            let end = (positions.iter().max().unwrap() + 5).min(ordered.len());
            let context: Vec<_> = ordered[start..end]
                .iter()
                .map(|&i| &page["blocks"][i])
                .filter(|b| !keeps_original(b) && !crate::ocr_review::suppressed(b) && b["overlay"] != "skip" && b["text_kind"] != "noise")
                .map(|b| json!({"id":text(&b["id"]),"ja":b["source"]}))
                .collect();
            (ids, Value::Object(sources), json!(context))
        };
        observer.progress(&format!(
            "正在翻译第 {} / {} 组…",
            job_index + 1,
            jobs.len()
        ))?;
        let payload =
            translation::block_payload(service, options.model, context, &ids, dialogue.is_some());
        let started = Instant::now();
        let mut event = json!({"model":options.model,"image":options.image.map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),
            "batch_ids":ids,"unit":if dialogue.is_some(){"dialogues"}else{"blocks"},"route":service.chat_route(),
            "provider":service.provider,"base_url":service.base_url});
        let outcome: Result<()> = async {
            let response = service
                .request(
                    service.chat_route(),
                    Some(&payload),
                    Duration::from_secs(300),
                )
                .await?;
            for key in [
                "prompt_eval_count",
                "prompt_eval_duration",
                "eval_count",
                "eval_duration",
                "load_duration",
                "total_duration",
            ] {
                if response[key].is_number() {
                    event[key] = response[key].clone();
                }
            }
            if response["done_reason"]
                .as_str()
                .is_some_and(|s| ["stop", "length", "load", "unload"].contains(&s))
            {
                event["done_reason"] = response["done_reason"].clone();
            }
            let (translated, errors) = translation::parse_translations(
                service.translation_content(&response)?,
                &ids,
                Some(&sources),
            )?;
            if let Some(di) = dialogue_index {
                let key = &ids[0];
                let Some(full) = translated[key].as_str() else {
                    bail!(
                        "{}",
                        errors[key].as_str().unwrap_or("模型遗漏了这句，原文已保留")
                    );
                };
                let compact_len = |s: &str| {
                    s.chars()
                        .filter(|c| !c.is_whitespace() && !('\u{1c}'..='\u{1f}').contains(c))
                        .count()
                };
                ensure!(
                    compact_len(full)
                        <= 80.max(
                            compact_len(
                                dialogue.as_ref().unwrap()["source"].as_str().unwrap_or("")
                            ) * 6
                        ),
                    "对白译文异常过长"
                );
                let (mapping, status) = translation::split_dialogue(full, &members);
                let d = &mut page["dialogues"][*di];
                d["full_translation"] = json!(full);
                d["translation_error"] = json!("");
                d["translation_config"] = config.clone();
                d["mapping"] = mapping.clone();
                d["mapping_status"] = json!(status);
                d["quality_warnings"] =
                    json!(translation::quality_warnings(&d["source"], &json!(full)));
                for &i in indices {
                    let b = &mut page["blocks"][i];
                    b["translation"] = mapping[&text(&b["id"])].clone();
                    b["error"] = json!("");
                    b["quality_warnings"] = json!(translation::quality_warnings(
                        &b["source"],
                        &b["translation"]
                    ));
                }
            } else {
                for &i in indices {
                    let b = &mut page["blocks"][i];
                    let key = text(&b["id"]);
                    b["translation"] = translated.get(&key).cloned().unwrap_or(json!(""));
                    b["error"] = errors.get(&key).cloned().unwrap_or(json!(""));
                    b["quality_warnings"] = json!(translation::quality_warnings(
                        &b["source"],
                        &b["translation"]
                    ));
                }
                if let Some(dialogues) = page["dialogues"].as_array_mut() {
                    for d in dialogues {
                        if array(&d["block_ids"])
                            .iter()
                            .any(|id| ids.contains(&text(id)))
                        {
                            remove(d, &["full_translation", "mapping", "mapping_status",
                                "translation_error", "translation_config", "quality_warnings"]);
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            let reason = format!("翻译失败：{}", service.safe_error(error));
            event["error"] = json!(reason);
            if let Some(di) = dialogue_index {
                let d = &mut page["dialogues"][*di];
                d["translation_error"] = json!(reason);
                remove(d, &["full_translation", "mapping"]);
            }
            for &i in indices {
                page["blocks"][i]["translation"] = json!("");
                page["blocks"][i]["error"] = json!(reason);
            }
        }
        event["wall_seconds"] = json!(started.elapsed().as_secs_f64());
        observer.metrics(&event)?;
        save_results(page, options.image, observer)?;
    }
    structure::ensure(page, options.image)?;
    observer.save(page)
}

#[derive(Default)]
pub struct Trace {
    pub saves: Vec<Value>,
    pub progress: Vec<String>,
    pub metrics: Vec<Value>,
}
impl Observer for Trace {
    fn progress(&mut self, message: &str) -> Result<()> {
        self.progress.push(message.into());
        Ok(())
    }
    fn save(&mut self, page: &Value) -> Result<()> {
        self.saves.push(page.clone());
        Ok(())
    }
    fn metrics(&mut self, event: &Value) -> Result<()> {
        self.metrics.push(event.clone());
        Ok(())
    }
}

pub async fn probe(request: &Value) -> Result<Value> {
    let service = TranslationService::new(
        request["provider"].as_str().unwrap_or("ollama"),
        request["base_url"].as_str(),
        request["key"].as_str(),
    )?;
    let mut page = request["page"].clone();
    let mut trace = Trace::default();
    let options = Options {
        model: request["model"].as_str().unwrap_or("same-model"),
        unit: request["unit"].as_str().unwrap_or("blocks"),
        batch_size: request["batch_size"].as_u64().unwrap_or(8) as usize,
        block_id: request["block_id"].as_str(),
        image: request["image"].as_str().map(Path::new),
    };
    let result = translate(&mut page, &service, &options, &mut trace).await;
    Ok(
        json!({"page":page,"saves":trace.saves,"progress":trace.progress,"metrics":trace.metrics,
        "error":result.err().map(|e|service.safe_error(e))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoring_a_block_after_switching_models_rechecks_its_own_translation() {
        let old = json!({"model":"A"});
        let new = json!({"model":"B"});
        for excluded in [json!({"keep_original":true}), json!({"ocr_review":{"status":"suspect"}})] {
            let mut block = json!({"id":"1","translation":"旧模型译文","translation_config_override":old});
            block.as_object_mut().unwrap().extend(excluded.as_object().unwrap().clone());
            let mut page = json!({"model":"A","translation_config":old,"blocks":[block]});
            prepare_config(&mut page, &new, true, None);
            assert_eq!(page["blocks"][0]["translation"], "旧模型译文");
            page["blocks"][0]["keep_original"] = json!(false);
            page["blocks"][0]["ocr_confirmed"] = json!(true);
            let mut back_to_old = page.clone();
            prepare_config(&mut back_to_old, &old, true, None);
            assert_eq!(back_to_old["blocks"][0]["translation"], "旧模型译文");
            assert!(prepare_config(&mut page, &new, false, None));
            assert_eq!(page["blocks"][0]["translation"], "");
        }
        let mut legacy = json!({"model":"A","translation_config":old,"blocks":[
            {"id":"1","keep_original":true,"translation":"无法追溯的旧译文"}]});
        prepare_config(&mut legacy, &new, true, None);
        assert_eq!(legacy["blocks"][0]["translation"], "无法追溯的旧译文");
        legacy["blocks"][0]["keep_original"] = json!(false);
        prepare_config(&mut legacy, &old, true, None);
        assert_eq!(legacy["blocks"][0]["translation"], "");
    }

    #[test]
    fn single_block_model_change_preserves_siblings_and_reconciles_the_next_bulk_run() {
        let old = json!({"model":"A"});
        let new = json!({"model":"B"});
        let mut page = json!({"model":"A", "translation_config":old, "blocks":[
            {"id":"first","translation":"一"}, {"id":"other","translation":"二"},
            {"id":"manual","translation":"手改","edited":true}],
            "dialogues":[{"block_ids":["first"],"full_translation":"一"},
                         {"block_ids":["other"],"full_translation":"二"}]});
        let other = page["blocks"][1].clone();
        let manual = page["blocks"][2].clone();
        let other_dialogue = page["dialogues"][1].clone();
        assert!(prepare_config(&mut page, &new, true, Some("first")));
        assert_eq!(page["model"], "A");
        assert_eq!(page["translation_config"], old);
        assert_eq!(page["blocks"][0]["translation_config_override"], new);
        assert_eq!(page["blocks"][1], other);
        assert_eq!(page["blocks"][2], manual);
        assert_eq!(page["dialogues"][1], other_dialogue);
        page["blocks"][0]["translation"] = json!("新一");

        let mut bulk_a = page.clone();
        assert!(prepare_config(&mut bulk_a, &old, false, None));
        assert_eq!(bulk_a["blocks"][0]["translation"], "");
        assert_eq!(bulk_a["blocks"][1], other);

        assert!(prepare_config(&mut page, &new, true, None));
        assert_eq!(page["translation_config"], new);
        assert_eq!(page["blocks"][0]["translation"], "新一");
        assert!(page["blocks"][0].get("translation_config_override").is_none());
        assert_eq!(page["blocks"][1]["translation"], "");
        assert_eq!(page["blocks"][2], manual);
    }

    #[tokio::test]
    async fn failed_model_lookup_preserves_the_callers_page_in_both_modes() -> Result<()> {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let original = json!({"width":80,"height":80,"model":"old","translation_config":{"model":"old"},
            "blocks":[{"id":"1","box":[4,4,28,40],"source":"こんにちは","translation":"旧译文"}]});
        for vision in [false, true] {
            for body in [r#"{"data":[]}"#, r#"{"data":false}"#] {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let url = format!("http://{}/v1", listener.local_addr()?);
                let server = std::thread::spawn(move || -> std::io::Result<()> {
                    let (mut stream, _) = listener.accept()?;
                    for line in BufReader::new(&stream).lines() {
                        if line?.is_empty() { break; }
                    }
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                });
                let service = TranslationService::new("omlx", Some(&url), None)?;
                let mut page = original.clone();
                let mut trace = Trace::default();
                let result = if vision {
                    crate::vision_pipeline::translate(&mut page, Path::new("unused.png"), None, "new",
                        &service, None, &mut trace).await
                } else {
                    let options = Options { model:"new", unit:"blocks", block_id:None, batch_size:8, image:None };
                    translate(&mut page, &service, &options, &mut trace).await
                };
                assert!(result.unwrap_err().to_string().contains("模型列表"));
                server.join().unwrap()?;
                assert_eq!(page, original);
                assert!(trace.saves.is_empty() && trace.metrics.is_empty() && trace.progress.is_empty());
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn complete_legacy_text_page_can_be_reused_offline() -> Result<()> {
        let mut page = json!({"width":100,"height":100,"model":"offline", "blocks":[
            {"id":"1","source":"こんにちは","translation":"你好","box":[10,10,40,40]}]});
        let service = TranslationService::new("ollama", None, None)?;
        let options = Options { model:"offline", unit:"blocks", block_id:None, batch_size:8, image:None };
        let mut trace = Trace::default();
        translate(&mut page, &service, &options, &mut trace).await?;
        assert_eq!(page["blocks"][0]["translation"], "你好");
        assert!(trace.progress.is_empty() && trace.metrics.is_empty());
        Ok(())
    }
}
