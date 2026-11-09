//! Native translation orchestration. The page is caller-owned; every completed
//! batch is checkpointed through an observer, suitable for resumable queued work.
use crate::{
    compat::{array, text, truth},
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

fn save_results(
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
        && block["overlay"] != "skip"
        && block["text_kind"] != "noise"
        && block_id.is_none_or(|id| text(&block["id"]) == id)
}

pub async fn translate(
    page: &mut Value,
    service: &TranslationService,
    options: &Options<'_>,
    observer: &mut impl Observer,
) -> Result<()> {
    ensure!(
        ["blocks", "dialogues"].contains(&options.unit),
        "未知翻译单位"
    );
    if array(&page["blocks"]).is_empty() {
        return Ok(());
    }
    translation::audit_cache(page);
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
        return Ok(());
    }
    ensure!(
        service
            .list_models()
            .await?
            .iter()
            .any(|m| m == options.model),
        "所选模型不在服务的本地模型列表中，请刷新模型列表并选择已下载模型"
    );
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
    if invalidate {
        for block in page["blocks"].as_array_mut().unwrap() {
            if !truth(&block["edited"]) {
                block["translation"] = json!("");
                block["error"] = json!("");
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
                        "translation_error",
                        "translation_config",
                    ],
                );
            }
        }
    }
    page["model"] = json!(options.model);
    page["translation_config"] = config.clone();
    if invalidate {
        save_results(page, options.image, observer)?;
    }
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
            if ids.is_empty() || ids.iter().any(|id| !eligible_ids.contains(&text(id))) {
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
                .filter(|b| b["overlay"] != "skip" && b["text_kind"] != "noise")
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
                            remove(d, &["full_translation", "mapping"]);
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
