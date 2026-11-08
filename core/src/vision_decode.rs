//! Match Pillow's RGB conversion/orientation for the reader's input formats.
use anyhow::{Result, bail, ensure};
use opencv::{
    core::{self, CV_8U, CV_8UC3, CV_16U, CV_32F, CV_32S, Mat, Scalar, Vector},
    imgcodecs,
    prelude::*,
};
use std::path::Path;

unsafe extern "C" {
    fn koma_jpeg_cmyk(
        data: *const u8,
        length: usize,
        result: *mut *mut u8,
        width: *mut u32,
        height: *mut u32,
    ) -> i32;
    fn koma_jpeg_free(data: *mut u8);
}
struct CmykBuffer(*mut u8);
impl Drop for CmykBuffer {
    fn drop(&mut self) {
        unsafe {
            koma_jpeg_free(self.0);
        }
    }
}

fn jpeg_cmyk(bytes: &[u8]) -> Result<Option<Mat>> {
    let (mut pointer, mut width, mut height) = (std::ptr::null_mut(), 0, 0);
    // The shim bounds dimensions, allocates exactly w*h*4 bytes, and transfers
    // ownership only on success. Its allocator is paired with its own free.
    let result = unsafe {
        koma_jpeg_cmyk(
            bytes.as_ptr(),
            bytes.len(),
            &mut pointer,
            &mut width,
            &mut height,
        )
    };
    if result == 0 {
        return Ok(None);
    }
    ensure!(result == 1 && !pointer.is_null(), "invalid JPEG image");
    let owned = CmykBuffer(pointer);
    let raw = unsafe { std::slice::from_raw_parts(owned.0, width as usize * height as usize * 4) };
    let mut color =
        Mat::new_rows_cols_with_default(height as i32, width as i32, CV_8UC3, Scalar::all(0.))?;
    for (src, dst) in raw
        .chunks_exact(4)
        .zip(color.data_bytes_mut()?.chunks_exact_mut(3))
    {
        for c in 0..3 {
            dst[c] = ((src[2 - c] as u32 * src[3] as u32 + 127) / 255) as u8;
        }
    }
    Ok(Some(color))
}

fn tiff_orientation(bytes: &[u8]) -> Option<u32> {
    let bytes = bytes.strip_prefix(b"Exif\0\0").unwrap_or(bytes);
    let little = match bytes.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let short = |at: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let long = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if short(2)? != 42 {
        return None;
    }
    let offset = long(4)? as usize;
    let count = short(offset)? as usize;
    for index in 0..count {
        let at = offset.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
        if short(at)? == 274 && short(at + 2)? == 3 && long(at + 4)? == 1 {
            return short(at + 8).map(u32::from);
        }
    }
    None
}

fn orientation(bytes: &[u8]) -> u32 {
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let mut pos = 12usize;
        while let Some(header) = bytes.get(pos..pos.saturating_add(8)) {
            let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let Some(end) = pos.checked_add(8).and_then(|v| v.checked_add(size)) else {
                break;
            };
            let Some(chunk) = bytes.get(pos + 8..end) else {
                break;
            };
            if &header[..4] == b"EXIF" {
                return tiff_orientation(chunk).unwrap_or(1);
            }
            pos = end.saturating_add(size % 2);
        }
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut pos = 8usize;
        while let Some(header) = bytes.get(pos..pos.saturating_add(8)) {
            let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            let Some(end) = pos.checked_add(8).and_then(|v| v.checked_add(size)) else {
                break;
            };
            let Some(chunk) = bytes.get(pos + 8..end) else {
                break;
            };
            if &header[4..] == b"eXIf" {
                return tiff_orientation(chunk).unwrap_or(1);
            }
            pos = end.saturating_add(4);
        }
    } else if bytes.starts_with(b"\xff\xd8") {
        let mut pos = 2usize;
        while bytes.get(pos) == Some(&255) {
            while bytes.get(pos) == Some(&255) {
                pos += 1;
            }
            let Some(&marker) = bytes.get(pos) else {
                break;
            };
            pos += 1;
            if marker == 0xda || marker == 0xd9 {
                break;
            }
            if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                continue;
            }
            let Some(length) = bytes.get(pos..pos.saturating_add(2)) else {
                break;
            };
            let size = u16::from_be_bytes(length.try_into().unwrap()) as usize;
            if size < 2 {
                break;
            }
            let Some(end) = pos.checked_add(size) else {
                break;
            };
            let Some(chunk) = bytes.get(pos + 2..end) else {
                break;
            };
            if marker == 0xe1 && chunk.starts_with(b"Exif\0\0") {
                return tiff_orientation(chunk).unwrap_or(1);
            }
            pos = end;
        }
    }
    1
}

