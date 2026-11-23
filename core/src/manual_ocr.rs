//! A selected rectangle is read from the original pixels. The preview never
//! publishes a page. Confirmation is bound to the same source and revision.
use super::{
    PageRevisionChanged, Runtime, cache_source_digest, committed_page, page_source, read_page_image,
};
use crate::{
    compat::{array, keeps_original, valid_box},
    imaging,
    ink::{self, Box4},
    overlay, persistence, structure,
};
use anyhow::{Context, Result, ensure};
use opencv::{core::Mat, imgcodecs, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};

fn crop_box(value: &Value, width: i32, height: i32) -> Result<Box4> {
    let bounds: Box4 = serde_json::from_value(value.clone()).context("选区需要四个整数像素坐标")?;
    let [l, t, r, b] = bounds;
    ensure!(
        l >= 0
            && t >= 0
            && r <= width
            && b <= height
            && i64::from(r) - i64::from(l) >= 4
            && i64::from(b) - i64::from(t) >= 4,
        "请在原图内框选至少 4 × 4 像素的文字"
    );
    Ok(bounds)
}

fn page_snapshot(cache: &Path, revision: Option<&str>) -> Result<Option<Value>> {
    let current = committed_page(cache)?;
    ensure!(
        current.as_ref().map(|(_, hash)| hash.as_str()) == revision,
        PageRevisionChanged
    );
    Ok(current.map(|(page, _)| page))
}

// Older enhanced caches use a larger layout canvas. Read the same physical
// rectangle from the original file, rounding outwards only at pixel edges.
fn original_crop(image: &Mat, bounds: Box4, width: i32, height: i32) -> Result<Mat> {
    let scale = |n: i32, source: i32, canvas: i32, upper: bool| {
        let numerator = i64::from(n) * i64::from(source);
        ((numerator + if upper { i64::from(canvas) - 1 } else { 0 }) / i64::from(canvas)) as i32
    };
    let l = scale(bounds[0], image.cols(), width, false);
    let r = scale(bounds[2], image.cols(), width, true);
    let t = scale(bounds[1], image.rows(), height, false);
    let b = scale(bounds[3], image.rows(), height, true);
    crate::raster::crop(image, l, t, r - l, b - t)
}

fn new_id(blocks: &[Value]) -> String {
    let ids: HashSet<_> = blocks.iter().filter_map(|b| b["id"].as_str()).collect();
    if let Some(id) = ids
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .checked_add(1)
    {
        return id.to_string();
    }
    (1..)
        .map(|n| format!("manual-{n}"))
        .find(|id| !ids.contains(id.as_str()))
        .unwrap()
}

/// Completing a previously untouched page must retain its confirmed blocks,
/// including their IDs, corrections and translations. Discard newly detected
/// boxes that substantially repeat those manually selected source pixels.
pub(super) fn finish_partial_page(page: &mut Value, detected: &Value) -> Result<()> {
    ensure!(
        page["width"] == detected["width"] && page["height"] == detected["height"],
        "自动识别和手动选区的页面尺寸不同"
    );
    let (width, height) = (
        page["width"].as_i64().unwrap() as i32,
        page["height"].as_i64().unwrap() as i32,
    );
    let blocks = page["blocks"].as_array_mut().context("页面缺少文字块")?;
    let manual: Vec<_> = blocks
        .iter()
        .flat_map(|block| ink::source_boxes(block, width, height))
        .collect();
    for candidate in array(&detected["blocks"]) {
        let Some([l, t, r, b]) = ink::clipped_box(candidate, width, height) else {
            continue;
        };
        let area = i64::from(r - l) * i64::from(b - t);
        let overlaps = manual.iter().any(|o| {
            let w = (r.min(o[2]) - l.max(o[0])).max(0);
            let h = (b.min(o[3]) - t.max(o[1])).max(0);
            i64::from(w) * i64::from(h) * 2 >= area
        });
        if !overlaps {
            let mut block = candidate.clone();
            block["id"] = json!(new_id(blocks));
            blocks.push(block);
        }
    }
    page["ocr"] = detected["ocr"].clone();
    page.as_object_mut()
        .unwrap()
        .remove("manual_ocr_partial_page");
    Ok(())
}

