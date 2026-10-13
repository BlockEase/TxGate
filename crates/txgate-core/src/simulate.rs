//! Transaction simulation through the Tenderly Simulation API, plus a link to
//! Tenderly's web simulator as a fallback when no API key is configured.

use std::time::Duration;

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::TenderlyConfig;
use crate::decode::TxFields;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The sender receives this asset.
    In,
    /// The sender loses this asset.
    Out,
    /// Movement between third parties.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetChange {
    pub direction: Direction,
    pub symbol: String,
    pub token_address: Option<String>,
    /// Decimal-adjusted amount as reported by Tenderly.
    pub amount: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub dollar_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalChange {
    pub token_address: Option<String>,
    pub owner: Option<String>,
    pub spender: Option<String>,
    /// Raw amount, or `"all"` for `ApprovalForAll(…, true)`.
    pub amount: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationResult {
    pub success: bool,
    pub error: Option<String>,
    pub gas_used: Option<u64>,
    pub block_number: Option<u64>,
    pub asset_changes: Vec<AssetChange>,
    pub approvals: Vec<ApprovalChange>,
    pub simulation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SimulationOutcome {
    Ok { result: SimulationResult, ts_ms: u64 },
    Error { message: String, ts_ms: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum SimulateError {
    #[error("Tenderly is not configured")]
    NotConfigured,
    #[error("transaction has no sender")]
    NoSender,
    #[error("Tenderly request failed: {0}")]
    Request(String),
    #[error("Tenderly returned HTTP {0}: {1}")]
    Http(u16, String),
}

/// Link to Tenderly's web simulator prefilled with the transaction. `caller`
/// overrides `tx.from` as the simulated sender (the EntryPoint of a UserOperation).
pub fn web_simulator_url(chain_id: u64, tx: &TxFields, caller: Option<Address>) -> String {
    let mut url = format!("https://dashboard.tenderly.co/simulator/new?network={chain_id}");
    if let Some(from) = caller.or(tx.from) {
        url.push_str(&format!("&from={from}"));
    }
    if let Some(to) = tx.to {
        url.push_str(&format!("&contractAddress={to}"));
    }
    if !tx.input.is_empty() {
        url.push_str(&format!("&rawFunctionInput={}", tx.input));
    }
    url.push_str(&format!("&value={}", tx.value));
    if let Some(gas) = tx.gas_limit {
        url.push_str(&format!("&gas={gas}"));
    }
    url
}

/// The Tenderly request; `caller` overrides `tx.from` as the simulated sender.
pub fn request_body(chain_id: u64, tx: &TxFields, caller: Option<Address>) -> Result<Value, SimulateError> {
    let from = caller.or(tx.from).ok_or(SimulateError::NoSender)?;
    let mut body = json!({
        "network_id": chain_id.to_string(),
        "from": from.to_string(),
        "input": tx.input.to_string(),
        "value": tx.value.to_string(),
        "gas_price": "0",
        "save": true,
        "save_if_fails": true,
        "simulation_type": "full",
    });
    if let Some(to) = tx.to {
        body["to"] = json!(to.to_string());
    }
    if let Some(gas) = tx.gas_limit {
        body["gas"] = json!(gas);
    }
    Ok(body)
}

/// Simulates `tx`; balance changes are reported for `tx.from` even when
/// `caller` sends the call.
pub async fn simulate(cfg: &TenderlyConfig, chain_id: u64, tx: &TxFields, caller: Option<Address>) -> Result<SimulationResult, SimulateError> {
    if !cfg.is_configured() {
        return Err(SimulateError::NotConfigured);
    }
    let body = request_body(chain_id, tx, caller)?;
    let url = format!(
        "https://api.tenderly.co/api/v1/account/{}/project/{}/simulate",
        cfg.account.trim(),
        cfg.project.trim()
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| SimulateError::Request(e.to_string()))?;
    let resp = client
        .post(url)
        .header("X-Access-Key", cfg.access_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| SimulateError::Request(e.without_url().to_string()))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| SimulateError::Request(e.to_string()))?;
    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| text.chars().take(300).collect());
        return Err(SimulateError::Http(status.as_u16(), msg));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| SimulateError::Request(e.to_string()))?;
    Ok(parse_response(&v, tx.from))
}

fn lower(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_lowercase())
}

