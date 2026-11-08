//! Image-backed balloon geometry. These are conservative proposals, not
//! permission to paint whole balloons; erasure remains limited to source ink.
use crate::{
    compat::{array, number, truth},
    ink::Box4,
    raster::*,
};
use anyhow::Result;
use opencv::{
    core::{Mat, Point, Vector},
    imgproc,
    prelude::*,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};

#[derive(Clone)]
pub struct Candidate {
    pub mask: Mat,
    pub origin: [i32; 2],
    pub area: usize,
    pub points: Vec<[i32; 2]>,
    pub contour: Vector<Point>,
    pub members: Vec<usize>,
    pub fill_color: [i32; 3],
    pub score: f64,
    pub solidity: Option<f64>,
    pub textured: Option<bool>,
    pub text_box: Option<Box4>,
}

impl Candidate {
    pub fn snapshot(&self, boxes: &[Option<Box4>]) -> Result<Value> {
        let inner = inner_rectangle(&self.mask)?;
        let mut value = json!({"origin": self.origin, "area": self.area, "points": self.points,
            "members": self.members, "fill_color": self.fill_color, "score": self.score,
            "mask_shape": [self.mask.rows(), self.mask.cols()],
            "mask_sha256": format!("{:x}", Sha256::digest(self.mask.data_bytes()?)),
            "inner_rectangle": inner, "erase_points": erase_polygons(self, boxes)?});
        if let Some(v) = self.solidity {
            value["solidity"] = json!(v);
        }
        if let Some(v) = self.textured {
            value["textured"] = json!(v);
        }
        if let Some(v) = self.text_box {
            value["text_box"] = json!(v);
        }
        Ok(value)
    }
}

pub fn shape(mask: &Mat, origin: [i32; 2]) -> Result<Option<Candidate>> {
    let found = contours(mask, imgproc::RETR_EXTERNAL)?;
    let mut largest = None;
    let mut area = -1.0;
    for contour in found {
        let size = imgproc::contour_area_def(&contour)?;
        if size > area {
            area = size;
            largest = Some(contour);
        }
    }
    let Some(contour) = largest else {
        return Ok(None);
    };
    if contour.len() < 3 {
        return Ok(None);
    }
    let filled = filled(mask.cols(), mask.rows(), &contour)?;
    let area = filled.data_bytes()?.iter().map(|&v| v as usize).sum();
    let points = contour
        .iter()
        .map(|p| [p.x + origin[0], p.y + origin[1]])
        .collect();
    Ok(Some(Candidate {
        mask: filled,
        origin,
        area,
        points,
        contour,
        members: vec![],
        fill_color: [255; 3],
        score: 0.0,
        solidity: None,
        textured: None,
        text_box: None,
    }))
}

pub fn coverage(candidate: &Candidate, bounds: Option<Box4>) -> Result<f64> {
    let Some([a, b, c, d]) = bounds else {
        return Ok(0.0);
    };
    let [x, y] = candidate.origin;
    let width = candidate.mask.cols();
    let (left, top, right, bottom) = (
        x.max(a),
        y.max(b),
        (x + width).min(c),
        (y + candidate.mask.rows()).min(d),
    );
    if right <= left || bottom <= top {
        return Ok(0.0);
    }
    let pixels = candidate.mask.data_bytes()?;
    let mut count = 0usize;
    for row in top..bottom {
        count += pixels
            [((row - y) * width + left - x) as usize..((row - y) * width + right - x) as usize]
            .iter()
            .map(|&p| p as usize)
            .sum::<usize>();
    }
    Ok(count as f64 / ((c - a) * (d - b)) as f64)
}

pub fn members(candidate: &Candidate, boxes: &[Option<Box4>]) -> Result<Vec<usize>> {
    let mut result = vec![];
    for (i, &bbox) in boxes.iter().enumerate() {
        let covered = coverage(candidate, bbox)?;
        if covered > 0.02 && covered < 0.94 {
            return Ok(vec![]);
        }
        if covered >= 0.94 {
            result.push(i);
        }
    }
    Ok(result)
}

