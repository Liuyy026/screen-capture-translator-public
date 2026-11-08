//! OCR grouping and translation synchronization, ported without changing the
//! accepted ordering rules. Geometry proposals never authorize image painting.
use crate::{
    compat::{array, digest, string_or_empty, text, truth, valid_box},
    panels,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};
use unicode_normalization::UnicodeNormalization;

const VERSION: u32 = 5;

fn nfkc(source: &Value) -> String {
    string_or_empty(source).nfkc().collect()
}
// Python's Unicode whitespace includes these four ASCII information separators.
fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
fn trim(s: &str) -> &str {
    s.trim_matches(whitespace)
}

pub fn classify(source: &Value) -> &'static str {
    let normalized = nfkc(source);
    let value = trim(&normalized);
    if value.is_empty() {
        return "noise";
    }
    if !value.chars().any(|c| {
        c.is_alphanumeric()
            || c == '_'
            || ('\u{3040}'..='\u{30ff}').contains(&c)
            || ('\u{4e00}'..='\u{9fff}').contains(&c)
    }) {
        return "utterance_candidate";
    }
    if value.chars().all(|c| {
        ('ァ'..='ヶ').contains(&c) || "ーッっぁぃぅぇぉ…！？!?,、。".contains(c) || whitespace(c)
    }) {
        return "sfx_candidate";
    }
    if value.chars().any(|c| c.is_ascii_alphabetic())
        && !value
            .chars()
            .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))
    {
        return "page_text_candidate";
    }
    "dialogue"
}

fn ocr_signature(page: &Value) -> String {
    let mut blocks = array(&page["blocks"])
        .iter()
        .map(|b| json!([text(&b["id"]), b["box"], b["source"], truth(&b["vertical"])]))
        .collect::<Vec<_>>();
    blocks.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    digest(&json!([page["width"], page["height"], blocks]), false)
}

fn structure_signature(page: &Value) -> String {
    let regions = array(&page["overlay_regions"])
        .iter()
        .filter(|r| score(r) > 0.5)
        .map(|r| {
            let mut result = json!({});
            for key in ["id", "block_ids", "vertical", "points", "text_box"] {
                result[key] = r[key].clone();
            }
            result
        })
        .collect::<Vec<_>>();
    let keys = array(&page["blocks"])
        .iter()
        .map(|b| json!([text(&b["id"]), b["dialogue_group"], b["text_kind_override"]]))
        .collect::<Vec<_>>();
    digest(
        &json!([
            VERSION,
            ocr_signature(page),
            regions,
            page.get("panels").unwrap_or(&json!([])),
            keys
        ]),
        false,
    )
}

fn score(value: &Value) -> f64 {
    value
        .get("overlay_score")
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
}
fn rect(value: &Value) -> [f64; 4] {
    valid_box(&value["box"]).expect("validated geometry")
}

pub fn reading_order(items: Vec<Value>, vertical: bool, page: bool) -> Vec<Value> {
    if items.len() < 2 {
        return items;
    }
    for axis in if page || vertical { [0, 1] } else { [1, 0] } {
        let mut sorted = items.clone();
        sorted.sort_by(|a, b| {
            rect(a)[axis]
                .total_cmp(&rect(b)[axis])
                .then_with(|| text(&a["id"]).cmp(&text(&b["id"])))
        });
        let mut bands: Vec<(f64, Vec<Value>)> = Vec::new();
        for item in sorted {
            let b = rect(&item);
            match bands.last_mut() {
                Some((hi, band)) if b[axis] < *hi => {
                    *hi = hi.max(b[axis + 2]);
                    band.push(item);
                }
                _ => bands.push((b[axis + 2], vec![item])),
            }
        }
        if bands.len() > 1 {
            if axis == 0 && (page || vertical) {
                bands.reverse();
            }
            return bands
                .into_iter()
                .flat_map(|(_, band)| reading_order(band, vertical, page))
                .collect();
        }
    }
    let mut sorted = items;
    let key = |v: &Value| {
        let b = rect(v);
        let (x, y) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        if vertical || page { (-x, y) } else { (y, x) }
    };
    sorted.sort_by(|a, b| {
        let x = key(a);
        let y = key(b);
        x.0.total_cmp(&y.0)
            .then(x.1.total_cmp(&y.1))
            .then_with(|| text(&a["id"]).cmp(&text(&b["id"])))
    });
    sorted
}

