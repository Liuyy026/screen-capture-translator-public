//! Final display regions. OCR and translation caches are immutable inputs.
use crate::{
    background,
    compat::{array, text, truth},
    geometry::{self, Candidate},
    ink::{self, Box4},
    model_images::ImageModels,
    raster::*,
};
use anyhow::{Result, ensure};
use opencv::{
    core::{self, Mat},
    prelude::*,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

fn merge(region: &mut Value, patch: Value) {
    region
        .as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
}

fn union(boxes: &[Box4]) -> Box4 {
    [
        boxes.iter().map(|b| b[0]).min().unwrap(),
        boxes.iter().map(|b| b[1]).min().unwrap(),
        boxes.iter().map(|b| b[2]).max().unwrap(),
        boxes.iter().map(|b| b[3]).max().unwrap(),
    ]
}

pub async fn regions(
    page: &Value,
    gray: &Mat,
    color: Option<&Mat>,
    source: Option<&Mat>,
    mut models: Option<&mut ImageModels>,
) -> Result<Vec<Value>> {
    ensure!(
        page.is_object() && gray.typ() == core::CV_8UC1 && !gray.empty(),
        "invalid page or image"
    );
    let (width, height) = (gray.cols(), gray.rows());
    ensure!(
        page["width"].as_f64() == Some(width as f64)
            && page["height"].as_f64() == Some(height as f64),
        "image size does not match cached OCR coordinates"
    );
    ensure!(
        page.get("blocks").is_none_or(Value::is_array),
        "invalid OCR blocks"
    );
    let blocks = array(&page["blocks"]);
    ensure!(blocks.iter().all(Value::is_object), "invalid OCR blocks");
    let ids: Vec<_> = blocks.iter().map(|b| text(&b["id"])).collect();
    ensure!(
        ids.iter().collect::<HashSet<_>>().len() == ids.len()
            && blocks.iter().all(|b| !b["id"].is_null()),
        "missing or duplicate OCR identifiers"
    );
    if let Some(color) = color {
        ensure!(
            color.typ() == core::CV_8UC3 && color.size()? == gray.size()?,
            "invalid color image"
        );
    }
    let preserved = ink::preserved_markers(page);
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| {
            if preserved.get(text(&b["id"])).is_some() {
                None
            } else {
                ink::clipped_box(b, width, height)
            }
        })
        .collect();
    let (boxes, supplemented) = ink::complete_vertical_strokes(gray, &boxes, blocks, source)?;
    let source = supplemented.as_ref();
    let bubbles = geometry::bubbles(gray, &boxes)?;
    let narrations = geometry::narration_groups(gray, blocks, &boxes, &bubbles)?;
    let mut regions = vec![];
    let mut accepted: Vec<Candidate> = vec![];
    let mut claimed: HashSet<String> = HashSet::new();
    let mut ordered: Vec<_> = bubbles.iter().collect();
    ordered.sort_by_key(|c| c.members.iter().min().copied());
    for candidate in ordered {
        let indexes = &candidate.members;
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        if !indexes.iter().all(|&i| ink::ready(&blocks[i])) {
            continue;
        }
        let Some(inner) = geometry::inner_rectangle(&candidate.mask)? else {
            continue;
        };
        let [x, y] = candidate.origin;
        let mut region = json!({"id":format!("reading-{}",regions.len()+1),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"balloon","overlay_score":candidate.score,"overlay_reason":"阅读显示：已确认封闭气泡，仅清除原文字范围",
            "points":candidate.points,"text_box":[x+inner[0],y+inner[1],x+inner[2],y+inner[3]],
            "erase_points":geometry::erase_polygons(candidate,&boxes)?,"fill_color":candidate.fill_color,
            "vertical":indexes.iter().all(|&i| truth(&blocks[i]["vertical"]))});
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&candidate.mask, candidate.origin)),
            candidate.textured.unwrap_or(false),
            source,
        )?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        accepted.push(candidate.clone());
    }
    for candidate in &narrations {
        let mut indexes = candidate.members.clone();
        indexes.sort_by(|&a, &b| {
            blocks[a]["reading_order"]
                .as_f64()
                .unwrap_or(1e9)
                .total_cmp(&blocks[b]["reading_order"].as_f64().unwrap_or(1e9))
                .then(boxes[b].unwrap()[0].cmp(&boxes[a].unwrap()[0]))
        });
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        if !indexes.iter().all(|&i| ink::ready(&blocks[i])) {
            continue;
        }
        let mut region = json!({"id":format!("reading-{}",regions.len()+1),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"narration","overlay_score":0.85,"overlay_reason":"阅读显示：连续旁白横排，保留背景纹理",
            "points":candidate.points,"text_box":candidate.text_box,"erase_points":[],"fill_color":[255,255,255],"vertical":false});
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&candidate.mask, candidate.origin)),
            true,
            source,
        )?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        accepted.push(candidate.clone());
    }
    let by_id: HashMap<_, _> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    for old in array(&page["overlay_regions"]) {
        if !old.is_object()
            || !matches!(old["overlay"].as_str(), Some("balloon" | "rectangle"))
            || array(&old["block_ids"]).is_empty()
        {
            continue;
        }
        let members: Vec<_> = array(&old["block_ids"]).iter().map(text).collect();
        if members.iter().any(|id| {
            claimed.contains(id) || by_id.get(id).is_none_or(|&i| !ink::ready(&blocks[i]))
        }) {
            continue;
        }
        let Some(shape) = geometry::cached_shape(old, width, height)? else {
            continue;
        };
        let mut overlaps = false;
        for other in bubbles.iter().chain(&narrations).chain(&accepted) {
            if geometry::overlap(&shape, other)? > 0.0 {
                overlaps = true;
                break;
            }
        }
        if overlaps || members.iter().any(|id| boxes[by_id[id]].is_none()) {
            continue;
        }
        let member_boxes: Vec<_> = members.iter().map(|id| boxes[by_id[id]].unwrap()).collect();
        let mut region = old.clone();
        let [x, y] = shape.origin;
        let w = shape.mask.cols().min(width - x);
        let h = shape.mask.rows().min(height - y);
        let textured =
            w > 0 && h > 0 && fraction(crop(gray, x, y, w, h)?.data_bytes()?, |v| v < 210) > 0.18;
        let clip = (old["overlay"] == "balloon").then_some((&shape.mask, shape.origin));
        let plan = background::prepare(gray, &member_boxes, clip, textured, source)?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        claimed.extend(members);
        accepted.push(shape);
    }
    let mut units = vec![];
    let mut grouped = HashSet::new();
    for dialogue in array(&page["dialogues"]) {
        let keys = array(&dialogue["block_ids"]);
        if !(2..=4).contains(&keys.len())
            || keys.iter().any(|k| {
                k.as_str().is_none_or(|id| {
                    !by_id.contains_key(id) || claimed.contains(id) || grouped.contains(id)
                })
            })
        {
            continue;
        }
        let indexes: Vec<_> = keys.iter().map(|id| by_id[id.as_str().unwrap()]).collect();
        if !indexes
            .iter()
            .all(|&i| boxes[i].is_some() && ink::ready(&blocks[i]))
        {
            continue;
        }
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let [a, b, c, d] = union(&member_boxes);
        let source_area: i32 = member_boxes
            .iter()
            .map(|b| (b[2] - b[0]) * (b[3] - b[1]))
            .sum();
        if ((c - a) * (d - b)) as f64 > source_area as f64 * 1.7 {
            continue;
        }
        grouped.extend(keys.iter().map(|k| k.as_str().unwrap().to_owned()));
        units.push(indexes);
    }
    units.extend(
        (0..blocks.len())
            .filter(|&i| !grouped.contains(&ids[i]))
            .map(|i| vec![i]),
    );
    for indexes in units {
        let index = indexes[0];
        let block = &blocks[index];
        if indexes
            .iter()
            .any(|&i| boxes[i].is_none() || claimed.contains(&ids[i]) || !ink::ready(&blocks[i]))
        {
            continue;
        }
        let source_text: String = indexes
            .iter()
            .map(|&i| blocks[i]["source"].as_str().unwrap_or(""))
            .collect::<String>()
            .nfkc()
            .collect();
        let translated: String = indexes
            .iter()
            .map(|&i| blocks[i]["translation"].as_str().unwrap())
            .collect::<String>()
            .nfkc()
            .collect();
        if source_text.trim() == translated.trim() {
            continue;
        }
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let [a, b, c, d] = union(&member_boxes);
        let [x, y, r, end] = [
            (a - 8).max(0),
            (b - 8).max(0),
            (c + 8).min(width),
            (d + 8).min(height),
        ];
        let mut mask = blank(r - x, end - y, 1)?;
        for other in &accepted {
            let [ox, oy] = other.origin;
            let (ow, oh) = (other.mask.cols(), other.mask.rows());
            let (aa, bb, cc, dd) = (x.max(ox), y.max(oy), r.min(ox + ow), end.min(oy + oh));
            if cc <= aa || dd <= bb {
                continue;
            }
            let target = mask.data_bytes_mut()?;
            let occupied = other.mask.data_bytes()?;
            for yy in bb..dd {
                for xx in aa..cc {
                    target[((yy - y) * (r - x) + xx - x) as usize] &=
                        1 - occupied[((yy - oy) * ow + xx - ox) as usize];
                }
            }
        }
        let Some(shape) = geometry::shape(&mask, [x, y])? else {
            continue;
        };
        let mut covered = true;
        for &bbox in &member_boxes {
            if geometry::coverage(&shape, Some(bbox))? < 0.97 {
                covered = false;
                break;
            }
        }
        if !covered {
            continue;
        }
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&shape.mask, shape.origin)),
            true,
            source,
        )?;
        // This is exactly the alpha channel that the old path checked after
        // PNG encoding; count it directly without a redundant decode.
        if plan
            .byte_mask
            .data_bytes()?
            .iter()
            .filter(|&&v| v > 0)
            .count()
            < 8
        {
            continue;
        }
        let mut region = json!({"id":format!("text-{}",ids[index]),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"text","overlay_score":0.6,"overlay_reason":"原文字范围内排版，仅修复字迹，保留画面",
            "points":shape.points,"text_box":[x,y,r,end],"erase_points":[],"fill_color":[255,255,255],"vertical":truth(&block["vertical"])});
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        accepted.push(shape);
    }
    Ok(regions)
}