pub fn overlap(first: &Candidate, second: &Candidate) -> Result<f64> {
    let [x, y] = first.origin;
    let [a, b] = second.origin;
    let (mw, nw) = (first.mask.cols(), second.mask.cols());
    let (left, top, right, bottom) = (
        x.max(a),
        y.max(b),
        (x + mw).min(a + nw),
        (y + first.mask.rows()).min(b + second.mask.rows()),
    );
    if right <= left || bottom <= top {
        return Ok(0.0);
    }
    let (m, n) = (first.mask.data_bytes()?, second.mask.data_bytes()?);
    let mut common = 0usize;
    for yy in top..bottom {
        for xx in left..right {
            common += usize::from(
                m[((yy - y) * mw + xx - x) as usize] > 0
                    && n[((yy - b) * nw + xx - a) as usize] > 0,
            );
        }
    }
    Ok(common as f64 / first.area.min(second.area).max(1) as f64)
}

pub fn inner_rectangle(mask: &Mat) -> Result<Option<Box4>> {
    let distance = distance(mask, true)?;
    let values = distance.data_typed::<f32>()?;
    let width = mask.cols() as usize;
    let threshold = (mask.cols().min(mask.rows()) as f64 * 0.04).max(3.0) as f32;
    let mut heights = vec![0i32; width + 1];
    let (mut best_area, mut best) = (0, None);
    for y in 0..mask.rows() {
        for x in 0..width {
            heights[x] = if values[y as usize * width + x] >= threshold {
                heights[x] + 1
            } else {
                0
            };
        }
        let mut stack: Vec<(i32, i32)> = vec![];
        for (x, &value) in heights.iter().enumerate() {
            let mut start = x as i32;
            while let Some(&(left, h)) = stack.last() {
                if h <= value {
                    break;
                }
                stack.pop();
                let area = (x as i32 - left) * h;
                if area > best_area && x as i32 - left >= 12 && h >= 12 {
                    best_area = area;
                    best = Some([left, y - h + 1, x as i32, y + 1]);
                }
                start = left;
            }
            if stack.last().is_none_or(|&(_, h)| h < value) {
                stack.push((start, value));
            }
        }
    }
    Ok(best)
}

pub fn erase_polygons(candidate: &Candidate, boxes: &[Option<Box4>]) -> Result<Vec<Vec<[i32; 2]>>> {
    let [x, y] = candidate.origin;
    let (w, h) = (candidate.mask.cols(), candidate.mask.rows());
    let mut mask = blank(w, h, 0)?;
    let pixels = mask.data_bytes_mut()?;
    for &index in &candidate.members {
        let [a, b, c, d] = boxes[index].expect("candidate member has bounds");
        let pad = (((c - a).min(d - b) as f64 * 0.08).round_ties_even() as i32).clamp(3, 7);
        let (left, right) = ((a - x - pad).max(0), (c - x + pad).min(w));
        for row in (b - y - pad).max(0)..(d - y + pad).min(h) {
            pixels[(row * w + left) as usize..(row * w + right) as usize].fill(1);
        }
    }
    for (p, &inside) in pixels.iter_mut().zip(candidate.mask.data_bytes()?) {
        *p &= inside;
    }
    Ok(contours(&mask, imgproc::RETR_EXTERNAL)?
        .into_iter()
        .filter(|c| c.len() >= 3)
        .map(|c| c.iter().map(|p| [p.x + x, p.y + y]).collect())
        .collect())
}

