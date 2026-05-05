//! K-NN top-5 scan over a quantized `[i8; 16]` reference dataset.
//!
//! Two implementations live here:
//!
//! * [`knn5_naive_i32`] — single-thread, no SIMD, integer math. Used as the
//!   correctness oracle and as the fallback on non-AVX2 targets.
//! * [`knn5_naive_f32`] — single-thread, no SIMD, `f32` math. Used in unit
//!   tests as a second oracle: equivalent ordering on the same i8 inputs.
//! * [`knn5_avx2`] — `unsafe` AVX2 kernel (`target_feature`-gated, `x86_64` only),
//!   processes 4 reference vectors per iteration with independent
//!   accumulators for ILP. Same `(dist, idx)` ordering as the naive scan.
//!
//! [`knn5`] dispatches between AVX2 and naive at runtime.

use shared::{DIMS, PAD, format::label_bit};

#[derive(Debug, Clone, Copy)]
pub struct Top5 {
    pub dist: [i32; 5],
    pub idx: [u32; 5],
}

impl Default for Top5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Top5 {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dist: [i32::MAX; 5],
            idx: [u32::MAX; 5],
        }
    }

    /// Insert `(dist, idx)` if it improves the current top-5.
    /// Stable tie-break: equal distances are ordered by ascending index.
    #[inline]
    pub fn try_insert(&mut self, dist: i32, idx: u32) {
        // Reject anything that can't beat the current worst slot.
        let worst_dist = self.dist[4];
        let worst_idx = self.idx[4];
        if dist > worst_dist || (dist == worst_dist && idx >= worst_idx) {
            return;
        }
        // Insertion sort: walk the new entry up the array until ordering holds.
        let mut i = 4_usize;
        while i > 0 {
            let prev = i - 1;
            let p_dist = self.dist[prev];
            let p_idx = self.idx[prev];
            if dist < p_dist || (dist == p_dist && idx < p_idx) {
                self.dist[i] = p_dist;
                self.idx[i] = p_idx;
                i = prev;
            } else {
                break;
            }
        }
        self.dist[i] = dist;
        self.idx[i] = idx;
    }
}

/// Scan over `&[i8]` reference dataset using pure i32 arithmetic.
///
/// `refs.len()` must be a multiple of [`PAD`] (16 bytes per vector).
#[must_use]
pub fn knn5_naive_i32(query: &[i8; PAD], refs: &[i8]) -> Top5 {
    let mut top = Top5::new();
    let n = refs.len() / PAD;
    for i in 0..n {
        let row = &refs[i * PAD..(i + 1) * PAD];
        let mut sum: i32 = 0;
        for d in 0..PAD {
            let diff = i32::from(query[d]) - i32::from(row[d]);
            sum += diff * diff;
        }
        top.try_insert(sum, i as u32);
    }
    top
}

/// Same as [`knn5_naive_i32`] but computes the distance in `f32` and emits
/// the rounded `i32` for ordering. Used as a parity oracle in unit tests —
/// it must produce identical top-5 indices as the integer scan.
#[must_use]
pub fn knn5_naive_f32(query: &[i8; PAD], refs: &[i8]) -> Top5 {
    let mut top = Top5::new();
    let n = refs.len() / PAD;
    for i in 0..n {
        let row = &refs[i * PAD..(i + 1) * PAD];
        let mut sum = 0.0_f32;
        for d in 0..PAD {
            let diff = f32::from(query[d]) - f32::from(row[d]);
            sum += diff * diff;
        }
        // Distances are integer-valued (bounded by 14 * 101² < 2^18) so
        // round-tripping through f32 is exact.
        top.try_insert(sum as i32, i as u32);
    }
    top
}

/// Compute `fraud_score = popcount(labels[idx_i] for i in 0..5) / 5.0`.
#[must_use]
pub fn fraud_score(top: &Top5, labels: &[u8]) -> f32 {
    let mut count = 0_u32;
    for &i in &top.idx {
        if label_bit(labels, i as usize) {
            count += 1;
        }
    }
    count as f32 / 5.0
}

