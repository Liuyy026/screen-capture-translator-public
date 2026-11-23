//! Final display regions. OCR and translation caches are immutable inputs.
use crate::{
    background,
    compat::{array, keeps_original, text, truth},
    geometry::{self, Candidate},
    ink::{self, Box4},
    model_images::ImageModels,
    preserved_bubbles,
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

fn intersects([a, b, c, d]: Box4, [x, y, r, s]: Box4) -> bool {
    a < r && x < c && b < s && y < d
}

fn padded([x, y, r, b]: Box4, pad: i32, width: i32, height: i32) -> Box4 {
    [(x - pad).max(0), (y - pad).max(0), (r + pad).min(width), (b + pad).min(height)]
}

fn trim_around_originals(mut area: Box4, text: Box4, originals: &[Box4]) -> Option<Box4> {
    for &[left, top, right, bottom] in originals {
        if !intersects(area, [left, top, right, bottom]) {
            continue;
        }
        let [x, y, r, b] = area;
        // Keep the translated text's ink bounds outside the preserved frame.
        area = [[x, y, left, b], [right, y, r, b], [x, y, r, top], [x, bottom, r, b]]
            .into_iter()
            .filter(|[a, b, c, d]| *a <= text[0] && *b <= text[1] && *c >= text[2] && *d >= text[3])
            .max_by_key(|[a, b, c, d]| i64::from(c - a) * i64::from(d - b))?;
    }
    Some(area)
}

// Adjacent vertical annotations can share a repair patch while retaining their
// original positions and independent typesetting. Slot boundaries lie in the
// gutter between columns, irrespective of OCR or dialogue order.
fn column_slots(columns: &[Box4], area: Box4) -> Option<Vec<Box4>> {
    if columns.len() < 2 || columns.iter().any(|&[l, t, r, b]| b - t < 2 * (r - l)) {
        return None;
    }
    let mut order: Vec<_> = (0..columns.len()).collect();
    order.sort_by_key(|&i| columns[i][0]);
    let mut slots = columns.iter().map(|&[l,t,r,b]| [
        (l - 8).max(area[0]), (t - 8).max(area[1]), (r + 8).min(area[2]), (b + 8).min(area[3]),
    ]).collect::<Vec<_>>();
    for pair in order.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        let a = columns[left];
        let b = columns[right];
        let gap = b[0] - a[2];
        let width = (a[2] - a[0]).min(b[2] - b[0]);
        let overlap = a[3].min(b[3]) - a[1].max(b[1]);
        if !(0..=(width / 2).clamp(3, 16)).contains(&gap)
            || 2 * overlap < (a[3] - a[1]).min(b[3] - b[1]) {
            return None;
        }
        let divider = a[2] + gap / 2;
        slots[left][2] = slots[left][2].min(divider);
        slots[right][0] = slots[right][0].max(divider);
    }
    Some(slots)
}

fn conflicting_columns(
    page: &Value, blocks: &[Value], boxes: &[Option<Box4>], by_id: &HashMap<String, usize>,
    claimed: &HashSet<String>, originals: &[Box4], width: i32, height: i32,
) -> Result<HashSet<usize>> {
    let mut deferred = HashSet::new();
    for dialogue in array(&page["dialogues"]) {
        let keys = array(&dialogue["block_ids"]);
        if !(2..=4).contains(&keys.len()) { continue; }
        let Some(indexes) = keys.iter().map(|id| by_id.get(id.as_str()?).copied()).collect::<Option<Vec<_>>>() else { continue };
        if indexes.iter().any(|&i| boxes[i].is_none() || !ink::ready(&blocks[i])
            || !truth(&blocks[i]["vertical"]) || claimed.contains(text(&blocks[i]["id"]).as_str())) {
            continue;
        }
        let columns: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let area = union(&columns);
        if column_slots(&columns, area).is_none()
            || originals.iter().any(|&b| intersects(area, b))
            || boxes.iter().enumerate().any(|(i,b)| !indexes.contains(&i) && b.is_some_and(|b| intersects(area,b))) {
            continue;
        }
        for old in array(&page["overlay_regions"]) {
            let [id] = array(&old["block_ids"]) else { continue };
            if old["overlay"] != "rectangle" || !keys.contains(id) { continue; }
            let Some(shape) = geometry::cached_shape(old, width, height)? else { continue };
            for (&i, key) in indexes.iter().zip(keys) {
                if key != id && geometry::coverage(&shape, boxes[i])? > 0.0 {
                    deferred.extend(&indexes);
                    break;
                }
            }
            if deferred.contains(&indexes[0]) { break; }
        }
    }
    Ok(deferred)
}

