mod compat;
mod imaging;
mod ink;
mod models;
mod panels;
mod structure;

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
