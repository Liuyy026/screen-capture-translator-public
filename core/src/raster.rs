//! Small image operations shared by the native geometry and cleanup stages.
use anyhow::Result;
use opencv::{
    core::{self, Mat, Point, Rect, Scalar, Vector},
    imgproc,
    prelude::*,
};

pub fn blank(width: i32, height: i32, value: u8) -> Result<Mat> {
    Ok(Mat::new_rows_cols_with_default(
        height,
        width,
        core::CV_8UC1,
        Scalar::all(value as f64),
    )?)
}

pub fn crop(image: &Mat, x: i32, y: i32, width: i32, height: i32) -> Result<Mat> {
    Ok(Mat::roi(image, Rect::new(x, y, width, height))?.try_clone()?)
}

pub fn pad(image: &Mat, margin: i32, replicate: bool) -> Result<Mat> {
    let mut result = Mat::default();
    core::copy_make_border(
        image,
        &mut result,
        margin,
        margin,
        margin,
        margin,
        if replicate {
            core::BORDER_REPLICATE
        } else {
            core::BORDER_CONSTANT
        },
        Scalar::all(0.0),
    )?;
    Ok(result)
}

pub fn threshold(image: &Mat, predicate: impl Fn(u8) -> bool) -> Result<Mat> {
    let mut result = blank(image.cols(), image.rows(), 0)?;
    for (out, &pixel) in result.data_bytes_mut()?.iter_mut().zip(image.data_bytes()?) {
        *out = u8::from(predicate(pixel));
    }
    Ok(result)
}

pub fn contours(mask: &Mat, mode: i32) -> Result<Vector<Vector<Point>>> {
    let mut result = Vector::new();
    imgproc::find_contours_def(mask, &mut result, mode, imgproc::CHAIN_APPROX_SIMPLE)?;
    Ok(result)
}

pub fn filled(width: i32, height: i32, contour: &Vector<Point>) -> Result<Mat> {
    let mut result = blank(width, height, 0)?;
    let contours = Vector::<Vector<Point>>::from_iter([contour.clone()]);
    imgproc::draw_contours(
        &mut result,
        &contours,
        -1,
        Scalar::all(1.0),
        imgproc::FILLED,
        imgproc::LINE_8,
        &core::no_array(),
        i32::MAX,
        Point::new(0, 0),
    )?;
    Ok(result)
}

pub fn components(mask: &Mat) -> Result<(Mat, Vec<[i32; 5]>)> {
    let (mut labels, mut stats, mut centers) = (Mat::default(), Mat::default(), Mat::default());
    imgproc::connected_components_with_stats(
        mask,
        &mut labels,
        &mut stats,
        &mut centers,
        8,
        core::CV_32S,
    )?;
    let stats = stats
        .data_typed::<i32>()?
        .chunks_exact(5)
        .map(|row| row.try_into().unwrap())
        .collect();
    Ok((labels, stats))
}

pub fn morph(mask: &Mat, size: i32, dilate: bool) -> Result<Mat> {
    let kernel = blank(size, size, 1)?;
    let mut result = Mat::default();
    if dilate {
        imgproc::dilate_def(mask, &mut result, &kernel)?;
    } else {
        imgproc::erode_def(mask, &mut result, &kernel)?;
    }
    Ok(result)
}

pub fn ring(mask: &Mat, margin: i32) -> Result<Mat> {
    let padded = pad(mask, margin, false)?;
    let mut result = morph(&padded, margin * 2 + 1, true)?;
    for (out, &inside) in result
        .data_bytes_mut()?
        .iter_mut()
        .zip(padded.data_bytes()?)
    {
        *out -= inside;
    }
    Ok(result)
}

pub fn distance(mask: &Mat, padded: bool) -> Result<Mat> {
    let input = if padded {
        pad(mask, 1, false)?
    } else {
        mask.try_clone()?
    };
    let mut result = Mat::default();
    imgproc::distance_transform_def(&input, &mut result, imgproc::DIST_L2, 5)?;
    if padded {
        crop(&result, 1, 1, mask.cols(), mask.rows())
    } else {
        Ok(result)
    }
}

pub fn select(image: &Mat, mask: &Mat) -> Result<Vec<u8>> {
    anyhow::ensure!(
        image.size()? == mask.size()? && image.typ() == core::CV_8UC1,
        "invalid selection images"
    );
    Ok(image
        .data_bytes()?
        .iter()
        .zip(mask.data_bytes()?)
        .filter_map(|(&v, &m)| (m > 0).then_some(v))
        .collect())
}

pub fn fraction(values: &[u8], predicate: impl Fn(u8) -> bool) -> f64 {
    values.iter().filter(|&&v| predicate(v)).count() as f64 / values.len() as f64
}

/// NumPy's default linear percentile on a uint8 array, without sorting pixels.
pub fn percentile(values: &[u8], percent: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut histogram = [0usize; 256];
    for &v in values {
        histogram[v as usize] += 1;
    }
    let rank = (values.len() - 1) as f64 * (percent / 100.0);
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    let mut total = 0;
    let (mut low, mut high) = (None, 255.0);
    for (value, &count) in histogram.iter().enumerate() {
        total += count;
        if total > lo && low.is_none() {
            low = Some(value as f64);
        }
        if total > hi {
            high = value as f64;
            break;
        }
    }
    let low = low.unwrap();
    let t = rank - lo as f64;
    // Same stable lerp branch as NumPy.
    if t >= 0.5 {
        high - (high - low) * (1.0 - t)
    } else {
        low + (high - low) * t
    }
}

pub fn clear_rect(mask: &mut Mat, bounds: [i32; 4]) -> Result<()> {
    let width = mask.cols();
    let height = mask.rows();
    let [left, top, right, bottom] = bounds;
    let (left, right) = (left.clamp(0, width), right.clamp(0, width));
    let pixels = mask.data_bytes_mut()?;
    if right <= left {
        return Ok(());
    }
    for y in top.clamp(0, height)..bottom.clamp(0, height) {
        pixels[(y * width + left) as usize..(y * width + right) as usize].fill(0);
    }
    Ok(())
}

pub fn blur(image: &Mat, size: i32, sigma: f64) -> Result<Mat> {
    let mut result = Mat::default();
    imgproc::gaussian_blur_def(image, &mut result, core::Size::new(size, size), sigma)?;
    Ok(result)
}