fn bounded(
    gray: &Mat,
    labels: &Mat,
    stat: [i32; 5],
    label: i32,
    boxes: &[Option<Box4>],
) -> Result<Option<Candidate>> {
    let (width, height) = (gray.cols(), gray.rows());
    let [x, y, w, h, pixels] = stat;
    if w.min(h) < 24
        || pixels < 200
        || x < 4
        || y < 4
        || x + w > width - 4
        || w as f64 > width as f64 * 0.48
        || h as f64 > height as f64 * 0.43
        || pixels as f64 > (width * height) as f64 * 0.085
        || (w * h) as f64 > (width * height) as f64 * 0.14
    {
        return Ok(None);
    }
    let local = crop(labels, x, y, w, h)?;
    let mut component = blank(w, h, 0)?;
    for (p, &id) in component
        .data_bytes_mut()?
        .iter_mut()
        .zip(local.data_typed::<i32>()?)
    {
        *p = u8::from(id == label);
    }
    let Some(mut candidate) = shape(&component, [x, y])? else {
        return Ok(None);
    };
    let area = imgproc::contour_area_def(&candidate.contour)?;
    let mut hull = Vector::<Point>::new();
    imgproc::convex_hull_def(&candidate.contour, &mut hull)?;
    let solidity = area / imgproc::contour_area_def(&hull)?.max(1.0);
    let fill = candidate.area as f64 / (w * h) as f64;
    if !(0.42..=0.93).contains(&fill) || solidity < 0.72 {
        return Ok(None);
    }
    candidate.members = members(&candidate, boxes)?;
    if candidate.members.is_empty() || (pixels as f64 / candidate.area.max(1) as f64) < 0.64 {
        return Ok(None);
    }
    let patch = crop(gray, x, y, w, h)?;
    let mut background = candidate.mask.try_clone()?;
    for &i in &candidate.members {
        let [a, b, c, d] = boxes[i].unwrap();
        clear_rect(
            &mut background,
            [a - x - 7, b - y - 7, c - x + 7, d - y + 7],
        )?;
    }
    let values = select(&patch, &background)?;
    if values.len() < 100 {
        return Ok(None);
    }
    let interior = percentile(&values, 50.0);
    if interior < 220.0
        || fraction(&values, |v| v >= 210) < 0.94
        || fraction(&values, |v| (v as f64) < interior - 65.0) > 0.025
    {
        return Ok(None);
    }
    let ring = ring(&candidate.mask, 3)?;
    let exterior = pad(gray, 3, true)?;
    let exterior = crop(&exterior, x, y, w + 6, h + 6)?;
    let values = select(&exterior, &ring)?;
    if values.len() < 40
        || fraction(&values, |v| (v as f64) < interior - 55.0) < 0.45
        || interior - percentile(&values, 35.0) < 60.0
    {
        return Ok(None);
    }
    candidate.fill_color = [interior.round_ties_even() as i32; 3];
    candidate.solidity = Some(solidity);
    candidate.score = 0.9;
    Ok(Some(candidate))
}

