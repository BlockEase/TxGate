//! Client for the Safe Transaction Service, where Safe{Wallet} stores proposed
//! transactions and off-chain signatures.
//!
//! The service is a third party and may be compromised (as the Safe{Wallet}
//! front-end was in the Bybit attack). Nothing it says is trusted: every
//! transaction is re-hashed from its raw fields and decoded locally, and its
//! `dataDecoded` is ignored.

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::SafeConfig;
use crate::safe::{safe_tx_hashes, SafeHashes, SafeTx};
use crate::upstream::Upstream;

/// EIP-3770 short names the hosted service uses, by chain id.
const SHORT_NAMES: &[(u64, &str)] = &[
    (1, "eth"),
    (10, "oeth"),
    (56, "bnb"),
    (100, "gno"),
    (130, "unichain"),
    (137, "pol"),
    (146, "sonic"),
    (324, "zksync"),
    (480, "wc"),
    (5000, "mantle"),
    (8453, "base"),
    (42161, "arb1"),
    (42220, "celo"),
    (43114, "avax"),
    (59144, "linea"),
    (84532, "basesep"),
    (534352, "scr"),
    (11155111, "sep"),
];

/// Base URL of the Transaction Service for `chain_id`, if there is one.
pub fn service_base(cfg: &SafeConfig, chain_id: u64) -> Option<String> {
    let custom = cfg.service_url.trim().trim_end_matches('/');
    if !custom.is_empty() {
        return Some(custom.to_string());
    }
    SHORT_NAMES
        .iter()
        .find(|(id, _)| *id == chain_id)
        .map(|(_, name)| format!("https://api.safe.global/tx-service/{name}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub owner: Address,
    pub signature: Bytes,
}

/// A multisig transaction as stored by the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTx {
    pub safe: Address,
    pub tx: SafeTx,
    /// The hash the service claims for this transaction.
    pub reported_hash: B256,
    pub proposer: Option<Address>,
    pub submission_date: Option<String>,
    pub is_executed: bool,
    pub confirmations: Vec<Confirmation>,
    /// Free text the proposing app attached (e.g. a description); untrusted.
    pub origin: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServiceError {
    #[error("the Safe Transaction Service is not available for chain {0}")]
    Unsupported(u64),
    #[error("Safe Transaction Service: {0}")]
    Request(String),
    #[error("unexpected Safe Transaction Service response: {0}")]
    Format(String),
}

impl ServiceError {
    /// The message for translation.
    pub fn text(&self) -> crate::text::Text {
        use crate::text::Text;
        match self {
            Self::Unsupported(chain) => Text::new("watch.service_unsupported", [("chain", chain.to_string())]),
            Self::Request(e) => Text::new("watch.service_request", [("error", e.clone())]),
            Self::Format(e) => Text::new("watch.service_format", [("error", e.clone())]),
        }
    }
}

impl ServiceTx {
    pub fn from_json(v: &Value) -> Result<Self, ServiceError> {
        let tx = SafeTx::from_json(v).map_err(ServiceError::Format)?;
        let confirmations = v
            .get("confirmations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|c| {
                        Some(Confirmation {
                            owner: c.get("owner")?.as_str()?.parse().ok()?,
                            signature: c.get("signature")?.as_str()?.parse().ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            safe: address_field(v, "safe")?,
            tx,
            reported_hash: v
                .get("safeTxHash")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| ServiceError::Format("missing safeTxHash".into()))?,
            proposer: v.get("proposer").and_then(Value::as_str).and_then(|s| s.parse().ok()),
            submission_date: v.get("submissionDate").and_then(Value::as_str).map(str::to_string),
            is_executed: v.get("isExecuted").and_then(Value::as_bool).unwrap_or(false),
            confirmations,
            origin: v.get("origin").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// Hashes computed locally from the raw fields.
    pub fn hashes(&self, chain_id: u64, version: Option<&str>) -> SafeHashes {
        safe_tx_hashes(chain_id, self.safe, version, &self.tx)
    }
}

fn address_field(v: &Value, k: &str) -> Result<Address, ServiceError> {
    v.get(k).and_then(Value::as_str).and_then(|s| s.parse().ok()).ok_or_else(|| ServiceError::Format(format!("missing {k}")))
}

async fn get(up: &dyn Upstream, cfg: &SafeConfig, url: &str) -> Result<Value, ServiceError> {
    let key = Some(cfg.service_api_key.as_str()).filter(|k| !k.trim().is_empty());
    up.get(url, key).await.map_err(|e| ServiceError::Request(e.to_string()))
}

/// Not yet executed transactions of `safe` with a nonce of at least `from_nonce`, lowest nonce first.
pub async fn queued(up: &dyn Upstream, cfg: &SafeConfig, chain_id: u64, safe: Address, from_nonce: U256) -> Result<Vec<ServiceTx>, ServiceError> {
    let base = service_base(cfg, chain_id).ok_or(ServiceError::Unsupported(chain_id))?;
    let url = format!(
        "{base}/api/v2/safes/{}/multisig-transactions/?executed=false&nonce__gte={from_nonce}&ordering=nonce&limit=100",
        safe.to_checksum(None)
    );
    let doc = get(up, cfg, &url).await?;
    let results = doc.get("results").and_then(Value::as_array).ok_or_else(|| ServiceError::Format("missing results".into()))?;
    results.iter().map(ServiceTx::from_json).collect()
}

/// The transaction the service stores under `safe_tx_hash`.
pub async fn by_hash(up: &dyn Upstream, cfg: &SafeConfig, chain_id: u64, safe_tx_hash: B256) -> Result<ServiceTx, ServiceError> {
    let base = service_base(cfg, chain_id).ok_or(ServiceError::Unsupported(chain_id))?;
    let doc = get(up, cfg, &format!("{base}/api/v2/multisig-transactions/{safe_tx_hash}/")).await?;
    ServiceTx::from_json(&doc)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::safe::fixtures;
    use crate::upstream::fake::FakeUpstream;
    use serde_json::json;

    /// The fixture transaction as the service returns it.
    pub fn service_json() -> Value {
        let t = fixtures::tx();
        json!({
            "safe": fixtures::SAFE.to_checksum(None),
            "to": t.to.to_checksum(None),
            "value": "0",
            "data": t.data.to_string(),
            "operation": 1,
            "gasToken": "0x0000000000000000000000000000000000000000",
            "safeTxGas": "0",
            "baseGas": "0",
            "gasPrice": "0",
            "refundReceiver": "0x0000000000000000000000000000000000000000",
            "nonce": "2867",
            "safeTxHash": fixtures::SAFE_TX_HASH.to_string(),
            "proposer": "0x964aE5aB627f1d536d5d5796734B7a44AD6531E3",
            "isExecuted": false,
            "confirmations": [
                {"owner": "0x964aE5aB627f1d536d5d5796734B7a44AD6531E3", "signature": "0xa51db4a84de84081833627336ff5d1b49a4acc0f60164ab705cd71f1ff672e7367b50680602f230ca982ca95050142211a662f52c4aa8890c5e737459ba6a5281c"}
            ],
            "dataDecoded": {"method": "transfer", "parameters": []}
        })
    }

    #[test]
    fn parses_a_service_transaction_and_rehashes_it() {
        let s = ServiceTx::from_json(&service_json()).unwrap();
        assert_eq!(s.tx, fixtures::tx());
        assert_eq!(s.confirmations.len(), 1);
        assert_eq!(s.hashes(1, Some("1.3.0")).safe_tx_hash, s.reported_hash);
    }

    #[test]
    fn a_tampered_transaction_no_longer_matches_its_reported_hash() {
        let mut doc = service_json();
        doc["operation"] = json!(0);
        let s = ServiceTx::from_json(&doc).unwrap();
        assert_ne!(s.hashes(1, Some("1.3.0")).safe_tx_hash, s.reported_hash);
    }

    #[test]
    fn uses_the_hosted_service_or_a_custom_url() {
        let mut cfg = SafeConfig::default();
        assert_eq!(service_base(&cfg, 137).as_deref(), Some("https://api.safe.global/tx-service/pol"));
        assert_eq!(service_base(&cfg, 999_999), None);
        cfg.service_url = "https://safe.example/ ".into();
        assert_eq!(service_base(&cfg, 999_999).as_deref(), Some("https://safe.example"));
    }

    #[tokio::test]
    async fn fetches_queued_transactions_with_the_api_key() {
        let up = FakeUpstream::new();
        up.on_get("https://api.safe.global/tx-service/eth/api/v2/safes/", json!({"results": [service_json()]}));
        let cfg = SafeConfig { service_api_key: "k".into(), ..Default::default() };
        let list = queued(&up, &cfg, 1, fixtures::SAFE, U256::from(2867)).await.unwrap();
        assert_eq!(list.len(), 1);
        let (url, key) = &up.gets()[0];
        assert!(url.contains("0x849D52316331967b6fF1198e5E32A0eB168D039d") && url.contains("nonce__gte=2867"), "{url}");
        assert_eq!(key.as_deref(), Some("k"));
    }
}
