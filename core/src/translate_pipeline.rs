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
    time::Instant,
};

pub struct Options<'a> {
    pub model: &'a str,
    pub unit: &'a str,
    pub batch_size: usize,
    pub block_id: Option<&'a str>,
    pub image: Option<&'a Path>,
}

pub trait Observer {
    /// A retry can read a temporary copy while reporting the original page.
    fn source_image(&self) -> Option<&Path> { None }
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
pub(super) fn allowed(block: &Value, block_id: Option<&str>) -> bool {
    !truth(&block["edited"])
        && !keeps_original(block)
        && !crate::ocr_review::suppressed(block)
        && block["overlay"] != "skip"
        && block["text_kind"] != "noise"
        && block_id.is_none_or(|id| text(&block["id"]) == id)
}

// An absent override inherits the page configuration. An explicit null means
// the producer of legacy text is unknown and the result must be refreshed.
fn config_matches(block: &Value, config: &Value, page_changed: bool) -> bool {
    !truth(&block["source_translation_pending"])
        && block.get("translation_config_override").map_or(!page_changed, |prior| prior == config)
}

pub(super) fn needs_translation(block: &Value, config: &Value, page_changed: bool) -> bool {
    !has_content(block) || !config_matches(block, config, page_changed)
}

pub(super) fn accept_block(block: &mut Value, value: &Value, config: &Value, page_changed: bool) {
    block["translation"] = value.clone();
    block["error"] = json!("");
    remove(block, &["retranslation_error", "vision_source", "source_translation_pending"]);
    block["quality_warnings"] = json!(translation::quality_warnings(&block["source"], value));
    if page_changed {
        block["translation_config_override"] = config.clone();
    } else {
        remove(block, &["translation_config_override"]);
    }
}

pub(super) fn record_failure(block: &mut Value, reason: &Value) {
    // The overlay hides blocks carrying an error. A failed attempt does not
    // invalidate the text (or the complete dialogue) that is still displayed.
    if has_content(block) || truth(&block["dialogue_translation_complete"]) {
        block["retranslation_error"] = reason.clone();
    } else {
        remove(block, &["retranslation_error"]);
        block["error"] = reason.clone();
    }
}

// Some old dialogues have a usable full translation but no per-box text.
// Keep that single result intact across checkpoints until every requested box
// in the dialogue has a replacement. Other boxes commit independently.
const INCOMPLETE_DIALOGUE: &str = "这组对白的旧译文只有整段。请一起翻译该对白的全部文字框，以保留其他框的内容。";

#[derive(Default)]
pub(super) struct BlockReplacements {
    whole_dialogues: Vec<Vec<usize>>,
    incomplete: HashSet<usize>,
    staged: HashMap<usize, Value>,
}

impl BlockReplacements {
    pub(super) fn new(page: &Value, pending: &[usize]) -> Self {
        let mut replacements = Self::default();
        let by_id: HashMap<_, _> = array(&page["blocks"]).iter().enumerate()
            .map(|(i, b)| (text(&b["id"]), i)).collect();
        let targets: HashSet<_> = pending.iter().copied().collect();
        for d in array(&page["dialogues"]) {
            let has_unmapped_boxes = d["mapping"].as_object()
                .is_some_and(|mapping| mapping.values().any(|value| !truth(value)));
            if d["translation_mode"] != "dialogue" || !has_unmapped_boxes {
                continue;
            }
            let mut group = Vec::new();
            let mut uncovered = false;
            for id in array(&d["block_ids"]) {
                let Some(&index) = by_id.get(&text(id)) else { continue };
                if targets.contains(&index) {
                    group.push(index);
                } else if !has_content(&page["blocks"][index]) {
                    uncovered = true;
                }
            }
            if uncovered {
                replacements.incomplete.extend(group);
            } else if group.len() > 1 {
                replacements.whole_dialogues.push(group);
            }
        }
        replacements
    }

