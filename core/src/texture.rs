//! Deterministic non-model screentone repair. Tone and texture are computed in
//! Rust/OpenCV; Python is not involved in the fallback path.
use crate::raster::*;
use anyhow::Result;
use opencv::{core::Mat, imgproc, photo, prelude::*};

// Match the reference float32 reduction order, including its 128-value blocks.
fn pairwise_sum(values: &[f32]) -> f32 {
    let n = values.len();
    if n < 8 {
        return values.iter().fold(-0.0, |a, &b| a + b);
    }
    if n <= 128 {
        let mut sums: [f32; 8] = values[..8].try_into().unwrap();
        let mut i = 8;
        while i + 8 <= n {
            for j in 0..8 {
                sums[j] += values[i + j];
            }
            i += 8;
        }
        let mut total = ((sums[0] + sums[1]) + (sums[2] + sums[3]))
            + ((sums[4] + sums[5]) + (sums[6] + sums[7]));
        while i < n {
            total += values[i];
            i += 1;
        }
        total
    } else {
        let midpoint = (n / 2) / 8 * 8;
        pairwise_sum(&values[..midpoint]) + pairwise_sum(&values[midpoint..])
    }
}

pub fn fill(patch: &Mat, ink: &Mat) -> Result<Mat> {
    let smooth = blur(patch, 0, 2.2)?;
    let tone_mask = morph(ink, 13, true)?;
    let mut restored = Mat::default();
    photo::inpaint(&smooth, &tone_mask, &mut restored, 7.0, photo::INPAINT_NS)?;
    let mut gradient = Mat::default();
    imgproc::morphology_ex_def(
        &smooth,
        &mut gradient,
        imgproc::MORPH_GRADIENT,
        &blank(7, 7, 1)?,
    )?;
    let (width, height) = (patch.cols(), patch.rows());
    let (pixels, smoothed, mask, tones, grad) = (
        patch.data_bytes()?,
        smooth.data_bytes()?,
        ink.data_bytes()?,
        tone_mask.data_bytes()?,
        gradient.data_bytes()?,
    );
    let details: Vec<f32> = pixels
        .iter()
        .zip(smoothed)
        .map(|(&a, &b)| a as f32 - b as f32)
        .collect();
    let clean: Vec<bool> = tones
        .iter()
        .zip(grad)
        .map(|(&t, &g)| t == 0 && g < 55)
        .collect();
    let mut shifts = vec![];
    for dy in [-96i32, -64, -32, -16, 0, 16, 32, 64, 96] {
        for dx in (-160i32..=160).step_by(4) {
            if dx.abs() + dy.abs() < 12 {
                continue;
            }
            let mut count = 0usize;
            let mut errors = vec![];
            for y in dy.max(0)..(height + dy).min(height) {
                for x in dx.max(0)..(width + dx).min(width) {
                    let p = (y * width + x) as usize;
                    let donor = ((y - dy) * width + x - dx) as usize;
                    if clean[p] && clean[donor] {
                        count += 1;
                        if y % 2 == 0 && x % 2 == 0 {
                            let delta = details[p] - details[donor];
                            errors.push(delta * delta);
                        }
                    }
                }
            }
            if count < 300 {
                continue;
            }
            let error = pairwise_sum(&errors) / errors.len() as f32;
            shifts.push((error, dx, dy));
        }
    }
    // Reference sorts tuples by phase error, then x/y offset, stably.
    shifts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let original_restored = restored.data_bytes()?.to_vec();
    let result = restored.data_bytes_mut()?;
    let mut best = vec![f32::INFINITY; pixels.len()];
    for (phase_error, dx, dy) in shifts {
        let displacement = (0.025f64 * (dx.abs() + dy.abs()) as f64) as f32;
        let phase_cost = (0.002f64 * phase_error as f64) as f32;
        for y in dy.max(0)..(height + dy).min(height) {
            for x in dx.max(0)..(width + dx).min(width) {
                let p = (y * width + x) as usize;
                let donor = ((y - dy) * width + x - dx) as usize;
                if !clean[donor] || mask[p] == 0 {
                    continue;
                }
                let tone_delta = (smoothed[donor] as f32 - original_restored[p] as f32).abs();
                if tone_delta >= 22.0 {
                    continue;
                }
                let cost = (tone_delta + displacement) + phase_cost;
                if cost < best[p] {
                    result[p] = pixels[donor];
                    best[p] = cost;
                }
            }
        }
    }
    Ok(restored)
}
