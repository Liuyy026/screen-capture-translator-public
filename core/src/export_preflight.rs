//! Compare saved translations without changing page data or generating display regions.
use crate::{
    compat::{array, keeps_original, text, truth, valid_box},
    ink, ocr_review, preserved_bubbles, structure,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy)]
enum Translation<'a> {
    Ready(&'a str),
    Empty,
    Blocked,
}

struct Block<'a> {
    data: &'a Value,
    source: Option<String>,
    bounds: Option<[f64; 4]>,
    translation: Translation<'a>,
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.trim().is_empty())
}

fn skips_translation(block: &Value) -> bool {
    block["overlay"] == "skip" || block["text_kind"] == "noise"
}

fn intersects([l, t, r, b]: ink::Box4, [x, y, right, bottom]: ink::Box4) -> bool {
    l < right && x < r && t < bottom && y < b
}

fn blocks(page: &Value) -> Vec<Block<'_>> {
    let dimension = |field| {
        page[field]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or_default()
    };
    let (width, height) = (dimension("width"), dimension("height"));
    let blocks = array(&page["blocks"]);
    let preserved = ink::preserved_markers(page);
    let mut protected = preserved_bubbles::protected_bounds(page, width, height);
    protected.extend(
        blocks
            .iter()
            .filter(|b| preserved.get(text(&b["id"])).is_some())
            .filter_map(|b| ink::clipped_box(b, width, height)),
    );
    blocks
        .iter()
        .map(|data| {
            let source = data["source"]
                .as_str()
                .and_then(structure::normalized_source);
            let bounds = valid_box(&data["box"]).and_then(|[l, t, r, b]| {
                (width > 0 && height > 0).then(|| {
                    let (w, h) = (f64::from(width), f64::from(height));
                    [l / w, t / h, r / w, b / h]
                })
            });
            let translation = if keeps_original(data)
                || skips_translation(data)
                || preserved.get(text(&data["id"])).is_some()
                || truth(&data["error"])
                || ink::clipped_box(data, width, height)
                    .is_none_or(|area| protected.iter().any(|&p| intersects(area, p)))
            {
                Translation::Blocked
            } else if let Some(translation) = nonempty(&data["translation"]) {
                Translation::Ready(translation)
            } else {
                Translation::Empty
            };
            Block {
                data,
                source,
                bounds,
                translation,
            }
        })
        .collect()
}

fn matches_bounds([l, t, r, b]: [f64; 4], [x, y, right, bottom]: [f64; 4]) -> bool {
    let overlap = (r.min(right) - l.max(x)).max(0.0) * (b.min(bottom) - t.max(y)).max(0.0);
    let combined = (r - l) * (b - t) + (right - x) * (bottom - y) - overlap;
    overlap / combined >= 0.5
}

enum Match {
    None,
    Unique(usize),
    Ambiguous,
}