fn apply_selection(page: &mut Value, request: &Value, bounds: Box4) -> Result<String> {
    let source = request["source"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("请先确认选区日文")?;
    let translation = request
        .get("translation")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .context("请填写确认后的整句译文")
        })
        .transpose()?;
    let placement = request["placement"]
        .as_str()
        .context("请选择合并位置或新增对白")?;
    let (width, height) = (
        page["width"].as_i64().unwrap() as i32,
        page["height"].as_i64().unwrap() as i32,
    );
    let blocks = page["blocks"].as_array_mut().context("页面缺少文字块")?;
    let block = match placement {
        "new" => {
            ensure!(request["block_id"].is_null(), "新增对白时请勿指定合并目标");
            let id = new_id(blocks);
            blocks.push(json!({"id":id,"source":source,"translation":"","error":"","edited":false,
                "vertical":(bounds[3]-bounds[1]) >= (bounds[2]-bounds[0]), "box":bounds,"source_boxes":[bounds]}));
            blocks.last_mut().unwrap()
        }
        "prepend" | "append" => {
            let id = request["block_id"].as_str().context("缺少合并目标")?;
            let block = blocks
                .iter_mut()
                .find(|b| b["id"] == id)
                .context("未找到要合并的对白")?;
            ensure!(!keeps_original(block), "请先恢复这个对话框的翻译再合并");
            let previous = block["source"].as_str().context("原对白文本无效")?;
            ensure!(
                ink::clipped_box(block, width, height).is_some(),
                "原对白坐标无效，请新增为独立对白"
            );
            let previous_bounds = valid_box(&block["box"]).context("原对白坐标无效")?;
            let mut original_boxes = block["source_boxes"]
                .as_array()
                .filter(|boxes| !boxes.is_empty() && boxes.iter().all(|b| valid_box(b).is_some()))
                .cloned()
                .unwrap_or_else(|| vec![block["box"].clone()]);
            let original_anchor = block["box"].clone();
            if placement == "prepend" {
                block["source"] = json!(format!("{source}{previous}"));
                original_boxes.insert(0, json!(bounds));
            } else {
                block["source"] = json!(format!("{previous}{source}"));
                original_boxes.push(json!(bounds));
            }
            block
                .as_object_mut()
                .unwrap()
                .entry("typography_anchor")
                .or_insert(original_anchor);
            block["box"] = json!([
                previous_bounds[0].min(f64::from(bounds[0])),
                previous_bounds[1].min(f64::from(bounds[1])),
                previous_bounds[2].max(f64::from(bounds[2])),
                previous_bounds[3].max(f64::from(bounds[3]))
            ]);
            block["source_boxes"] = json!(original_boxes);
            if translation.is_none()
                && block["translation"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
            {
                block["source_translation_pending"] = json!(true);
            }
            block
        }
        _ => anyhow::bail!("未知的合并位置"),
    };
    // Only explicitly selected areas authorize extra ink, including where the
    // automatic detector returned an empty mask.
    if block["manual_ocr_boxes"].is_null() {
        block["manual_ocr_boxes"] = json!([]);
    }
    block["manual_ocr_boxes"]
        .as_array_mut()
        .context("已保存的选区坐标无效")?
        .push(json!(bounds));
    block["ocr_confirmed"] = json!(true);
    block["error"] = json!("");
    let object = block.as_object_mut().unwrap();
    for key in ["quality_warnings", "vision_source", "retranslation_error"] {
        object.remove(key);
    }
    if let Some(translation) = translation {
        object.remove("source_translation_pending");
        object.remove("translation_config_override");
        block["translation"] = json!(translation);
        block["edited"] = json!(true);
    }
    Ok(block["id"].as_str().unwrap().to_owned())
}

