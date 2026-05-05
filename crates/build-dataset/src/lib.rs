//! Library entry-point for the offline dataset builder.
//!
//! Reads `references.json.gz` (or any other JSON-array dataset of
//! `{ vector: [..14 floats..], label: "fraud" | "legit" }`) and emits the two
//! binary artifacts consumed by the runtime:
//!
//! * `references.i8.bin` — magic header + `N × [i8; 16]`
//! * `labels.bits` — packed `N`-bit bitset (`1` = fraud)
//!
//! The JSON array is parsed in streaming mode via a custom `Visitor`: peak
//! RSS during the build is bounded by `serde_json`'s internal buffering plus
//! one `Reference` allocation at a time, so the full 3M dataset never lives
//! in memory simultaneously.

use std::cell::RefCell;
use std::fmt;
use std::fs::{File, create_dir_all};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use flate2::read::GzDecoder;
use serde::Deserialize;
use serde::de::Deserializer as _;

use shared::{DIMS, LabelBitsetWriter, PAD, SENTINEL_I8, quantize, write_references_header};

#[derive(Debug, Deserialize)]
struct Reference {
    vector: Vec<f32>,
    label: String,
}

/// Statistics returned by [`build`] for callers that want to assert on shape.
#[derive(Debug, Clone, Copy)]
pub struct BuildStats {
    pub vectors: u64,
    pub fraud_count: u64,
    pub refs_bytes: u64,
    pub labels_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum InputCompression {
    Gzip,
    Plain,
}

impl InputCompression {
    fn detect(path: &Path) -> Self {
        if path.extension().is_some_and(|e| e == "gz") {
            Self::Gzip
        } else {
            Self::Plain
        }
    }
}

/// Build the binary artifacts from a JSON dataset.
///
/// `input` may end in `.gz` (auto-detected) or be plain JSON. The output
/// directory is created if missing.
pub fn build(input: &Path, output_dir: &Path) -> Result<BuildStats> {
    create_dir_all(output_dir)
        .with_context(|| format!("creating output dir {}", output_dir.display()))?;

    let refs_path = output_dir.join("references.i8.bin");
    let labels_path = output_dir.join("labels.bits");

    let file = File::open(input).with_context(|| format!("opening {}", input.display()))?;
    let buffered = BufReader::new(file);
    let reader: Box<dyn Read> = match InputCompression::detect(input) {
        InputCompression::Gzip => Box::new(GzDecoder::new(buffered)),
        InputCompression::Plain => Box::new(buffered),
    };

    // Reserve the 16-byte magic header up front; the final `N` is patched
    // back in once the streaming visitor finishes counting entries.
    let mut refs_buf = BufWriter::new(
        File::create(&refs_path)
            .with_context(|| format!("creating {}", refs_path.display()))?,
    );
    write_references_header(&mut refs_buf, 0)?;

    let labels_buf = BufWriter::new(
        File::create(&labels_path)
            .with_context(|| format!("creating {}", labels_path.display()))?,
    );

    let sink = ArraySink {
        refs: RefCell::new(refs_buf),
        labels: RefCell::new(LabelBitsetWriter::new(labels_buf)),
        count: RefCell::new(0),
        fraud_count: RefCell::new(0),
    };

    let mut de = serde_json::Deserializer::from_reader(reader);
    de.deserialize_seq(&sink)
        .with_context(|| format!("parsing JSON array from {}", input.display()))?;

    let (refs_buf, labels_writer, n, fraud_count) = sink.into_parts();

    // Finalize labels: flush the trailing partial byte (if any), then drop
    // the BufWriter so its internal buffer is committed to disk.
    let labels_buf = labels_writer.finish()?;
    let labels_file = labels_buf
        .into_inner()
        .map_err(|e| anyhow!("flushing labels.bits: {e}"))?;
    labels_file.sync_all().context("syncing labels.bits")?;

    // Patch the references header in-place now that we know N. Reusing the
    // same File avoids re-opening it.
    let mut refs_file = refs_buf
        .into_inner()
        .map_err(|e| anyhow!("flushing references.i8.bin: {e}"))?;
    refs_file
        .seek(SeekFrom::Start(0))
        .context("rewinding refs file to patch header")?;
    write_references_header(&mut refs_file, n)?;
    refs_file.sync_all().context("syncing references.i8.bin")?;

    Ok(BuildStats {
        vectors: n,
        fraud_count,
        refs_bytes: file_size(&refs_path)?,
        labels_bytes: file_size(&labels_path)?,
    })
}

/// Streaming sink for a JSON array of `Reference` objects.
///
/// Owns the output writers and the running counters; `serde_json`'s
/// `Visitor::visit_seq` calls a `&ArraySink` so each element flows straight
/// to disk without ever building a `Vec<Reference>`.
struct ArraySink<W1: Write, W2: Write> {
    refs: RefCell<BufWriter<W1>>,
    labels: RefCell<LabelBitsetWriter<BufWriter<W2>>>,
    count: RefCell<u64>,
    fraud_count: RefCell<u64>,
}

impl<W1: Write, W2: Write> ArraySink<W1, W2> {
    fn into_parts(
        self,
    ) -> (
        BufWriter<W1>,
        LabelBitsetWriter<BufWriter<W2>>,
        u64,
        u64,
    ) {
        (
            self.refs.into_inner(),
            self.labels.into_inner(),
            self.count.into_inner(),
            self.fraud_count.into_inner(),
        )
    }

