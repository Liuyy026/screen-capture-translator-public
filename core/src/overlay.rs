//! OCR-side region classification retained for cache/translation compatibility.
//! Final display uses reading.rs; these regions also inform dialogue grouping.
use crate::{
    compat::{array, number, python_json, string_or_empty, text, truth},
    geometry::{self, Candidate},
    imaging,
    ink::Box4,
    raster::*,
    translation,
};
use anyhow::Result;
use opencv::{core::Mat, imgproc, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};

struct Proposal {
    shape: Candidate,
    // Integer vs floating coordinates participate in legacy JSON signatures.
    layout: Value,
    kind: &'static str,
}

fn bounds(block: &Value, w: i32, h: i32) -> Option<Box4> {
    let values = block["box"].as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut b = [0; 4];
    for (i, value) in values.iter().enumerate() {
        let n = number(value)?;
        if !n.is_finite() {
            return None;
        }
        b[i] = n.round_ties_even() as i32;
    }
    b[0] = b[0].max(0);
    b[1] = b[1].max(0);
    b[2] = b[2].min(w);
    b[3] = b[3].min(h);
    (b[2] - b[0] >= 4 && b[3] - b[1] >= 4).then_some(b)
}

fn union(boxes: &[Box4]) -> Box4 {
    [
        boxes.iter().map(|b| b[0]).min().unwrap(),
        boxes.iter().map(|b| b[1]).min().unwrap(),
        boxes.iter().map(|b| b[2]).max().unwrap(),
        boxes.iter().map(|b| b[3]).max().unwrap(),
    ]
}

