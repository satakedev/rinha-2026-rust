//! Integration tests: spin up the API in-process with a 10 000-vector
//! synthetic dataset, then exercise `/ready` and `/fraud-score` across the
//! happy path, sentinel cases, missing MCC, extra fields and bad payloads.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use api::{AppState, RefBytes, router};
use axum::body::{self, Body};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use shared::{
    DIMS, LabelBitsetWriter, MccRisk, Normalization, PAD, REFS_HEADER_LEN, quantize,
    write_references_header,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const N: usize = 10_000;

const NORMALIZATION_JSON: &str = r#"{
    "max_amount": 10000,
    "max_installments": 12,
    "amount_vs_avg_ratio": 10,
    "max_minutes": 1440,
    "max_km": 1000,
    "max_tx_count_24h": 20,
    "max_merchant_avg_amount": 10000
}"#;

const MCC_RISK_JSON: &str = r#"{
    "5411": 0.15,
    "5812": 0.30,
    "5912": 0.20,
    "5944": 0.45,
    "7801": 0.80,
    "7802": 0.75,
    "7995": 0.85,
    "4511": 0.35,
    "5311": 0.25,
    "5999": 0.50
}"#;

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn xorshift_unit(state: &mut u64) -> f32 {
    ((xorshift(state) >> 11) as f32) / ((1_u64 << 53) as f32)
}

/// Build a synthetic 10k-vector dataset (refs buffer + labels bitset).
fn build_dataset(n: usize, mut seed: u64) -> (Vec<u8>, Vec<u8>) {
    let mut refs = Vec::with_capacity(REFS_HEADER_LEN + n * PAD);
    write_references_header(&mut refs, n as u64).unwrap();

    let mut label_writer = LabelBitsetWriter::new(Vec::with_capacity(n.div_ceil(8)));
    for i in 0..n {
        let mut v = [0_f32; DIMS];
        for slot in &mut v {
            *slot = xorshift_unit(&mut seed);
        }
        // Every 7th sample carries the sentinel at dims 5,6 — same convention
        // as build-dataset's roundtrip test.
        if i % 7 == 0 {
            v[5] = f32::NAN;
            v[6] = f32::NAN;
        }
        let q = quantize(&v);
        // i8 → u8 reinterpret keeps bytes identical.
        refs.extend(q.iter().map(|b| *b as u8));
        label_writer.push(i % 3 == 0).unwrap();
    }
    let labels = label_writer.finish().unwrap();
    (refs, labels)
}

fn build_state() -> Arc<AppState> {
    let (refs, labels) = build_dataset(N, 0xCAFE_F00D);
    let norm = Normalization::from_json_str(NORMALIZATION_JSON).unwrap();
    let mcc = MccRisk::from_json_str(MCC_RISK_JSON).unwrap();
    let state = Arc::new(AppState {
        ready: AtomicBool::new(false),
        refs: RefBytes::Owned(refs),
        labels: RefBytes::Owned(labels),
        n: N as u32,
        norm,
        mcc,
    });
    state.mark_ready();
    state
}

fn happy_payload() -> Value {
    serde_json::json!({
        "id": "tx-1329056812",
        "transaction": {
            "amount": 41.12,
            "installments": 2,
            "requested_at": "2026-03-11T18:45:53Z"
        },
        "customer": {
            "avg_amount": 82.24,
            "tx_count_24h": 3,
            "known_merchants": ["MERC-003", "MERC-016"]
        },
        "merchant": {
            "id": "MERC-016",
            "mcc": "5411",
            "avg_amount": 60.25
        },
        "terminal": {
            "is_online": false,
            "card_present": true,
            "km_from_home": 29.23
        },
        "last_transaction": null
    })
}

