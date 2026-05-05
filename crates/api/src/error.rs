//! Public error type for handler responses.
//!
//! Only `400` is ever produced from a request — the scan kernel is total and
//! `AppState` boot validation rejects malformed artifacts up front, so no `5xx`
//! is reachable from a well-formed deployment.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum ApiError {
    BadRequest,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, ()).into_response(),
        }
    }
}