    fn process_entry(&self, i: u64, entry: &Reference) -> Result<()> {
        if entry.vector.len() != DIMS {
            return Err(anyhow!(
                "entry {i}: expected {DIMS}-dim vector, got {} dims",
                entry.vector.len()
            ));
        }
        let mut buf = [0_f32; DIMS];
        for (slot, raw) in buf.iter_mut().zip(entry.vector.iter().copied()) {
            // The dataset uses `-1` (and only `-1`) as the missing-data
            // sentinel. Translate it to NaN so `quantize()` emits SENTINEL_I8
            // through the same code path as the runtime payloads.
            *slot = if is_sentinel_minus_one(raw) {
                f32::NAN
            } else {
                raw
            };
        }
        let q = quantize(&buf);

        // Sentinel placement is invariant under quantize() and is unit-tested
        // in `shared`; only re-check in debug to keep the hot path tight.
        if cfg!(debug_assertions) {
            for (idx, &raw) in entry.vector.iter().enumerate() {
                debug_assert!(
                    !is_sentinel_minus_one(raw) || q[idx] == SENTINEL_I8,
                    "entry {i}: sentinel at dim {idx} did not round-trip"
                );
            }
            debug_assert_eq!(q[14], 0);
            debug_assert_eq!(q[15], 0);
        }

        // `i8 → u8` is a bit-preserving cast, so the runtime can later
        // `mmap` the file and reinterpret the bytes as `&[i8]`.
        let bytes: [u8; PAD] = q.map(|b| b as u8);
        self.refs.borrow_mut().write_all(&bytes)?;

        let fraud = match entry.label.as_str() {
            "fraud" => true,
            "legit" => false,
            other => return Err(anyhow!("entry {i}: unknown label {other:?}")),
        };
        if fraud {
            *self.fraud_count.borrow_mut() += 1;
        }
        self.labels.borrow_mut().push(fraud)?;
        Ok(())
    }
}

impl<'de, W1: Write, W2: Write> serde::de::Visitor<'de> for &ArraySink<W1, W2> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of reference objects")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while let Some(entry) = seq.next_element::<Reference>()? {
            let i = *self.count.borrow();
            self.process_entry(i, &entry)
                .map_err(serde::de::Error::custom)?;
            *self.count.borrow_mut() += 1;
        }
        Ok(())
    }
}

/// Bit-exact check for the dataset sentinel value `-1.0`.
fn is_sentinel_minus_one(x: f32) -> bool {
    x.to_bits() == (-1.0_f32).to_bits()
}

fn file_size(p: &Path) -> Result<u64> {
    Ok(std::fs::metadata(p)
        .with_context(|| format!("stat {}", p.display()))?
        .len())
}

/// Default output dir under the workspace target — used by the CLI and the
/// integration test for parity.
#[must_use]
pub fn default_output_dir() -> PathBuf {
    PathBuf::from("target").join("dataset")
}

/// Default input path: `resources/references.json.gz` relative to the
/// workspace root.
#[must_use]
pub fn default_input_path() -> PathBuf {
    PathBuf::from("resources").join("references.json.gz")
}

