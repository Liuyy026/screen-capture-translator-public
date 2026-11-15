use crate::{
    compat::{array, keeps_original},
    geometry::{self, Candidate},
    imaging,
    ink::{self, Box4},
};
use anyhow::{Context, Result, ensure};
use opencv::{imgcodecs, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize)]
struct Bubble {
    block_ids: Vec<String>,
    bounds: Option<Box4>,
}

fn saved(page: &Value) -> Vec<Bubble> {
    array(&page["preserved_bubbles"])
        .iter()
        .filter_map(|bubble| serde_json::from_value(bubble.clone()).ok())
        .collect()
}

pub fn legacy_ids(page: &Value) -> Vec<String> {
    let groups = saved(page);
    array(&page["blocks"])
        .iter()
        .filter(|block| keeps_original(block))
        .filter_map(|block| block["id"].as_str())
        .filter(|id| {
            !groups
                .iter()
                .any(|group| group.block_ids.iter().any(|member| member == id))
        })
        .map(str::to_owned)
        .collect()
}

pub fn protected_bounds(page: &Value, width: i32, height: i32) -> Vec<Box4> {
    let blocks = array(&page["blocks"]);
    let padding = (width.max(height) / 400).clamp(3, 7);
    saved(page)
        .into_iter()
        .filter_map(|bubble| bubble.bounds)
        .chain(
            blocks
                .iter()
                .filter(|b| keeps_original(b))
                .filter_map(|b| ink::clipped_box(b, width, height)),
        )
        .map(|[x, y, r, b]| {
            [
                x.saturating_sub(padding).max(0),
                y.saturating_sub(padding).max(0),
                r.saturating_add(padding).min(width),
                b.saturating_add(padding).min(height),
            ]
        })
        .filter(|[x, y, r, b]| x < r && y < b)
        .collect()
}

pub fn touches(candidate: &Candidate, bounds: &[Box4]) -> Result<bool> {
    for &area in bounds {
        if geometry::coverage(candidate, Some(area))? > 0.0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn outline(candidate: &Candidate) -> Box4 {
    let [x, y] = candidate.origin;
    [x, y, x + candidate.mask.cols(), y + candidate.mask.rows()]
}

fn enclosing(a: Option<Box4>, b: Option<Box4>) -> Option<Box4> {
    match (a, b) {
        (Some([x, y, r, d]), Some([a, b, c, e])) => Some([x.min(a), y.min(b), r.max(c), d.max(e)]),
        (a, b) => a.or(b),
    }
}

fn find(page: &Value, id: &str, image: &[u8], reading_regions: &[Value]) -> Result<Bubble> {
    let width = page["width"].as_i64().context("missing page width")? as i32;
    let height = page["height"].as_i64().context("missing page height")? as i32;
    let blocks = array(&page["blocks"]);
    let index = blocks
        .iter()
        .position(|block| block["id"] == id)
        .context("缓存中没有找到这条对白。")?;
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| ink::clipped_box(b, width, height))
        .collect();

    let belongs = |region: &Value| {
        array(&region["block_ids"])
            .iter()
            .any(|member| member == id)
    };
    let reading = reading_regions
        .iter()
        .filter(|region| region["overlay"] == "balloon" || array(&region["block_ids"]).len() > 1);
    let stored = array(&page["overlay_regions"])
        .iter()
        .filter(|region| region["overlay"] == "balloon" || array(&region["block_ids"]).len() > 1);
    for region in reading.chain(stored).filter(|region| belongs(region)) {
        if let Some(shape) = geometry::cached_shape(region, width, height)? {
            let block_ids = array(&region["block_ids"])
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            return Ok(Bubble {
                block_ids,
                bounds: Some(outline(&shape)),
            });
        }
    }

    // Older pages can lack a stored contour. Resolve membership here, once,
    // using geometry alone. The protected area is reused on later renders.
    let gray = imaging::decode_bytes(image, imgcodecs::IMREAD_GRAYSCALE)?;
    ensure!(
        gray.cols() == width && gray.rows() == height,
        "漫画尺寸已更新，请重新打开页面。"
    );
    if let Some(shape) = geometry::bubbles(&gray, &boxes)?
        .into_iter()
        .find(|shape| shape.members.contains(&index))
    {
        let block_ids = shape
            .members
            .iter()
            .filter_map(|&i| blocks[i]["id"].as_str())
            .map(str::to_owned)
            .collect();
        return Ok(Bubble {
            block_ids,
            bounds: Some(outline(&shape)),
        });
    }

    if let Some(dialogue) = array(&page["dialogues"]).iter().find(|group| {
        belongs(group) && matches!(group["grouping_method"].as_str(), Some("manual" | "region"))
    }) {
        let block_ids: Vec<_> = array(&dialogue["block_ids"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let bounds = blocks
            .iter()
            .zip(boxes)
            .filter_map(|(block, area)| {
                area.filter(|_| {
                    block["id"]
                        .as_str()
                        .is_some_and(|id| block_ids.iter().any(|member| member == id))
                })
            })
            .fold(None, |whole, area| enclosing(whole, Some(area)));
        return Ok(Bubble { block_ids, bounds });
    }
    // Without an established outline, retain only the known text area.
    Ok(Bubble {
        block_ids: vec![id.to_owned()],
        bounds: boxes[index],
    })
}

pub fn set(
    page: &mut Value,
    id: &str,
    keep: bool,
    image: &[u8],
    reading_regions: &[Value],
) -> Result<()> {
    let mut groups = saved(page);
    let existing = groups
        .iter()
        .position(|group| group.block_ids.iter().any(|member| member == id));
    let ids = if keep {
        if existing.is_some() {
            return Ok(());
        }
        let mut bubble = find(page, id, image, reading_regions)?;
        groups.retain(|other| {
            if !other
                .block_ids
                .iter()
                .any(|member| bubble.block_ids.contains(member))
            {
                return true;
            }
            bubble.block_ids.extend(other.block_ids.iter().cloned());
            bubble.bounds = enclosing(bubble.bounds, other.bounds);
            false
        });
        bubble.block_ids.sort();
        bubble.block_ids.dedup();
        let ids = bubble.block_ids.clone();
        groups.push(bubble);
        ids
    } else {
        existing.map_or_else(
            || vec![id.to_owned()],
            |index| groups.remove(index).block_ids,
        )
    };
    let blocks = page["blocks"]
        .as_array_mut()
        .context("invalid page blocks")?;
    ensure!(
        blocks.iter().any(|block| block["id"] == id),
        "缓存中没有找到这条对白。"
    );
    for block in blocks {
        if block["id"]
            .as_str()
            .is_some_and(|id| ids.iter().any(|member| member == id))
        {
            block["keep_original"] = json!(keep);
        }
    }
    page["preserved_bubbles"] = json!(groups);
    Ok(())
}