pub fn decode(path: &Path) -> Result<Mat> {
    let bytes = std::fs::read(path)?;
    let cmyk = if bytes.starts_with(b"\xff\xd8") {
        jpeg_cmyk(&bytes)?
    } else {
        None
    };
    let mut color = if let Some(color) = cmyk {
        color
    } else {
        let raw = imgcodecs::imdecode(&Vector::from_slice(&bytes), imgcodecs::IMREAD_UNCHANGED)?;
        ensure!(!raw.empty(), "image decoder returned an empty image");
        let channels = raw.channels() as usize;
        ensure!([1, 3, 4].contains(&channels), "unsupported image channels");
        let mut out =
            Mat::new_rows_cols_with_default(raw.rows(), raw.cols(), CV_8UC3, Scalar::all(0.))?;
        let pixels = out.data_bytes_mut()?;
        let storage = raw.data_bytes()?;
        let depth = raw.depth();
        for (i, dst) in pixels.chunks_exact_mut(3).enumerate() {
            for c in 0..3 {
                let index = i * channels + if channels == 1 { 0 } else { c };
                dst[c] = match depth {
                    CV_8U => storage[index],
                    CV_16U => {
                        let v = u16::from_ne_bytes(
                            storage[index * 2..index * 2 + 2].try_into().unwrap(),
                        );
                        if channels == 1 {
                            v.min(255) as u8
                        } else {
                            (v >> 8) as u8
                        }
                    }
                    CV_32S => {
                        i32::from_ne_bytes(storage[index * 4..index * 4 + 4].try_into().unwrap())
                            .clamp(0, 255) as u8
                    }
                    CV_32F => {
                        f32::from_ne_bytes(storage[index * 4..index * 4 + 4].try_into().unwrap())
                            .clamp(0., 255.) as u8
                    }
                    _ => bail!("unsupported image depth"),
                };
            }
        }
        out
    };
    // OpenCV's TIFF decoder applies TIFF orientation internally even with
    // IMREAD_UNCHANGED. Other formats are oriented here from bounded metadata.
    let turn = orientation(&bytes);
    if (2..=8).contains(&turn) {
        let mut out = Mat::default();
        match turn {
            2 => core::flip(&color, &mut out, 1)?,
            3 => core::flip(&color, &mut out, -1)?,
            4 => core::flip(&color, &mut out, 0)?,
            5 => core::transpose(&color, &mut out)?,
            6 => core::rotate(&color, &mut out, core::ROTATE_90_CLOCKWISE)?,
            7 => {
                let mut transposed = Mat::default();
                core::transpose(&color, &mut transposed)?;
                core::flip(&transposed, &mut out, -1)?;
            }
            8 => core::rotate(&color, &mut out, core::ROTATE_90_COUNTERCLOCKWISE)?,
            _ => unreachable!(),
        }
        color = out;
    }
    Ok(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_jpeg_returns_an_error_across_the_c_boundary() {
        for bytes in [
            b"\xff\xd8".as_slice(),
            b"\xff\xd8\xff\xe1\0\x20Exif\0\0".as_slice(),
        ] {
            assert!(jpeg_cmyk(bytes).is_err());
        }
    }

    #[test]
    fn malformed_orientation_metadata_cannot_escape_its_container() {
        assert_eq!(
            orientation(b"RIFF\xff\xff\xff\xffWEBPEXIF\xff\xff\xff\xff"),
            1
        );
        assert_eq!(orientation(b"\x89PNG\r\n\x1a\n\xff\xff\xff\xffeXIf"), 1);
        assert_eq!(tiff_orientation(b"II\x2a\0\xff\xff\xff\xff"), None);
        assert_eq!(tiff_orientation(b"MM\0\x2a\0\0\0\x08\xff\xff"), None);
    }
}