/// Runtime-dispatched k-NN scan. Calls [`knn5_avx2`] when AVX2 is available,
/// otherwise falls back to [`knn5_naive_i32`].
#[must_use]
pub fn knn5(query: &[i8; PAD], refs: &[i8]) -> Top5 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            // Safety: the runtime feature check above is the contract for
            // `#[target_feature(enable = "avx2")]`.
            return unsafe { knn5_avx2(query, refs) };
        }
    }
    knn5_naive_i32(query, refs)
}

/// AVX2 batch kernel. Processes 4 reference vectors per iteration with
/// independent accumulators (ILP) and falls through a 1-vector tail.
///
/// Each reference vector is exactly `PAD = 16` bytes — a single xmm load —
/// so we use `_mm_sub_epi8` (xmm) plus `_mm256_cvtepi8_epi16`/`_mm256_madd_epi16`
/// (ymm) per ref, with 4-way unrolling for ILP, instead of packing two refs
/// into a ymm and broadcasting the query across `_mm256_sub_epi8`. Both shapes
/// match the techspec's instruction list; the xmm-per-ref form avoids the
/// extra broadcast and lane-extracts and produces bit-exact parity vs the
/// naïve scan (verified on 100k random refs).
///
/// # Safety
/// The caller must ensure AVX2 is available on the running CPU.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[must_use]
pub unsafe fn knn5_avx2(query: &[i8; PAD], refs: &[i8]) -> Top5 {
    use core::arch::x86_64::{
        _mm_add_epi32, _mm_cvtsi128_si32, _mm_loadu_si128, _mm_shuffle_epi32, _mm_sub_epi8,
        _mm256_castsi256_si128, _mm256_cvtepi8_epi16, _mm256_extracti128_si256,
        _mm256_madd_epi16,
    };

    // Safety: aligned-or-not, `loadu` is unaligned-safe.
    let q_xmm = unsafe { _mm_loadu_si128(query.as_ptr().cast()) };

    let n = refs.len() / PAD;
    let chunks = n / 4;
    let tail = n % 4;

    let mut p = refs.as_ptr();
    let mut idx: u32 = 0;
    let mut top = Top5::new();

    for _ in 0..chunks {
        // Safety: each load reads 16 bytes that fall within `refs`.
        let r0 = unsafe { _mm_loadu_si128(p.cast()) };
        let r1 = unsafe { _mm_loadu_si128(p.add(PAD).cast()) };
        let r2 = unsafe { _mm_loadu_si128(p.add(2 * PAD).cast()) };
        let r3 = unsafe { _mm_loadu_si128(p.add(3 * PAD).cast()) };

        // q,r ∈ {[-1] ∪ [0, 100]} → diff ∈ [-101, 101] fits in i8.
        let d0_xmm = _mm_sub_epi8(q_xmm, r0);
        let d1_xmm = _mm_sub_epi8(q_xmm, r1);
        let d2_xmm = _mm_sub_epi8(q_xmm, r2);
        let d3_xmm = _mm_sub_epi8(q_xmm, r3);

        // Sign-extend 16 i8 → 16 i16 (in a ymm).
        let d0 = _mm256_cvtepi8_epi16(d0_xmm);
        let d1 = _mm256_cvtepi8_epi16(d1_xmm);
        let d2 = _mm256_cvtepi8_epi16(d2_xmm);
        let d3 = _mm256_cvtepi8_epi16(d3_xmm);

        // Square via diff*diff and horizontal-pair-add into 8 i32.
        let s0 = _mm256_madd_epi16(d0, d0);
        let s1 = _mm256_madd_epi16(d1, d1);
        let s2 = _mm256_madd_epi16(d2, d2);
        let s3 = _mm256_madd_epi16(d3, d3);

        // Reduce 8 i32 → 1 i32 per accumulator.
        let dist0 = {
            let lo = _mm256_castsi256_si128(s0);
            let hi = _mm256_extracti128_si256::<1>(s0);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
            _mm_cvtsi128_si32(s)
        };
        let dist1 = {
            let lo = _mm256_castsi256_si128(s1);
            let hi = _mm256_extracti128_si256::<1>(s1);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
            _mm_cvtsi128_si32(s)
        };
        let dist2 = {
            let lo = _mm256_castsi256_si128(s2);
            let hi = _mm256_extracti128_si256::<1>(s2);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
            _mm_cvtsi128_si32(s)
        };
        let dist3 = {
            let lo = _mm256_castsi256_si128(s3);
            let hi = _mm256_extracti128_si256::<1>(s3);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
            let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
            _mm_cvtsi128_si32(s)
        };

        top.try_insert(dist0, idx);
        top.try_insert(dist1, idx + 1);
        top.try_insert(dist2, idx + 2);
        top.try_insert(dist3, idx + 3);

        // Safety: `chunks * 4 * PAD <= refs.len()` so this advance stays in
        // bounds for every iteration.
        p = unsafe { p.add(4 * PAD) };
        idx += 4;
    }

    for _ in 0..tail {
        let r = unsafe { _mm_loadu_si128(p.cast()) };
        let d_xmm = _mm_sub_epi8(q_xmm, r);
        let d = _mm256_cvtepi8_epi16(d_xmm);
        let s = _mm256_madd_epi16(d, d);
        let lo = _mm256_castsi256_si128(s);
        let hi = _mm256_extracti128_si256::<1>(s);
        let s = _mm_add_epi32(lo, hi);
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
        let dist = _mm_cvtsi128_si32(s);
        top.try_insert(dist, idx);
        p = unsafe { p.add(PAD) };
        idx += 1;
    }

    top
}