fn component(gray: &Mat, b: Box4) -> Result<Option<Proposal>> {
    let [x1, y1, x2, y2] = b;
    let (bw, bh) = (x2 - x1, y2 - y1);
    let mx = ((gray.cols() as f64 * 0.22) as i32).min(28.max((bw as f64 * 1.55) as i32));
    let my = ((gray.rows() as f64 * 0.22) as i32).min(28.max((bh as f64 * 1.55) as i32));
    let (left, top, right, bottom) = (
        (x1 - mx).max(0),
        (y1 - my).max(0),
        (x2 + mx).min(gray.cols()),
        (y2 + my).min(gray.rows()),
    );
    let patch = crop(gray, left, top, right - left, bottom - top)?;
    if patch.empty() {
        return Ok(None);
    }
    let level = (percentile(patch.data_bytes()?, 72.) - 8.).clamp(185., 238.) as u8;
    let bright = threshold(&patch, |p| p >= level)?;
    let (labels, stats) = components(&bright)?;
    let label_data = labels.data_typed::<i32>()?;
    let mut best: Option<Proposal> = None;
    for (label, stat) in stats.iter().enumerate().skip(1) {
        let [x, y, w, h, pixels] = *stat;
        if pixels < 30.max((bw as f64 * bh as f64 * 0.22) as i32) {
            continue;
        }
        let mut inside = 0usize;
        for yy in y1 - top..y2 - top {
            for xx in x1 - left..x2 - left {
                inside +=
                    usize::from(label_data[(yy * patch.cols() + xx) as usize] == label as i32);
            }
        }
        let coverage = inside as f64 / (bw * bh) as f64;
        if coverage < 0.58
            || x <= 0
            || y <= 0
            || x + w >= patch.cols() - 1
            || y + h >= patch.rows() - 1
        {
            continue;
        }
        let ratio = pixels as f64 / (bw * bh).max(1) as f64;
        let bbox_ratio = (w * h) as f64 / (bw * bh).max(1) as f64;
        if ratio > 8. || bbox_ratio > 4. || w as f64 > bw as f64 * 2.6 || h as f64 > bh as f64 * 2.6
        {
            continue;
        }
        let mut mask = blank(w, h, 0)?;
        for yy in 0..h {
            for xx in 0..w {
                mask.data_bytes_mut()?[(yy * w + xx) as usize] = u8::from(
                    label_data[((y + yy) * patch.cols() + x + xx) as usize] == label as i32,
                );
            }
        }
        let Some(mut shape) = geometry::shape(&mask, [left + x, top + y])? else {
            continue;
        };
        let contour_area = imgproc::contour_area_def(&shape.contour)?;
        if contour_area < (bw * bh) as f64 * 0.18 {
            continue;
        }
        let fill = (contour_area / (w * h) as f64).min(1.);
        let padded = pad(&shape.mask, 2, false)?;
        let dilated = morph(&padded, 5, true)?;
        let mut ring_mask = dilated.try_clone()?;
        for (r, &p) in ring_mask
            .data_bytes_mut()?
            .iter_mut()
            .zip(padded.data_bytes()?)
        {
            *r -= p;
        }
        let interior_patch = crop(gray, left + x, top + y, w, h)?;
        let exterior = if top + y >= 2
            && left + x >= 2
            && top + y + h + 2 <= gray.rows()
            && left + x + w + 2 <= gray.cols()
        {
            crop(gray, left + x - 2, top + y - 2, w + 4, h + 4)?
        } else {
            pad(&interior_patch, 2, true)?
        };
        let ring_pixels = select(&exterior, &ring_mask)?;
        let interior_pixels = select(&interior_patch, &mask)?;
        if interior_pixels.len() < 20 || ring_pixels.len() < 20 {
            continue;
        }
        let interior = percentile(&interior_pixels, 50.);
        let contrast = ((interior - percentile(&ring_pixels, 25.)) / 90.).clamp(0., 1.);
        let score =
            0.40 * coverage + 0.22 * fill + 0.20 * contrast + 0.18 * (1. - ratio / 8.).max(0.);
        if contrast < 0.45 || score < 0.72 {
            continue;
        }
        let Some(inner) = geometry::inner_rectangle(&shape.mask)? else {
            continue;
        };
        shape.score = score;
        shape.fill_color = [interior.round_ties_even() as i32; 3];
        let layout = json!([
            x + left + inner[0],
            y + top + inner[1],
            x + left + inner[2],
            y + top + inner[3],
        ]);
        if best.as_ref().is_none_or(|p| score > p.shape.score) {
            best = Some(Proposal {
                shape,
                layout,
                kind: "balloon",
            });
        }
    }
    Ok(best)
}

fn rectangle(gray: &Mat, b: Box4) -> Result<Option<Proposal>> {
    let [x1, y1, x2, y2] = b;
    let (px, py) = (
        4.max(((x2 - x1) as f64 * 0.08) as i32),
        4.max(((y2 - y1) as f64 * 0.06) as i32),
    );
    let (l, t, r, b) = (
        (x1 - px).max(0),
        (y1 - py).max(0),
        (x2 + px).min(gray.cols()),
        (y2 + py).min(gray.rows()),
    );
    let patch = crop(gray, l, t, r - l, b - t)?;
    let pixels = patch.data_bytes()?;
    if pixels.len() < 100 {
        return Ok(None);
    }
    let median = percentile(pixels, 50.);
    let spread = percentile(pixels, 90.) - percentile(pixels, 10.);
    if median < 190. || spread > 70. || fraction(pixels, |p| (p as f64) < median - 55.) < 0.008 {
        return Ok(None);
    }
    let mut shape = geometry::shape(&blank(r - l, b - t, 1)?, [l, t])?.expect("rectangle");
    shape.score = 0.52;
    shape.fill_color = [median.round_ties_even() as i32; 3];
    shape.points = vec![[l, t], [r, t], [r, b], [l, b]];
    let coordinate = |edge: i32, pad: i32, sign: i32| {
        if pad as f64 * 0.45 <= 3. {
            json!(edge + sign * 3)
        } else {
            json!(edge as f64 + sign as f64 * (pad as f64 * 0.45))
        }
    };
    Ok(Some(Proposal {
        shape,
        layout: json!([
            coordinate(l, px, 1),
            coordinate(t, py, 1),
            coordinate(r, px, -1),
            coordinate(b, py, -1)
        ]),
        kind: "rectangle",
    }))
}

