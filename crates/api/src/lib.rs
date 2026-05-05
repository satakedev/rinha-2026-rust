//! Library entry-point for the runtime fraud-detection API.
//!
//! `main.rs` wires this into a Tokio current-thread runtime; integration tests
//! exercise the same `Router` directly via `tower::ServiceExt::oneshot`.

pub mod error;
pub mod routes;
pub mod search;
pub mod state;

pub use error::ApiError;
pub use routes::router;
pub use search::{Top5, fraud_score, knn5};
pub use state::{AppState, RefBytes, load_state, warmup};