// A changed OCR input invalidates the previous whole-dialogue result. Keep
// that source, mapping and translation available for recovery if regrouping
// invalidates a legacy result that cannot be reconstructed from its blocks.
fn retain_previous_dialogues(page: &mut Value, previous: &[Value], changed_id: &str) -> Result<()> {
    let groups = page["dialogues"]
        .as_array_mut()
        .context("页面缺少对白分组")?;
    let mut displaced = Vec::new();
    let mut restored_legacy = false;
    for old in previous {
        let unchanged = groups.iter_mut().find(|d| {
            let same_input = match old["input_signature"].as_str() {
                Some(signature) => d["input_signature"] == signature,
                None => {
                    old["source"].is_string()
                        && d["source"] == old["source"]
                        && !array(&old["block_ids"]).iter().any(|id| id == changed_id)
                }
            };
            same_input && d["block_ids"] == old["block_ids"]
        });
        if let Some(group) = unchanged {
            restored_legacy |= old["input_signature"].is_null()
                && old["full_translation"].is_string()
                && group.get("full_translation").is_none();
            // Rebuilt structure owns the current derived fields. Preserve
            // extra metadata from older/newer clients on the same input.
            for (key, value) in old.as_object().context("原对白分组格式无效")? {
                group
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        } else if old["full_translation"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        {
            displaced.push(old.clone());
        }
    }
    if !displaced.is_empty() {
        let archive = page
            .as_object_mut()
            .unwrap()
            .entry("prior_dialogue_translations")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("历史对白译文格式无效")?;
        for group in displaced {
            if !archive.contains(&group) {
                archive.push(group);
            }
        }
    }
    if restored_legacy {
        structure::sync(page)?;
    }
    Ok(())
}

impl Runtime {
    pub async fn ocr_crop(&self, request: &Value) -> Result<Value> {
        let source = Path::new(request["image"].as_str().context("缺少原图路径")?);
        let cache = Path::new(request["cache"].as_str().context("缺少页面缓存路径")?);
        ensure!(
            cache.parent() == Some(self.cache_root.as_path())
                || cache.parent() == Some(self.cache_root.join("vision-v1").as_path()),
            "选区只能保存到当前页面缓存"
        );
        let revision = request
            .get("revision")
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().context("无效页面版本"))
            .transpose()?;
        let _guard = self.cache_policy.lock_page(cache).await?;
        let original = read_page_image(source, cache)?;
        let color = imaging::decode_bytes(&original, imgcodecs::IMREAD_COLOR)?;
        let mut page = page_snapshot(cache, revision)?.unwrap_or_else(|| {
            json!({
            "version":2,"width":color.cols(),"height":color.rows(),"blocks":[],"model":"",
            "ocr":"manual / manga-ocr","manual_ocr_partial_page":true})
        });
        let width = page["width"].as_i64().unwrap() as i32;
        let height = page["height"].as_i64().unwrap() as i32;
        ensure!(
            (width == color.cols() && height == color.rows())
                || (page["layout_provenance"]["version"] == 1
                    && page["layout_provenance"]["source_sha256"] == cache_source_digest(cache)?),
            "页面缓存和原图尺寸不同，请重新载入"
        );
        let bounds = crop_box(&request["box"], width, height)?;
        if request["op"] == "ocr_crop_preview" {
            let crop = original_crop(&color, bounds, width, height)?;
            let text = self.models.lock().await.ocr_crop(&crop).await?;
            read_page_image(source, cache)?;
            page_snapshot(cache, revision)?;
            return Ok(json!({"source":text, "box":bounds, "revision":revision}));
        }

        let previous_groups = array(&page["dialogues"]).to_vec();
        let id = apply_selection(&mut page, request, bounds)?;
        let canvas = page_source::source_canvas(&page, &original)?;
        let gray = if matches!(&canvas, std::borrow::Cow::Borrowed(_)) {
            imaging::color_to_gray(&color)?
        } else {
            imaging::color_to_gray(&imaging::decode_bytes(&canvas, imgcodecs::IMREAD_COLOR)?)?
        };
        crate::ocr_review::ensure_for_source(&mut page, cache_source_digest(cache)?, |_| {
            Ok(gray.try_clone()?)
        })?;
        overlay::ensure_gray(&mut page, Some(&gray))?;
        crate::panels::ensure_gray(&mut page, &gray)?;
        structure::ensure(&mut page, None)?;
        retain_previous_dialogues(&mut page, &previous_groups, &id)?;
        crate::translation_cache::mark_manual(&mut page);
        let encoded = serde_json::to_vec_pretty(&page)?;
        read_page_image(source, cache)?;
        page_snapshot(cache, revision)?;
        persistence::atomic_bytes(cache, &encoded)?;
        Ok(
            json!({"cache":cache,"page":page,"revision":format!("{:x}",Sha256::digest(&encoded)),"block_id":id}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_legacy_canvas_crops_exact_original_pixels_without_upscaling() -> Result<()> {
        let mut image = crate::raster::blank(12, 18, 45)?;
        *image.at_2d_mut::<u8>(3, 2)? = 123;
        let crop = original_crop(&image, [5, 7, 15, 20], 24, 36)?;
        assert_eq!((crop.cols(), crop.rows()), (6, 7));
        assert_eq!(*crop.at_2d::<u8>(0, 0)?, 123);
        Ok(())
    }

    #[test]
    fn merge_retains_order_manual_text_and_exact_type_anchor() -> Result<()> {
        let mut page = json!({"width":128,"height":100,"blocks":[
            {"id":"7","box":[60.25,13.75,80.4,62.25],"source":"中","translation":"手工译文",
                "edited":true,"custom":"留存","translation_config_override":{"model":"earlier"}},
            {"id":"9","box":[90,10,115,60],"source":"別","translation":"另一句","keep_original":true}]});
        let original = page["blocks"][0].clone();
        let untouched = page["blocks"][1].clone();
        let before = [82, 12, 91, 50];
        let after = [38, 15, 58, 75];
        let prepend = json!({"placement":"prepend","block_id":"7","source":"前"});
        let append = json!({"placement":"append","block_id":"7","source":"後"});
        apply_selection(&mut page, &prepend, before)?;
        apply_selection(&mut page, &append, after)?;
        let block = &page["blocks"][0];
        assert_eq!(block["source"], "前中後");
        assert_eq!(
            block["source_boxes"],
            json!([before, original["box"], after])
        );
        assert_eq!(block["typography_anchor"], original["box"]);
        assert_eq!(
            block["translation_config_override"],
            original["translation_config_override"]
        );
        assert_eq!(block["translation"], original["translation"]);
        assert_eq!(block["custom"], "留存");
        assert_eq!(block["edited"], true);
        assert_eq!(block["source_translation_pending"], true);
        assert!(!ink::ready(block));
        assert!(ink::preserved_markers(&page).get("7").is_some());
        assert_eq!(page["blocks"][1], untouched);
        let confirmed = json!({"placement":"append","block_id":"7","source":"！","translation":"确认后的整句译文"});
        apply_selection(&mut page, &confirmed, [10, 10, 20, 40])?;
        let block = &page["blocks"][0];
        assert!(ink::ready(block));
        assert!(
            block.get("source_translation_pending").is_none()
                && block.get("translation_config_override").is_none()
        );
        assert_eq!(block["typography_anchor"], original["box"]);
        Ok(())
    }

    #[test]
    fn stale_machine_text_becomes_eligible_and_only_success_can_replace_it() -> Result<()> {
        let config = json!({"model":"local"});
        let mut page = json!({"width":128,"height":100,"blocks":[
            {"id":"7","box":[60,15,80,65],"source":"本","translation":"旧机译","edited":false}]});
        apply_selection(
            &mut page,
            &json!({"placement":"append","source":"屋","block_id":"7"}),
            [40, 15, 59, 60],
        )?;
        let block = &mut page["blocks"][0];
        assert!(crate::translate_pipeline::needs_translation(
            block, &config, false
        ));
        crate::translate_pipeline::record_failure(block, &json!("服务器离线"));
        assert_eq!(block["translation"], "旧机译");
        assert!(!ink::ready(block));
        crate::translate_pipeline::accept_block(block, &json!("书店"), &config, false);
        assert!(ink::ready(block));
        assert!(!crate::translate_pipeline::needs_translation(
            block, &config, false
        ));
        Ok(())
    }

    #[test]
    fn manual_first_page_keeps_confirmed_id_when_full_detector_adds_other_lines() -> Result<()> {
        let mut page = json!({"width":100,"height":128,"blocks":[],"manual_ocr_partial_page":true});
        apply_selection(
            &mut page,
            &json!({"placement":"new","source":"先生","translation":"老师"}),
            [30, 35, 48, 96],
        )?;
        let original = page["blocks"][0].clone();
        let detected = json!({"width":100,"height":128,"ocr":"test","blocks":[
            {"id":"99","box":[31,36,48,92],"source":"竟","translation":""},
            {"id":"1","box":[55,45,77,92],"source":"ありがとう","translation":""}]});
        finish_partial_page(&mut page, &detected)?;
        assert_eq!(array(&page["blocks"]).len(), 2);
        assert_eq!(page["blocks"][0], original);
        assert_eq!(page["blocks"][1]["id"], "2");
        assert!(page.get("manual_ocr_partial_page").is_none());
        Ok(())
    }

    #[test]
    fn whole_dialogues_survive_regrouping_without_reusing_a_stale_translation() -> Result<()> {
        let stable = json!({"id":"d1","block_ids":["1","2"],"input_signature":"same","full_translation":"整句一",
            "mapping_status":"whole_only","mapping":{"1":"整句一","2":""},"legacy_metadata":{"custom":4}});
        let displaced = json!({"id":"d2","block_ids":["3"],"input_signature":"old","full_translation":"旧整句"});
        let mut page = json!({"dialogues":[{"block_ids":["1","2"],"input_signature":"same","id":"d1"},
            {"block_ids":["3","4"],"input_signature":"changed","id":"d2"}]});
        retain_previous_dialogues(&mut page, &[stable.clone(), displaced.clone()], "3")?;
        assert_eq!(page["dialogues"][0], stable);
        assert!(page["dialogues"][1].get("full_translation").is_none());
        assert_eq!(page["prior_dialogue_translations"], json!([displaced]));
        let mut legacy = json!({"blocks":[{"id":"8","source":"猫","translation":"猫咪"}],
            "dialogues":[{"id":"d1","block_ids":["8"],"source":"猫","input_signature":"modern"}]});
        let old = json!({"id":"d1","block_ids":["8"],"source":"猫","full_translation":"猫咪","mapping":{"8":"猫咪"}});
        retain_previous_dialogues(&mut legacy, &[old], "3")?;
        assert_eq!(legacy["dialogues"][0]["full_translation"], "猫咪");
        assert_eq!(legacy["dialogues"][0]["translation_mode"], "dialogue");
        Ok(())
    }

    #[tokio::test]
    async fn first_page_requires_an_absent_revision_and_has_atomic_stale_rejection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let worker = root.join("core/python/worker.py");
        std::fs::create_dir_all(worker.parent().unwrap())?;
        std::fs::write(&worker, "")?;
        let runtime = Runtime::new(
            &root.join("pages"),
            &root.join("display"),
            None,
            Path::new("python3"),
            &worker,
            root,
            None,
        )?;
        let image = root.join("page.pgm");
        std::fs::write(
            &image,
            [b"P5\n36 36\n255\n".as_slice(), &[250; 1296]].concat(),
        )?;
        let cache = runtime.cache_for(&image, "text")?;
        let mut req = json!({"op":"ocr_crop_apply","image":image,"cache":cache,"box":[8,4,14,29],
            "placement":"new","source":"確認"});
        let first = runtime.ocr_crop(&req).await?;
        assert!(cache.is_file());
        assert_eq!(first["page"]["cache_retention"]["kind"], "manual");
        assert_eq!(first["block_id"], "1");
        let bytes = std::fs::read(&cache)?;
        assert!(
            runtime
                .ocr_crop(&req)
                .await
                .unwrap_err()
                .is::<PageRevisionChanged>()
        );
        assert_eq!(std::fs::read(&cache)?, bytes);
        req["revision"] = first["revision"].clone();
        req["box"] = json!([-1, 25, 31, 30]);
        assert!(runtime.ocr_crop(&req).await.is_err());
        assert_eq!(std::fs::read(&cache)?, bytes);
        Ok(())
    }
}