fn duplicate(a: &Value, b: &Value) -> bool {
    let normalized = |v: &Value| {
        nfkc(&v["source"])
            .chars()
            .filter(|c| !whitespace(*c))
            .collect::<String>()
    };
    let s = normalized(a);
    if s.is_empty() || s != normalized(b) {
        return false;
    }
    let (x, y) = (rect(a), rect(b));
    let area =
        (x[2].min(y[2]) - x[0].max(y[0])).max(0.0) * (x[3].min(y[3]) - x[1].max(y[1])).max(0.0);
    area / ((x[2] - x[0]) * (x[3] - x[1])).min((y[2] - y[0]) * (y[3] - y[1])) >= 0.85
}

fn near(a: &Value, b: &Value, vertical: bool) -> bool {
    let (x, y) = (rect(a), rect(b));
    let (along, across) = if vertical { (1, 0) } else { (0, 1) };
    let overlap = x[along + 2].min(y[along + 2]) - x[along].max(y[along]);
    let length = (x[along + 2] - x[along]).min(y[along + 2] - y[along]);
    let thickness = (x[across + 2] - x[across]).min(y[across + 2] - y[across]);
    let gap = (x[across].max(y[across]) - x[across + 2].min(y[across + 2])).max(0.0);
    overlap >= 0.55 * length && gap <= (0.75 * thickness).max(4.0)
}

struct Groups<'a> {
    blocks: &'a [Value],
    by_id: &'a HashMap<String, usize>,
    panel: &'a HashMap<String, Value>,
    used: HashSet<String>,
    groups: Vec<Value>,
}

impl Groups<'_> {
    fn add(&mut self, ids: Vec<String>, method: &str, region: Option<&Value>) {
        let mut buckets: Vec<(Value, Vec<String>)> = vec![];
        for id in ids {
            let panel = self.panel.get(&id).cloned().unwrap_or(Value::Null);
            if let Some((_, members)) = buckets.iter_mut().find(|(p, _)| *p == panel) {
                members.push(id);
            } else {
                buckets.push((panel, vec![id]));
            }
        }
        for (panel, ids) in buckets {
            let ids: Vec<_> = ids
                .into_iter()
                .filter(|id| self.by_id.contains_key(id) && !self.used.contains(id))
                .collect();
            if ids.is_empty() {
                continue;
            }
            let members: Vec<_> = ids.iter().map(|id| &self.blocks[self.by_id[id]]).collect();
            let vertical = region
                .and_then(|r| r.get("vertical"))
                .map(truth)
                .unwrap_or_else(|| {
                    members.iter().filter(|b| truth(&b["vertical"])).count() * 2 >= members.len()
                });
            let mut bounds = rect(members[0]);
            for member in &members[1..] {
                let b = rect(member);
                for axis in 0..2 {
                    bounds[axis] = bounds[axis].min(b[axis]);
                    bounds[axis + 2] = bounds[axis + 2].max(b[axis + 2]);
                }
            }
            // Preserve integer geometry in serialized cache hashes where possible.
            let mut bbox = vec![];
            for (axis, v) in bounds.iter().enumerate() {
                bbox.push(
                    members
                        .iter()
                        .find_map(|b| (rect(b)[axis] == *v).then(|| b["box"][axis].clone()))
                        .unwrap(),
                );
            }
            self.groups.push(
                json!({"id": ids.iter().min(), "block_ids": ids, "box": bbox,
                "panel_id": panel, "vertical": vertical, "grouping_method": method,
                "overlay_region_id": region.map(|r| text(&r["id"]))}),
            );
            self.used.extend(ids);
        }
    }
}