fn split_lobes(candidate: Candidate, boxes: &[Option<Box4>]) -> Result<Vec<Candidate>> {
    if candidate.members.len() < 2 || candidate.solidity.unwrap() >= 0.94 {
        return Ok(vec![candidate]);
    }
    let mask = &candidate.mask;
    let distance = distance(mask, true)?;
    let radius = (mask.cols().min(mask.rows()) as f64 * 0.085).clamp(5.0, 24.0) as f32;
    let mut cores = blank(mask.cols(), mask.rows(), 0)?;
    for (v, &d) in cores
        .data_bytes_mut()?
        .iter_mut()
        .zip(distance.data_typed::<f32>()?)
    {
        *v = u8::from(d > radius);
    }
    let (labels, stats) = components(&cores)?;
    let sizeable: HashSet<_> = stats
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, s)| s[4] as f64 >= candidate.area as f64 * 0.12)
        .map(|(i, _)| i as i32)
        .collect();
    if sizeable.len() < 2 {
        return Ok(vec![candidate]);
    }
    for (v, id) in cores
        .data_bytes_mut()?
        .iter_mut()
        .zip(labels.data_typed::<i32>()?)
    {
        *v = u8::from(sizeable.contains(id));
    }
    let inverse = threshold(&cores, |v| v == 0)?;
    let (mut nearest, mut unused) = (Mat::default(), Mat::default());
    imgproc::distance_transform_with_labels(
        &inverse,
        &mut unused,
        &mut nearest,
        imgproc::DIST_L2,
        5,
        imgproc::DIST_LABEL_CCOMP,
    )?;
    let nearest = nearest.data_typed::<i32>()?;
    let ids: BTreeSet<_> = nearest
        .iter()
        .zip(cores.data_bytes()?)
        .filter_map(|(&id, &c)| (c > 0).then_some(id))
        .collect();
    let mut pieces = vec![];
    let mut assigned = HashSet::new();
    for id in ids {
        let mut part = blank(mask.cols(), mask.rows(), 0)?;
        for ((v, &n), &inside) in part
            .data_bytes_mut()?
            .iter_mut()
            .zip(nearest)
            .zip(mask.data_bytes()?)
        {
            *v = u8::from(n == id && inside > 0);
        }
        let Some(mut piece) = shape(&part, candidate.origin)? else {
            return Ok(vec![candidate]);
        };
        piece.members = members(&piece, boxes)?;
        if piece.members.is_empty() || piece.members.iter().any(|i| assigned.contains(i)) {
            return Ok(vec![candidate]);
        }
        assigned.extend(piece.members.iter().copied());
        piece.fill_color = candidate.fill_color;
        piece.score = candidate.score;
        pieces.push(piece);
    }
    if assigned == candidate.members.iter().copied().collect() {
        Ok(pieces)
    } else {
        Ok(vec![candidate])
    }
}

pub fn bubbles(gray: &Mat, boxes: &[Option<Box4>]) -> Result<Vec<Candidate>> {
    let bright = threshold(gray, |v| v >= 210)?;
    let (labels, stats) = components(&bright)?;
    let pixels = labels.data_typed::<i32>()?;
    let mut chosen = BTreeSet::new();
    for &[a, b, c, d] in boxes.iter().flatten() {
        let mut counts = vec![0usize; stats.len()];
        for y in b..d {
            for x in a..c {
                counts[pixels[(y * gray.cols() + x) as usize] as usize] += 1;
            }
        }
        counts[0] = 0;
        let mut best = 0;
        for i in 1..counts.len() {
            if counts[i] > counts[best] {
                best = i;
            }
        }
        if best > 0 && counts[best] as f64 >= ((c - a) * (d - b)) as f64 * 0.40 {
            chosen.insert(best);
        }
    }
    let mut result = vec![];
    for id in chosen {
        if let Some(candidate) = bounded(gray, &labels, stats[id], id as i32, boxes)? {
            result.extend(split_lobes(candidate, boxes)?);
        }
    }
    let outlined = outlined_bubbles(gray, boxes, &result)?;
    result.extend(outlined);
    Ok(result)
}