struct RepairSources<'a> {
    blocks: &'a [Value],
    boxes: &'a [Option<Box4>],
    parts: &'a [Vec<SourcePart>],
    originals: &'a [Box4],
}

#[derive(Clone, Copy)]
struct SourcePart {
    bounds: Box4,
    selected: bool,
}

// Dilation around a detected character can reach a nearby hair or panel line.
// Follow the connected stroke into the original pixels outside the selection.
// Exclude that stroke from the final repair only inside the selected segment.
fn preserve_crossing_lines(
    plan: &mut background::BackgroundPlan,
    gray: &Mat,
    source: &Mat,
    selection: Box4,
) -> Result<()> {
    let [l, t, r, b] = selection;
    let margin = ((r - l).min(b - t) / 2).clamp(8, 20);
    let [x, y, right, bottom] = padded(selection, margin, gray.cols(), gray.rows());
    let patch = crop(gray, x, y, right - x, bottom - y)?;
    let sample = crop(&patch, l - x, t - y, r - l, b - t)?;
    let ground = percentile(sample.data_bytes()?, 50.0);
    let foreground = threshold(&patch, |v| {
        if ground >= 128.0 { f64::from(v) < ground - 45.0 } else { f64::from(v) > ground + 45.0 }
    })?;
    let (labels, stats) = components(&foreground)?;
    let labels = labels.data_typed::<i32>()?;
    let evidence = source.data_bytes()?;
    let mut counts = vec![(0usize, 0usize); stats.len()];
    let index = |px: i32, py: i32| ((py - y) * patch.cols() + px - x) as usize;
    for py in t..b {
        for px in l..r {
            let id = labels[index(px, py)] as usize;
            counts[id].0 += 1;
            counts[id].1 += usize::from(evidence[(py * gray.cols() + px) as usize] != 0);
        }
    }
    let crosses: Vec<_> = stats.iter().zip(counts).enumerate().map(|(id, (bounds, (inside, hits)))| {
        let [left, top, w, h, _] = *bounds;
        let away = (l - x - left).max(t - y - top)
            .max(x + left + w - r).max(y + top + h - b);
        id != 0 && inside >= 5 && away >= margin / 2 && w.max(h) >= margin
            && hits * 10 <= inside
    }).collect();
    if !crosses.iter().any(|&v| v) { return Ok(()); }

    let mut lines = blank(right - x, bottom - y, 0)?;
    for (pixel, &id) in lines.data_bytes_mut()?.iter_mut().zip(labels) {
        *pixel = u8::from(crosses[id as usize]);
    }
    let protected = morph(&lines, 3, true)?;
    let protected = protected.data_bytes()?;
    let [ox, oy, _, _] = plan.bounds;
    let width = plan.byte_mask.cols();
    let alpha = plan.byte_mask.data_bytes_mut()?;
    for py in t..b {
        for px in l..r {
            if protected[index(px, py)] != 0 {
                alpha[((py - oy) * width + px - ox) as usize] = 0;
            }
        }
    }
    Ok(())
}