    pub(super) fn protect_uncovered(
        &self, page: &mut Value, pending: &mut Vec<usize>, block_id: Option<&str>,
    ) -> Result<bool> {
        if self.incomplete.is_empty() { return Ok(true); }
        ensure!(block_id.is_none(), "{INCOMPLETE_DIALOGUE}");
        for &index in &self.incomplete {
            record_failure(&mut page["blocks"][index], &json!(INCOMPLETE_DIALOGUE));
        }
        pending.retain(|index| !self.incomplete.contains(index));
        Ok(false)
    }

    pub(super) fn accept(
        &mut self, page: &mut Value, index: usize, block: Value, updated: &mut HashSet<String>,
    ) {
        if let Some(group) = self.whole_dialogues.iter().find(|g| g.contains(&index)) {
            self.staged.insert(index, block);
            if group.iter().all(|i| self.staged.contains_key(i)) {
                for &i in group {
                    let ready = self.staged.remove(&i).expect("complete dialogue");
                    updated.insert(text(&ready["id"]));
                    page["blocks"][i] = ready;
                }
            }
        } else {
            updated.insert(text(&block["id"]));
            page["blocks"][index] = block;
        }
    }
}

pub(super) fn invalidate_dialogues(page: &mut Value, updated: &HashSet<String>) {
    if let Some(dialogues) = page["dialogues"].as_array_mut() {
        for d in dialogues {
            if array(&d["block_ids"]).iter().any(|id| updated.contains(&text(id))) {
                remove(d, &["full_translation", "mapping", "mapping_status",
                    "translation_error", "translation_config", "quality_warnings"]);
            }
        }
    }
}

// Successful batches carry per-block provenance until the entire requested
// page is current. A cancelled or partially failed pass keeps its old default.
pub(super) fn finish_config(page: &mut Value, config: &Value, page_changed: bool, block_id: Option<&str>) -> bool {
    let mut changed = false;
    let prior = page["translation_config"].clone();
    for b in page["blocks"].as_array_mut().expect("validated blocks") {
        if truth(&b["edited"]) || block_id.is_some_and(|id| b["id"] != id) {
            continue;
        }
        let eligible = allowed(b, block_id);
        if eligible && b.get("retranslation_error").is_some() {
            changed = true;
            remove(b, &["retranslation_error"]);
        }
        if block_id.is_some() {
            continue;
        }
        if eligible && b.get("translation_config_override") == Some(config) {
            changed = true;
            remove(b, &["translation_config_override"]);
        } else if page_changed && !eligible && truth(&b["translation"])
            && b.get("translation_config_override").is_none() {
            // Historically hidden translations did not reliably inherit the
            // page model. Keep their unknown provenance explicit.
            changed = true;
            b["translation_config_override"] = if keeps_original(b) || crate::ocr_review::suppressed(b) {
                Value::Null
            } else {
                prior.clone()
            };
        }
    }
    if block_id.is_none() && (page["model"] != config["model"] || prior != *config) {
        changed = true;
        page["model"] = config["model"].clone();
        page["translation_config"] = config.clone();
    }
    changed
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
    let pending: Vec<_> = eligible
        .iter()
        .copied()
        .filter(|&i| needs_translation(&page["blocks"][i], &config, invalidate))
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
            if ids.is_empty() || array(&d["block_ids"]).is_empty()
                || ids.iter().any(|id| !eligible_ids.contains(&text(id)))
                || array(&d["block_ids"]).iter().any(|id| !eligible_ids.contains(&text(id)))
            {
                continue;
            }
            let members: Vec<_> = ids.iter().map(|id| by_id[&text(id)]).collect();
            handled.extend(array(&d["block_ids"]).iter().map(text));
            if d["translation_mode"] == "dialogue" && truth(&d["translation_complete"])
                && d.get("translation_config").map_or(!invalidate, |prior| prior == &config)
                && array(&d["block_ids"]).iter().all(|id| config_matches(&page["blocks"][by_id[&text(id)]], &config, invalidate)) {
                continue;
            }
            jobs.push((members, Some(i)));
        }
    }
    let mut pending: Vec<_> = pending
        .into_iter()
        .filter(|&i| !handled.contains(&text(&page["blocks"][i]["id"])))
        .collect();
    let mut replacements = BlockReplacements::new(page, &pending);
    let mut all_succeeded = replacements.protect_uncovered(page, &mut pending, options.block_id)?;
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
        let mut event = json!({"model":options.model,"image":observer.source_image().or(options.image).map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),
            "batch_ids":ids,"unit":if dialogue.is_some(){"dialogues"}else{"blocks"},"route":service.chat_route(),
            "provider":service.provider,"base_url":service.base_url});
        let outcome: Result<()> = async {
            let response = service.chat(&payload).await?;
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
                if status == "whole_only" {
                    ensure!(!members.iter().any(has_content),
                        "整段译文无法对应到各文字框，已保留原有译文");
                } else {
                    for b in &members {
                        let value = &mapping[&text(&b["id"])];
                        let error = translation::translation_error(value);
                        ensure!(error.is_empty(), "{error}");
                        ensure!(!translation::same_as_source(&b["source"], value), "{}", translation::ECHO_ERROR);
                    }
                }
                // A canonical OCR box can have duplicates. Resolve every
                // mapping before touching the cached dialogue or its members.
                let mut boxes = Vec::new();
                for id in array(&dialogue.as_ref().unwrap()["block_ids"]) {
                    let index = by_id[&text(id)];
                    let b = &page["blocks"][index];
                    let mapped = mapping.get(&text(id)).or_else(|| {
                        b["duplicate_of"].as_str().and_then(|origin| mapping.get(origin))
                    });
                    boxes.push((index, mapped.ok_or_else(|| anyhow::anyhow!("对白译文缺少文字框映射"))?));
                }
                let d = &mut page["dialogues"][*di];
                d["full_translation"] = json!(full);
                d["translation_error"] = json!("");
                d["translation_config"] = config.clone();
                d["mapping"] = mapping.clone();
                d["mapping_status"] = json!(status);
                d["quality_warnings"] =
                    json!(translation::quality_warnings(&d["source"], &json!(full)));
                for (index, value) in boxes {
                    accept_block(&mut page["blocks"][index], value, &config, invalidate);
                }
            } else {
                let mut updated = HashSet::new();
                for &i in indices {
                    let key = text(&page["blocks"][i]["id"]);
                    if let Some(value) = translated.get(&key) {
                        let mut replacement = page["blocks"][i].clone();
                        accept_block(&mut replacement, value, &config, invalidate);
                        replacements.accept(page, i, replacement, &mut updated);
                    } else {
                        all_succeeded = false;
                        record_failure(&mut page["blocks"][i], &errors[&key]);
                    }
                }
                invalidate_dialogues(page, &updated);
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            let reason = format!("翻译失败：{}", service.safe_error(error));
            event["error"] = json!(reason);
            all_succeeded = false;
            if let Some(di) = dialogue_index {
                let d = &mut page["dialogues"][*di];
                if d["translation_mode"] != "dialogue" && !members.iter().any(has_content) {
                    d["translation_error"] = json!(reason);
                }
            }
            for &i in indices {
                record_failure(&mut page["blocks"][i], &event["error"]);
            }
        }
        event["wall_seconds"] = json!(started.elapsed().as_secs_f64());
        observer.metrics(&event)?;
        save_results(page, options.image, observer)?;
    }
    if all_succeeded { finish_config(page, &config, invalidate, options.block_id); }
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
            finish_config(&mut page, &new, true, None);
            assert_eq!(page["blocks"][0]["translation"], "旧模型译文");
            page["blocks"][0]["keep_original"] = json!(false);
            page["blocks"][0]["ocr_confirmed"] = json!(true);
            assert!(!needs_translation(&page["blocks"][0], &old, true));
            assert!(needs_translation(&page["blocks"][0], &new, false));
        }
        let mut hidden = json!({"translation_config":old,"blocks":[{"id":"1","translation":"新版译文",
            "keep_original":true,"translation_config_override":new}]});
        finish_config(&mut hidden, &new, true, None);
        finish_config(&mut hidden, &old, true, None);
        assert_eq!(hidden["blocks"][0]["translation_config_override"], new);
        let mut legacy = json!({"model":"A","translation_config":old,"blocks":[
            {"id":"1","keep_original":true,"translation":"无法追溯的旧译文"}]});
        finish_config(&mut legacy, &new, true, None);
        assert_eq!(legacy["blocks"][0]["translation"], "无法追溯的旧译文");
        legacy["blocks"][0]["keep_original"] = json!(false);
        assert!(needs_translation(&legacy["blocks"][0], &old, true));
        assert_eq!(legacy["blocks"][0]["translation_config_override"], Value::Null);
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
        assert!(needs_translation(&page["blocks"][0], &new, true));
        accept_block(&mut page["blocks"][0], &json!("新一"), &new, true);
        invalidate_dialogues(&mut page, &HashSet::from(["first".into()]));
        finish_config(&mut page, &new, true, Some("first"));
        assert_eq!(page["model"], "A");
        assert_eq!(page["translation_config"], old);
        assert_eq!(page["blocks"][0]["translation_config_override"], new);
        assert!(needs_translation(&page["blocks"][0], &old, false));
        assert!(!needs_translation(&page["blocks"][0], &new, true));
        assert!(needs_translation(&page["blocks"][1], &new, true));
        assert_eq!(page["blocks"][1], other);
        assert_eq!(page["blocks"][2], manual);
        assert_eq!(page["dialogues"][1], other_dialogue);
        accept_block(&mut page["blocks"][1], &json!("新二"), &new, true);
        finish_config(&mut page, &new, true, None);
        assert_eq!(page["translation_config"], new);
        assert_eq!(page["blocks"][0]["translation"], "新一");
        assert!(page["blocks"][0].get("translation_config_override").is_none());
        assert_eq!(page["blocks"][1]["translation"], "新二");
        assert_eq!(page["blocks"][2], manual);
    }

    fn mock_service(replies: Vec<Value>) -> Result<(TranslationService, std::thread::JoinHandle<std::io::Result<()>>)> {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/v1", listener.local_addr()?);
        let handle = std::thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept()?;
                let mut input = BufReader::new(&stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if input.read_line(&mut line)? == 0 || line.trim().is_empty() { break; }
                    if let Some(size) = line.to_lowercase().strip_prefix("content-length:") {
                        length = size.trim().parse().unwrap();
                    }
                }
                input.read_exact(&mut vec![0; length])?;
                let body = reply.to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
            }
            Ok(())
        });
        Ok((TranslationService::new("omlx", Some(&url), None)?, handle))
    }

    fn chat(reply: Value) -> Value {
        json!({"choices":[{"finish_reason":"stop","message":{"content":reply.to_string()}}]})
    }

    #[tokio::test]
    async fn cancellation_after_model_lookup_cannot_publish_a_cleared_page() -> Result<()> {
        struct Cancel;
        impl Observer for Cancel {
            fn progress(&mut self, _: &str) -> Result<()> { bail!("cancelled") }
            fn save(&mut self, _: &Value) -> Result<()> { panic!("unexpected checkpoint before first response") }
            fn metrics(&mut self, _: &Value) -> Result<()> { panic!("unexpected response") }
        }
        let folder = tempfile::tempdir()?;
        let image = folder.path().join("current.pgm");
        std::fs::write(&image, b"P5\n1 1\n255\n\x00")?;
        for mode in ["blocks", "dialogues", "vision"] {
            let (service, server) = mock_service(vec![json!({"data":[{"id":"new"}]})])?;
            let options = Options { model:"new", unit:mode, block_id:None, batch_size:8, image:None };
            let mut page = json!({"width":100,"height":100,"model":"old","translation_config":{"model":"old"},
                "blocks":[{"id":"a","box":[3,3,20,30],"source":"ありがとう","translation":"旧译","vision_source":"旧建议"}]});
            let result = if mode == "vision" {
                crate::vision_pipeline::translate(&mut page, &image, None, None, "new", &service, None, &mut Cancel).await
            } else {
                translate(&mut page, &service, &options, &mut Cancel).await
            };
            server.join().unwrap()?;
            assert_eq!(result.unwrap_err().to_string(), "cancelled");
            assert_eq!(page["translation_config"]["model"], "old");
            assert_eq!(page["blocks"][0]["translation"], "旧译");
            assert_eq!(page["blocks"][0]["vision_source"], "旧建议");
            assert!(page["blocks"][0].get("translation_config_override").is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn partial_bulk_results_keep_old_text_and_resume_only_failed_ids_in_both_modes() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let image = folder.path().join("current.pgm");
        std::fs::write(&image, b"P5\n1 1\n255\n\x00")?;
        for vision in [false, true] {
            let models = json!({"data":[{"id":"new"}]});
            let partial = chat(json!({"translations":{"a":"新一","b":"えっ"},"source_suggestions":{"a":"","b":"无效建议"}}));
            let corrected = chat(json!({"translations":{"b":"新二"},"source_suggestions":{"b":""}}));
            let (service, server) = mock_service(vec![models.clone(), partial, models, corrected])?;
            let options = Options { model:"new", unit:"blocks", block_id:None, batch_size:8, image:None };
            let mut page = json!({"width":100,"height":100,"model":"old","translation_config":{"model":"old"},
                "blocks":[{"id":"a","box":[3,3,20,30],"source":"ありがとう","translation":"旧一"},
                          {"id":"b","box":[60,60,85,95],"source":"えっ","translation":"旧二","error":"",
                           "vision_source":"旧建议","quality_warnings":["旧警示"]},
                          {"id":"m","box":[3,60,25,95],"source":"なぜ","translation":"手改","edited":true}]});
            for retry in [false, true] {
                let mut trace = Trace::default();
                if vision {
                    crate::vision_pipeline::translate(&mut page, &image, None, None, "new", &service, None, &mut trace).await?;
                } else {
                    translate(&mut page, &service, &options, &mut trace).await?;
                }
                assert_eq!(trace.metrics.len(), 1);
                let b = &page["blocks"][1];
                assert_eq!(page["blocks"][0]["translation"], "新一");
                assert_eq!(page["blocks"][2]["translation"], "手改");
                if !retry {
                    assert_eq!(page["model"], "old");
                    assert_eq!(page["blocks"][0]["translation_config_override"]["model"], "new");
                    assert_eq!(b["translation"], "旧二");
                    assert_eq!(b["error"], "");
                    assert_eq!(b["retranslation_error"], translation::ECHO_ERROR);
                    assert_eq!(b["vision_source"], "旧建议");
                    assert_eq!(b["quality_warnings"], json!(["旧警示"]));
                    assert!(b.get("translation_config_override").is_none());
                    assert!(trace.saves.iter().all(|p| p["blocks"][1]["translation"] == "旧二" && p["model"] == "old"));
                    page = trace.saves.pop().unwrap();
                } else {
                    assert_eq!(trace.metrics[0]["batch_ids"], json!(["b"]));
                    assert_eq!(page["model"], "new");
                    assert_eq!(b["translation"], "新二");
                    assert!(b.get("retranslation_error").is_none());
                    assert!(array(&page["blocks"]).iter().all(|b| b.get("translation_config_override").is_none()));
                }
            }
            server.join().unwrap()?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_dialogue_switch_preserves_complete_mapping_until_a_valid_response() -> Result<()> {
        let models = json!({"data":[{"id":"new"}]});
        let (service, server) = mock_service(vec![models.clone(), json!({"error":"unavailable"}), models,
            chat(json!({"translations":{"d1":"新对白"}}))])?;
        let mut page = json!({"width":100,"height":100,"model":"old","translation_config":{"model":"old"},
            "blocks":[{"id":"a","box":[3,3,20,30],"source":"ただいま","translation":"旧对白","error":""}]});
        structure::ensure(&mut page, None)?;
        let d = &mut page["dialogues"][0];
        d["full_translation"] = json!("旧对白");
        d["mapping"] = json!({"a":"旧对白"});
        d["mapping_status"] = json!("single");
        d["translation_config"] = json!({"model":"old"});
        structure::sync(&mut page)?;
        let options = Options { model:"new", unit:"dialogues", block_id:None, batch_size:8, image:None };
        let mut trace = Trace::default();
        translate(&mut page, &service, &options, &mut trace).await?;
        let d = &page["dialogues"][0];
        assert_eq!(page["translation_config"]["model"], "old");
        assert_eq!(d["translation_config"]["model"], "old");
        assert_eq!(d["mapping"], json!({"a":"旧对白"}));
        assert_eq!(d["full_translation"], "旧对白");
        assert_eq!(d["translation_mode"], "dialogue");
        assert_eq!(page["blocks"][0]["error"], "");
        assert!(truth(&page["blocks"][0]["retranslation_error"]));
        translate(&mut page, &service, &options, &mut trace).await?;
        server.join().unwrap()?;
        assert_eq!(page["translation_config"]["model"], "new");
        assert_eq!(page["dialogues"][0]["mapping"], json!({"a":"新对白"}));
        assert_eq!(page["dialogues"][0]["translation_config"], page["translation_config"]);
        assert!(page["blocks"][0].get("retranslation_error").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn unmappable_full_reply_cannot_erase_existing_per_box_translations() -> Result<()> {
        let (service, server) = mock_service(vec![json!({"data":[{"id":"new"}]}),
            chat(json!({"translations":{"d1":"整段译文未分行"}}))])?;
        let mut page = json!({"width":100,"height":100,"model":"old","translation_config":{"model":"old"},
            "blocks":[{"id":"a","box":[3,3,20,30],"source":"ただいま","translation":"旧一","dialogue_group":"pair"},
                      {"id":"b","box":[60,12,85,45],"source":"お帰り","translation":"旧二","dialogue_group":"pair"}]});
        let options = Options { model:"new", unit:"dialogues", block_id:None, batch_size:8, image:None };
        let mut trace = Trace::default();
        translate(&mut page, &service, &options, &mut trace).await?;
        server.join().unwrap()?;
        assert_eq!(trace.metrics.len(), 1);
        assert_eq!(page["blocks"][0]["translation"], "旧一");
        assert_eq!(page["blocks"][1]["translation"], "旧二");
        for b in array(&page["blocks"]) {
            assert!(text(&b["retranslation_error"]).contains("无法对应"));
            assert!(b.get("translation_config_override").is_none());
        }
        assert_eq!(page["translation_config"]["model"], "old");
        Ok(())
    }

    #[tokio::test]
    async fn a_single_target_cannot_discard_another_boxes_only_translation() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let image = folder.path().join("current.pgm");
        std::fs::write(&image, b"P5\n1 1\n255\n\x00")?;
        for vision in [false, true] {
            let mut page = json!({"width":140,"height":140,"model":"old","translation_config":{"model":"old"},
                "blocks":[{"id":"a","box":[3,3,20,30],"source":"ただいま","translation":"","dialogue_group":"pair"},
                          {"id":"b","box":[60,12,85,45],"source":"お帰り","translation":"","dialogue_group":"pair"},
                          {"id":"c","box":[85,90,110,120],"source":"またね","translation":"旧三"}]});
            structure::ensure(&mut page, None)?;
            let group = array(&page["dialogues"]).iter().position(|d| array(&d["block_ids"]).len() == 2).unwrap();
            let d = &mut page["dialogues"][group];
            d["full_translation"] = json!("整段旧对白");
            d["mapping"] = json!({"a":"","b":""});
            d["mapping_status"] = json!("whole_only");
            d["translation_config"] = json!({"model":"old"});
            structure::sync(&mut page)?;
            let original = page.clone();
            let offline = TranslationService::new("omlx", Some("http://127.0.0.1:1/v1"), None)?;
            let mut options = Options { model:"new", unit:"blocks", block_id:Some("a"), batch_size:8, image:None };
            let mut trace = Trace::default();
            let result = if vision {
                crate::vision_pipeline::translate(&mut page, &image, None, None, "new", &offline, Some("a"), &mut trace).await
            } else {
                translate(&mut page, &offline, &options, &mut trace).await
            };
            assert_eq!(result.unwrap_err().to_string(), INCOMPLETE_DIALOGUE);
            assert_eq!(page, original);
            assert!(trace.saves.is_empty());

            // A bulk request may also exclude one member. The independent
            // box still commits and the original dialogue remains readable.
            page["blocks"][1]["keep_original"] = json!(true);
            options.block_id = None;
            let reply = chat(json!({"translations":{"c":"新三"},"source_suggestions":{"c":""}}));
            let (service, server) = mock_service(vec![json!({"data":[{"id":"new"}]}), reply])?;
            if vision {
                crate::vision_pipeline::translate(&mut page, &image, None, None, "new", &service, None, &mut trace).await?;
            } else {
                translate(&mut page, &service, &options, &mut trace).await?;
            }
            server.join().unwrap()?;
            assert_eq!(trace.metrics[0]["batch_ids"], json!(["c"]));
            assert_eq!(page["dialogues"][group]["full_translation"], "整段旧对白");
            assert_eq!(page["dialogues"][group]["translation_mode"], "dialogue");
            assert_eq!(page["blocks"][0]["retranslation_error"], INCOMPLETE_DIALOGUE);
            assert_eq!(page["blocks"][0]["translation"], "");
            assert_eq!(page["blocks"][2]["translation"], "新三");
            assert_eq!(page["model"], "old");
        }
        Ok(())
    }

    #[test]
    fn whole_only_dialogue_can_span_checkpoints_without_losing_its_only_translation() {
        let mut page = json!({"blocks":[{"id":"a","translation":""},{"id":"b","translation":""},{"id":"c","translation":"旧三"}],
            "dialogues":[{"block_ids":["a","b"],"full_translation":"完整旧对白","mapping":{"a":"","b":""},
            "mapping_status":"whole_only","translation_mode":"dialogue"}]});
        let mut replacements = BlockReplacements::new(&page, &[0, 1, 2]);
        let mut updated = HashSet::new();
        replacements.accept(&mut page, 0, json!({"id":"a","translation":"新一"}), &mut updated);
        replacements.accept(&mut page, 2, json!({"id":"c","translation":"新三"}), &mut updated);
        invalidate_dialogues(&mut page, &updated);
        assert_eq!(page["blocks"][0]["translation"], "");
        assert_eq!(page["blocks"][2]["translation"], "新三");
        assert_eq!(page["dialogues"][0]["full_translation"], "完整旧对白");
        updated.clear();
        replacements.accept(&mut page, 1, json!({"id":"b","translation":"新二"}), &mut updated);
        invalidate_dialogues(&mut page, &updated);
        assert_eq!(page["blocks"][0]["translation"], "新一");
        assert_eq!(page["blocks"][1]["translation"], "新二");
        assert!(page["dialogues"][0].get("mapping").is_none());
    }

    #[tokio::test]
    async fn failed_model_lookup_preserves_the_callers_page_in_both_modes() -> Result<()> {
        let original = json!({"width":80,"height":80,"model":"old","translation_config":{"model":"old"},
            "blocks":[{"id":"1","box":[4,4,28,40],"source":"こんにちは","translation":"旧译文"}]});
        for vision in [false, true] {
            for body in [json!({"data":[]}), json!({"data":false})] {
                let (service, server) = mock_service(vec![body])?;
                let mut page = original.clone();
                let mut trace = Trace::default();
                let result = if vision {
                    crate::vision_pipeline::translate(&mut page, Path::new("unused.png"), None, None, "new",
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
