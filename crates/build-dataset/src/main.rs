//! Offline builder that turns `references.json.gz` into the binary artifacts
//! consumed by the runtime API. Runs once, in the Docker `dataset` stage and
//! locally before integration tests.

use std::env;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, anyhow};

use build_dataset::{build, default_input_path, default_output_dir};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let input = args
        .next()
        .map_or_else(default_input_path, PathBuf::from);
    let output_dir = args
        .next()
        .map_or_else(default_output_dir, PathBuf::from);

    if !input.exists() {
        return Err(anyhow!(
            "input file not found: {}\n\
             pass an input path as the first CLI arg, or place the dataset at \
             resources/references.json.gz (DATASET.md describes the file).",
            input.display()
        ));
    }

    eprintln!(
        "build-dataset: input={} output_dir={}",
        input.display(),
        output_dir.display()
    );

    let started = Instant::now();
    let stats = build(&input, &output_dir)?;
    let elapsed = started.elapsed();

    eprintln!(
        "build-dataset: done in {:?} — {} vectors ({} fraud), refs={} B labels={} B",
        elapsed, stats.vectors, stats.fraud_count, stats.refs_bytes, stats.labels_bytes
    );

    Ok(())
}
