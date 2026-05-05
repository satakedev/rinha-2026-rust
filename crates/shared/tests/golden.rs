//! Golden cases pulled directly from `REGRAS_DE_DETECCAO.md`.
//!
//! Both scenarios validate that `vectorize()` produces the same 14-dim float
//! vector documented in the spec, and that `quantize()` collapses to the
//! expected `[i8; 16]` (with the `-1` sentinel preserved at indices 5 and 6
//! and the trailing two padding bytes zeroed).

use shared::{DIMS, MccRisk, Normalization, Payload, SENTINEL_I8, quantize, vectorize};

const NORMALIZATION: &str = r#"{
    "max_amount": 10000,
    "max_installments": 12,
    "amount_vs_avg_ratio": 10,
    "max_minutes": 1440,
    "max_km": 1000,
    "max_tx_count_24h": 20,
    "max_merchant_avg_amount": 10000
}"#;

const MCC_RISK: &str = r#"{
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

const LEGIT_PAYLOAD: &str = r#"{
    "id": "tx-1329056812",
    "transaction":      { "amount": 41.12, "installments": 2, "requested_at": "2026-03-11T18:45:53Z" },
    "customer":         { "avg_amount": 82.24, "tx_count_24h": 3, "known_merchants": ["MERC-003", "MERC-016"] },
    "merchant":         { "id": "MERC-016", "mcc": "5411", "avg_amount": 60.25 },
    "terminal":         { "is_online": false, "card_present": true, "km_from_home": 29.23 },
    "last_transaction": null
}"#;

const FRAUD_PAYLOAD: &str = r#"{
    "id": "tx-3330991687",
    "transaction":      { "amount": 9505.97, "installments": 10, "requested_at": "2026-03-14T05:15:12Z" },
    "customer":         { "avg_amount": 81.28, "tx_count_24h": 20, "known_merchants": ["MERC-008", "MERC-007", "MERC-005"] },
    "merchant":         { "id": "MERC-068", "mcc": "7802", "avg_amount": 54.86 },
    "terminal":         { "is_online": false, "card_present": true, "km_from_home": 952.27 },
    "last_transaction": null
}"#;

fn test_inputs() -> (Normalization, MccRisk) {
    (
        Normalization::from_json_str(NORMALIZATION).unwrap(),
        MccRisk::from_json_str(MCC_RISK).unwrap(),
    )
}

fn assert_vec_eq(actual: &[f32; DIMS], expected: &[f32; DIMS], tol: f32) {
    for i in 0..DIMS {
        let a = actual[i];
        let e = expected[i];
        if e.is_nan() {
            assert!(a.is_nan(), "idx {i}: expected NaN, got {a}");
        } else {
            assert!(
                (a - e).abs() < tol,
                "idx {i}: expected {e}, got {a} (delta {} > tol {tol})",
                (a - e).abs()
            );
        }
    }
}

#[test]
fn legitimate_transaction_matches_spec_vector() {
    let (norm, mcc) = test_inputs();
    let payload: Payload = serde_json::from_str(LEGIT_PAYLOAD).unwrap();
    let v = vectorize(&payload, &norm, &mcc).unwrap();

    // Spec vector — NaN models the `-1` sentinel for indices 5 and 6.
    let expected: [f32; DIMS] = [
        0.004_112,  // 41.12 / 10000
        0.166_667,  // 2 / 12
        0.05,       // (41.12 / 82.24) / 10
        0.782_609,  // 18 / 23
        0.333_333,  // weekday Wed=2 / 6
        f32::NAN,
        f32::NAN,
        0.029_23,   // 29.23 / 1000
        0.15,       // 3 / 20
        0.0,        // is_online=false
        1.0,        // card_present=true
        0.0,        // MERC-016 IS in known_merchants → 0
        0.15,       // mcc 5411 risk
        0.006_025,  // 60.25 / 10000
    ];
    assert_vec_eq(&v, &expected, 5e-4);

    let q = quantize(&v);
    let expected_q: [i8; 16] = [
        0,    // 0.0041 * 100 ≈ 0.41 → 0
        17,   // 0.1667 * 100 ≈ 16.67 → 17
        5,    // 0.05 * 100 = 5
        78,   // 0.7826 * 100 ≈ 78.26 → 78
        33,   // 0.3333 * 100 ≈ 33.33 → 33
        SENTINEL_I8,
        SENTINEL_I8,
        3,    // 0.0292 * 100 ≈ 2.92 → 3
        15,
        0,
        100,
        0,
        15,
        1,    // 0.006 * 100 = 0.6 → 1
        0, 0, // padding
    ];
    assert_eq!(q, expected_q);
}

#[test]
fn fraudulent_transaction_matches_spec_vector() {
    let (norm, mcc) = test_inputs();
    let payload: Payload = serde_json::from_str(FRAUD_PAYLOAD).unwrap();
    let v = vectorize(&payload, &norm, &mcc).unwrap();

    let expected: [f32; DIMS] = [
        0.950_597,  // 9505.97 / 10000
        0.833_333,  // 10 / 12
        1.0,        // (9505.97 / 81.28) / 10 → clamp 1
        0.217_391,  // 5 / 23
        0.833_333,  // weekday Sat=5 / 6
        f32::NAN,
        f32::NAN,
        0.952_27,   // 952.27 / 1000
        1.0,        // 20 / 20
        0.0,
        1.0,
        1.0,        // MERC-068 NOT in known_merchants → 1
        0.75,       // mcc 7802 risk
        0.005_486,  // 54.86 / 10000
    ];
    assert_vec_eq(&v, &expected, 5e-4);

    let q = quantize(&v);
    let expected_q: [i8; 16] = [
        95,  // 0.9506 * 100 ≈ 95.06 → 95
        83,  // 0.8333 * 100 ≈ 83.33 → 83
        100, // 1.0 * 100 = 100
        22,  // 0.2174 * 100 ≈ 21.74 → 22
        83,  // 0.8333 * 100 ≈ 83.33 → 83
        SENTINEL_I8,
        SENTINEL_I8,
        95,  // 0.9523 * 100 ≈ 95.23 → 95
        100,
        0,
        100,
        100,
        75,  // 0.75 * 100 = 75
        1,   // 0.0055 * 100 = 0.55 → 1
        0, 0, // padding
    ];
    assert_eq!(q, expected_q);
}

#[test]
fn empty_known_merchants_marks_unknown() {
    let payload_json = r#"{
        "id": "tx-001",
        "transaction": { "amount": 10.0, "installments": 1, "requested_at": "2026-01-01T00:00:00Z" },
        "customer": { "avg_amount": 100.0, "tx_count_24h": 1, "known_merchants": [] },
        "merchant": { "id": "MERC-X", "mcc": "9999", "avg_amount": 50.0 },
        "terminal": { "is_online": true, "card_present": false, "km_from_home": 5.0 },
        "last_transaction": { "timestamp": "2025-12-31T23:00:00Z", "km_from_current": 1.5 }
    }"#;
    let (norm, mcc) = test_inputs();
    let payload: Payload = serde_json::from_str(payload_json).unwrap();
    let v = vectorize(&payload, &norm, &mcc).unwrap();

    // unknown_merchant must be 1 because the customer has no history.
    assert!((v[11] - 1.0).abs() < 1e-6);
    // mcc 9999 not in table → default 0.5
    assert!((v[12] - 0.5).abs() < 1e-6);
    // 60-minute gap → 60 / 1440 ≈ 0.0417
    assert!((v[5] - 0.041_667).abs() < 5e-4);
    assert!(!v[5].is_nan(), "last_transaction present, no sentinel");
}