pub fn build(page: &mut Value) -> Result<()> {
    anyhow::ensure!(page.is_object(), "expected page object");
    let old: HashMap<_, _> = array(&page["dialogues"])
        .iter()
        .map(|d| (text(&d["input_signature"]), d.clone()))
        .collect();
    let mut blocks = array(&page["blocks"]).to_vec();
    let mut by_id = HashMap::new();
    let mut ids = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if !block["id"].is_null() && valid_box(&block["box"]).is_some() {
            let id = text(&block["id"]);
            if !by_id.contains_key(&id) {
                ids.push(id.clone());
            }
            by_id.insert(id, index);
        }
    }
    let mut panels = array(&page["panels"]).to_vec();
    panels.sort_by(|a, b| {
        a["reading_order"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["reading_order"].as_f64().unwrap_or(0.0))
    });
    let panel: HashMap<_, _> = panels
        .iter()
        .flat_map(|p| {
            array(&p["block_ids"])
                .iter()
                .map(|bid| (text(bid), p["id"].clone()))
        })
        .collect();
    for b in &mut blocks {
        let object = b
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("expected block object"))?;
        for key in [
            "dialogue_id",
            "dialogue_order",
            "reading_order",
            "duplicate_of",
            "panel_id",
        ] {
            object.remove(key);
        }
        b["candidate_kind"] = json!(classify(&b["source"]));
        b["text_kind"] = if truth(&b["text_kind_override"]) {
            b["text_kind_override"].clone()
        } else {
            b["candidate_kind"].clone()
        };
    }
    let mut groups = Groups {
        blocks: &blocks,
        by_id: &by_id,
        panel: &panel,
        used: HashSet::new(),
        groups: vec![],
    };
    let mut manual: Vec<(String, Vec<String>)> = vec![];
    for id in &ids {
        let b = &blocks[by_id[id]];
        if truth(&b["dialogue_group"]) {
            let key = text(&b["dialogue_group"]);
            if let Some((_, members)) = manual.iter_mut().find(|(k, _)| *k == key) {
                members.push(id.clone());
            } else {
                manual.push((key, vec![id.clone()]));
            }
        }
    }
    for (_, members) in manual {
        groups.add(members, "manual", None);
    }
    let mut regions = array(&page["overlay_regions"]).to_vec();
    regions.sort_by(|a, b| {
        array(&b["block_ids"])
            .len()
            .cmp(&array(&a["block_ids"]).len())
            .then_with(|| text(&a["id"]).cmp(&text(&b["id"])))
    });
    for region in &regions {
        if score(region) > 0.5 {
            groups.add(
                array(&region["block_ids"]).iter().map(text).collect(),
                "region",
                Some(region),
            );
        }
    }
    let mut remaining: Vec<_> = ids
        .iter()
        .filter(|id| !groups.used.contains(*id))
        .cloned()
        .collect();
    remaining.sort();
    for seed_id in &remaining {
        if groups.used.contains(seed_id) {
            continue;
        }
        let seed = &blocks[by_id[seed_id]];
        let mut members = vec![seed_id.clone()];
        for id in &remaining {
            if id == seed_id || groups.used.contains(id) || panel.get(id) != panel.get(seed_id) {
                continue;
            }
            let b = &blocks[by_id[id]];
            if truth(&b["vertical"]) == truth(&seed["vertical"])
                && members.iter().all(|aid| {
                    let a = &blocks[by_id[aid]];
                    duplicate(a, b)
                        || (a["text_kind"] == b["text_kind"]
                            && near(a, b, truth(&seed["vertical"])))
                })
            {
                members.push(id.clone());
            }
        }
        let method = if members.len() > 1 {
            "geometry_candidate"
        } else {
            "singleton"
        };
        groups.add(members, method, None);
    }
    let all_groups = groups.groups;
    let panel_order: Vec<_> = panels
        .iter()
        .map(|p| p["id"].clone())
        .chain(std::iter::once(Value::Null))
        .collect();
    let ordered: Vec<_> = panel_order
        .iter()
        .flat_map(|pid| {
            reading_order(
                all_groups
                    .iter()
                    .filter(|g| g["panel_id"] == *pid)
                    .cloned()
                    .collect(),
                false,
                true,
            )
        })
        .collect();
    let mut dialogues = vec![];
    let mut ordinal = 0;
    for (index, mut group) in ordered.into_iter().enumerate() {
        let members = reading_order(
            array(&group["block_ids"])
                .iter()
                .map(|id| blocks[by_id[&text(id)]].clone())
                .collect(),
            truth(&group["vertical"]),
            false,
        );
        let mut canonical: Vec<Value> = vec![];
        for b in &members {
            if let Some(a) = canonical
                .iter()
                .find(|a| duplicate(a, b) && !truth(&a["edited"]) && !truth(&b["edited"]))
            {
                blocks[by_id[&text(&b["id"])]]["duplicate_of"] = json!(text(&a["id"]));
            } else {
                canonical.push(b.clone());
            }
        }
        let ids = members.iter().map(|b| text(&b["id"])).collect::<Vec<_>>();
        let signature = digest(
            &json!(
                ids.iter()
                    .map(|id| {
                        let b = &blocks[by_id[id]];
                        json!([id, b["source"], b["box"]])
                    })
                    .collect::<Vec<_>>()
            ),
            false,
        );
        group["id"] = json!(format!("d{}", index + 1));
        group["block_ids"] = json!(ids);
        group["source_block_ids"] =
            json!(canonical.iter().map(|b| text(&b["id"])).collect::<Vec<_>>());
        group["source"] = json!(
            canonical
                .iter()
                .map(|b| trim(&text(b.get("source").unwrap_or(&json!("")))).to_owned())
                .collect::<Vec<_>>()
                .join("\n")
        );
        group["input_signature"] = json!(signature);
        group["needs_review"] = json!(matches!(
            group["grouping_method"].as_str(),
            Some("geometry_candidate" | "singleton")
        ));
        if let Some(previous) = old.get(&signature) {
            for key in [
                "full_translation",
                "mapping",
                "mapping_status",
                "translation_error",
                "translation_config",
                "quality_warnings",
            ] {
                if let Some(value) = previous.get(key) {
                    group[key] = value.clone();
                }
            }
        }
        for (order, id) in ids.iter().enumerate() {
            ordinal += 1;
            let b = &mut blocks[by_id[id]];
            b["dialogue_id"] = group["id"].clone();
            b["dialogue_order"] = json!(order + 1);
            b["reading_order"] = json!(ordinal);
            b["panel_id"] = group["panel_id"].clone();
        }
        dialogues.push(group);
    }
    page["blocks"] = json!(blocks);
    page["dialogues"] = json!(dialogues);
    page["structure"] = json!({"version": VERSION, "signature": structure_signature(page), "ocr_signature": ocr_signature(page),
        "reading_order": dialogues.iter().map(|d| &d["id"]).collect::<Vec<_>>(), "block_count": blocks.len(), "dialogue_count": dialogues.len(),
        "order_method": if panels.is_empty() { "OCR geometry fallback" } else { "image panel gutters, then bubble geometry and internal direction" },
        "panel_order": panels.iter().map(|p| &p["id"]).collect::<Vec<_>>()});
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for b in &blocks {
        *kinds.entry(text(&b["text_kind"])).or_default() += 1;
    }
    page["diagnostics"] = json!({
        "panels": {"count": panels.len(), "unassigned_blocks": blocks.iter().filter(|b| !panel.contains_key(&text(&b["id"]))).count(),
            "reason": page.get("panel_error").cloned().unwrap_or_else(|| json!(if panels.is_empty() { "未确认图像分镜，使用文字框顺序" } else { "" }))},
        "ocr": {"blocks": blocks.len(), "missing_box": blocks.iter().filter(|b| valid_box(&b["box"]).is_none()).count(), "duplicates": blocks.iter().filter(|b| truth(&b["duplicate_of"])).count()},
        "grouping": {"dialogues": dialogues.len(), "panels": panels.len(), "unassigned": blocks.iter().filter(|b| b.get("dialogue_id").is_none()).count(), "review_candidates": dialogues.iter().filter(|d| truth(&d["needs_review"])).count()},
        "text_kinds": kinds});
    sync(page)
}