pub fn adjacent(gray: &Mat, first: Box4, second: Box4, vertical: bool) -> Result<bool> {
    let (mut a, mut b) = (first, second);
    let (gap, limit, axis, strip_bounds);
    if vertical
        && ((a[2].min(b[2]) - a[0].max(b[0])) as f64)
            > 0.5 * ((a[2] - a[0]).min(b[2] - b[0])) as f64
    {
        if a[1] > b[1] {
            std::mem::swap(&mut a, &mut b);
        }
        gap = b[1] - a[3];
        limit = 0.55 * (a[2] - a[0]).min(b[2] - b[0]) as f64;
        axis = 1;
        strip_bounds = [a[0].max(b[0]), a[3], a[2].min(b[2]), b[1]];
    } else if vertical {
        if a[0] > b[0] {
            std::mem::swap(&mut a, &mut b);
        }
        if ((a[3].min(b[3]) - a[1].max(b[1])) as f64) < 0.6 * (a[3] - a[1]).min(b[3] - b[1]) as f64
        {
            return Ok(false);
        }
        gap = b[0] - a[2];
        limit = 0.6 * (a[2] - a[0]).max(b[2] - b[0]) as f64;
        axis = 0;
        strip_bounds = [a[2], a[1].max(b[1]), b[0], a[3].min(b[3])];
    } else {
        if a[1] > b[1] {
            std::mem::swap(&mut a, &mut b);
        }
        if ((a[2].min(b[2]) - a[0].max(b[0])) as f64) < 0.6 * (a[2] - a[0]).min(b[2] - b[0]) as f64
        {
            return Ok(false);
        }
        gap = b[1] - a[3];
        limit = 0.6 * (a[3] - a[1]).max(b[3] - b[1]) as f64;
        axis = 1;
        strip_bounds = [a[0].max(b[0]), a[3], a[2].min(b[2]), b[1]];
    }
    if gap < 0 || gap as f64 > limit.max(6.) {
        return Ok(false);
    }
    if gap == 0 {
        return Ok(true);
    }
    let [l, t, r, b] = strip_bounds;
    if r <= l || b <= t {
        return Ok(false);
    }
    let strip = crop(gray, l, t, r - l, b - t)?;
    let pixels = strip.data_bytes()?;
    if fraction(pixels, |p| p >= 220) < 0.8 {
        return Ok(false);
    }
    let (w, h) = (strip.cols() as usize, strip.rows() as usize);
    let barrier = if axis == 1 {
        (0..h).any(|y| fraction(&pixels[y * w..(y + 1) * w], |p| p < 130) >= 0.65)
    } else {
        (0..w)
            .any(|x| (0..h).filter(|&y| pixels[y * w + x] < 130).count() as f64 / h as f64 >= 0.65)
    };
    Ok(!barrier)
}

fn member_indexes(candidate: &Candidate, boxes: &[Option<Box4>]) -> Result<Vec<usize>> {
    let mut members = vec![];
    for (i, &b) in boxes.iter().enumerate() {
        let v = geometry::coverage(candidate, b)?;
        if v > 0.15 && v < 0.94 {
            return Ok(vec![]);
        }
        if v >= 0.94 {
            members.push(i);
        }
    }
    Ok(members)
}

pub fn ensure(page: &mut Value, image: Option<&Path>) -> Result<bool> {
    // Missing/unreadable image has the same conservative cache behavior as
    // legacy OCR-side classification. Display decoding still fails explicitly.
    let gray = image.and_then(|path| imaging::decode_gray(path).ok());
    ensure_gray(page, gray.as_ref())
}

