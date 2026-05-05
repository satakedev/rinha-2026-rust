//! Tokio current-thread entry-point for the fraud-detection API.
//!
//! Boot sequence:
//!
//! 1. Parse env config (artifact paths + bind address).
//! 2. mmap dataset artifacts, parse JSONs, build [`AppState`].
//! 3. Run [`api::warmup`] (`MADV_WILLNEED` + sequential touch).
//! 4. Flip `ready=true`, bind TCP, serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use api::{AppState, load_state, router, warmup};
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();

    let cfg = Config::from_env();
    info!(
        refs = %cfg.refs.display(),
        labels = %cfg.labels.display(),
        bind = %cfg.bind,
        "starting api"
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building current-thread runtime")?;

    rt.block_on(async move { run(cfg).await })
}

async fn run(cfg: Config) -> Result<()> {
    let started = Instant::now();
    let state: Arc<AppState> = load_state(&cfg.refs, &cfg.labels, &cfg.normalization, &cfg.mcc_risk)
        .context("loading app state")?;

    warmup(&state);
    state.mark_ready();
    let elapsed_ms = started.elapsed().as_millis();
    info!(refs = state.n, ready_in_ms = elapsed_ms, "api ready in {elapsed_ms}ms, refs={}", state.n);

    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("binding {}", cfg.bind))?;
    let app = router(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum::serve failed")?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!("ctrl_c handler failed: {e}");
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .expect("static EnvFilter parses");
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .compact()
        .init();
}

struct Config {
    refs: PathBuf,
    labels: PathBuf,
    normalization: PathBuf,
    mcc_risk: PathBuf,
    bind: SocketAddr,
}

impl Config {
    fn from_env() -> Self {
        let refs = env_path("RINHA_REFS", "target/dataset/references.i8.bin");
        let labels = env_path("RINHA_LABELS", "target/dataset/labels.bits");
        let normalization = env_path("RINHA_NORMALIZATION", "resources/normalization.json");
        let mcc_risk = env_path("RINHA_MCC_RISK", "resources/mcc_risk.json");
        let bind: SocketAddr = std::env::var("RINHA_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()
            .expect("RINHA_BIND must be a valid SocketAddr");
        Self {
            refs,
            labels,
            normalization,
            mcc_risk,
            bind,
        }
    }
}

fn env_path(key: &str, default: &str) -> PathBuf {
    std::env::var(key).map_or_else(|_| PathBuf::from(default), PathBuf::from)
}