fn outlined_bubbles(
    gray: &Mat,
    boxes: &[Option<Box4>],
    existing: &[Candidate],
) -> Result<Vec<Candidate>> {
    let (width, height) = (gray.cols(), gray.rows());
    let mut assigned: HashSet<_> = existing
        .iter()
        .flat_map(|c| c.members.iter().copied())
        .collect();
    let pending: Vec<_> = boxes
        .iter()
        .enumerate()
        .filter(|(i, b)| b.is_some() && !assigned.contains(i))
        .map(|(i, _)| i)
        .collect();
    if pending.is_empty() {
        return Ok(vec![]);
    }
    let mut candidates = vec![];
    for level in [95, 125, 155, 210, 240] {
        let mut found = vec![];
        for below in [true, false] {
            let binary = threshold(gray, |v| (v < level) == below)?;
            found.extend(contours(&binary, imgproc::RETR_LIST)?);
        }
        for contour in found {
            let r = imgproc::bounding_rect(&contour)?;
            let (x, y, w, h) = (r.x, r.y, r.width, r.height);
            let area = imgproc::contour_area_def(&contour)?;
            if w.min(h) < 30
                || area < 500.0
                || area > (width * height) as f64 * 0.085
                || x < 4
                || y < 4
                || x + w > width - 4
                || y + h > height - 4
                || w as f64 > width as f64 * 0.48
                || h as f64 > height as f64 * 0.43
                || !(0.45..=0.995).contains(&(area / (w * h).max(1) as f64))
            {
                continue;
            }
            if !pending.iter().any(|&i| {
                let b = boxes[i].unwrap();
                x <= b[0] && y <= b[1] && x + w >= b[2] && y + h >= b[3]
            }) {
                continue;
            }
            let mut hull = Vector::<Point>::new();
            imgproc::convex_hull_def(&contour, &mut hull)?;
            let solidity = area / imgproc::contour_area_def(&hull)?.max(1.0);
            if solidity < 0.82 {
                continue;
            }
            let local =
                Vector::<Point>::from_iter(contour.iter().map(|p| Point::new(p.x - x, p.y - y)));
            let mask = filled(w, h, &local)?;
            let Some(mut candidate) = shape(&mask, [x, y])? else {
                continue;
            };
            candidate.members = members(&candidate, boxes)?;
            if candidate.members.is_empty()
                || candidate.members.iter().any(|i| assigned.contains(i))
            {
                continue;
            }
            if area / (w * h) as f64 > 0.93 {
                let mut valid = true;
                for &i in &candidate.members {
                    let [a, b, c, d] = boxes[i].unwrap();
                    let ink = crop(gray, a, b, c - a, d - b)?;
                    if fraction(ink.data_bytes()?, |v| v < 155) <= 0.008 {
                        valid = false;
                        break;
                    }
                }
                if !valid {
                    continue;
                }
            }
            let mut background = morph(&mask, 9, false)?;
            for &i in &candidate.members {
                let [a, b, c, d] = boxes[i].unwrap();
                clear_rect(
                    &mut background,
                    [a - x - 10, b - y - 10, c - x + 10, d - y + 10],
                )?;
            }
            if background
                .data_bytes()?
                .iter()
                .map(|&v| v as usize)
                .sum::<usize>()
                < 300
            {
                continue;
            }
            let patch = crop(gray, x, y, w, h)?;
            let values = select(&patch, &background)?;
            let smooth = select(&blur(&patch, 9, 2.0)?, &background)?;
            let median = percentile(&values, 50.0);
            if median < 115.0 || fraction(&smooth, |v| v < 75) > 0.02 {
                continue;
            }
            let ring = ring(&mask, 3)?;
            let ring_values = select(&crop(gray, x - 3, y - 3, w + 6, h + 6)?, &ring)?;
            if fraction(&ring_values, |v| (v as f64) < median - 45.0) < 0.40
                || median - percentile(&ring_values, 30.0) < 60.0
            {
                continue;
            }
            candidate.fill_color = [median.round_ties_even() as i32; 3];
            candidate.solidity = Some(solidity);
            candidate.score = 0.88;
            candidate.textured = Some(fraction(&values, |v| v < 210) > 0.15);
            let mut overlaps = false;
            for other in existing {
                if overlap(&candidate, other)? > 0.0 {
                    overlaps = true;
                    break;
                }
            }
            if !overlaps {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by_key(|c| c.area);
    let mut accepted = vec![];
    for candidate in candidates {
        if candidate.members.iter().any(|i| assigned.contains(i)) {
            continue;
        }
        let mut overlaps = false;
        for other in &accepted {
            if overlap(&candidate, other)? > 0.0 {
                overlaps = true;
                break;
            }
        }
        if overlaps {
            continue;
        }
        assigned.extend(candidate.members.iter().copied());
        accepted.push(candidate);
    }
    Ok(accepted)
}

pub fn narration_groups(
    gray: &Mat,
    blocks: &[Value],
    boxes: &[Option<Box4>],
    bubbles: &[Candidate],
) -> Result<Vec<Candidate>> {
    let (width, height) = (gray.cols(), gray.rows());
    let assigned: HashSet<_> = bubbles
        .iter()
        .flat_map(|c| c.members.iter().copied())
        .collect();
    let mut pending: Vec<_> = boxes
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            !assigned.contains(i)
                && truth(&blocks[*i]["vertical"])
                && b.is_some_and(|b| (b[3] - b[1]) as f64 >= height as f64 * 0.08)
        })
        .map(|(i, _)| i)
        .collect();
    let mut groups = vec![];
    while !pending.is_empty() {
        let mut group = vec![pending.remove(0)];
        let mut changed = true;
        while changed {
            changed = false;
            for i in pending.clone() {
                let a = boxes[i].unwrap();
                let mut join = false;
                for &j in &group {
                    let b = boxes[j].unwrap();
                    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
                    let gap = a[0].max(b[0]) - a[2].min(b[2]);
                    if (a[1] - b[1]).abs() as f64 > (a[3] - a[1]).max(b[3] - b[1]) as f64 * 0.35
                        || (overlap as f64) < (a[3] - a[1]).min(b[3] - b[1]) as f64 * 0.65
                        || gap < -5
                        || gap as f64 > width as f64 * 0.12
                    {
                        continue;
                    }
                    let (first, second) = if a[0] <= b[0] { (a, b) } else { (b, a) };
                    let (yy, dd) = (a[1].max(b[1]), a[3].min(b[3]));
                    if second[0] > first[2] && dd > yy {
                        let band = crop(gray, first[0], yy, second[2] - first[0], dd - yy)?;
                        if percentile(band.data_bytes()?, 50.0) > 100.0 {
                            let strip = crop(gray, first[2], yy, second[0] - first[2], dd - yy)?;
                            let smooth = blur(&strip, 5, 1.0)?;
                            let (_, stats) = components(&threshold(&smooth, |v| v < 65)?)?;
                            if stats
                                .iter()
                                .skip(1)
                                .any(|s| s[3] as f64 > (dd - yy) as f64 * 0.8)
                            {
                                continue;
                            }
                        }
                    }
                    join = true;
                    break;
                }
                if join {
                    group.push(i);
                    pending.retain(|&p| p != i);
                    changed = true;
                }
            }
        }
        if group.len() < 2 {
            continue;
        }
        let left = (group.iter().map(|&i| boxes[i].unwrap()[0]).min().unwrap() - 10).max(0);
        let top = (group.iter().map(|&i| boxes[i].unwrap()[1]).min().unwrap() - 10).max(0);
        let right = (group.iter().map(|&i| boxes[i].unwrap()[2]).max().unwrap() + 10).min(width);
        let bottom = (group.iter().map(|&i| boxes[i].unwrap()[3]).max().unwrap() + 10).min(height);
        if ((right - left) as f64) < width as f64 * 0.12 {
            continue;
        }
        let (w, h) = (right - left, bottom - top);
        let mut mask = blank(w, h, 1)?;
        for other in bubbles {
            let [x, y] = other.origin;
            let (a, b, c, d) = (
                left.max(x - 5),
                top.max(y - 5),
                right.min(x + other.mask.cols() + 5),
                bottom.min(y + other.mask.rows() + 5),
            );
            if c <= a || d <= b {
                continue;
            }
            let exclusion = morph(&pad(&other.mask, 5, false)?, 11, true)?;
            let excluded = exclusion.data_bytes()?;
            let ew = exclusion.cols();
            let target = mask.data_bytes_mut()?;
            for yy in b..d {
                for xx in a..c {
                    target[((yy - top) * w + xx - left) as usize] &=
                        1 - excluded[((yy - y + 5) * ew + xx - x + 5) as usize];
                }
            }
        }
        let Some(mut candidate) = shape(&mask, [left, top])? else {
            continue;
        };
        let mut covered = true;
        for &i in &group {
            if coverage(&candidate, boxes[i])? < 0.94 {
                covered = false;
                break;
            }
        }
        if !covered {
            continue;
        }
        candidate.members = group;
        candidate.text_box = Some([left, top, right, bottom]);
        candidate.textured = Some(true);
        let mut background = candidate.mask.try_clone()?;
        for &i in &candidate.members {
            let [a, b, c, d] = boxes[i].unwrap();
            clear_rect(
                &mut background,
                [a - left - 8, b - top - 8, c - left + 8, d - top + 8],
            )?;
        }
        if background
            .data_bytes()?
            .iter()
            .map(|&p| p as usize)
            .sum::<usize>()
            < 100
        {
            continue;
        }
        let patch = crop(gray, left, top, w, h)?;
        let low = blur(&patch, 9, 2.0)?;
        let ground = select(&low, &background)?;
        let source: Vec<_> = low
            .data_bytes()?
            .iter()
            .zip(candidate.mask.data_bytes()?)
            .zip(background.data_bytes()?)
            .filter_map(|((&v, &inside), &bg)| (inside > 0 && bg == 0).then_some(v))
            .collect();
        let source_dark = if source.is_empty() {
            0.0
        } else {
            fraction(&source, |v| v < 65)
        };
        let ground_median = percentile(&ground, 50.0);
        let dark_ground =
            !source.is_empty() && percentile(&source, 25.0) < 65.0 && ground_median < 65.0;
        if !dark_ground && fraction(&ground, |v| v < 65) > (source_dark + 0.18).max(0.06) {
            continue;
        }
        let barrier_tone = (ground_median - 55.0).min(90.0);
        let pixels = patch.data_bytes()?;
        if barrier_tone > 0.0
            && (0..w).any(|x| {
                (0..h)
                    .filter(|&y| (pixels[(y * w + x) as usize] as f64) < barrier_tone)
                    .count() as f64
                    / h as f64
                    > 0.82
            })
        {
            continue;
        }
        let mut intruder = false;
        for (i, &b) in boxes.iter().enumerate() {
            if b.is_some() && !candidate.members.contains(&i) && coverage(&candidate, b)? > 0.02 {
                intruder = true;
                break;
            }
        }
        if !intruder {
            groups.push(candidate);
        }
    }
    Ok(groups)
}

/// Rasterize old cached boundaries solely for overlap rejection. These bounds
/// do not authorize expansion of an erasure or editing of an OCR cache.
pub fn cached_shape(region: &Value, width: i32, height: i32) -> Result<Option<Candidate>> {
    let values = array(&region["points"]);
    if values.len() < 3 {
        return Ok(None);
    }
    let mut points = Vector::<Point>::new();
    for value in values {
        let pair = array(value);
        if pair.len() != 2 {
            return Ok(None);
        }
        let (Some(x), Some(y)) = (number(&pair[0]), number(&pair[1])) else {
            return Ok(None);
        };
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x > width as f64
            || y > height as f64
        {
            return Ok(None);
        }
        points.push(Point::new(
            x.round_ties_even() as i32,
            y.round_ties_even() as i32,
        ));
    }
    let r = imgproc::bounding_rect(&points)?;
    if (r.width * r.height) as f64 > (width * height) as f64 * 0.14 {
        return Ok(None);
    }
    let contour =
        Vector::<Point>::from_iter(points.iter().map(|p| Point::new(p.x - r.x, p.y - r.y)));
    let mut mask = blank(r.width, r.height, 0)?;
    imgproc::fill_poly_def(
        &mut mask,
        &Vector::<Vector<Point>>::from_iter([contour.clone()]),
        opencv::core::Scalar::all(1.0),
    )?;
    let area = mask.data_bytes()?.iter().map(|&p| p as usize).sum();
    Ok(Some(Candidate {
        mask,
        origin: [r.x, r.y],
        area,
        points: points.iter().map(|p| [p.x, p.y]).collect(),
        contour,
        members: vec![],
        fill_color: [255; 3],
        score: 0.0,
        solidity: None,
        textured: None,
        text_box: None,
    }))
}
