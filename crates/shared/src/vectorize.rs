//! 14-dimension vectorization of fraud-detection payloads.
//!
//! See `REGRAS_DE_DETECCAO.md` for the formulas. The output is a `[f32; 14]`
//! where every dimension is clamped to `[0.0, 1.0]` *except* indices 5 and 6
//! when `last_transaction` is `null` — in that case both are encoded as
//! `f32::NAN` and `quantize()` later converts them to `SENTINEL_I8`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::DIMS;
use crate::datetime::Utc;

#[derive(Debug, Clone, Deserialize)]
pub struct Normalization {
    pub max_amount: f32,
    pub max_installments: f32,
    pub amount_vs_avg_ratio: f32,
    pub max_minutes: f32,
    pub max_km: f32,
    pub max_tx_count_24h: f32,
    pub max_merchant_avg_amount: f32,
}

impl Normalization {
    pub fn from_json_str(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

/// Mapping `mcc → risk` with a default of `0.5` for unseen codes.
#[derive(Debug, Clone, Default)]
pub struct MccRisk {
    table: HashMap<String, f32>,
}

impl MccRisk {
    pub fn from_json_str(s: &str) -> serde_json::Result<Self> {
        let table: HashMap<String, f32> = serde_json::from_str(s)?;
        Ok(Self { table })
    }

    #[must_use]
    pub fn risk(&self, mcc: &str) -> f32 {
        self.table.get(mcc).copied().unwrap_or(0.5)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Payload {
    pub id: String,
    pub transaction: Transaction,
    pub customer: Customer,
    pub merchant: Merchant,
    pub terminal: Terminal,
    pub last_transaction: Option<LastTransaction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transaction {
    pub amount: f32,
    pub installments: u32,
    pub requested_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Customer {
    pub avg_amount: f32,
    pub tx_count_24h: u32,
    pub known_merchants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Merchant {
    pub id: String,
    pub mcc: String,
    pub avg_amount: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Terminal {
    pub is_online: bool,
    pub card_present: bool,
    pub km_from_home: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LastTransaction {
    pub timestamp: String,
    pub km_from_current: f32,
}

#[derive(Debug, Clone)]
pub enum VectorizeError {
    Timestamp(crate::datetime::ParseError),
}

impl core::fmt::Display for VectorizeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Timestamp(e) => write!(f, "invalid timestamp: {e}"),
        }
    }
}

impl std::error::Error for VectorizeError {}

impl From<crate::datetime::ParseError> for VectorizeError {
    fn from(e: crate::datetime::ParseError) -> Self {
        Self::Timestamp(e)
    }
}

/// Apply the 14-dimension vectorization formulas. The output uses `f32::NAN`
/// in indices 5 and 6 when `last_transaction` is `null`; `quantize()` is
/// responsible for emitting `SENTINEL_I8` for those slots.
pub fn vectorize(
    payload: &Payload,
    norm: &Normalization,
    mcc: &MccRisk,
) -> Result<[f32; DIMS], VectorizeError> {
    let now = Utc::parse(&payload.transaction.requested_at)?;
    let mut v = [0.0_f32; DIMS];

    v[0] = clamp01(payload.transaction.amount / norm.max_amount);
    v[1] = clamp01(payload.transaction.installments as f32 / norm.max_installments);
    v[2] = clamp01((payload.transaction.amount / payload.customer.avg_amount) / norm.amount_vs_avg_ratio);
    v[3] = now.hour as f32 / 23.0;
    v[4] = now.weekday_monday0() as f32 / 6.0;

    if let Some(last) = &payload.last_transaction {
        let prev = Utc::parse(&last.timestamp)?;
        let minutes = (now.unix_seconds() - prev.unix_seconds()) as f32 / 60.0;
        v[5] = clamp01(minutes / norm.max_minutes);
        v[6] = clamp01(last.km_from_current / norm.max_km);
    } else {
        v[5] = f32::NAN;
        v[6] = f32::NAN;
    }

    v[7] = clamp01(payload.terminal.km_from_home / norm.max_km);
    v[8] = clamp01(payload.customer.tx_count_24h as f32 / norm.max_tx_count_24h);
    v[9] = if payload.terminal.is_online { 1.0 } else { 0.0 };
    v[10] = if payload.terminal.card_present { 1.0 } else { 0.0 };
    v[11] = if payload
        .customer
        .known_merchants
        .iter()
        .any(|m| m == &payload.merchant.id)
    {
        0.0
    } else {
        1.0
    };
    v[12] = mcc.risk(&payload.merchant.mcc);
    v[13] = clamp01(payload.merchant.avg_amount / norm.max_merchant_avg_amount);

    Ok(v)
}

/// Clamp to `[0.0, 1.0]`. NaN inputs (e.g. divisions by zero on customer
/// `avg_amount=0`) collapse to `0.0` so that only indices 5 and 6 can carry
/// a NaN sentinel through to `quantize()`.
fn clamp01(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn normalization_parses_canonical_file() {
        let n = Normalization::from_json_str(NORMALIZATION_JSON).unwrap();
        assert!((n.max_amount - 10_000.0).abs() < 1e-3);
        assert!((n.max_installments - 12.0).abs() < 1e-3);
        assert!((n.amount_vs_avg_ratio - 10.0).abs() < 1e-3);
        assert!((n.max_minutes - 1440.0).abs() < 1e-3);
        assert!((n.max_km - 1000.0).abs() < 1e-3);
        assert!((n.max_tx_count_24h - 20.0).abs() < 1e-3);
        assert!((n.max_merchant_avg_amount - 10_000.0).abs() < 1e-3);
    }

    #[test]
    fn mcc_risk_returns_known_values_and_default() {
        let m = MccRisk::from_json_str(MCC_RISK_JSON).unwrap();
        assert!((m.risk("5411") - 0.15).abs() < 1e-3);
        assert!((m.risk("7802") - 0.75).abs() < 1e-3);
        // unknown MCC → default 0.5
        assert!((m.risk("0000") - 0.5).abs() < 1e-3);
        assert!((m.risk("") - 0.5).abs() < 1e-3);
        assert_eq!(m.len(), 10);
    }

    #[test]
    fn mcc_risk_default_when_table_empty() {
        let m = MccRisk::default();
        assert!(m.is_empty());
        assert!((m.risk("5411") - 0.5).abs() < 1e-3);
    }
}
