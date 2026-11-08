//! Assemble a page from model-only OCR output, then classify/group in Rust.
use crate::{model_images::ImageModels,overlay,structure};
use anyhow::{Result,ensure,Context};
use serde_json::{Value,json};
use std::path::Path;

pub fn assemble(raw:&Value)->Result<Value>{
    let width=raw["img_width"].as_u64().context("OCR image width")?;
    let height=raw["img_height"].as_u64().context("OCR image height")?;
    ensure!(width>0&&height>0&&width<=i32::MAX as u64&&height<=i32::MAX as u64,"invalid OCR image dimensions");
    let mut blocks=vec![];
    for raw_block in raw["blocks"].as_array().context("OCR blocks")?{
        let lines=raw_block["lines"].as_array().context("OCR lines")?;
        let mut source=String::new();
        for line in lines{source.push_str(line.as_str().context("OCR line must be text")?);}
        let source=source.trim_matches(|c:char|c.is_whitespace()||('\u{1c}'..='\u{1f}').contains(&c));
        if source.is_empty(){continue;}
        let values=raw_block["box"].as_array().context("OCR rectangle")?;
        ensure!(values.len()==4,"invalid OCR rectangle");
        let mut bounds=vec![];
        for n in values{let n=n.as_f64().context("OCR coordinate")?;ensure!(n.is_finite(),"nonfinite OCR coordinate");bounds.push(n);}
        blocks.push(json!({"id":(blocks.len()+1).to_string(),"box":bounds,"source":source,"translation":"","error":"",
            "vertical":raw_block["vertical"].as_bool().context("OCR text orientation")?,"edited":false}));
    }
    Ok(json!({"version":2,"width":width,"height":height,"blocks":blocks,"model":"","ocr":"mokuro 0.2.0 / manga-ocr"}))
}

pub async fn recognize(image:&Path,models:&mut ImageModels)->Result<Value>{
    let raw=models.ocr(image).await?;
    let mut page=assemble(&raw)?;
    overlay::ensure(&mut page,Some(image))?;
    structure::ensure(&mut page,Some(image))?;
    Ok(page)
}