/// Normalises a Tenderly simulation response.
pub fn parse_response(v: &Value, sender: Option<Address>) -> SimulationResult {
    let tx = &v["transaction"];
    let info = &tx["transaction_info"];
    let sender = sender.map(|a| a.to_string().to_lowercase());

    let asset_changes = info["asset_changes"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|c| {
                    let from = lower(&c["from"]);
                    let to = lower(&c["to"]);
                    let direction = if sender.is_some() && from == sender {
                        Direction::Out
                    } else if sender.is_some() && to == sender {
                        Direction::In
                    } else {
                        Direction::Other
                    };
                    let ti = &c["token_info"];
                    AssetChange {
                        direction,
                        symbol: ti["symbol"].as_str().or(ti["name"].as_str()).unwrap_or("?").to_uppercase(),
                        token_address: ti["contract_address"].as_str().map(str::to_string),
                        amount: c["amount"].as_str().map(str::to_string).unwrap_or_else(|| c["raw_amount"].to_string()),
                        from,
                        to,
                        dollar_value: c["dollar_value"].as_str().map(str::to_string),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let approvals = info["logs"]
        .as_array()
        .map(|logs| {
            logs.iter()
                .filter_map(|log| {
                    let name = log["name"].as_str()?;
                    let inputs = log["inputs"].as_array()?;
                    let input = |idx: usize| inputs.get(idx).and_then(|i| i["value"].as_str().map(str::to_string).or_else(|| i["value"].as_bool().map(|b| b.to_string())));
                    let token_address = log["raw"]["address"].as_str().map(str::to_string);
                    // Only allowances granted by the sender matter to them.
                    if sender.is_some() && inputs.first().and_then(|i| lower(&i["value"])) != sender {
                        return None;
                    }
                    match name {
                        "Approval" => Some(ApprovalChange { token_address, owner: input(0), spender: input(1), amount: input(2).unwrap_or_default() }),
                        "ApprovalForAll" if input(2).as_deref() == Some("true") => {
                            Some(ApprovalChange { token_address, owner: input(0), spender: input(1), amount: "all".into() })
                        }
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let success = tx["status"].as_bool().unwrap_or(false);
    let error = if success {
        None
    } else {
        tx["error_message"]
            .as_str()
            .or(info["call_trace"]["error"].as_str())
            .map(str::to_string)
            .or(Some("Execution reverted".into()))
    };

    SimulationResult {
        success,
        error,
        gas_used: tx["gas_used"].as_u64(),
        block_number: tx["block_number"].as_u64().or(v["simulation"]["block_number"].as_u64()),
        asset_changes,
        approvals,
        simulation_id: v["simulation"]["id"].as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Bytes, U256};

    const ME: Address = address!("3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE");

    fn fixture() -> Value {
        json!({
            "transaction": {
                "status": true,
                "gas_used": 142318,
                "block_number": 21643052,
                "transaction_info": {
                    "asset_changes": [
                        {"token_info": {"symbol": "usdc", "contract_address": "0xa0b8"}, "type": "Transfer",
                         "from": "0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be", "to": "0xpool", "amount": "1000", "dollar_value": "1000"},
                        {"token_info": {"symbol": "eth"}, "type": "Transfer",
                         "from": "0xrouter", "to": "0x3f5ce5fbfe3e9af3971dd833d26ba9b5c936f0be", "amount": "0.3124"}
                    ],
                    "logs": [
                        {"name": "Approval", "raw": {"address": "0xa0b8"},
                         "inputs": [{"value": "0x3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE"}, {"value": "0x9e1f"}, {"value": "115792089237316195423570985008687907853269984665640564039457584007913129639935"}]},
                        {"name": "Approval", "raw": {"address": "0xdead"},
                         "inputs": [{"value": "0xsomeoneelse"}, {"value": "0x1234"}, {"value": "5"}]},
                        {"name": "Transfer", "inputs": []}
                    ]
                }
            },
            "simulation": {"id": "sim-1"}
        })
    }

    #[test]
    fn parses_asset_changes_relative_to_sender() {
        let r = parse_response(&fixture(), Some(ME));
        assert!(r.success);
        assert_eq!(r.gas_used, Some(142318));
        assert_eq!(r.asset_changes[0].direction, Direction::Out);
        assert_eq!(r.asset_changes[0].symbol, "USDC");
        assert_eq!(r.asset_changes[1].direction, Direction::In);
        // Only approvals granted by the sender count; the pool's own approval is not theirs.
        assert_eq!(r.approvals.len(), 1);
        assert_eq!(r.approvals[0].spender.as_deref(), Some("0x9e1f"));
        assert_eq!(r.simulation_id.as_deref(), Some("sim-1"));
    }

    #[test]
    fn failed_simulation_reports_error() {
        let v = json!({"transaction": {"status": false, "error_message": "execution reverted: STF", "transaction_info": {}}});
        let r = parse_response(&v, Some(ME));
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("execution reverted: STF"));
    }

    #[test]
    fn web_url_carries_the_transaction() {
        let tx = TxFields {
            from: Some(ME),
            to: Some(ME),
            input: Bytes::from(vec![0x09, 0x5e]),
            value: U256::from(5),
            ..Default::default()
        };
        let url = web_simulator_url(1, &tx, None);
        assert!(url.starts_with("https://dashboard.tenderly.co/simulator/new?network=1"));
        assert!(url.contains("rawFunctionInput=0x095e"));
        assert!(url.contains("&value=5"));
    }

    #[test]
    fn request_needs_a_sender() {
        assert!(matches!(request_body(1, &TxFields::default(), None), Err(SimulateError::NoSender)));
    }
}
