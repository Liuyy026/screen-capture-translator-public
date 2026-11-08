//! Native visual-model input encoding compatible with Pillow RGB thumbnails.
//! Lanczos coefficients, fixed-point rounding and integer pre-reduction follow
//! Pillow 12.3.0 (MIT-CMU); see ../LICENSES/Pillow.txt and THIRD_PARTY.md.
use crate::vision_decode;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use opencv::{
    core::{CV_8UC3, Mat, Scalar, Vector},
    imgcodecs,
    prelude::*,
};
use std::path::Path;

pub const IMAGE_EDGE: usize = 1775;
const PRECISION: i64 = 1 << 22;

pub struct EncodedImage {
    pub url: String,
    pub width: usize,
    pub height: usize,
}

fn dimensions(w: usize, h: usize, edge: usize) -> (usize, usize) {
    if w <= edge && h <= edge {
        return (w, h);
    }
    let aspect = w as f64 / h as f64;
    if aspect <= 1. {
        let n = edge as f64 * aspect;
        let (lo, hi) = (n.floor(), n.ceil());
        let rounded = if (aspect - lo / edge as f64).abs() <= (aspect - hi / edge as f64).abs() {
            lo
        } else {
            hi
        };
        (rounded.max(1.) as usize, edge)
    } else {
        let n = edge as f64 / aspect;
        let (lo, hi) = (n.floor(), n.ceil());
        let error = |v: f64| {
            if v == 0. {
                0.
            } else {
                (aspect - edge as f64 / v).abs()
            }
        };
        let rounded = if error(lo) <= error(hi) { lo } else { hi };
        (edge, rounded.max(1.) as usize)
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0. {
        1.
    } else {
        let p = x * std::f64::consts::PI;
        p.sin() / p
    }
}
fn coefficients(input: usize, extent: f64, output: usize) -> Vec<(usize, Vec<i64>)> {
    // libImaging receives its box as float32, even when Python used doubles.
    let scale = (extent as f32) as f64 / output as f64;
    let filter_scale = scale.max(1.);
    let support = 3. * filter_scale;
    let inverse = 1. / filter_scale;
    (0..output)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let start = ((center - support + 0.5) as isize).max(0) as usize;
            let end = ((center + support + 0.5) as usize).min(input);
            let mut values: Vec<_> = (start..end)
                .map(|x| {
                    let position = (x as f64 - center + 0.5) * inverse;
                    if (-3.0..3.0).contains(&position) {
                        sinc(position) * sinc(position / 3.)
                    } else {
                        0.
                    }
                })
                .collect();
            let sum: f64 = values.iter().sum();
            if sum != 0. {
                for v in &mut values {
                    *v /= sum;
                }
            }
            let fixed = values
                .into_iter()
                .map(|v| ((if v < 0. { -0.5 } else { 0.5 }) + v * PRECISION as f64) as i64)
                .collect();
            (start, fixed)
        })
        .collect()
}

fn reduce(input: &[u8], w: usize, h: usize, fx: usize, fy: usize) -> (Vec<u8>, usize, usize) {
    let (rw, rh) = (w.div_ceil(fx), h.div_ceil(fy));
    let mut result = vec![0; rw * rh * 3];
    for y in 0..rh {
        for x in 0..rw {
            let (left, top, right, bottom) =
                (x * fx, y * fy, ((x + 1) * fx).min(w), ((y + 1) * fy).min(h));
            let count = (right - left) * (bottom - top);
            let multiplier = ((1u64 << 32) as f32 / (256 * count) as f32) as u64;
            let mut sums = [(count / 2) as u64; 3];
            for yy in top..bottom {
                for xx in left..right {
                    for c in 0..3 {
                        sums[c] += input[(yy * w + xx) * 3 + c] as u64;
                    }
                }
            }
            for c in 0..3 {
                result[(y * rw + x) * 3 + c] = ((sums[c] * multiplier) >> 24) as u8;
            }
        }
    }
    (result, rw, rh)
}

fn horizontal(input: &[u8], w: usize, h: usize, output: usize, extent: f64) -> Vec<u8> {
    if w == output && extent == output as f64 {
        return input.to_vec();
    }
    let coefficients = coefficients(w, extent, output);
    let mut result = vec![0; output * h * 3];
    for y in 0..h {
        for (x, (start, weights)) in coefficients.iter().enumerate() {
            for c in 0..3 {
                let mut sum = PRECISION / 2;
                for (i, &weight) in weights.iter().enumerate() {
                    sum += input[(y * w + start + i) * 3 + c] as i64 * weight;
                }
                result[(y * output + x) * 3 + c] = (sum >> 22).clamp(0, 255) as u8;
            }
        }
    }
    result
}
fn vertical(input: &[u8], w: usize, h: usize, output: usize, extent: f64) -> Vec<u8> {
    if h == output && extent == output as f64 {
        return input.to_vec();
    }
    let coefficients = coefficients(h, extent, output);
    let mut result = vec![0; w * output * 3];
    for (y, (start, weights)) in coefficients.iter().enumerate() {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = PRECISION / 2;
                for (i, &weight) in weights.iter().enumerate() {
                    sum += input[((start + i) * w + x) * 3 + c] as i64 * weight;
                }
                result[(y * w + x) * 3 + c] = (sum >> 22).clamp(0, 255) as u8;
            }
        }
    }
    result
}

pub fn thumbnail(color: &Mat, edge: usize) -> Result<Mat> {
    ensure!(
        color.typ() == CV_8UC3 && !color.empty() && edge > 0,
        "invalid visual input image"
    );
    let (w, h) = (color.cols() as usize, color.rows() as usize);
    let (tw, th) = dimensions(w, h, edge);
    if (w, h) == (tw, th) {
        return Ok(color.try_clone()?);
    }
    let (fx, fy) = (
        ((w as f64 / tw as f64 / 2.) as usize).max(1),
        ((h as f64 / th as f64 / 2.) as usize).max(1),
    );
    let (pixels, rw, rh) = if fx > 1 || fy > 1 {
        reduce(color.data_bytes()?, w, h, fx, fy)
    } else {
        (color.data_bytes()?.to_vec(), w, h)
    };
    let (extent_x, extent_y) = (w as f64 / fx as f64, h as f64 / fy as f64);
    let result = if rh > rw * 100 && th < rh {
        horizontal(
            &vertical(&pixels, rw, rh, th, extent_y),
            rw,
            th,
            tw,
            extent_x,
        )
    } else {
        vertical(
            &horizontal(&pixels, rw, rh, tw, extent_x),
            tw,
            rh,
            th,
            extent_y,
        )
    };
    let mut output =
        Mat::new_rows_cols_with_default(th as i32, tw as i32, CV_8UC3, Scalar::all(0.))?;
    output.data_bytes_mut()?.copy_from_slice(&result);
    Ok(output)
}

pub fn encode(path: &Path) -> Result<EncodedImage> {
    let original = vision_decode::decode(path)?;
    let image = thumbnail(&original, IMAGE_EDGE)?;
    let mut bytes = Vector::new();
    imgcodecs::imencode(
        ".jpg",
        &image,
        &mut bytes,
        &Vector::from_slice(&[imgcodecs::IMWRITE_JPEG_QUALITY, 92]),
    )?;
    Ok(EncodedImage {
        url: format!(
            "data:image/jpeg;base64,{}",
            STANDARD.encode(bytes.as_slice())
        ),
        width: image.cols() as usize,
        height: image.rows() as usize,
    })
}
