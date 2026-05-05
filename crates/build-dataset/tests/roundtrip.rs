//! End-to-end test for the offline dataset builder on a 10 000-vector subset.
//!
//! Generates a synthetic JSON dataset, gzips it, runs `build()`, then asserts
//! on artifact size, magic header, payload bytes and the bitset population.

use std::fs::{File, read};
use std::io::{BufReader, Write};
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::json;

use build_dataset::build;
use shared::{
    DIMS, LABELS_BIT_PER_ENTRY, MAGIC, PAD, REFS_HEADER_LEN, SENTINEL_I8, dataset_byte_len,
    label_bit, labels_byte_len, read_references_header,
};

const N: usize = 10_000;

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rinha2026-build-dataset-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn error_chain_contains(err: &anyhow::Error, needle: &str) -> bool {
    err.chain().any(|cause| cause.to_string().contains(needle))
}

fn pseudo_random(seed: u64) -> f32 {
    // xorshift64 → unit-interval f32. Cheap, deterministic, no extra dep.
    let mut x = seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as f64 / u64::MAX as f64) as f32
}

fn build_synthetic_dataset(n: usize) -> (String, usize) {
    let mut entries = Vec::with_capacity(n);
    let mut fraud_count = 0;
    for i in 0..n {
        let mut vec = Vec::with_capacity(DIMS);
        for d in 0..DIMS {
            // Every 7th entry's last_transaction is null → indices 5, 6 are -1.
            if (d == 5 || d == 6) && i % 7 == 0 {
                vec.push(json!(-1));
            } else {
                vec.push(json!(pseudo_random(((i * 13 + d) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))));
            }
        }
        // Roughly 1/3 fraud labels.
        let fraud = i % 3 == 0;
        if fraud {
            fraud_count += 1;
        }
        entries.push(json!({
            "vector": vec,
            "label": if fraud { "fraud" } else { "legit" },
        }));
    }
    (serde_json::to_string(&entries).unwrap(), fraud_count)
}

fn write_gzipped(json: &str, dest: &PathBuf) {
    let f = File::create(dest).unwrap();
    let mut enc = GzEncoder::new(f, Compression::default());
    enc.write_all(json.as_bytes()).unwrap();
    enc.finish().unwrap();
}

#[test]
fn build_emits_correct_artifacts_for_10k_subset() {
    let dir = workdir();
    let input = dir.join("references.json.gz");
    let output_dir = dir.join("out");

    let (json, expected_fraud) = build_synthetic_dataset(N);
    write_gzipped(&json, &input);

    let stats = build(&input, &output_dir).expect("build succeeds");

    assert_eq!(stats.vectors as usize, N);
    assert_eq!(stats.fraud_count as usize, expected_fraud);
    assert_eq!(stats.refs_bytes as usize, dataset_byte_len(N));
    assert_eq!(stats.labels_bytes as usize, labels_byte_len(N));
    assert_eq!(LABELS_BIT_PER_ENTRY, 1);

    // refs file: header + payload sizes are exact.
    let refs_path = output_dir.join("references.i8.bin");
    let labels_path = output_dir.join("labels.bits");
    assert_eq!(std::fs::metadata(&refs_path).unwrap().len() as usize, REFS_HEADER_LEN + N * PAD);
    assert_eq!(std::fs::metadata(&labels_path).unwrap().len() as usize, N.div_ceil(8));

    // Header validates and reports N.
    let mut reader = BufReader::new(File::open(&refs_path).unwrap());
    let n = read_references_header(&mut reader).unwrap();
    assert_eq!(n as usize, N);

    // Random spot-checks on the i8 payload: first vector's bytes 14 and 15
    // (padding) must be zero, and entries with i%7==0 must hold the sentinel
    // at indices 5 and 6.
    let raw = read(&refs_path).unwrap();
    assert_eq!(&raw[..8], &MAGIC);

    let payload = &raw[REFS_HEADER_LEN..];
    for i in 0..N {
        let row = &payload[i * PAD..(i + 1) * PAD];
        assert_eq!(row[14], 0, "padding byte 14 non-zero at row {i}");
        assert_eq!(row[15], 0, "padding byte 15 non-zero at row {i}");
        if i % 7 == 0 {
            assert_eq!(row[5] as i8, SENTINEL_I8, "row {i}: dim 5 sentinel");
            assert_eq!(row[6] as i8, SENTINEL_I8, "row {i}: dim 6 sentinel");
        }
    }

    // Labels bitset: every 3rd entry should be fraud (bit set).
    let bits = read(&labels_path).unwrap();
    let mut popcount = 0;
    for i in 0..N {
        let bit = label_bit(&bits, i);
        assert_eq!(bit, i % 3 == 0, "bit at idx {i}");
        if bit {
            popcount += 1;
        }
    }
    assert_eq!(popcount, expected_fraud);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_accepts_plain_json_without_gzip() {
    let dir = workdir();
    let input = dir.join("references.json");
    let output_dir = dir.join("out");
    let (json, _) = build_synthetic_dataset(64);
    std::fs::write(&input, json).unwrap();

    let stats = build(&input, &output_dir).unwrap();
    assert_eq!(stats.vectors, 64);
    assert_eq!(stats.refs_bytes as usize, dataset_byte_len(64));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_rejects_unknown_label() {
    let dir = workdir();
    let input = dir.join("references.json");
    let output_dir = dir.join("out");
    let zeros14: Vec<f32> = vec![0.0; 14];
    let bad = json!([{
        "vector": zeros14,
        "label": "maybe-fraud"
    }]);
    std::fs::write(&input, bad.to_string()).unwrap();

    let err = build(&input, &output_dir).expect_err("must reject unknown label");
    assert!(
        error_chain_contains(&err, "maybe-fraud"),
        "error chain missing label snippet: {err:?}",
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn build_rejects_wrong_dim_vector() {
    let dir = workdir();
    let input = dir.join("references.json");
    let output_dir = dir.join("out");
    let zeros13: Vec<f32> = vec![0.0; 13];
    let bad = json!([{
        "vector": zeros13,
        "label": "legit"
    }]);
    std::fs::write(&input, bad.to_string()).unwrap();

    let err = build(&input, &output_dir).expect_err("must reject wrong dim");
    assert!(
        error_chain_contains(&err, "13 dims"),
        "error chain missing dim snippet: {err:?}",
    );

    std::fs::remove_dir_all(&dir).ok();
}
