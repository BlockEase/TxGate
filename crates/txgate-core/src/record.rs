//! The request record: one JSON-RPC call as seen by TxGate. It is the monitor
//! row, the persisted log entry and what the UI renders.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decode::{Analysis, Authorization, TxFields};
use crate::enrich::Enrichment;
use crate::jsonrpc::Category;
use crate::preview::PreviewMatch;
use crate::simulate::SimulationOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Read/estimate request forwarded and answered by the upstream.
    Forwarded,
    /// Upstream unreachable or returned a transport error.
    Failed,
    /// Rejected locally before reaching any upstream (bad request).
    Invalid,
    /// Sending request waiting for the user's decision.
    Pending,
    /// Waiting for a decision, but the wallet's HTTP connection has closed.
    Disconnected,
    /// Approved and broadcast; the upstream accepted it.
    Approved,
    /// Approved, but the broadcast upstream returned an error.
    BroadcastFailed,
    /// Same transaction was approved before, forwarded without asking.
    AutoApproved,
    Rejected,
    /// Was pending when TxGate shut down; the wallet connection no longer exists.
    Abandoned,
}

impl Status {
    /// Still waiting for a decision.
    pub fn is_open(self) -> bool {
        matches!(self, Status::Pending | Status::Disconnected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Rejected,
}

/// Serialises a unit enum to its serde name (used for SQLite text columns).
pub(crate) fn name_of<T: Serialize>(v: T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Parses a serde name back into a unit enum.
pub(crate) fn parse_name<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub id: u64,
    pub ts_ms: u64,
    pub chain_id: u64,
    pub method: String,
    pub category: Category,
    /// Remote address of the wallet connection.
    pub client: String,
    pub summary: String,
    pub status: Status,
    pub duration_ms: Option<u64>,
    pub tx_hash: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub request: Value,
    pub response: Option<Value>,
    /// Decoded transactions and their checks; estimate and send calls only.
    pub detail: Option<TxDetail>,
}

/// Analysis of one transaction inside a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TxView {
    pub hash: Option<String>,
    pub tx_type: Option<String>,
    pub fields: TxFields,
    /// Wei, decimal string.
    pub max_fee_per_gas: Option<String>,
    pub max_priority_fee_per_gas: Option<String>,
    pub authorizations: Vec<Authorization>,
    pub analysis: Analysis,
    /// Chain-state checks; `None` until they finish.
    pub enrichment: Option<Enrichment>,
    pub simulator_url: String,
    /// ERC-4337: the EntryPoint that calls the smart account (`fields.from`),
    /// so simulations run with it as the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<alloy_primitives::Address>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TxDetail {
    pub txs: Vec<TxView>,
    /// Send calls only: how this transaction relates to recent `eth_estimateGas` previews.
    pub preview: Option<PreviewMatch>,
    pub decode_error: Option<String>,
    pub dedupe_key: Option<String>,
    /// Times the wallet submitted this same transaction while it was held.
    pub attempts: u32,
    /// Wallet HTTP connections currently waiting for the decision.
    pub wallet_connections: usize,
    pub simulation: Option<SimulationOutcome>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_names_round_trip() {
        assert_eq!(name_of(Status::BroadcastFailed), "broadcast_failed");
        assert_eq!(parse_name::<Status>("auto_approved"), Some(Status::AutoApproved));
        assert_eq!(parse_name::<Category>("estimate"), Some(Category::Estimate));
        assert_eq!(parse_name::<Status>("nonsense"), None);
    }
}