// Run the original stroke recovery once, on the individual columns it was
// designed for. A confirmed selection has a hard edge; its original neighbour
// can still recover detector-backed punctuation beyond the old OCR rectangle.
fn complete_sources(
    gray: &Mat, blocks: &[Value], boxes: &[Option<Box4>], source: Option<&Mat>,
) -> Result<(Vec<Option<Box4>>, Vec<Vec<SourcePart>>, Option<Mat>)> {
    if !blocks.iter().any(|b| b["source_boxes"].is_array()) {
        let (boxes, mask) = ink::complete_vertical_strokes(gray, boxes, blocks, source)?;
        return Ok((boxes, vec![], mask));
    }
    let mut segments = Vec::new();
    let mut inputs = Vec::new();
    let mut owners = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        for rect in ink::source_boxes(block, gray.cols(), gray.rows()) {
            let selected = array(&block["manual_ocr_boxes"]).iter().any(|area| *area == json!(rect));
            let mut segment = block.clone();
            segment["box"] = json!(rect);
            segment.as_object_mut().unwrap().remove("source_boxes");
            if selected {
                segment["manual_ocr_boxes"] = json!([rect]);
                segment["vertical"] = json!(false);
            } else {
                segment.as_object_mut().unwrap().remove("manual_ocr_boxes");
            }
            inputs.push(boxes[i].map(|_| rect));
            segments.push(segment);
            owners.push((i, selected));
        }
    }
    let (expanded, mask) = ink::complete_vertical_strokes(gray, &inputs, &segments, source)?;
    let mut parts = vec![Vec::new(); blocks.len()];
    for ((owner, selected), bounds) in owners.into_iter().zip(expanded) {
        if let Some(bounds) = bounds { parts[owner].push(SourcePart { bounds, selected }); }
    }
    let boxes = parts.iter().map(|p| (!p.is_empty()).then(|| {
        union(&p.iter().map(|s| s.bounds).collect::<Vec<_>>())
    })).collect();
    Ok((boxes, parts, mask))
}

