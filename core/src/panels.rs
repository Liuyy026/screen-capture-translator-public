//! Conservative full-span gutter detection. Panel proposals affect reading
//! order only; they never authorize erasure of image pixels.
use crate::{
    compat::{array, digest, text, valid_box},
    imaging,
};
use anyhow::Result;
use opencv::prelude::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

const VERSION: u32 = 1;

struct Detector<'a> {
    pixels: &'a [u8],
    width: usize,
    height: usize,
    boxes: BTreeMap<String, [f64; 4]>,
    leaves: Vec<([usize; 4], Vec<String>)>,
    cuts: Vec<Value>,
}

impl Detector<'_> {
    fn recurse(&mut self, bounds: [usize; 4], ids: Vec<String>, depth: usize) {
        if depth >= 12 || ids.len() < 2 {
            self.leaves.push((bounds, ids));
            return;
        }
        for axis in [1, 0] {
            let extent = if axis == 1 { self.height } else { self.width };
            let minimum = (extent as f64 * 0.004).round_ties_even().max(3.0) as usize;
            let across = 1 - axis;
            let span = bounds[across + 2] - bounds[across];
            let mut runs = Vec::new();
            let mut start = None;
            for position in bounds[axis]..=bounds[axis + 2] {
                let blank = if position == bounds[axis + 2] {
                    false
                } else {
                    let mut dark = 0;
                    let mut light = 0;
                    for other in bounds[across]..bounds[across + 2] {
                        let index = if axis == 1 {
                            position * self.width + other
                        } else {
                            other * self.width + position
                        };
                        let pixel = self.pixels[index];
                        dark += usize::from(pixel <= 24);
                        light += usize::from(pixel >= 245);
                    }
                    dark.max(light) as f64 / span as f64 >= 0.985
                };
                if blank && start.is_none() {
                    start = Some(position);
                }
                if !blank && let Some(lo) = start.take() {
                    runs.push((lo, position));
                }
            }
            // Stable, widest-first then smallest coordinate, matching Python.
            runs.sort_by_key(|&(lo, hi)| (std::cmp::Reverse(hi - lo), lo));
            for (start, end) in runs {
                if end - start < minimum
                    || ((start - bounds[axis]) as f64) < extent as f64 * 0.09
                    || ((bounds[axis + 2] - end) as f64) < extent as f64 * 0.09
                {
                    continue;
                }
                let before: Vec<_> = ids
                    .iter()
                    .filter(|id| self.boxes[*id][axis + 2] <= start as f64)
                    .cloned()
                    .collect();
                let after: Vec<_> = ids
                    .iter()
                    .filter(|id| self.boxes[*id][axis] >= end as f64)
                    .cloned()
                    .collect();
                if before.is_empty() || after.is_empty() || before.len() + after.len() != ids.len()
                {
                    continue;
                }
                let mid = (start + end) / 2;
                let mut a = bounds;
                let mut b = bounds;
                a[axis + 2] = mid;
                b[axis] = mid;
                self.cuts.push(json!({"axis": if axis == 1 { "horizontal" } else { "vertical" }, "start": start, "end": end, "bounds": bounds}));
                if axis == 1 {
                    self.recurse(a, before, depth + 1);
                    self.recurse(b, after, depth + 1);
                } else {
                    self.recurse(b, after, depth + 1);
                    self.recurse(a, before, depth + 1);
                }
                return;
            }
        }
        self.leaves.push((bounds, ids));
    }
}

pub fn detect(gray: &Mat, blocks: &[Value]) -> Result<(Value, Value)> {
    anyhow::ensure!(
        gray.typ() == opencv::core::CV_8UC1 && gray.is_continuous(),
        "expected contiguous gray image"
    );
    let (width, height) = (gray.cols() as usize, gray.rows() as usize);
    let mut boxes = BTreeMap::new();
    for block in blocks {
        if !block["id"].is_null()
            && let Some(b) = valid_box(&block["box"])
            && b[0] >= 0.0
            && b[1] >= 0.0
            && b[2] <= width as f64
            && b[3] <= height as f64
        {
            boxes.insert(text(&block["id"]), b);
        }
    }
    let ids = boxes.keys().cloned().collect::<Vec<_>>();
    let mut detector = Detector {
        pixels: gray.data_bytes()?,
        width,
        height,
        boxes,
        leaves: vec![],
        cuts: vec![],
    };
    if !ids.is_empty() {
        detector.recurse([0, 0, width, height], ids, 0);
    }
    let method = if detector.cuts.is_empty() {
        "page_fallback"
    } else {
        "image_gutters"
    };
    let panels = detector
        .leaves
        .into_iter()
        .enumerate()
        .map(|(i, (bounds, ids))| {
            json!({"id": format!("p{}", i + 1), "box": bounds, "block_ids": ids,
            "reading_order": i + 1, "method": method, "needs_review": true})
        })
        .collect::<Vec<_>>();
    Ok((json!(panels), json!(detector.cuts)))
}

fn fallback(page: &mut Value, reason: &str) -> bool {
    let changed = page.get("panels").is_some() || page["panel_error"] != reason;
    let object = page.as_object_mut().expect("page object");
    for key in ["panels", "panel_signature", "panel_cuts"] {
        object.remove(key);
    }
    page["panel_error"] = json!(reason);
    changed
}

pub fn ensure(page: &mut Value, image: Option<&Path>) -> Result<bool> {
    anyhow::ensure!(page.is_object(), "expected page object");
    let mut inputs = array(&page["blocks"])
        .iter()
        .map(|b| json!([text(&b["id"]), b["box"]]))
        .collect::<Vec<_>>();
    inputs.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    let source = digest(&json!(inputs), true);
    let old = array(&page["panel_signature"]);
    let Some(path) = image else {
        if old.len() == 3 && old[0] == VERSION && old[1] == source && page.get("panels").is_some() {
            return Ok(false);
        }
        return Ok(fallback(
            page,
            "缺少图像且 OCR 几何已变化，回退到文字框顺序",
        ));
    };
    let gray = match imaging::decode_gray(path) {
        Ok(gray) => gray,
        Err(_) => return Ok(fallback(page, "无法读取图像，回退到文字框顺序")),
    };
    let signature = json!([
        VERSION,
        source,
        format!("{:x}", Sha256::digest(gray.data_bytes()?))
    ]);
    if page["panel_signature"] == signature && page.get("panels").is_some() {
        return Ok(false);
    }
    let (panels, cuts) = detect(&gray, array(&page["blocks"]))?;
    page.as_object_mut().unwrap().remove("panel_error");
    page["panels"] = panels;
    page["panel_cuts"] = cuts;
    page["panel_signature"] = signature;
    Ok(true)
}
