//! Quantize a 14-dim `f32` vector into the on-disk / in-memory `[i8; 16]`
//! layout.
//!
//! Mapping:
//! * NaN → `SENTINEL_I8` (`-1`). The runtime `vectorize()` produces NaN in
//!   indices 5 and 6 when `last_transaction` is `null`.
//! * Otherwise the value is clamped to `[0.0, 1.0]` and mapped to `[0, 100]`
//!   with `(x * 100.0).round() as i8`. The `i32` accumulator used by the
//!   AVX2 kernel can therefore never overflow over 14 dimensions.
//! * Padding bytes 14 and 15 are zeroed so SIMD loads never see uninitialised
//!   memory and L2² stays consistent across the padding lanes.

use crate::{DIMS, PAD, SENTINEL_I8};

#[must_use]
pub fn quantize(v: &[f32; DIMS]) -> [i8; PAD] {
    let mut out = [0_i8; PAD];
    for (slot, &x) in out.iter_mut().zip(v.iter()) {
        *slot = if x.is_nan() {
            SENTINEL_I8
        } else {
            let clamped = x.clamp(0.0, 1.0);
            (clamped * 100.0).round() as i8
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_to_zero_one_hundred() {
        let v = [
            -10.0, 0.0, 0.005, 0.5, 1.0, 1.5, 0.999, 0.1, 0.25, 0.33, 0.66, 0.9, 0.123, 1.0,
        ];
        let q = quantize(&v);
        assert_eq!(q[0], 0, "negative clamps to 0");
        assert_eq!(q[1], 0);
        assert_eq!(q[2], 1, "0.005 rounds to 1 (banker-friendly: half-to-even or round)");
        assert_eq!(q[3], 50);
        assert_eq!(q[4], 100);
        assert_eq!(q[5], 100, "above-1.0 clamps to 100");
        assert_eq!(q[6], 100, "0.999*100 ≈ 99.9 rounds to 100");
        // padding bytes are zero
        assert_eq!(q[14], 0);
        assert_eq!(q[15], 0);
    }

    #[test]
    fn nan_becomes_sentinel() {
        let mut v = [0.0_f32; DIMS];
        v[5] = f32::NAN;
        v[6] = f32::NAN;
        let q = quantize(&v);
        assert_eq!(q[5], SENTINEL_I8);
        assert_eq!(q[6], SENTINEL_I8);
        // every other slot is 0
        for (i, byte) in q.iter().enumerate().take(DIMS) {
            if i != 5 && i != 6 {
                assert_eq!(*byte, 0, "unexpected non-zero at idx {i}");
            }
        }
    }

    #[test]
    fn idempotent_under_quantize_dequantize() {
        // Round-trip f32 → i8 → f32_estimated → i8 must reproduce the same i8.
        // The dequantize step is the natural inverse: q as f32 / 100.0.
        let cases: [f32; DIMS] = [
            0.0, 0.01, 0.05, 0.5, 0.99, 1.0, 0.123, 0.456, 0.789, 0.111, 0.222, 0.333, 0.444,
            0.555,
        ];
        let q1 = quantize(&cases);
        let mut deq = [0_f32; DIMS];
        for (slot, q) in deq.iter_mut().zip(q1.iter().take(DIMS)) {
            *slot = f32::from(*q) / 100.0;
        }
        let q2 = quantize(&deq);
        assert_eq!(q1, q2);
    }

    #[test]
    fn sentinel_survives_roundtrip() {
        let mut v = [0.5_f32; DIMS];
        v[5] = f32::NAN;
        v[6] = f32::NAN;
        let q1 = quantize(&v);
        // simulate a roundtrip where consumer uses sentinel as -1
        let mut deq = [0_f32; DIMS];
        for (slot, q) in deq.iter_mut().zip(q1.iter().take(DIMS)) {
            *slot = if *q == SENTINEL_I8 {
                f32::NAN
            } else {
                f32::from(*q) / 100.0
            };
        }
        let q2 = quantize(&deq);
        assert_eq!(q1, q2);
    }

    #[test]
    fn padding_is_zero() {
        let v = [1.0_f32; DIMS];
        let q = quantize(&v);
        assert_eq!(q[14], 0);
        assert_eq!(q[15], 0);
    }
}