pub fn ensure_gray(page: &mut Value, gray: Option<&Mat>) -> Result<bool> {
    let gray = gray.filter(|m| !m.empty() && m.channels() == 1);
    let signature_data: Vec<_> = array(&page["blocks"])
        .iter()
        .map(|b| {
            json!([
                b["id"],
                b["box"],
                b["source"],
                b["vertical"],
                !string_or_empty(&b["translation"]).trim().is_empty(),
                truth(&b["error"]),
                truth(&b["edited"])
            ])
        })
        .collect();
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(python_json(&json!(signature_data), false).as_bytes())
    );
    let image_key = gray
        .map(|m| {
            m.data_bytes()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        })
        .transpose()?;
    let signature = json!([8, fingerprint, image_key]);
    if page["overlay_signature"] == signature && page.get("overlay_regions").is_some() {
        return Ok(false);
    }
    let mut blocks = array(&page["blocks"]).to_vec();
    for block in &mut blocks {
        mark(
            block,
            "uncertain",
            0.,
            "未确认封闭且有明显边界的对话框，保留原图",
            None,
        );
    }
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| gray.and_then(|g| bounds(b, g.cols(), g.rows())))
        .collect();
    let mut candidates = vec![];
    if let Some(gray) = gray {
        for (i, &b) in boxes.iter().enumerate() {
            let Some(b) = b else {
                continue;
            };
            let mut candidate = component(gray, b)?;
            if candidate.is_none() && !translation::low_impact(&blocks[i]["source"], false) {
                candidate = rectangle(gray, b)?;
            }
            if let Some(mut p) = candidate {
                p.shape.members = member_indexes(&p.shape, &boxes)?;
                if !p.shape.members.contains(&i) {
                    continue;
                }
                let text_area: i32 = p
                    .shape
                    .members
                    .iter()
                    .map(|&i| {
                        let b = boxes[i].unwrap();
                        (b[2] - b[0]) * (b[3] - b[1])
                    })
                    .sum();
                if p.shape.mask.total() as f64 > 3.5 * text_area as f64 {
                    continue;
                }
                candidates.push(p);
            }
        }
        for first in 0..boxes.len() {
            let Some(a) = boxes[first] else {
                continue;
            };
            for second in first + 1..boxes.len() {
                let Some(b) = boxes[second] else {
                    continue;
                };
                let vertical = truth(&blocks[first]["vertical"]);
                if vertical != truth(&blocks[second]["vertical"])
                    || !adjacent(gray, a, b, vertical)?
                {
                    continue;
                }
                let Some(mut p) = component(gray, union(&[a, b]))? else {
                    continue;
                };
                p.shape.members = member_indexes(&p.shape, &boxes)?;
                if !p.shape.members.contains(&first) || !p.shape.members.contains(&second) {
                    continue;
                }
                let bounds = union(
                    &p.shape
                        .members
                        .iter()
                        .map(|&i| boxes[i].unwrap())
                        .collect::<Vec<_>>(),
                );
                if p.shape.mask.total()
                    > 4 * ((bounds[2] - bounds[0]) * (bounds[3] - bounds[1])) as usize
                {
                    continue;
                }
                candidates.push(p);
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.shape
            .members
            .len()
            .cmp(&a.shape.members.len())
            .then(b.shape.score.total_cmp(&a.shape.score))
    });
    let mut accepted: Vec<Candidate> = vec![];
    let mut assigned = HashSet::new();
    let mut regions = vec![];
    for p in candidates {
        if p.shape.members.iter().any(|i| assigned.contains(i)) {
            continue;
        }
        let mut overlap = false;
        for previous in &accepted {
            if geometry::overlap(&p.shape, previous)? > 0.50 {
                overlap = true;
                break;
            }
        }
        if overlap {
            continue;
        }
        let id = (regions.len() + 1).to_string();
        let reason = if p.kind == "balloon" {
            "已确认封闭边界，使用气泡内部轮廓"
        } else {
            "背景均匀，使用限于文字框的小范围填充"
        };
        let ids: Vec<_> = p
            .shape
            .members
            .iter()
            .map(|&i| text(&blocks[i]["id"]))
            .collect();
        let vertical = p
            .shape
            .members
            .iter()
            .all(|&i| truth(&blocks[i]["vertical"]));
        regions.push(json!({"id":id,"block_ids":ids,"overlay":p.kind,"overlay_score":p.shape.score,"overlay_reason":reason,
            "points":p.shape.points,"text_box":p.layout,"erase_points":geometry::erase_polygons(&p.shape,&boxes)?,"fill_color":p.shape.fill_color,"vertical":vertical}));
        for &i in &p.shape.members {
            assigned.insert(i);
            mark(&mut blocks[i], p.kind, p.shape.score, reason, Some(&id));
        }
        accepted.push(p.shape);
    }
    if let Some(gray) = gray {
        for (i, block) in blocks.iter_mut().enumerate() {
            let Some([x1, y1, x2, y2]) = boxes[i] else {
                continue;
            };
            if assigned.contains(&i) {
                continue;
            }
            if translation::low_impact(&block["source"], false) {
                mark(
                    block,
                    "skip",
                    0.,
                    "无可靠气泡背景的短语气词，保留原图",
                    None,
                );
                continue;
            }
            if string_or_empty(&block["translation"]).trim().is_empty()
                || truth(&block["error"]) && !truth(&block["edited"])
            {
                continue;
            }
            let (px, py) = (
                4.max(((x2 - x1) as f64 * 0.08) as i32),
                4.max(((y2 - y1) as f64 * 0.06) as i32),
            );
            let (l, t, r, b) = (
                (x1 - px).max(0),
                (y1 - py).max(0),
                (x2 + px).min(gray.cols()),
                (y2 + py).min(gray.rows()),
            );
            let patch = crop(gray, l, t, r - l, b - t)?;
            let pixels = patch.data_bytes()?;
            let light: Vec<_> = pixels.iter().copied().filter(|&v| v >= 220).collect();
            if light.is_empty() || light.len() as f64 / (pixels.len() as f64) < 0.68 {
                continue;
            }
            let candidate = geometry::shape(&blank(r - l, b - t, 1)?, [l, t])?.expect("rectangle");
            let mut overlap = false;
            for previous in &accepted {
                if geometry::overlap(&candidate, previous)? > 0.02 {
                    overlap = true;
                    break;
                }
            }
            for (j, &other) in boxes.iter().enumerate() {
                if j != i && geometry::coverage(&candidate, other)? > 0.1 {
                    overlap = true;
                    break;
                }
            }
            if overlap {
                continue;
            }
            let id = (regions.len() + 1).to_string();
            let reason = "未确认气泡边界，使用限制在文字框附近的小范围填充";
            regions.push(json!({"id":id,"block_ids":[text(&block["id"])],"overlay":"rectangle","overlay_score":0.50,"overlay_reason":reason,
                "points":[[l,t],[r,t],[r,b],[l,b]],"text_box":[l as f64+px as f64*0.35,t as f64+py as f64*0.35,r as f64-px as f64*0.35,b as f64-py as f64*0.35],
                "fill_color":vec![percentile(&light,50.).round_ties_even() as i32;3],"vertical":truth(&block["vertical"])}));
            accepted.push(candidate);
            mark(block, "rectangle", 0.50, reason, Some(&id));
        }
    }
    page["blocks"] = json!(blocks);
    page["version"] = json!(number(&page["version"]).unwrap_or(0.).max(2.) as i64);
    page["overlay_regions"] = json!(regions);
    page["overlay_signature"] = signature;
    Ok(true)
}

fn mark(block: &mut Value, kind: &str, score: f64, reason: &str, id: Option<&str>) {
    block["overlay"] = json!(kind);
    block["overlay_score"] = json!(score);
    block["overlay_reason"] = json!(reason);
    block["overlay_region_id"] = json!(id);
}
