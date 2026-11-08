use anyhow::{Context, Result, bail};
use opencv::{imgcodecs, imgproc, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn decode_color(path: &Path) -> Result<Mat> {
    decode(path, imgcodecs::IMREAD_COLOR)
}

pub fn decode_gray(path: &Path) -> Result<Mat> {
    decode(path, imgcodecs::IMREAD_GRAYSCALE)
}

pub fn display_gray(path: &Path) -> Result<Mat> {
    let color = decode_color(path)?;
    let mut gray = Mat::default();
    imgproc::cvt_color_def(&color, &mut gray, imgproc::COLOR_BGR2GRAY)?;
    Ok(gray)
}

fn decode(path: &Path, flags: i32) -> Result<Mat> {
    let bytes = std::fs::read(path).context("read image bytes")?;
    decode_bytes(&bytes,flags)
}

pub fn decode_bytes(bytes:&[u8],flags:i32)->Result<Mat>{
    let encoded = opencv::core::Vector::<u8>::from_slice(bytes);
    let image = imgcodecs::imdecode(&encoded, flags)?;
    if image.empty() {
        bail!("image decoder returned an empty image")
    }
    Ok(image)
}

pub fn image_probe(path: &Path) -> Result<Value> {
    let image = decode_color(path)?;
    Ok(
        json!({"image": path, "width": image.cols(), "height": image.rows(),
        "channels": image.channels(), "bgr_sha256": format!("{:x}", Sha256::digest(image.data_bytes()?))}),
    )
}