impl RepairSources<'_> {
    fn prepare(
        &self, gray: &Mat, indexes: &[usize], clip: Option<(&Mat, [i32; 2])>,
        textured: bool, source: Option<&Mat>,
    ) -> Result<background::BackgroundPlan> {
        let dimensions = (gray.cols(), gray.rows());
        if !indexes.iter().any(|&i| self.blocks[i]["source_boxes"].is_array()) {
            let boxes: Vec<_> = indexes.iter().map(|&i| self.boxes[i].unwrap()).collect();
            return background::prepare(gray, &boxes, clip, textured, source);
        }
        // A correction may be appended on the far side of artwork. Its
        // enclosing OCR rectangle is for layout; only the confirmed source
        // rectangles can supply pixels to erase.
        let sources: Vec<_> = indexes.iter().flat_map(|&i| &self.parts[i]).collect();
        let boxes: Vec<_> = sources.iter().map(|s| s.bounds).collect();
        // Background preparation erodes the clip by 3 pixels. Compensate
        // exactly at a selection; the ordinary OCR parts retain their margin.
        let parts: Vec<_> = sources.iter().map(|s| {
            padded(s.bounds, if s.selected { 3 } else { 11 }, dimensions.0, dimensions.1)
        }).collect();
        let [x, y, r, b] = union(&parts);
        let mut mask = blank(r - x, b - y, 0)?;
        for &[l,t,r,b] in &parts {
            let mut roi = Mat::roi_mut(&mut mask, core::Rect::new(l-x,t-y,r-l,b-t))?;
            roi.set_to_def(&core::Scalar::all(1.0))?;
        }
        if let Some((shape, [sx, sy])) = clip {
            let data = shape.data_bytes()?;
            let width = mask.cols();
            for (i, pixel) in mask.data_bytes_mut()?.iter_mut().enumerate() {
                let px = (i as i32 % width) + x - sx;
                let py = (i as i32 / width) + y - sy;
                if px < 0 || py < 0 || px >= shape.cols() || py >= shape.rows() {
                    *pixel = 0;
                } else {
                    *pixel &= data[(py * shape.cols() + px) as usize];
                }
            }
        }
        let others = self.boxes.iter().enumerate()
            .filter_map(|(i, &b)| if indexes.contains(&i) { None } else { b });
        for [l,t,r,b] in self.originals.iter().copied().chain(others) {
            clear_rect(&mut mask, [l-x,t-y,r-x,b-y])?;
        }
        let mut plan = background::prepare(gray, &boxes, Some((&mask, [x,y])), textured, source)?;
        if let Some(source) = source {
            for selected in sources.iter().filter(|s| s.selected) {
                preserve_crossing_lines(&mut plan, gray, source, selected.bounds)?;
            }
        }
        Ok(plan)
    }
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
    let mut originals = preserved_bubbles::protected_bounds(page, width, height);
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| {
            let bounds = ink::clipped_box(b, width, height);
            if preserved.get(text(&b["id"])).is_some() {
                originals.extend(bounds);
                None
            } else if keeps_original(b) {
                None
            } else {
                bounds
            }
        })
        .collect();
    if boxes.iter().all(Option::is_none) {
        return Ok(vec![]);
    }
    let (boxes, parts, supplemented) = complete_sources(gray, blocks, &boxes, source)?;
    let source = supplemented.as_ref();
    let repair = RepairSources { blocks, boxes: &boxes, parts: &parts, originals: &originals };
    let bubbles = geometry::bubbles(gray, &boxes)?;
    let narrations = geometry::narration_groups(gray, blocks, &boxes, &bubbles)?;
    let mut regions = vec![];
    let mut accepted: Vec<Candidate> = vec![];
    let mut claimed: HashSet<String> = HashSet::new();
    let mut ordered: Vec<_> = bubbles.iter().collect();
    ordered.sort_by_key(|c| c.members.iter().min().copied());
    for candidate in ordered {
        let indexes = &candidate.members;
        if preserved_bubbles::touches(candidate, &originals)? {
            continue;
        }
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
        let plan = repair.prepare(
            gray,
            indexes,
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
        if preserved_bubbles::touches(candidate, &originals)? {
            continue;
        }
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
        let plan = repair.prepare(
            gray,
            &indexes,
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
    let deferred = conflicting_columns(page, blocks, &boxes, &by_id, &claimed, &originals, width, height)?;
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
        if preserved_bubbles::touches(&shape, &originals)? {
            continue;
        }
        // A legacy rectangle's padding can occupy an adjacent column's OCR
        // box and permanently exclude its translation. Let the fallback plan
        // both columns before either one claims their shared margin.
        if old["overlay"] == "rectangle" && members.iter().any(|id| deferred.contains(&by_id[id])) {
            continue;
        }
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
        if old["overlay"] == "rectangle" && members.iter().any(|id| {
            boxes[by_id[id]] != ink::clipped_box(&blocks[by_id[id]], width, height)
        }) {
            let mut fits = true;
            for &bounds in &member_boxes {
                if geometry::coverage(&shape, Some(bounds))? < 0.97 { fits = false; break; }
            }
            if !fits { continue; }
        }
        let mut region = old.clone();
        let [x, y] = shape.origin;
        let w = shape.mask.cols().min(width - x);
        let h = shape.mask.rows().min(height - y);
        let textured =
            w > 0 && h > 0 && fraction(crop(gray, x, y, w, h)?.data_bytes()?, |v| v < 210) > 0.18;
        let clip = (old["overlay"] == "balloon" || !originals.is_empty()).then_some((&shape.mask, shape.origin));
        let indexes: Vec<_> = members.iter().map(|id| by_id[id]).collect();
        let plan = repair.prepare(gray, &indexes, clip, textured, source)?;
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
        if originals.iter().any(|&bounds| intersects([a, b, c, d], bounds)) {
            continue;
        }
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
        let bounds = union(&member_boxes);
        let expanded = padded(bounds, 8, width, height);
        let mut protected = originals.clone();
        if indexes.iter().all(|i| deferred.contains(i)) {
            protected.extend(boxes.iter().enumerate()
                .filter_map(|(i, &other)| if indexes.contains(&i) { None } else { other }));
        }
        let Some([x, y, r, end]) = trim_around_originals(expanded, bounds, &protected) else {
            continue;
        };
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
        let plan = repair.prepare(
            gray,
            &indexes,
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
        if indexes.iter().all(|i| deferred.contains(i))
            && let Some(slots) = column_slots(&member_boxes, [x,y,r,end])
        {
            region["member_slots"] = indexes.iter().zip(slots)
                .map(|(&i, slot)| (ids[i].clone(), json!(slot))).collect();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use opencv::{core::{Point, Rect, Scalar, Size, Vector}, imgcodecs, imgproc};

    #[tokio::test]
    async fn adjacent_cached_columns_keep_both_translations_and_their_own_slots() -> Result<()> {
        let mut gray = blank(480, 640, 255)?;
        let mut ink = blank(480, 640, 0)?;
        for x in [95, 129] {
            for y in (165..295).step_by(15) {
                let mark = Rect::new(x, y, 10, 8);
                imgproc::rectangle(&mut gray, mark, Scalar::all(10.), -1, 8, 0)?;
                imgproc::rectangle(&mut ink, mark, Scalar::all(255.), -1, 8, 0)?;
            }
        }
        imgproc::line(&mut gray, Point::new(80, 90), Point::new(80, 350), Scalar::all(0.), 2, 8, 0)?;
        let mut page = json!({"width":480,"height":640,"blocks":[
            {"id":"R","box":[120,120,148,309],"vertical":true,"source":"ああ","translation":"第一列"},
            {"id":"L","box":[88,164,118,301],"vertical":true,"source":"いい","translation":"第二列"}
        ],"dialogues":[{"block_ids":["R","L"]}], "overlay_regions":[]});
        for (id, pts) in [("R",[116,110,152,319]),("L",[84,156,122,309])] {
            let [l,t,r,b] = pts;
            page["overlay_regions"].as_array_mut().unwrap().push(json!({
                "id":id,"block_ids":[id],"overlay":"rectangle", "text_box":pts,
                "points":[[l,t],[r,t],[r,b],[l,b]],"fill_color":[255,255,255],"vertical":true
            }));
        }
        let result = regions(&page, &gray, None, Some(&ink), None).await?;
        assert_eq!(result.len(), 1);
        let region = &result[0];
        assert_eq!(region["block_ids"], json!(["R","L"]));
        assert_eq!(region["member_slots"]["L"], json!([80,156,119,309]));
        assert_eq!(region["member_slots"]["R"], json!([119,112,156,317]));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(region["background_png"].as_str().unwrap())?;
        let patch = imgcodecs::imdecode(&Vector::from_slice(&bytes), imgcodecs::IMREAD_UNCHANGED)?;
        let [x,y,_,_] = serde_json::from_value::<Box4>(region["background_box"].clone())?;
        let alpha = |px,py| patch.at_2d::<core::Vec4b>(py-y,px-x).unwrap()[3];
        assert_eq!(alpha(99,168), 255);
        assert_eq!(alpha(133,168), 255);
        assert!((110..315).all(|py| alpha(80,py) == 0));

        // A suspect adjacent column is protected, even when the neighbouring
        // legacy rectangle was authored before review markers existed.
        page["blocks"][1]["ocr_review"] = json!({"status":"suspect","reason":"核对"});
        let result = regions(&page, &gray, None, Some(&ink), None).await?;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["block_ids"], json!(["R"]));
        let region = geometry::cached_shape(&result[0],480,640)?.unwrap();
        assert_eq!(geometry::coverage(&region, Some([88,164,118,301]))?,0.0);
        Ok(())
    }

    #[test]
    fn corrected_sources_leave_the_gap_and_other_text_untouched() -> Result<()> {
        let gray = blank(260, 160, 240)?;
        // Even a completely erroneous detector mask must not grant permission
        // to erase an intervening picture or unowned neighbouring text.
        let source = blank(260, 160, 255)?;
        let blocks = vec![
            json!({"box":[30,45,180,115],"source_boxes":[[30,45,50,115],[160,45,180,115]],
                   "manual_ocr_boxes":[[160,45,180,115]]}),
            json!({"box":[174,60,195,98]}),
        ];
        let boxes = vec![Some([30,45,180,115]), Some([174,60,195,98])];
        let (boxes,parts,_) = complete_sources(&gray,&blocks,&boxes,Some(&source))?;
        let repair = RepairSources { blocks: &blocks, boxes: &boxes, parts: &parts, originals: &[] };
        let plan = repair.prepare(&gray, &[0], None, true, Some(&source))?;
        let [x,y,_,_] = plan.bounds;
        let alpha = |px,py| *plan.byte_mask.at_2d::<u8>(py-y,px-x).unwrap();
        assert_eq!(alpha(40,75),255);
        assert_eq!(alpha(166,75),255);
        assert_eq!(alpha(159,75),0);
        assert_eq!(alpha(175,75),0);
        assert!((70..140).all(|px| alpha(px,75) == 0));
        Ok(())
    }

    #[test]
    fn merged_column_recovers_its_old_final_mark_without_expanding_the_selection() -> Result<()> {
        let mut gray = blank(400, 500, 255)?;
        let mut source = blank(400, 500, 0)?;
        for x in [82, 110] {
            for y in [70, 95, 120, 145, 170, 193] {
                let glyph = Rect::new(x, y, 4, 9);
                imgproc::rectangle(&mut gray, glyph, Scalar::all(0.), -1, 8, 0)?;
                imgproc::rectangle(&mut source, glyph, Scalar::all(255.), -1, 8, 0)?;
            }
        }
        let blocks = vec![json!({"id":"1","vertical":true,"translation":"修订译文","box":[78,65,122,190],
            "source_boxes":[[105,65,122,190],[78,65,92,190]],"manual_ocr_boxes":[[105,65,122,190]]})];
        let (boxes, parts, mask) = complete_sources(&gray, &blocks, &[Some([78,65,122,190])], Some(&source))?;
        assert_eq!(boxes, vec![Some([78,65,122,204])]);
        let repair = RepairSources { blocks: &blocks, boxes: &boxes, parts: &parts, originals: &[] };
        let plan = repair.prepare(&gray, &[0], None, true, mask.as_ref())?;
        let [x,y,_,_] = plan.bounds;
        assert_eq!(*plan.byte_mask.at_2d::<u8>(196-y,83-x)?, 255);
        assert_eq!(*plan.byte_mask.at_2d::<u8>(196-y,111-x)?, 0);
        Ok(())
    }

    #[test]
    fn manual_repair_dilation_preserves_a_crossing_drawing_next_to_a_glyph() -> Result<()> {
        let mut gray = blank(180, 180, 250)?;
        let mut source = blank(180, 180, 0)?;
        imgproc::line(&mut gray, Point::new(35,25), Point::new(110,100), Scalar::all(5.), 1, 8, 0)?;
        let glyph = Rect::new(72,74,8,15);
        imgproc::rectangle(&mut gray, glyph, Scalar::all(10.), -1, 8, 0)?;
        imgproc::rectangle(&mut source, glyph, Scalar::all(255.), -1, 8, 0)?;
        let rect = [62,48,97,91];
        let mut plan = background::prepare(&gray, &[rect], None, true, Some(&source))?;
        let [l,t,_,_] = plan.bounds;
        assert_eq!(*plan.byte_mask.at_2d::<u8>(69-t,79-l)?, 255);
        preserve_crossing_lines(&mut plan, &gray, &source, rect)?;
        for px in 62..97 {
            assert_eq!(*plan.byte_mask.at_2d::<u8>(px-10-t,px-l)?, 0);
        }
        assert_eq!(*plan.byte_mask.at_2d::<u8>(78-t,74-l)?, 255);
        Ok(())
    }

    #[tokio::test]
    async fn preserves_all_members_of_one_bubble_and_renders_a_separate_bubble() -> Result<()> {
        let mut gray = blank(480, 640, 25)?;
        for centre in [180, 355] {
            imgproc::ellipse(&mut gray, Point::new(centre, 220), Size::new(64, 80), 0.0, 0.0, 360.0,
                             Scalar::all(255.0), imgproc::FILLED, imgproc::LINE_8, 0)?;
        }
        for x in [154, 197, 347] {
            for y in (180..250).step_by(15) {
                imgproc::rectangle(&mut gray, Rect::new(x, y, 12, 8), Scalar::all(30.0),
                                   imgproc::FILLED, imgproc::LINE_8, 0)?;
            }
        }
        let mut page = json!({"width": 480, "height": 640, "blocks": [
            {"id": "1", "box": [151, 172, 172, 260], "source": "こんにちは", "translation": "你好", "vertical": true},
            {"id": "2", "box": [195, 172, 213, 260], "source": "おはよう", "translation": "早上好", "vertical": true},
            {"id": "3", "box": [343, 172, 363, 260], "source": "またね", "translation": "回头见", "vertical": true}
        ]});
        let combined = regions(&page, &gray, None, None, None).await?;
        assert!(combined.iter().any(|region| region["block_ids"] == json!(["1", "2"])));

        // A neighbouring translation must not be typeset over an unconfirmed
        // OCR candidate in the same balloon, even if that candidate is omitted.
        let mut suspect = page.clone();
        suspect["blocks"][1]["ocr_review"] = json!({"status": "suspect", "reason": "需要核对"});
        let protected_box = [195, 172, 213, 260];
        let cautious = regions(&suspect, &gray, None, None, None).await?;
        assert!(cautious.iter().any(|r| r["block_ids"] == json!(["1"])));
        assert!(cautious.iter().any(|r| r["block_ids"] == json!(["3"])));
        for region in &cautious {
            let shape = geometry::cached_shape(region, 480, 640)?.unwrap();
            assert_eq!(geometry::coverage(&shape, Some(protected_box))?, 0.0);
        }
        suspect["blocks"][1]["ocr_confirmed"] = json!(true);
        assert_eq!(regions(&suspect, &gray, None, None, None).await?, combined);

        let mut image = Vector::new();
        imgcodecs::imencode(".png", &gray, &mut image, &Vector::new())?;
        page["blocks"][1]["keep_original"] = json!(true);
        assert_eq!(preserved_bubbles::legacy_ids(&page), ["2"]);
        preserved_bubbles::set(&mut page, "2", true, image.as_slice(), &[])?;
        assert!(preserved_bubbles::legacy_ids(&page).is_empty());
        assert!(keeps_original(&page["blocks"][0]));
        assert!(keeps_original(&page["blocks"][1]));
        assert!(!keeps_original(&page["blocks"][2]));
        let displayed = regions(&page, &gray, None, None, None).await?;
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0]["block_ids"], json!(["3"]));
        assert!(array(&displayed[0]["points"]).iter().all(|point| point[0].as_i64().unwrap() > 245));

        crate::overlay::ensure_gray(&mut page, Some(&gray))?;
        assert!(array(&page["overlay_regions"]).iter().all(|region| array(&region["block_ids"]).iter().all(|id| id == "3")));
        preserved_bubbles::set(&mut page, "1", false, &[], &[])?;
        assert!(array(&page["blocks"]).iter().all(|block| !keeps_original(block)));
        assert_eq!(page["blocks"][0]["translation"], "你好");
        assert_eq!(page["blocks"][1]["translation"], "早上好");
        Ok(())
    }
}