async fn post_fraud_score(
    app: axum::Router,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method("POST")
        .uri("/fraud-score")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

#[tokio::test]
async fn ready_flips_to_200_after_mark_ready() {
    let state = Arc::new(AppState {
        ready: AtomicBool::new(false),
        refs: RefBytes::Owned({
            let mut buf = Vec::new();
            write_references_header(&mut buf, 0).unwrap();
            buf
        }),
        labels: RefBytes::Owned(Vec::new()),
        n: 0,
        norm: Normalization::from_json_str(NORMALIZATION_JSON).unwrap(),
        mcc: MccRisk::from_json_str(MCC_RISK_JSON).unwrap(),
    });
    let app = router(Arc::clone(&state));

    let resp = app
        .clone()
        .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    state.mark_ready();
    let resp = app
        .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn happy_path_returns_well_formed_response() {
    let state = build_state();
    let app = router(state);
    let body = serde_json::to_vec(&happy_payload()).unwrap();
    let (status, bytes) = post_fraud_score(app, body).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_slice(&bytes).expect("response is JSON");
    let approved = v.get("approved").and_then(Value::as_bool).expect("approved");
    let score = v
        .get("fraud_score")
        .and_then(Value::as_f64)
        .expect("fraud_score") as f32;
    assert!((0.0..=1.0).contains(&score), "fraud_score out of range: {score}");
    assert_eq!(approved, score < 0.6);
    // Score must be one of {0.0, 0.2, 0.4, 0.6, 0.8, 1.0}.
    let s5 = (score * 5.0).round();
    assert!((s5 - score * 5.0).abs() < 1e-3, "score not n/5: {score}");
}

#[tokio::test]
async fn last_transaction_null_path() {
    // Already covered by happy_payload; this asserts the same explicitly.
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();
    payload["last_transaction"] = Value::Null;
    let (status, _) = post_fraud_score(app, serde_json::to_vec(&payload).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn unknown_mcc_uses_default_risk() {
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();
    payload["merchant"]["mcc"] = Value::String("9999".into());
    let (status, _) = post_fraud_score(app, serde_json::to_vec(&payload).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn extra_fields_are_ignored() {
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();
    payload["unrelated_field"] = serde_json::json!({"foo": "bar"});
    payload["transaction"]["surprise"] = Value::Number(42.into());
    let (status, _) = post_fraud_score(app, serde_json::to_vec(&payload).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn invalid_payload_returns_400_with_empty_body() {
    let state = build_state();
    let app = router(state);
    let (status, body) = post_fraud_score(app, b"not-json".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.is_empty(), "expected empty body, got {body:?}");
}

#[tokio::test]
async fn missing_required_field_returns_400() {
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();
    payload.as_object_mut().unwrap().remove("merchant");
    let (status, _) = post_fraud_score(app, serde_json::to_vec(&payload).unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn malformed_timestamp_returns_400() {
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();
    payload["transaction"]["requested_at"] = Value::String("yesterday".into());
    let (status, _) = post_fraud_score(app, serde_json::to_vec(&payload).unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn batch_of_200_requests_all_succeed() {
    // Full coverage requirement: ≥200 mixed requests over /fraud-score.
    let state = build_state();
    let app = router(state);
    let mut payload = happy_payload();

    for i in 0..200_u32 {
        // Vary the payload deterministically across iterations.
        let amount = 10.0 + f64::from(i);
        payload["transaction"]["amount"] = serde_json::Number::from_f64(amount).unwrap().into();
        if i % 4 == 0 {
            payload["last_transaction"] = Value::Null;
        } else {
            payload["last_transaction"] = serde_json::json!({
                "timestamp": "2026-03-11T14:58:35Z",
                "km_from_current": f64::from(i % 1000) * 0.7
            });
        }
        if i % 3 == 0 {
            payload["merchant"]["mcc"] = Value::String(format!("999{}", i % 10));
        } else {
            payload["merchant"]["mcc"] = Value::String("5411".into());
        }
        let body = serde_json::to_vec(&payload).unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/fraud-score")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "request {i} failed");
        let bytes = body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["approved"].is_boolean(), "request {i}: missing approved");
        assert!(v["fraud_score"].is_number(), "request {i}: missing fraud_score");
    }
}

#[tokio::test]
async fn binds_and_serves_over_real_tcp() {
    let state = build_state();
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let bound = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Bare HTTP/1.1 client over a TcpStream — avoids pulling in a full client
    // crate just for this smoke test.
    let mut stream = tokio::net::TcpStream::connect(bound).await.unwrap();
    let req = format!(
        "GET /ready HTTP/1.1\r\nHost: {bound}\r\nConnection: close\r\n\r\n",
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected /ready response: {response}",
    );

    server.abort();
}