fn compare_edits(selected: &[Block<'_>], other: &[Block<'_>]) -> Vec<Value> {
    let mut by_source: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, block) in selected.iter().enumerate() {
        if let Some(source) = block.source.as_deref() {
            by_source.entry(source).or_default().push(index);
        }
    }

    // Unedited, hidden, and failed blocks can still make an OCR match ambiguous.
    // Compute uniqueness before filtering to saved manual edits.
    let mut selected_matches = vec![0usize; selected.len()];
    let counterparts: Vec<_> = other
        .iter()
        .map(|block| {
            let candidates = block
                .source
                .as_deref()
                .and_then(|source| by_source.get(source));
            let (Some(candidates), Some(bounds)) = (candidates, block.bounds) else {
                return Match::None;
            };
            let mut counterpart = Match::None;
            for &index in candidates {
                if selected[index]
                    .bounds
                    .is_some_and(|area| matches_bounds(area, bounds))
                {
                    selected_matches[index] += 1;
                    counterpart = match counterpart {
                        Match::None => Match::Unique(index),
                        _ => Match::Ambiguous,
                    };
                }
            }
            counterpart
        })
        .collect();

    other
        .iter()
        .zip(counterparts)
        .filter_map(|(block, counterpart)| {
            let other_translation = nonempty(&block.data["translation"])?;
            if !truth(&block.data["edited"]) {
                return None;
            }
            let (kind, selected_translation) = match counterpart {
                Match::None => ("unmatched", None),
                Match::Unique(index) if selected_matches[index] == 1 => {
                    let target = &selected[index];
                    match (block.translation, target.translation) {
                        (Translation::Ready(a), Translation::Ready(b)) if a.trim() == b.trim() => {
                            return None;
                        }
                        (Translation::Ready(_), Translation::Ready(b)) => (
                            if truth(&target.data["edited"]) {
                                "both_edited"
                            } else {
                                "different"
                            },
                            Some(b),
                        ),
                        (Translation::Ready(_), Translation::Empty) => ("different", None),
                        (_, Translation::Ready(b)) => ("needs_review", Some(b)),
                        _ => ("needs_review", None),
                    }
                }
                _ => ("needs_review", None),
            };
            Some(json!({
                "kind": kind,
                "source": block.data["source"].as_str().unwrap_or_default(),
                "other_translation": other_translation,
                "selected_translation": selected_translation,
            }))
        })
        .collect()
}