/// Compile-time guard so the const `DIMS` shows up as referenced in this file
/// even on non-x86_64 targets where the AVX2 module is excluded.
#[doc(hidden)]
pub const _DIMS_USED: usize = DIMS;

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{SENTINEL_I8, quantize};

    /// `XorShift64` → uniform `f32` in `[0, 1)`. Deterministic and dependency-free.
    fn xorshift_f32(state: &mut u64) -> f32 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        ((x >> 11) as f32) / ((1_u64 << 53) as f32)
    }

    fn quantize_random_refs(n: usize, mut seed: u64) -> Vec<i8> {
        let mut buf = Vec::with_capacity(n * PAD);
        for _ in 0..n {
            let mut v = [0_f32; DIMS];
            for slot in &mut v {
                *slot = xorshift_f32(&mut seed);
            }
            let q = quantize(&v);
            buf.extend_from_slice(&q);
        }
        buf
    }

    #[test]
    fn top5_initialized_to_max() {
        let t = Top5::new();
        assert_eq!(t.dist, [i32::MAX; 5]);
        assert_eq!(t.idx, [u32::MAX; 5]);
    }

    #[test]
    fn top5_insertion_stable_on_ties() {
        // Six entries at distance 100; expect indices 0..5 (lowest five).
        let mut t = Top5::new();
        for i in (0..6_u32).rev() {
            t.try_insert(100, i);
        }
        assert_eq!(t.idx, [0, 1, 2, 3, 4]);
        assert_eq!(t.dist, [100; 5]);
    }

    #[test]
    fn top5_orders_by_distance_then_index() {
        let mut t = Top5::new();
        t.try_insert(50, 9);
        t.try_insert(10, 3);
        t.try_insert(50, 2);
        t.try_insert(20, 7);
        t.try_insert(10, 1);
        t.try_insert(50, 4);
        // Sorted: (10,1), (10,3), (20,7), (50,2), (50,4); (50,9) drops out.
        assert_eq!(t.dist, [10, 10, 20, 50, 50]);
        assert_eq!(t.idx, [1, 3, 7, 2, 4]);
    }

    #[test]
    fn top5_rejects_above_worst() {
        let mut t = Top5::new();
        for i in 0..5_u32 {
            t.try_insert(1, i);
        }
        // Worst is (1, 4); higher dist must not displace it.
        t.try_insert(2, 0);
        assert_eq!(t.dist, [1, 1, 1, 1, 1]);
        assert_eq!(t.idx, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn naive_i32_matches_naive_f32_on_random() {
        // 10 queries × 2_000 refs = 20k comparisons.
        let refs = quantize_random_refs(2_000, 0xDEAD_BEEF);
        let mut q_seed = 0x1234_5678_u64;
        for _ in 0..10 {
            let mut v = [0_f32; DIMS];
            for slot in &mut v {
                *slot = xorshift_f32(&mut q_seed);
            }
            let q = quantize(&v);
            let a = knn5_naive_i32(&q, &refs);
            let b = knn5_naive_f32(&q, &refs);
            assert_eq!(a.idx, b.idx);
            assert_eq!(a.dist, b.dist);
        }
    }

    #[test]
    fn fraud_score_counts_set_bits() {
        // labels: bits 0..5 = 1 0 1 1 0
        let labels = [0b0000_1101_u8];
        let mut top = Top5::new();
        for (slot, idx) in [0, 1, 2, 3, 4].iter().enumerate() {
            top.dist[slot] = (slot as i32) + 1;
            top.idx[slot] = *idx;
        }
        // Set bits among indices 0,1,2,3,4 → 0,2,3 = 3 → 3/5 = 0.6.
        let s = fraud_score(&top, &labels);
        assert!((s - 0.6).abs() < 1e-6);
    }

    #[test]
    fn naive_handles_sentinel_dimensions() {
        // Build refs where some have sentinel at dim 5/6.
        let mut refs = vec![0_i8; 2 * PAD];
        // Ref 0: zeros except dim 5,6 = SENTINEL.
        refs[5] = SENTINEL_I8;
        refs[6] = SENTINEL_I8;
        // Ref 1: zeros everywhere.
        // No-op.
        let mut q_arr = [0_i8; PAD];
        q_arr[5] = SENTINEL_I8;
        q_arr[6] = SENTINEL_I8;
        let top = knn5_naive_i32(&q_arr, &refs);
        // Ref 0 perfectly matches the query (dist=0); ref 1 has 2*1=2.
        assert_eq!(top.idx[0], 0);
        assert_eq!(top.dist[0], 0);
        assert_eq!(top.idx[1], 1);
        assert_eq!(top.dist[1], 2);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_matches_naive_on_100k_refs() {
        if !std::is_x86_feature_detected!("avx2") {
            // Cannot exercise on this CPU; skip silently.
            return;
        }
        // 100k random reference vectors, exactly matching the test scale
        // prescribed in tasks/prd-rinha-backend-2026/2_task.md.
        let refs = quantize_random_refs(100_000, 0x1357_9BDF);
        let mut q_seed = 0xCAFE_BABE_u64;
        let mut mismatches = 0_u32;
        let total = 32_u32;
        for _ in 0..total {
            let mut v = [0_f32; DIMS];
            for slot in &mut v {
                *slot = xorshift_f32(&mut q_seed);
            }
            let q = quantize(&v);
            let a = knn5_naive_i32(&q, &refs);
            let b = unsafe { knn5_avx2(&q, &refs) };
            if a.idx != b.idx || a.dist != b.dist {
                mismatches += 1;
            }
        }
        // Both kernels operate on the same i8 inputs with deterministic
        // integer math, so bit-exact parity is required (≥ 99.5% target
        // is trivially exceeded by an exact integer kernel).
        assert_eq!(
            mismatches, 0,
            "knn5_avx2 diverges from naive on {mismatches}/{total} queries",
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_tail_lengths_are_correct() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        // Loop tail = n % 4; cover all four residues with small datasets.
        for n in 1..=11 {
            let refs = quantize_random_refs(n, 0xABCD);
            let mut q_arr = [0_i8; PAD];
            q_arr[0] = 50;
            let a = knn5_naive_i32(&q_arr, &refs);
            let b = unsafe { knn5_avx2(&q_arr, &refs) };
            assert_eq!(a.idx, b.idx, "n={n}");
            assert_eq!(a.dist, b.dist, "n={n}");
        }
    }
}
