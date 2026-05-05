//! Application state shared across handlers.
//!
//! `RefBytes` lets the production runtime mmap the dataset artifacts read-only
//! while integration tests can pass in-memory buffers — the slice exposed to
//! the search kernel is identical in both cases.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use memmap2::Mmap;
use shared::{MccRisk, Normalization, PAD, REFS_HEADER_LEN, read_references_header};

/// Either an mmap'd file or an owned byte buffer. Both implement
/// `as_slice()` returning the raw bytes — the runtime uses the mmap variant,
/// integration tests use the owned variant.
pub enum RefBytes {
    Mapped(Mmap),
    /// Test-only escape hatch so integration tests can build an `AppState`
    /// from a synthetic in-memory dataset without touching the filesystem.
    /// Production callers should always go through [`load_state`].
    #[doc(hidden)]
    Owned(Vec<u8>),
}

impl RefBytes {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mapped(m) => m,
            Self::Owned(v) => v.as_slice(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

pub struct AppState {
    pub ready: AtomicBool,
    pub refs: RefBytes,
    pub labels: RefBytes,
    pub n: u32,
    pub norm: Normalization,
    pub mcc: MccRisk,
}

impl AppState {
    /// Reinterpret the references buffer as `&[i8]`, skipping the 16-byte
    /// magic+count header. The slice is `n * PAD` bytes long.
    #[must_use]
    pub fn refs_i8(&self) -> &[i8] {
        let bytes = &self.refs.as_slice()[REFS_HEADER_LEN..];
        // Safety: `i8` and `u8` share layout; alignment for primitives of size
        // 1 is trivially satisfied.
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<i8>(), bytes.len()) }
    }

    #[must_use]
    pub fn labels_bits(&self) -> &[u8] {
        self.labels.as_slice()
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }
}

/// Validate the references header and the labels size against `n`.
fn validate_artifacts(refs: &[u8], labels: &[u8]) -> Result<u32> {
    let n = read_references_header(refs)?;
    let n_usize = usize::try_from(n).context("references header N out of usize range")?;

    let expected_refs = REFS_HEADER_LEN + n_usize * PAD;
    if refs.len() != expected_refs {
        return Err(anyhow!(
            "references file size mismatch: expected {expected_refs} bytes for n={n}, got {}",
            refs.len()
        ));
    }
    let expected_labels = n_usize.div_ceil(8);
    if labels.len() != expected_labels {
        return Err(anyhow!(
            "labels file size mismatch: expected {expected_labels} bytes for n={n}, got {}",
            labels.len()
        ));
    }
    let n_u32 = u32::try_from(n).context("references header N out of u32 range")?;
    Ok(n_u32)
}

/// Load `AppState` from filesystem paths. Reads `normalization.json` and
/// `mcc_risk.json` into memory, mmap's the two binary artifacts, and runs
/// header/length validation.
///
/// `ready` starts as `false`; the caller must invoke [`warmup`] once and then
/// `state.mark_ready()` before serving traffic.
pub fn load_state(
    refs_path: &Path,
    labels_path: &Path,
    normalization_path: &Path,
    mcc_risk_path: &Path,
) -> Result<Arc<AppState>> {
    let refs_file = File::open(refs_path)
        .with_context(|| format!("opening {}", refs_path.display()))?;
    let labels_file = File::open(labels_path)
        .with_context(|| format!("opening {}", labels_path.display()))?;

    // Safety: the dataset files are immutable for the lifetime of the process
    // (mounted read-only inside the container). If another process writes to
    // them concurrently we'd see UB; that's documented in techspec.md.
    let refs_mmap = unsafe { Mmap::map(&refs_file) }
        .with_context(|| format!("mmap {}", refs_path.display()))?;
    let labels_mmap = unsafe { Mmap::map(&labels_file) }
        .with_context(|| format!("mmap {}", labels_path.display()))?;

    let n = validate_artifacts(&refs_mmap, &labels_mmap)?;

    let norm = read_to_string(normalization_path)
        .with_context(|| format!("reading {}", normalization_path.display()))?;
    let mcc = read_to_string(mcc_risk_path)
        .with_context(|| format!("reading {}", mcc_risk_path.display()))?;

    let norm = Normalization::from_json_str(&norm)
        .with_context(|| format!("parsing {}", normalization_path.display()))?;
    let mcc = MccRisk::from_json_str(&mcc)
        .with_context(|| format!("parsing {}", mcc_risk_path.display()))?;

    Ok(Arc::new(AppState {
        ready: AtomicBool::new(false),
        refs: RefBytes::Mapped(refs_mmap),
        labels: RefBytes::Mapped(labels_mmap),
        n,
        norm,
        mcc,
    }))
}

fn read_to_string(p: &Path) -> Result<String> {
    let mut f = File::open(p)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

/// Pre-fault every page of the references buffer so the first request never
/// pays the page-in cost. We hint `MADV_WILLNEED` first (so the kernel may
/// readahead) and then sequentially read one byte per 4 KiB page.
///
/// Returns the running checksum (intentionally consumed via `black_box` to
/// prevent the compiler from eliding the touch loop).
pub fn warmup(state: &AppState) {
    let bytes = state.refs.as_slice();
    if bytes.is_empty() {
        return;
    }

    // POSIX advisory: hint the kernel to readahead.
    if let RefBytes::Mapped(_) = &state.refs {
        // Safety: `bytes` came from a live mmap; ptr/len describe the same
        // mapping. `posix_madvise` is documented to accept any aligned range.
        let ret = unsafe {
            libc::posix_madvise(
                bytes.as_ptr() as *mut libc::c_void,
                bytes.len(),
                libc::POSIX_MADV_WILLNEED,
            )
        };
        if ret != 0 {
            tracing::warn!(ret, "posix_madvise WILLNEED returned non-zero");
        }
    }

    // Sequential touch — one byte per 4 KiB page is enough to fault it in.
    // `black_box` keeps the loop in release builds.
    let mut acc: u64 = 0;
    let page = 4096;
    let mut i = 0;
    while i < bytes.len() {
        acc = acc.wrapping_add(u64::from(bytes[i]));
        i += page;
    }
    // Last byte too, in case the file doesn't end on a page boundary.
    if let Some(&last) = bytes.last() {
        acc = acc.wrapping_add(u64::from(last));
    }
    std::hint::black_box(acc);
}
