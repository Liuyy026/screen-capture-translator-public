mod background;
mod compat;
mod geometry;
mod imaging;
mod ink;
mod model_images;
mod models;
mod overlay;
mod panels;
mod persistence;
mod raster;
mod reading;
mod service;
mod structure;
mod texture;
mod translate_pipeline;
mod translation;
mod vision_decode;
mod vision_image;
mod vision_pipeline;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Koma native processing core (migration in progress)")]
struct Cli {
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Encode exactly the visual-model image input, without sending it.
    VisionImage {
        #[arg(long)]
        image: PathBuf,
    },
    /// Translate an existing OCR page and checkpoint each batch atomically.
    Translate {
        #[arg(long, requires = "image")]
        vision: bool,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        status: Option<PathBuf>,
        #[arg(long)]
        metrics: Option<PathBuf>,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "omlx")]
        provider: String,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long,default_value="blocks",value_parser=["blocks","dialogues"])]
        unit: String,
        #[arg(long, default_value_t = 8)]
        batch_size: usize,
        #[arg(long)]
        block_id: Option<String>,
    },
    /// Validate full native translation orchestration against fake services.
    TranslationProbe {
        #[arg(long)]
        requests: PathBuf,
    },
    /// Validate visual translation/cache seeding against controlled fixtures.
    VisionProbe {
        #[arg(long)]
        requests: PathBuf,
    },
    /// Rebuild OCR-side region metadata without mutating the input file.
    OverlayProbe {
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long,default_value="ensure",value_parser=["ensure","adjacent"])]
        mode: String,
    },
    /// Exercise response validation and prompt construction against fixtures.
    ProtocolProbe {
        #[arg(long)]
        requests: PathBuf,
    },
    /// Exercise transport/provider behavior against controlled test endpoints.
    ServiceProbe {
        #[arg(long)]
        requests: PathBuf,
    },
    /// List existing local translation models, without loading a model.
    ListModels {
        #[arg(long, default_value = "omlx")]
        provider: String,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Generate final display regions from immutable OCR/translation inputs.
    Display {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long)]
        text_mask: Option<PathBuf>,
        #[arg(long,requires_all=["worker","weights"])]
        python: Option<PathBuf>,
        #[arg(long,requires_all=["python","weights"])]
        worker: Option<PathBuf>,
        #[arg(long,requires_all=["python","worker"])]
        weights: Option<PathBuf>,
    },
    /// Render local repair requests; optional model paths enable local LaMa.
    BackgroundRender {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        requests: PathBuf,
        #[arg(long)]
        mask: Option<PathBuf>,
        #[arg(long,requires_all=["worker","weights"])]
        python: Option<PathBuf>,
        #[arg(long,requires_all=["python","weights"])]
        worker: Option<PathBuf>,
        #[arg(long,requires_all=["python","worker"])]
        weights: Option<PathBuf>,
    },
    /// Inspect local restoration masks without running an image model.
    BackgroundProbe {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        requests: PathBuf,
        #[arg(long)]
        mask: Option<PathBuf>,
    },
    /// Inspect native balloon contours and source-bounded erasure geometry.
    GeometryProbe {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long)]
        mask: Option<PathBuf>,
    },
    /// Inspect native display boxes and completed detached-stroke masks.
    InkProbe {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long)]
        mask: Option<PathBuf>,
    },
    /// Run native grouping/synchronization without changing the input file.
    StructureProbe {
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        cache: PathBuf,
        #[arg(long, default_value = "ensure", value_parser = ["ensure", "build", "sync"])]
        mode: String,
    },
    /// Rebuild panel metadata without mutating the source cache.
    PanelProbe {
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        cache: PathBuf,
    },
    /// Decode images in native OpenCV and report exact BGR pixel hashes.
    ImageProbe {
        #[arg(required = true)]
        images: Vec<PathBuf>,
    },
    /// Validate the persistent model-process protocol without loading weights.
    ModelHealth {
        #[arg(long)]
        python: PathBuf,
        #[arg(long)]
        worker: PathBuf,
        #[arg(long)]
        weights: PathBuf,
    },
    /// Execute a model-only validation batch in a single persistent process.
    ModelRun {
        #[arg(long)]
        python: PathBuf,
        #[arg(long)]
        worker: PathBuf,
        #[arg(long)]
        weights: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        requests: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Operation::VisionProbe { requests } => {
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut replies = vec![];
            for request in requests {
                replies.push(vision_pipeline::probe(&request).await?);
            }
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::VisionImage { image } => {
            let encoded = vision_image::encode(&image)?;
            println!(
                "{}",
                json!({"url":encoded.url,"width":encoded.width,"height":encoded.height})
            );
        }
        Operation::TranslationProbe { requests } => {
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut replies = vec![];
            for request in requests {
                replies.push(translate_pipeline::probe(&request).await?);
            }
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::Translate {
            vision,
            cache,
            destination,
            image,
            status,
            metrics,
            model,
            provider,
            base_url,
            unit,
            batch_size,
            block_id,
        } => {
            let mut page = serde_json::from_slice(&std::fs::read(cache)?)?;
            let service = service::TranslationService::new(&provider, base_url.as_deref(), None)?;
            // Empty, skipped and fully edited pages intentionally need no
            // translation checkpoints, but still have a valid destination.
            persistence::atomic_json(&destination, &page)?;
            let options = translate_pipeline::Options {
                model: &model,
                unit: &unit,
                batch_size,
                block_id: block_id.as_deref(),
                image: image.as_deref(),
            };
            let mut observer = persistence::FileObserver {
                destination: &destination,
                status: status.as_deref(),
                metrics: metrics.as_deref(),
            };
            let outcome = if vision {
                vision_pipeline::translate(
                    &mut page,
                    image.as_deref().expect("clap requires image"),
                    &model,
                    &service,
                    block_id.as_deref(),
                    &mut observer,
                )
                .await
            } else {
                translate_pipeline::translate(&mut page, &service, &options, &mut observer).await
            };
            match outcome {
                Ok(()) => println!("{}", json!({"ok":true,"cache":destination})),
                Err(e) => {
                    println!(
                        "{}",
                        json!({"ok":false,"cache":destination,"error":service.safe_error(e)})
                    );
                    std::process::exit(1);
                }
            }
        }
        Operation::OverlayProbe { image, cache, mode } => {
            let mut page: serde_json::Value = serde_json::from_slice(&std::fs::read(cache)?)?;
            if mode == "adjacent" {
                let gray = imaging::decode_gray(
                    image
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("image required"))?,
                )?;
                let first = serde_json::from_value(page["first"].clone())?;
                let second = serde_json::from_value(page["second"].clone())?;
                println!(
                    "{}",
                    overlay::adjacent(&gray, first, second, compat::truth(&page["vertical"]))?
                );
            } else {
                let changed = overlay::ensure(&mut page, image.as_deref())?;
                println!(
                    "{}",
                    serde_json::to_string(&json!({"changed":changed,"page":page}))?
                );
            }
        }
        Operation::ProtocolProbe { requests } => {
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let replies: Vec<_> = requests
                .iter()
                .map(|r| match translation::protocol_probe(r) {
                    Ok(value) => json!({"ok":true,"value":value}),
                    Err(e) => json!({"ok":false,"error":e.to_string()}),
                })
                .collect();
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::ServiceProbe { requests } => {
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut replies = vec![];
            for request in requests {
                replies.push(match service::probe(&request).await {
                    Ok(value) => json!({"ok":true,"value":value}),
                    Err(e) => json!({"ok":false,"error":e.to_string()}),
                });
            }
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::ListModels { provider, base_url } => {
            let reply = match service::TranslationService::new(&provider, base_url.as_deref(), None)
            {
                Ok(service) => service.list_models().await,
                Err(e) => Err(e),
            };
            match reply {
                Ok(models) => println!("{}", json!({"ok":true,"models":models})),
                Err(e) => {
                    println!("{}", json!({"ok":false,"error":e.to_string()}));
                    std::process::exit(1);
                }
            }
        }
        Operation::Display {
            image,
            cache,
            text_mask,
            python,
            worker,
            weights,
        } => {
            use opencv::prelude::*;
            let page: serde_json::Value = serde_json::from_slice(&std::fs::read(cache)?)?;
            let color = imaging::decode_color(&image)?;
            let mut gray = opencv::core::Mat::default();
            opencv::imgproc::cvt_color_def(&color, &mut gray, opencv::imgproc::COLOR_BGR2GRAY)?;
            let has_color = color
                .data_bytes()?
                .chunks_exact(3)
                .any(|p| p.iter().max().unwrap() - p.iter().min().unwrap() > 8);
            let mut models =
                if let (Some(python), Some(worker), Some(weights)) = (python, worker, weights) {
                    Some(model_images::ImageModels::new(&python, &worker, &weights)?)
                } else {
                    None
                };
            let mut source = text_mask.as_deref().map(imaging::decode_gray).transpose()?;
            if source.is_none()
                && compat::array(&page["blocks"]).iter().any(ink::ready)
                && let Some(models) = models.as_mut()
            {
                source = Some(models.detect(&color).await?);
            }
            let regions = reading::regions(
                &page,
                &gray,
                if has_color { Some(&color) } else { None },
                source.as_ref(),
                models.as_mut(),
            )
            .await?;
            if let Some(models) = models {
                models.shutdown().await?;
            }
            println!(
                "{}",
                json!({"version":3,"regions":regions,"preserved":ink::preserved_markers(&page)})
            );
        }
        Operation::BackgroundRender {
            image,
            requests,
            mask,
            python,
            worker,
            weights,
        } => {
            let color = imaging::decode_color(&image)?;
            let gray = imaging::display_gray(&image)?;
            let source = mask.as_deref().map(imaging::decode_gray).transpose()?;
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut models =
                if let (Some(python), Some(worker), Some(weights)) = (python, worker, weights) {
                    Some(model_images::ImageModels::new(&python, &worker, &weights)?)
                } else {
                    None
                };
            let mut replies = vec![];
            for request in requests {
                let boxes: Vec<ink::Box4> = serde_json::from_value(request["boxes"].clone())?;
                let clip = request["clip_path"]
                    .as_str()
                    .map(|path| imaging::decode_gray(std::path::Path::new(path)))
                    .transpose()?;
                let origin: [i32; 2] = serde_json::from_value(
                    request.get("clip_origin").cloned().unwrap_or(json!([0, 0])),
                )?;
                let plan = background::prepare(
                    &gray,
                    &boxes,
                    clip.as_ref().map(|m| (m, origin)),
                    compat::truth(&request["textured"]),
                    source.as_ref(),
                )?;
                replies.push(
                    plan.render(
                        if compat::truth(&request["color"]) {
                            Some(&color)
                        } else {
                            None
                        },
                        models.as_mut(),
                    )
                    .await?,
                );
            }
            if let Some(models) = models {
                models.shutdown().await?;
            }
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::BackgroundProbe {
            image,
            requests,
            mask,
        } => {
            let gray = imaging::display_gray(&image)?;
            let source = mask.as_deref().map(imaging::decode_gray).transpose()?;
            let requests: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut replies = vec![];
            for request in requests {
                let boxes: Vec<ink::Box4> = serde_json::from_value(request["boxes"].clone())?;
                let clip = request["clip_path"]
                    .as_str()
                    .map(|path| imaging::decode_gray(std::path::Path::new(path)))
                    .transpose()?;
                let origin: [i32; 2] = serde_json::from_value(
                    request.get("clip_origin").cloned().unwrap_or(json!([0, 0])),
                )?;
                replies.push(
                    background::prepare(
                        &gray,
                        &boxes,
                        clip.as_ref().map(|m| (m, origin)),
                        compat::truth(&request["textured"]),
                        source.as_ref(),
                    )?
                    .snapshot()?,
                );
            }
            println!("{}", serde_json::to_string(&replies)?);
        }
        Operation::GeometryProbe { image, cache, mask } => {
            let page: serde_json::Value = serde_json::from_slice(&std::fs::read(cache)?)?;
            let gray = imaging::display_gray(&image)?;
            let source = mask.as_deref().map(imaging::decode_gray).transpose()?;
            use opencv::prelude::*;
            let blocks = compat::array(&page["blocks"]);
            let preserved = ink::preserved_markers(&page);
            let boxes: Vec<_> = blocks
                .iter()
                .map(|b| {
                    if preserved.get(compat::text(&b["id"])).is_some() {
                        None
                    } else {
                        ink::clipped_box(b, gray.cols(), gray.rows())
                    }
                })
                .collect();
            let (boxes, _) =
                ink::complete_vertical_strokes(&gray, &boxes, blocks, source.as_ref())?;
            let bubbles = geometry::bubbles(&gray, &boxes)?;
            let narrations = geometry::narration_groups(&gray, blocks, &boxes, &bubbles)?;
            let results: Result<Vec<_>> = bubbles.iter().map(|c| c.snapshot(&boxes)).collect();
            let narration_results: Result<Vec<_>> =
                narrations.iter().map(|c| c.snapshot(&boxes)).collect();
            let cached: Result<Vec<_>> = compat::array(&page["overlay_regions"]).iter().map(|r| {
                let candidate = geometry::cached_shape(r, gray.cols(), gray.rows())?;
                use sha2::{Digest, Sha256};
                candidate.map(|c| Ok(json!({"origin": c.origin, "area": c.area,
                    "mask_shape": [c.mask.rows(), c.mask.cols()], "mask_sha256": format!("{:x}", Sha256::digest(c.mask.data_bytes()?))}))).transpose()
            }).collect();
            println!(
                "{}",
                json!({"bubbles": results?, "narrations": narration_results?, "cached": cached?})
            );
        }
        Operation::InkProbe { image, cache, mask } => {
            let page = serde_json::from_slice(&std::fs::read(cache)?)?;
            let gray = imaging::display_gray(&image)?;
            let source = mask.as_deref().map(imaging::decode_gray).transpose()?;
            println!("{}", ink::probe(&page, &gray, source.as_ref())?);
        }
        Operation::StructureProbe { image, cache, mode } => {
            let mut page = serde_json::from_slice(&std::fs::read(cache)?)?;
            match mode.as_str() {
                "build" => structure::build(&mut page)?,
                "sync" => structure::sync(&mut page)?,
                _ => structure::ensure(&mut page, image.as_deref())?,
            }
            println!("{}", serde_json::to_string(&page)?);
        }
        Operation::PanelProbe { image, cache } => {
            let mut page = serde_json::from_slice(&std::fs::read(cache)?)?;
            panels::ensure(&mut page, image.as_deref())?;
            println!("{}", serde_json::to_string(&page)?);
        }
        Operation::ImageProbe { images } => {
            let results: Result<Vec<_>> = images
                .iter()
                .map(|path| imaging::image_probe(path))
                .collect();
            println!("{}", serde_json::to_string(&results?)?);
        }
        Operation::ModelHealth {
            python,
            worker,
            weights,
        } => {
            let directory = tempfile::tempdir()?;
            let worker =
                models::ModelWorker::spawn(&python, &worker, directory.path(), &weights).await?;
            worker.shutdown().await?;
            println!(
                "{}",
                json!({"ok": true, "protocol": 1, "models_loaded": false})
            );
        }
        Operation::ModelRun {
            python,
            worker,
            weights,
            workspace,
            requests,
        } => {
            let jobs: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(requests)?)?;
            let mut worker =
                models::ModelWorker::spawn(&python, &worker, &workspace, &weights).await?;
            let mut replies = Vec::with_capacity(jobs.len());
            for job in jobs {
                replies.push(
                    worker
                        .request(job, std::time::Duration::from_secs(300))
                        .await?,
                );
            }
            worker.shutdown().await?;
            println!("{}", serde_json::to_string(&replies)?);
        }
    }
    Ok(())
}
