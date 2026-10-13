//! Risk checks that need chain state, fetched from the read upstream.

use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::SafeConfig;
use crate::decode::{format_units, is_unlimited_amount, Analysis, Risk, Severity, TxFields};
use crate::safe::approve_hash_arg;
use crate::safe_service;
use crate::safe_state::{check_exec, review_service_tx, SafeCheck, SafeTxReport};
use crate::text::Text;
use crate::upstream::{rpc_result, Upstream};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub address: String,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrichment {
    pub risks: Vec<Risk>,
    /// Token metadata when the call targets an ERC-20/721 style contract.
    pub token: Option<TokenInfo>,
    /// Human readable amounts keyed by argument name, e.g. `amount -> "1,000 USDC"`.
    pub formatted_args: Vec<(String, String)>,
    /// The sender's next nonce according to the upstream (`pending` tag).
    pub account_nonce: Option<u64>,
    /// Whether `to` has contract code.
    pub to_is_contract: Option<bool>,
    /// `execTransaction`: the Safe's state, the hash at its nonce and the verified signatures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe: Option<SafeCheck>,
    /// `approveHash`: the transaction the approved hash stands for, from the
    /// Safe Transaction Service, re-hashed and decoded locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_approval: Option<SafeTxReport>,
}

const TOKEN_FUNCTIONS: &[&str] = &["approve", "increaseAllowance", "transfer", "transferFrom", "permit"];

async fn has_code(up: &dyn Upstream, url: &str, addr: &Address) -> Option<bool> {
    let code = rpc_result(up, url, "eth_getCode", json!([addr.to_string(), "latest"])).await.ok()?;
    let code = code.as_str()?;
    Some(code != "0x" && !code.is_empty())
}

async fn eth_call(up: &dyn Upstream, url: &str, to: &Address, data: &str) -> Option<Vec<u8>> {
    let v = rpc_result(up, url, "eth_call", json!([{"to": to.to_string(), "data": data}, "latest"])).await.ok()?;
    hex::decode(v.as_str()?.trim_start_matches("0x")).ok()
}

/// Decodes an ABI `string` return value, falling back to `bytes32` (e.g. MKR).
fn decode_string(ret: &[u8]) -> Option<String> {
    if ret.len() >= 64 {
        // The length comes from an arbitrary contract: never trust it.
        let len = U256::from_be_slice(&ret[32..64]).saturating_to::<usize>();
        if let Some(bytes) = 64usize.checked_add(len).and_then(|end| ret.get(64..end)) {
            return String::from_utf8(bytes.to_vec()).ok().filter(|s| !s.is_empty());
        }
    }
    if ret.len() == 32 {
        let trimmed: Vec<u8> = ret.iter().copied().take_while(|b| *b != 0).collect();
        return String::from_utf8(trimmed).ok().filter(|s| !s.is_empty());
    }
    None
}

async fn token_info(up: &dyn Upstream, url: &str, token: &Address) -> TokenInfo {
    // symbol() and decimals()
    let symbol = eth_call(up, url, token, "0x95d89b41").await.and_then(|r| decode_string(&r));
    let decimals = eth_call(up, url, token, "0x313ce567")
        .await
        .filter(|r| r.len() >= 32)
        .map(|r| U256::from_be_slice(&r[..32]))
        .filter(|d| *d <= U256::from(77))
        .map(|d| d.saturating_to::<u8>());
    TokenInfo { address: token.to_checksum(None), symbol, decimals }
}