pub(crate) fn analyze(selected: &Value, other: Option<&Value>) -> Value {
    let selected_blocks = blocks(selected);
    let edits = other.map_or_else(Vec::new, |other| {
        compare_edits(&selected_blocks, &blocks(other))
    });
    let (mut untranslated, mut failed, mut pending, mut suspect) = (0, 0, 0, 0);
    let mut models = BTreeSet::new();
    let mut unknown_models = 0;
    for block in selected_blocks {
        let data = block.data;
        if keeps_original(data) {
            continue;
        }
        if ocr_review::suppressed(data) {
            suspect += 1;
        } else if truth(&data["source_translation_pending"]) {
            pending += 1;
        } else if !skips_translation(data) {
            if truth(&data["error"]) {
                failed += 1;
            } else if nonempty(&data["translation"]).is_none() {
                untranslated += 1;
            }
        }
        if matches!(block.translation, Translation::Ready(_)) && !truth(&data["edited"]) {
            // Explicit null overrides and old page-level model names cannot
            // establish which model produced the current block translation.
            let config = data
                .get("translation_config_override")
                .unwrap_or(&selected["translation_config"]);
            if let Some(model) = nonempty(&config["model"]) {
                models.insert(model);
            } else {
                unknown_models += 1;
            }
        }
    }
    json!({
        "edits": edits,
        "issues": {"untranslated": untranslated, "failed": failed, "pending": pending, "suspect": suspect},
        "models": models,
        "unknown_models": unknown_models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, source: &str, translation: &str, edited: bool, bounds: [i32; 4]) -> Value {
        json!({"id": id, "source": source, "translation": translation, "edited": edited, "box": bounds})
    }

    fn page(blocks: Vec<Value>) -> Value {
        json!({"width": 128, "height": 128, "blocks": blocks})
    }

    #[test]
    fn comparison_uses_normalized_source_and_geometry_at_the_inclusive_iou_threshold() {
        let selected = page(vec![block(
            "new",
            "日本 Ａ\n語",
            "手改",
            false,
            [16, 16, 32, 48],
        )]);
        let mut other = page(vec![block(
            "old",
            "日本A　語",
            "手改",
            true,
            [32, 8, 96, 24],
        )]);
        other["width"] = json!(256);
        other["height"] = json!(64);
        assert_eq!(analyze(&selected, Some(&other))["edits"], json!([]));

        other["blocks"][0]["box"][2] = json!(97);
        assert_eq!(
            analyze(&selected, Some(&other))["edits"][0]["kind"],
            "unmatched"
        );
    }

    #[test]
    fn changed_or_empty_source_and_split_merge_still_require_review_with_equal_translations() {
        let bounds = [16, 16, 64, 40];
        let other = page(vec![
            block("a", "最初の話", "相同", true, bounds),
            block("b", "\n　", "相同", true, [16, 48, 64, 72]),
            block("c", "次の話", "相同", true, [16, 80, 40, 104]),
        ]);
        let selected = page(vec![
            block("a", "最初", "相同", false, [16, 16, 40, 40]),
            block("new", "の話", "相同", false, [40, 16, 64, 40]),
            block("b", "  ", "相同", false, [16, 48, 64, 72]),
            block("c", "次の話です", "相同", false, [16, 80, 64, 104]),
        ]);
        let result = analyze(&selected, Some(&other));
        assert_eq!(array(&result["edits"]).len(), 3);
        assert!(
            array(&result["edits"])
                .iter()
                .all(|edit| edit["kind"] == "unmatched" && edit["selected_translation"].is_null())
        );
    }

    #[test]
    fn competing_block_on_either_page_prevents_a_match_even_if_ineligible_or_unedited() {
        let original = block("source", "話し声", "手改", true, [16, 16, 40, 40]);
        let selected_block = block("target", "話し声", "手改", false, [16, 16, 40, 40]);
        let mut selected_competitor = block("bad", "話し声", "", false, [16, 16, 40, 40]);
        selected_competitor["error"] = json!("failed");
        let other = page(vec![original.clone()]);
        let selected = page(vec![selected_block.clone(), selected_competitor]);
        assert_eq!(
            analyze(&selected, Some(&other))["edits"][0]["kind"],
            "needs_review"
        );

        let other_competitor = block("auto", "話し声", "旧译", false, [18, 16, 42, 40]);
        let other = page(vec![original, other_competitor]);
        let selected = page(vec![selected_block]);
        let result = analyze(&selected, Some(&other));
        assert_eq!(array(&result["edits"]).len(), 1);
        assert_eq!(result["edits"][0]["kind"], "needs_review");
    }

    #[test]
    fn reports_different_and_both_edited_only_for_reliable_matches() {
        let mut other = page(vec![
            block("a", "一番", "新译", true, [16, 16, 40, 32]),
            block("b", "二番", "新手改", true, [16, 40, 40, 56]),
            block("c", "三番", "原手改", true, [16, 64, 40, 80]),
            block("d", "四番", "自动", false, [16, 88, 40, 104]),
            block("e", "五番", "　", true, [16, 112, 40, 124]),
        ]);
        // Failed retranslation left the previous usable text intact.
        other["blocks"][0]["retranslation_error"] = json!("retry failed");
        let selected = page(vec![
            block("aa", "一番", "旧译", false, [16, 16, 40, 32]),
            block("bb", "二番", "旧手改", true, [16, 40, 40, 56]),
            block("cc", "三番", " ", false, [16, 64, 40, 80]),
        ]);
        assert_eq!(
            analyze(&selected, Some(&other))["edits"],
            json!([
                {"kind":"different", "source":"一番", "other_translation":"新译", "selected_translation":"旧译"},
                {"kind":"both_edited", "source":"二番", "other_translation":"新手改", "selected_translation":"旧手改"},
                {"kind":"different", "source":"三番", "other_translation":"原手改", "selected_translation":null},
            ])
        );
    }

    #[test]
    fn comparison_ignores_outer_whitespace_and_reports_original_text() {
        let bounds = [16, 16, 40, 40];
        let other = page(vec![block("source", "原文です", "　手改\n", true, bounds)]);
        let mut selected = page(vec![block("target", "原文です", " 手改 ", true, bounds)]);
        assert_eq!(analyze(&selected, Some(&other))["edits"], json!([]));

        selected["blocks"][0]["translation"] = json!(" 手\n改 ");
        assert_eq!(
            analyze(&selected, Some(&other))["edits"],
            json!([
                {"kind":"both_edited", "source":"原文です", "other_translation":"　手改\n", "selected_translation":" 手\n改 "},
            ])
        );
    }

    #[test]
    fn saved_protection_pending_failed_or_suppressed_text_cannot_hide_a_warning() {
        let bounds = [16, 16, 40, 40];
        let original = block("source", "原文です", "手改", true, bounds);
        let target = block("target", "原文です", "手改", false, bounds);
        let selected = page(vec![target.clone()]);
        for (key, value) in [
            ("keep_original", json!(true)),
            ("source_translation_pending", json!(true)),
            ("manual_ink_uncertain", json!(true)),
            ("error", json!("failed")),
        ] {
            let mut blocked = original.clone();
            blocked[key] = value.clone();
            let other = page(vec![blocked]);
            let result = analyze(&selected, Some(&other));
            assert_eq!(result["edits"][0]["kind"], "needs_review", "source {key}");
            assert_eq!(result["edits"][0]["other_translation"], "手改");

            let mut blocked = target.clone();
            blocked[key] = value;
            let other = page(vec![original.clone()]);
            let result = analyze(&page(vec![blocked]), Some(&other));
            assert_eq!(result["edits"][0]["kind"], "needs_review", "selected {key}");
            assert!(result["edits"][0]["selected_translation"].is_null());
        }

        let mut suspect = target.clone();
        suspect["ocr_review"] = json!({"status": "suspect"});
        let other = page(vec![original]);
        assert_eq!(
            analyze(&page(vec![suspect.clone()]), Some(&other))["edits"][0]["kind"],
            "needs_review"
        );
        suspect["ocr_confirmed"] = json!(true);
        assert_eq!(
            analyze(&page(vec![suspect]), Some(&other))["edits"],
            json!([])
        );

        let mut saved_bubble = selected;
        saved_bubble["preserved_bubbles"] =
            json!([{"block_ids":["neighbor"], "bounds":[16,16,64,48]}]);
        assert_eq!(
            analyze(&saved_bubble, Some(&other))["edits"][0]["kind"],
            "needs_review"
        );
    }

    #[test]
    fn counts_actual_issues_and_explicit_automatic_provenance() {
        let entries = [
            json!({"translation":"正常"}),
            json!({"translation":"另一模型","translation_config_override":{"model":"B"}}),
            json!({"translation":"新尝试失败了","retranslation_error":"重试失败"}),
            json!({"translation":"未知","translation_config_override":null}),
            json!({"translation":"人工","edited":true,"translation_config_override":{"model":"previous"}}),
            json!({"translation":"已主动保留","keep_original":true,"error":"隐藏"}),
            json!({"translation":"旧译","source_translation_pending":true,"error":"过时错误"}),
            json!({"translation":"误识别","ocr_review":{"status":"suspect"},"error":"过时错误"}),
            json!({"translation":"确认","ocr_review":{"status":"suspect"},"ocr_confirmed":true}),
            json!({"translation":"","overlay":"skip"}),
            json!({"translation":"","text_kind":"noise"}),
            json!({"translation":"","error":"模型失败"}),
            json!({"translation":"　"}),
        ];
        let blocks = entries
            .into_iter()
            .enumerate()
            .map(|(index, mut data)| {
                let y = 8 + 8 * index;
                data["box"] = json!([12, y, 28, y + 6]);
                data["source"] = json!("原文です");
                data["id"] = json!(index.to_string());
                data
            })
            .collect();
        let mut selected = page(blocks);
        selected["translation_config"] = json!({"model":"A"});
        selected["model"] = json!("legacy hint");
        let result = analyze(&selected, None);
        assert_eq!(
            result["issues"],
            json!({"untranslated":1,"failed":1,"pending":1,"suspect":1})
        );
        assert_eq!(result["models"], json!(["A", "B"]));
        assert_eq!(result["unknown_models"], 1);
        assert_eq!(result["edits"], json!([]));

        selected
            .as_object_mut()
            .unwrap()
            .shift_remove("translation_config");
        let legacy = analyze(&selected, None);
        assert_eq!(legacy["models"], json!(["B"]));
        assert_eq!(legacy["unknown_models"], 4);
    }
}
