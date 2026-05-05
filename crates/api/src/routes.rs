//! HTTP routing layer.
//!
//! Two endpoints exposed by [`router`]:
//!
//! * `GET /ready` — `200` once boot warmup completes, `503` before.
//! * `POST /fraud-score` — JSON in/out, decision via the AVX2 k-NN scan.
//!
//! Invalid bodies always map to `400` with an empty body; no `5xx` is ever
//! produced.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;
use shared::{Payload, quantize, vectorize};

use crate::error::ApiError;
use crate::search::{fraud_score, knn5};
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ready", get(ready))
        .route("/fraud-score", post(fraud_score_handler))
        .with_state(state)
}

async fn ready(State(state): State<Arc<AppState>>) -> StatusCode {
    if state.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[derive(Serialize)]
struct FraudScoreResponse {
    approved: bool,
    fraud_score: f32,
}

async fn fraud_score_handler(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let payload: Payload = serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest)?;
    let v = vectorize(&payload, &state.norm, &state.mcc).map_err(|_| ApiError::BadRequest)?;
    let q = quantize(&v);
    let top = knn5(&q, state.refs_i8());
    let score = fraud_score(&top, state.labels_bits());
    let resp = FraudScoreResponse {
        approved: score < 0.6,
        fraud_score: score,
    };
    // Manual serialization keeps the response body content-type stable and
    // skips axum's default `Json` wrapping.
    let body = serde_json::to_vec(&resp).map_err(|_| ApiError::BadRequest)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}