fn group_thousands(s: &str) -> String {
    let (int, frac) = s.split_once('.').map_or((s, None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{out}.{f}"),
        None => out,
    }
}

/// Formats a raw token amount for display: "Unlimited" for max-ish values, else
/// decimals-adjusted with the symbol.
pub fn display_amount(raw: &str, token: &TokenInfo) -> String {
    let Ok(v) = raw.parse::<U256>() else { return raw.to_string() };
    let sym = token.symbol.clone().unwrap_or_default();
    if is_unlimited_amount(v) {
        return format!("Unlimited {sym}").trim().to_string();
    }
    match token.decimals {
        Some(d) => format!("{} {sym}", group_thousands(&format_units(v, d))).trim().to_string(),
        None => format!("{raw} (raw units)"),
    }
}

/// Looks up what an `approveHash` approves.
async fn safe_approval(up: &dyn Upstream, read_url: &str, chain_id: u64, cfg: &SafeConfig, safe: alloy_primitives::Address, hash: alloy_primitives::B256) -> Result<SafeTxReport, String> {
    if !cfg.service_enabled {
        return Err("the Safe Transaction Service is turned off in Settings".into());
    }
    let stx = safe_service::by_hash(up, cfg, chain_id, hash).await.map_err(|e| e.to_string())?;
    if stx.safe != safe {
        return Err(format!("the service has this hash for another Safe ({})", stx.safe.to_checksum(None)));
    }
    let report = review_service_tx(up, read_url, chain_id, &stx, None).await;
    if report.hashes.safe_tx_hash != hash {
        return Err("the transaction the service returned does not hash to the approved hash".into());
    }
    Ok(report)
}

/// Runs all state-dependent checks for one transaction.
pub async fn enrich(up: &dyn Upstream, read_url: &str, chain_id: u64, safe_cfg: &SafeConfig, tx: &TxFields, analysis: &Analysis) -> Enrichment {
    let mut out = Enrichment::default();

    if let Some(exec) = &analysis.safe {
        let check = check_exec(up, read_url, chain_id, exec, tx.from).await;
        out.risks.extend(check.risks.iter().cloned());
        out.safe = Some(check);
    }
    if let (Some(to), Some(hash)) = (tx.to, approve_hash_arg(&tx.input)) {
        match safe_approval(up, read_url, chain_id, safe_cfg, to, hash).await {
            Ok(report) => {
                out.risks.extend(report.risks.iter().map(|r| r.clone().prefixed(Text::plain("prefix.approved_transaction"))));
                out.safe_approval = Some(report);
            }
            Err(e) => out.risks.push(Risk::t(Severity::Danger, "safe_approve_hash_unknown", "safe_approve_hash_unknown", [("error", e)])),
        }
    }
    let func = analysis.call.function.as_deref().unwrap_or_default();

    if let Some(to) = &tx.to {
        out.to_is_contract = has_code(up, read_url, to).await;
        if out.to_is_contract == Some(false) && !tx.input.is_empty() {
            out.risks.push(Risk::t(Severity::Warning, "call_to_eoa", "call_to_eoa", [("address", crate::decode::full(to))]));
        }

        let is_permit2 = analysis.call.signature.as_deref() == Some("approve(address,address,uint160,uint48)");
        if out.to_is_contract == Some(true) && TOKEN_FUNCTIONS.contains(&func) && !is_permit2 {
            let info = token_info(up, read_url, to).await;
            for name in ["amount", "addedValue", "value"] {
                if let Some(raw) = analysis.call.arg(name) {
                    out.formatted_args.push((name.to_string(), display_amount(raw, &info)));
                }
            }
            out.token = Some(info);
        }
    }

    // Approving an account without code is the classic drainer pattern.
    let grantee = match func {
        "approve" | "increaseAllowance" | "permit" => analysis.call.arg("spender"),
        "setApprovalForAll" if analysis.call.arg("approved") == Some("true") => analysis.call.arg("operator"),
        _ => None,
    };
    if let Some(addr) = grantee.and_then(|a| a.parse::<Address>().ok())
        && has_code(up, read_url, &addr).await == Some(false) {
            out.risks.push(Risk::t(Severity::Danger, "grantee_is_eoa", "grantee_is_eoa", [("address", crate::decode::full(&addr))]));
        }

    if let (Some(from), Some(nonce)) = (tx.from, tx.nonce) {
        let next = rpc_result(up, read_url, "eth_getTransactionCount", json!([from.to_string(), "pending"]))
            .await
            .ok()
            .and_then(|v| v.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()));
        out.account_nonce = next;
        if let Some(next) = next {
            if nonce > next {
                out.risks.push(Risk::t(
                    Severity::Warning,
                    "nonce_gap",
                    "nonce_gap",
                    [("nonce", nonce.to_string()), ("next", next.to_string()), ("last", (nonce - 1).to_string())],
                ));
            } else if nonce < next {
                out.risks.push(Risk::t(Severity::Info, "nonce_replacement", "nonce_replacement", [("nonce", nonce.to_string())]));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::analyze;
    use crate::config::SafeConfig;
    use crate::upstream::fake::FakeUpstream;
    use alloy_primitives::{address, Bytes};

    const ME: Address = address!("3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE");
    const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
    const EOA: Address = address!("9e1F000000000000000000000000000000003c07");

    fn abi_string(s: &str) -> String {
        let mut out = vec![0u8; 31];
        out.push(0x20);
        let mut len = vec![0u8; 31];
        len.push(s.len() as u8);
        out.extend(len);
        let mut data = s.as_bytes().to_vec();
        data.resize(32, 0);
        out.extend(data);
        format!("0x{}", hex::encode(out))
    }

    fn approve_data(spender: Address, amount: U256) -> Bytes {
        let mut d = hex::decode("095ea7b3").unwrap();
        d.extend_from_slice(&[0u8; 12]);
        d.extend_from_slice(spender.as_slice());
        d.extend_from_slice(&amount.to_be_bytes::<32>());
        d.into()
    }

    fn chain() -> FakeUpstream {
        let up = FakeUpstream::new();
        let usdc = USDC.to_string().to_lowercase();
        up.on_fn("eth_getCode", move |req| {
            let addr = req["params"][0].as_str().unwrap().to_lowercase();
            let code = if addr == usdc { "0x6080" } else { "0x" };
            Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":code}))
        });
        let symbol = abi_string("USDC");
        up.on_fn("eth_call", move |req| {
            let data = req["params"][0]["data"].as_str().unwrap();
            let result = if data == "0x95d89b41" { symbol.clone() } else { format!("0x{:064x}", 6) };
            Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":result}))
        });
        up.on("eth_getTransactionCount", json!("0x2a"));
        up
    }

    #[tokio::test]
    async fn approval_to_eoa_is_dangerous_and_amount_is_formatted() {
        let up = chain();
        let tx = TxFields {
            from: Some(ME),
            to: Some(USDC),
            input: approve_data(EOA, U256::from(1_000_000_000u64)),
            nonce: Some(42),
            ..Default::default()
        };
        let a = analyze(&tx, &[], 1);
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &a).await;
        assert!(e.risks.iter().any(|r| r.code == "grantee_is_eoa"));
        assert_eq!(e.token.as_ref().unwrap().symbol.as_deref(), Some("USDC"));
        assert_eq!(e.formatted_args, vec![("amount".to_string(), "1,000 USDC".to_string())]);
        assert_eq!(e.account_nonce, Some(42));
        assert!(e.risks.iter().all(|r| !r.code.starts_with("nonce")));
    }

    #[tokio::test]
    async fn nonce_ahead_of_account_is_a_gap() {
        let up = chain();
        let tx = TxFields { from: Some(ME), to: Some(EOA), nonce: Some(44), ..Default::default() };
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &analyze(&tx, &[], 1)).await;
        assert!(e.risks.iter().any(|r| r.code == "nonce_gap"));
    }

    #[tokio::test]
    async fn calldata_to_an_account_without_code_is_flagged() {
        let up = chain();
        let tx = TxFields { to: Some(EOA), input: Bytes::from(vec![1, 2, 3, 4]), ..Default::default() };
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &analyze(&tx, &[], 1)).await;
        assert!(e.risks.iter().any(|r| r.code == "call_to_eoa"));
    }

    #[tokio::test]
    async fn safe_exec_transaction_is_checked_against_the_chain() {
        use crate::safe::fixtures;
        let up = chain(); // USDC is not a Safe: the check says so
        let input = fixtures::exec_calldata(&fixtures::tx(), &fixtures::signatures());
        let tx = TxFields { from: Some(ME), to: Some(USDC), input, ..Default::default() };
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &analyze(&tx, &[], 1)).await;
        assert!(e.safe.is_some());
        assert!(e.risks.iter().any(|r| r.code == "safe_unreadable"), "{:?}", e.risks);
    }

    fn approve_hash_tx(hash: alloy_primitives::B256) -> TxFields {
        let mut d = hex::decode("d4d9bdcd").unwrap();
        d.extend_from_slice(hash.as_slice());
        TxFields { from: Some(ME), to: Some(crate::safe::fixtures::SAFE), input: d.into(), ..Default::default() }
    }

    #[tokio::test]
    async fn approve_hash_shows_the_transaction_it_approves() {
        use crate::safe::fixtures;
        let up = chain();
        up.on_get("https://api.safe.global/tx-service/eth/api/v2/multisig-transactions/", crate::safe_service::tests::service_json());
        let tx = approve_hash_tx(fixtures::SAFE_TX_HASH);
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &analyze(&tx, &[], 1)).await;
        let report = e.safe_approval.expect("looked up");
        assert_eq!(report.analysis.calls.len(), 2);
        assert!(e.risks.iter().all(|r| r.code != "safe_approve_hash_unknown"), "{:?}", e.risks);
    }

    #[tokio::test]
    async fn approve_hash_of_an_unknown_transaction_is_flagged() {
        let up = chain();
        let tx = approve_hash_tx(alloy_primitives::B256::repeat_byte(1));
        let e = enrich(&up, "http://up", 1, &SafeConfig::default(), &tx, &analyze(&tx, &[], 1)).await;
        assert!(e.safe_approval.is_none());
        assert!(e.risks.iter().any(|r| r.code == "safe_approve_hash_unknown"), "{:?}", e.risks);
    }

    #[test]
    fn displays_unlimited_and_grouped_amounts() {
        let t = TokenInfo { address: String::new(), symbol: Some("DAI".into()), decimals: Some(18) };
        assert_eq!(display_amount(&U256::MAX.to_string(), &t), "Unlimited DAI");
        assert_eq!(display_amount("1234500000000000000000", &t), "1,234.5 DAI");
    }

    #[test]
    fn hostile_string_length_does_not_overflow() {
        // A token whose symbol() claims a length of 2^256 - 1.
        let mut ret = vec![0u8; 64];
        ret[31] = 0x20;
        ret[32..64].fill(0xff);
        assert_eq!(decode_string(&ret), None);
    }
}