pub fn ensure(page: &mut Value, image: Option<&Path>) -> Result<()> {
    if image.is_some() || page.get("panel_signature").is_some() {
        panels::ensure(page, image)?;
    }
    if page["structure"]["signature"] != structure_signature(page) {
        build(page)
    } else {
        sync(page)
    }
}

fn valid_block(b: &Value) -> bool {
    let translation = string_or_empty(&b["translation"]);
    let translation = trim(&translation);
    !translation.is_empty()
        && (truth(&b["edited"])
            || (!truth(&b["error"]) && translation != trim(&string_or_empty(&b["source"]))))
}

pub fn sync(page: &mut Value) -> Result<()> {
    let mut blocks = array(&page["blocks"]).to_vec();
    let by_id: HashMap<_, _> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (text(&b["id"]), i))
        .collect();
    for &index in by_id.values() {
        let object = blocks[index]
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("expected block object"))?;
        for key in [
            "layout_fallback",
            "dialogue_translation",
            "dialogue_translation_complete",
            "dialogue_member_ids",
        ] {
            object.remove(key);
        }
    }
    let mut dialogues = array(&page["dialogues"]).to_vec();
    for d in &mut dialogues {
        let members: Vec<_> = array(d.get("source_block_ids").unwrap_or(&d["block_ids"]))
            .iter()
            .filter_map(|id| by_id.get(&text(id)).copied())
            .collect();
        let all: Vec<_> = array(&d["block_ids"])
            .iter()
            .filter_map(|id| by_id.get(&text(id)).copied())
            .collect();
        let mapping = d["mapping"].as_object();
        let member_ids: HashSet<_> = members.iter().map(|&i| text(&blocks[i]["id"])).collect();
        let mapping_ids: HashSet<_> = mapping
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let full_current = valid_block(
            &json!({"source": d["source"], "translation": d["full_translation"], "error": d["translation_error"]}),
        ) && !members.is_empty()
            && all.len() == array(&d["block_ids"]).len()
            && mapping_ids == member_ids
            && all
                .iter()
                .all(|&i| !truth(&blocks[i]["edited"]) && blocks[i]["overlay"] != "skip")
            && members.iter().all(|&i| {
                !truth(&blocks[i]["error"])
                    && mapping.and_then(|m| m.get(&text(&blocks[i]["id"])))
                        == Some(blocks[i].get("translation").unwrap_or(&json!("")))
            });
        let complete = full_current
            || (!members.is_empty() && members.iter().all(|&i| valid_block(&blocks[i])));
        d["translation_complete"] = json!(complete);
        d["translation"] = if full_current {
            d["full_translation"].clone()
        } else {
            json!(
                members
                    .iter()
                    .map(|&i| trim(&string_or_empty(&blocks[i]["translation"])).to_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        for &i in &all {
            blocks[i]["dialogue_translation"] = if full_current {
                d["full_translation"].clone()
            } else {
                json!("")
            };
            blocks[i]["dialogue_translation_complete"] = json!(full_current);
            blocks[i]["dialogue_member_ids"] = json!(array(&d["block_ids"]));
            if full_current && let Some(&origin) = by_id.get(&text(&blocks[i]["duplicate_of"])) {
                blocks[i]["translation"] = blocks[origin]
                    .get("translation")
                    .cloned()
                    .unwrap_or(json!(""));
                blocks[i]["error"] = blocks[origin].get("error").cloned().unwrap_or(json!(""));
            }
            if !complete {
                blocks[i]["layout_fallback"] = json!("block");
            }
        }
        d["translation_mode"] = json!(if full_current { "dialogue" } else { "blocks" });
        if d.get("quality_warnings").is_none() {
            d["quality_warnings"] = json!([]);
        }
    }
    let failed = |d: &Value| {
        truth(&d["translation_error"])
            || array(&d["block_ids"]).iter().any(|id| {
                by_id
                    .get(&text(id))
                    .is_some_and(|&i| truth(&blocks[i]["error"]))
            })
    };
    let failed_count = dialogues.iter().filter(|d| failed(d)).count();
    let assigned: HashSet<_> = dialogues
        .iter()
        .flat_map(|d| array(&d["block_ids"]).iter().map(text))
        .collect();
    let unique: Vec<_> = by_id.values().map(|&i| &blocks[i]).collect();
    if page.get("diagnostics").is_none() {
        page["diagnostics"] = json!({});
    }
    let safe_regions = array(&page["overlay_regions"])
        .iter()
        .filter(|r| matches!(r["overlay"].as_str(), Some("balloon" | "rectangle")))
        .count();
    let diag = &mut page["diagnostics"];
    diag["translation"] = json!({"translated_blocks": unique.iter().filter(|b| valid_block(b)).count(),
        "errors": unique.iter().filter(|b| truth(&b["error"])).count(), "complete_dialogues": dialogues.iter().filter(|d| truth(&d["translation_complete"])).count(),
        "quality_warnings": unique.iter().map(|b| array(&b["quality_warnings"]).len()).sum::<usize>(),
        "dialogue_quality_warnings": dialogues.iter().map(|d| array(&d["quality_warnings"]).len()).sum::<usize>(),
        "failed_dialogues": failed_count, "pending_dialogues": dialogues.iter().filter(|d| !truth(&d["translation_complete"]) && !failed(d)).count()});
    diag["layout"] = json!({"status": "not_measured", "failed_regions": null,
        "uncertain_blocks": unique.iter().filter(|b| b["overlay"] == "uncertain").count(),
        "safe_regions": safe_regions,
        "fallback_dialogues": dialogues.iter().filter(|d| !truth(&d["translation_complete"])).count()});
    diag["failures"] = json!({"ocr": unique.iter().filter(|b| trim(&string_or_empty(&b["source"])).is_empty() || valid_box(&b["box"]).is_none()).count(),
        "grouping": dialogues.iter().filter(|d| !truth(&d["block_ids"])).count() + diag["grouping"]["unassigned"].as_u64().unwrap_or(0) as usize,
        "translation": failed_count + by_id.iter().filter(|(bid, i)| !assigned.contains(*bid) && truth(&blocks[**i]["error"])).count(),
        "layout": null, "grouping_review": dialogues.iter().filter(|d| truth(&d["needs_review"])).count()});
    diag["failure_units"] = json!({"ocr": "blocks with empty source or invalid box; not OCR accuracy",
        "grouping": "empty units plus unassigned blocks", "translation": "failed dialogue units plus unassigned error blocks, deduplicated",
        "layout": "unmeasured until a renderer attempts layout", "grouping_review": "review candidates, not confirmed failures"});
    page["blocks"] = json!(blocks);
    page["dialogues"] = json!(dialogues);
    Ok(())
}
