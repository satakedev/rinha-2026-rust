//! Shared utilities for the fraud-detection runtime and the offline dataset
//! builder.
//!
//! Both the `api` and `build-dataset` crates depend on this library so that
//! vectorization, quantization and the binary artifact format stay byte-for-byte
//! identical between build time and runtime.

pub mod datetime;
pub mod format;
pub mod quantize;
pub mod vectorize;

pub const DIMS: usize = 14;
pub const PAD: usize = 16;
pub const SENTINEL_I8: i8 = -1;

pub use format::{
    LABELS_BIT_PER_ENTRY, LabelBitsetWriter, MAGIC, REFS_HEADER_LEN, dataset_byte_len,
    label_bit, labels_byte_len, read_references_header, write_references_header,
};
pub use quantize::quantize;
pub use vectorize::{
    Customer, LastTransaction, MccRisk, Merchant, Normalization, Payload, Terminal, Transaction,
    vectorize,
};
