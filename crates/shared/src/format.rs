//! On-disk binary format for the pre-processed dataset artifacts.
//!
//! `references.i8.bin` layout:
//!
//! ```text
//!   offset  bytes  contents
//!   0       8      magic = b"RINHA26\x01"
//!   8       8      u64_le(N)               // number of vectors
//!   16+     N * 16 quantized [i8; 16]      // 14 dims + 2 padding bytes
//! ```
//!
//! `labels.bits` layout: little-endian bitset of `N` bits, one per vector
//! (`1` = fraud, `0` = legit). Length is `(N + 7) / 8` bytes — for the full
//! 3M dataset that's exactly 375 000 bytes.

use std::io::{self, Read, Write};

use crate::PAD;

pub const MAGIC: [u8; 8] = *b"RINHA26\x01";
pub const REFS_HEADER_LEN: usize = 16;
pub const LABELS_BIT_PER_ENTRY: usize = 1;

#[must_use]
pub const fn dataset_byte_len(n: usize) -> usize {
    REFS_HEADER_LEN + n * PAD
}

#[must_use]
pub const fn labels_byte_len(n: usize) -> usize {
    n.div_ceil(8)
}

/// Write the 16-byte header (`magic` + `u64_le(n)`) of `references.i8.bin`.
pub fn write_references_header<W: Write>(mut w: W, n: u64) -> io::Result<()> {
    w.write_all(&MAGIC)?;
    w.write_all(&n.to_le_bytes())?;
    Ok(())
}

/// Read and validate the 16-byte header of `references.i8.bin`. Returns the
/// vector count `n`.
pub fn read_references_header<R: Read>(mut r: R) -> io::Result<u64> {
    let mut magic = [0_u8; 8];
    r.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad references magic: {magic:?}"),
        ));
    }
    let mut count = [0_u8; 8];
    r.read_exact(&mut count)?;
    Ok(u64::from_le_bytes(count))
}

/// Bitset writer for the 1-bit-per-vector `labels.bits` artifact.
///
/// Internally accumulates 8 labels at a time and flushes a byte once full;
/// the final partial byte (if any) is flushed by [`Self::finish`].
pub struct LabelBitsetWriter<W: Write> {
    inner: W,
    cursor: u8,
    bit: u8,
}

impl<W: Write> LabelBitsetWriter<W> {
    #[must_use]
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            cursor: 0,
            bit: 0,
        }
    }

    /// Push the next label. `true` means fraud (bit set), `false` means legit.
    pub fn push(&mut self, fraud: bool) -> io::Result<()> {
        if fraud {
            self.cursor |= 1 << self.bit;
        }
        self.bit += 1;
        if self.bit == 8 {
            self.inner.write_all(&[self.cursor])?;
            self.cursor = 0;
            self.bit = 0;
        }
        Ok(())
    }

    /// Flush the trailing partial byte (if any) and return the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        if self.bit != 0 {
            self.inner.write_all(&[self.cursor])?;
        }
        Ok(self.inner)
    }
}

/// Read the `i`-th label from a `labels.bits` byte slice.
#[must_use]
pub fn label_bit(bits: &[u8], i: usize) -> bool {
    let byte = bits[i / 8];
    (byte >> (i % 8)) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let mut buf = Vec::new();
        write_references_header(&mut buf, 3_000_000).unwrap();
        assert_eq!(buf.len(), REFS_HEADER_LEN);
        assert_eq!(&buf[..8], &MAGIC);
        let n = read_references_header(&buf[..]).unwrap();
        assert_eq!(n, 3_000_000);
    }

    #[test]
    fn dataset_byte_len_matches_full_size() {
        // Full Rinha dataset: 16 bytes header + 3M × 16 bytes = 48 000 016.
        assert_eq!(dataset_byte_len(3_000_000), 16 + 3_000_000 * 16);
        assert_eq!(dataset_byte_len(0), 16);
    }

    #[test]
    fn labels_byte_len_rounds_up() {
        assert_eq!(labels_byte_len(3_000_000), 375_000);
        assert_eq!(labels_byte_len(0), 0);
        assert_eq!(labels_byte_len(1), 1);
        assert_eq!(labels_byte_len(8), 1);
        assert_eq!(labels_byte_len(9), 2);
    }

    #[test]
    fn rejects_bad_magic() {
        let bad = [0_u8; 16];
        let err = read_references_header(&bad[..]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn bitset_packs_little_endian() {
        // labels: F L F L F L L L F  → bits 1 0 1 0 1 0 0 0 1
        let labels = [true, false, true, false, true, false, false, false, true];
        let mut writer = LabelBitsetWriter::new(Vec::new());
        for &l in &labels {
            writer.push(l).unwrap();
        }
        let bytes = writer.finish().unwrap();
        // First byte: bits[0..8] = 1 0 1 0 1 0 0 0 → 0b00010101 = 0x15.
        assert_eq!(bytes[0], 0b0001_0101);
        // Second byte: bit[8] = 1, rest unset → 0b00000001 = 0x01.
        assert_eq!(bytes[1], 0b0000_0001);

        for (i, &expected) in labels.iter().enumerate() {
            assert_eq!(label_bit(&bytes, i), expected, "bit {i}");
        }
    }

    #[test]
    fn bitset_full_byte_then_finish_is_idempotent() {
        let mut writer = LabelBitsetWriter::new(Vec::new());
        for _ in 0..8 {
            writer.push(true).unwrap();
        }
        let bytes = writer.finish().unwrap();
        // Exactly one full byte, no trailing partial flush.
        assert_eq!(bytes.len(), 1);
        assert_eq!(bytes[0], 0xFF);
    }
}
