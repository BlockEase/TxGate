//! Transaction decoding and static risk analysis.
//!
//! Everything here is pure: given transaction bytes or fields it returns a
//! description and a list of risk flags. Facts that need chain state (does an
//! address have code, what are a token's decimals) are added later by
//! [`crate::enrich`].

use alloy_consensus::{transaction::SignerRecoverable, EthereumTxEnvelope, Transaction, TxEip4844};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fields of a transaction that matter for analysis, regardless of whether
/// it came from a signed raw transaction, `eth_estimateGas` or `eth_sendTransaction`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxFields {
    pub from: Option<Address>,
    pub to: Option<Address>,
    pub value: U256,
    pub input: Bytes,
    pub nonce: Option<u64>,
    pub gas_limit: Option<u64>,
    /// Chain id embedded in the transaction itself (signed transactions only).
    pub chain_id: Option<u64>,
}

impl TxFields {
    /// Parses a JSON-RPC transaction request object (as used by `eth_estimateGas`,
    /// `eth_call` and `eth_sendTransaction`). Unknown or malformed fields are ignored.
    pub fn from_request(obj: &Value) -> Self {
        let addr = |k: &str| obj.get(k).and_then(Value::as_str).and_then(|s| s.parse::<Address>().ok());
        let quantity = |k: &str| obj.get(k).and_then(Value::as_str).and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok());
        let input = obj
            .get("input")
            .or_else(|| obj.get("data"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<Bytes>().ok())
            .unwrap_or_default();
        Self {
            from: addr("from"),
            to: addr("to"),
            value: quantity("value").unwrap_or_default(),
            input,
            nonce: quantity("nonce").map(|n| n.saturating_to()),
            gas_limit: quantity("gas").map(|n| n.saturating_to()),
            chain_id: quantity("chainId").map(|n| n.saturating_to()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    pub chain_id: U256,
    /// Contract the EOA delegates its code to.
    pub address: Address,
    pub nonce: u64,
    /// Account that signed the authorization, when recoverable.
    pub authority: Option<Address>,
}

/// A fully decoded, signed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTx {
    pub hash: B256,
    pub tx_type: u8,
    pub tx_type_name: String,
    pub fields: TxFields,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: Option<u128>,
    pub authorizations: Vec<Authorization>,
    pub blob_count: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("not a valid signed transaction: {0}")]
    Invalid(String),
    #[error("cannot recover signer: {0}")]
    Signature(String),
}

/// Transaction hash of raw bytes, computed the same way nodes do (keccak of the
/// EIP-2718 envelope, excluding any blob sidecar).
pub fn decode_signed(raw: &[u8]) -> Result<SignedTx, DecodeError> {
    let envelope = decode_envelope(raw)?;
    let from = envelope.recover_signer().map_err(|e| DecodeError::Signature(e.to_string()))?;
    let tx_type = envelope.tx_type() as u8;
    let authorizations = envelope
        .authorization_list()
        .map(|list| {
            list.iter()
                .map(|a| Authorization {
                    chain_id: a.chain_id,
                    address: a.address,
                    nonce: a.nonce,
                    authority: a.recover_authority().ok(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SignedTx {
        hash: *envelope.tx_hash(),
        tx_type,
        tx_type_name: tx_type_name(tx_type).to_string(),
        fields: TxFields {
            from: Some(from),
            to: envelope.to(),
            value: envelope.value(),
            input: envelope.input().clone(),
            nonce: Some(envelope.nonce()),
            gas_limit: Some(envelope.gas_limit()),
            chain_id: envelope.chain_id(),
        },
        max_fee_per_gas: envelope.max_fee_per_gas(),
        max_priority_fee_per_gas: envelope.max_priority_fee_per_gas(),
        authorizations,
        blob_count: envelope.blob_versioned_hashes().map_or(0, |h| h.len()),
    })
}

/// Envelope without blob sidecar; the sidecar is irrelevant for analysis and hashing.
type Envelope = EthereumTxEnvelope<TxEip4844>;

fn decode_envelope(raw: &[u8]) -> Result<Envelope, DecodeError> {
    match Envelope::decode_2718(&mut &raw[..]) {
        Ok(env) => Ok(env),
        Err(first) => {
            // Blob transactions are submitted in network form (with sidecar).
            use alloy_consensus::transaction::PooledTransaction;
            PooledTransaction::decode_2718(&mut &raw[..])
                .map(Into::into)
                .map_err(|_| DecodeError::Invalid(first.to_string()))
        }
    }
}

pub fn tx_type_name(tx_type: u8) -> &'static str {
    match tx_type {
        0 => "Legacy",
        1 => "EIP-2930",
        2 => "EIP-1559",
        3 => "EIP-4844 (blob)",
        4 => "EIP-7702 (set code)",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Calldata decoding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Address,
    Uint,
    Bool,
}

/// Functions TxGate recognises without any network lookup. Only static
/// parameters are decoded; functions with dynamic arguments are named only.
struct KnownFn {
    signature: &'static str,
    params: &'static [(&'static str, Kind)],
}

const KNOWN_FNS: &[KnownFn] = &[
    KnownFn { signature: "approve(address,uint256)", params: &[("spender", Kind::Address), ("amount", Kind::Uint)] },
    KnownFn { signature: "increaseAllowance(address,uint256)", params: &[("spender", Kind::Address), ("addedValue", Kind::Uint)] },
    KnownFn { signature: "transfer(address,uint256)", params: &[("to", Kind::Address), ("amount", Kind::Uint)] },
    KnownFn { signature: "transferFrom(address,address,uint256)", params: &[("from", Kind::Address), ("to", Kind::Address), ("amount", Kind::Uint)] },
    KnownFn { signature: "setApprovalForAll(address,bool)", params: &[("operator", Kind::Address), ("approved", Kind::Bool)] },
    KnownFn { signature: "safeTransferFrom(address,address,uint256)", params: &[("from", Kind::Address), ("to", Kind::Address), ("tokenId", Kind::Uint)] },
    KnownFn { signature: "safeTransferFrom(address,address,uint256,bytes)", params: &[("from", Kind::Address), ("to", Kind::Address), ("tokenId", Kind::Uint)] },
    KnownFn {
        signature: "permit(address,address,uint256,uint256,uint8,bytes32,bytes32)",
        params: &[("owner", Kind::Address), ("spender", Kind::Address), ("value", Kind::Uint), ("deadline", Kind::Uint)],
    },
    KnownFn {
        signature: "permit(address,address,uint256,uint256,bool,uint8,bytes32,bytes32)",
        params: &[("holder", Kind::Address), ("spender", Kind::Address), ("nonce", Kind::Uint), ("expiry", Kind::Uint), ("allowed", Kind::Bool)],
    },
    // Permit2
    KnownFn {
        signature: "approve(address,address,uint160,uint48)",
        params: &[("token", Kind::Address), ("spender", Kind::Address), ("amount", Kind::Uint), ("expiration", Kind::Uint)],
    },
    KnownFn { signature: "permit(address,((address,uint160,uint48,uint48),address,uint256),bytes)", params: &[("owner", Kind::Address)] },
    KnownFn { signature: "permit(address,((address,uint160,uint48,uint48)[],address,uint256),bytes)", params: &[("owner", Kind::Address)] },
    // WETH
    KnownFn { signature: "deposit()", params: &[] },
    KnownFn { signature: "withdraw(uint256)", params: &[("amount", Kind::Uint)] },
    // Common routers / batching (named only)
    KnownFn { signature: "multicall(bytes[])", params: &[] },
    KnownFn { signature: "multicall(uint256,bytes[])", params: &[("deadline", Kind::Uint)] },
    KnownFn { signature: "execute(bytes,bytes[],uint256)", params: &[] },
    KnownFn { signature: "execute(bytes,bytes[])", params: &[] },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arg {
    pub name: String,
    /// Raw decoded value: checksummed address, decimal integer, or `true`/`false`.
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallInfo {
    /// `0x`-prefixed 4-byte selector, absent for plain transfers and deployments.
    pub selector: Option<String>,
    /// Full signature when recognised, e.g. `approve(address,uint256)`.
    pub signature: Option<String>,
    /// Function name when recognised.
    pub function: Option<String>,
    pub args: Vec<Arg>,
}

impl CallInfo {
    pub fn arg(&self, name: &str) -> Option<&str> {
        self.args.iter().find(|a| a.name == name).map(|a| a.value.as_str())
    }
}

fn selector_of(signature: &str) -> [u8; 4] {
    let h = keccak256(signature.as_bytes());
    [h[0], h[1], h[2], h[3]]
}

pub fn decode_call(input: &[u8]) -> CallInfo {
    if input.len() < 4 {
        return CallInfo::default();
    }
    let selector: [u8; 4] = input[..4].try_into().expect("length checked");
    let mut info = CallInfo { selector: Some(format!("0x{}", hex::encode(selector))), ..Default::default() };
    let Some(known) = KNOWN_FNS.iter().find(|f| selector_of(f.signature) == selector) else {
        return info;
    };
    info.signature = Some(known.signature.to_string());
    info.function = Some(known.signature.split('(').next().unwrap_or_default().to_string());
    let body = &input[4..];
    for (i, (name, kind)) in known.params.iter().enumerate() {
        let Some(word) = body.get(i * 32..(i + 1) * 32) else { break };
        let value = match kind {
            Kind::Address => Address::from_slice(&word[12..]).to_checksum(None),
            Kind::Uint => U256::from_be_slice(word).to_string(),
            Kind::Bool => (word[31] != 0).to_string(),
        };
        info.args.push(Arg { name: name.to_string(), value });
    }
    info
}

// ---------------------------------------------------------------------------
// Risk analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub severity: Severity,
    /// Stable machine-readable identifier.
    pub code: String,
    pub message: String,
}

impl Risk {
    fn new(severity: Severity, code: &str, message: impl Into<String>) -> Self {
        Self { severity, code: code.into(), message: message.into() }
    }
}

/// Static analysis of one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    pub call: CallInfo,
    pub risks: Vec<Risk>,
    /// One-line human summary for list views.
    pub summary: String,
}

/// Amounts at or above 2^159 are treated as "unlimited": this covers
/// `type(uint256).max`, `2**255` and Permit2's `type(uint160).max`, and no real
/// token balance comes close. The UI uses the same threshold.
pub fn is_unlimited_amount(v: U256) -> bool {
    v >= (U256::from(1) << 159)
}

fn is_unlimited(amount: &str) -> bool {
    amount.parse::<U256>().is_ok_and(is_unlimited_amount)
}

/// Risks that only apply to signed transactions.
pub fn signed_tx_risks(tx: &SignedTx) -> Vec<Risk> {
    let mut risks = Vec::new();
    if tx.fields.chain_id.is_none() {
        risks.push(Risk::new(
            Severity::Warning,
            "no_replay_protection",
            "Signed without a chain id (pre-EIP-155): anyone can replay this transaction on every EVM chain where the nonce matches",
        ));
    }
    risks
}

pub fn short(addr: &Address) -> String {
    let s = addr.to_checksum(None);
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

fn short_str(addr: &str) -> String {
    addr.parse::<Address>().map(|a| short(&a)).unwrap_or_else(|_| addr.to_string())
}

/// Analyses a transaction.
///
/// `route_chain_id` is the chain the wallet addressed (from the URL path);
/// `authorizations` are EIP-7702 delegations carried by the transaction.
pub fn analyze(tx: &TxFields, authorizations: &[Authorization], route_chain_id: u64) -> Analysis {
    let call = decode_call(&tx.input);
    let mut risks = Vec::new();

    if let Some(cid) = tx.chain_id
        && cid != route_chain_id {
            risks.push(Risk::new(
                Severity::Danger,
                "chain_mismatch",
                format!("Transaction is signed for chain {cid}, but was sent to the chain {route_chain_id} endpoint"),
            ));
        }

    for auth in authorizations {
        let chain = if auth.chain_id.is_zero() { "any chain".to_string() } else { format!("chain {}", auth.chain_id) };
        risks.push(Risk::new(
            Severity::Danger,
            "eip7702_delegation",
            format!(
                "EIP-7702: delegates account {} to contract {} on {chain}. That contract will fully control the account",
                auth.authority.as_ref().map(short).unwrap_or_else(|| "(unknown)".into()),
                auth.address.to_checksum(None)
            ),
        ));
    }

    if tx.to.is_none() {
        risks.push(Risk::new(Severity::Info, "contract_creation", "Deploys a new contract"));
    }

    let func = call.function.as_deref().unwrap_or_default();
    let sig = call.signature.as_deref().unwrap_or_default();
    let expected_args = KNOWN_FNS.iter().find(|f| f.signature == sig).map_or(0, |f| f.params.len());
    if call.args.len() < expected_args {
        risks.push(Risk::new(
            Severity::Warning,
            "malformed_calldata",
            format!("The call data is too short for {sig}; the arguments shown are incomplete"),
        ));
    }
    let is_permit2_approve = sig == "approve(address,address,uint160,uint48)";
    match func {
        "approve" | "increaseAllowance" => {
            let amount_key = if func == "approve" { "amount" } else { "addedValue" };
            let spender = call.arg("spender").map(short_str).unwrap_or_default();
            let amount = call.arg(amount_key);
            if is_permit2_approve {
                let token = call.arg("token").map(short_str).unwrap_or_default();
                risks.push(Risk::new(
                    Severity::Warning,
                    "permit2_approval",
                    format!("Permit2 allowance for token {token} to {spender}"),
                ));
                if amount.is_some_and(is_unlimited) {
                    risks.push(Risk::new(Severity::Danger, "unlimited_approval", format!("Unlimited Permit2 allowance to {spender}")));
                }
            } else if func == "approve" && amount == Some("0") {
                risks.push(Risk::new(Severity::Info, "revoke_approval", format!("Revokes the allowance of {spender}")));
            } else if amount.is_some_and(is_unlimited) {
                risks.push(Risk::new(
                    Severity::Danger,
                    "unlimited_approval",
                    format!("Unlimited allowance to {spender}: it can move all of this token at any time"),
                ));
            } else {
                risks.push(Risk::new(Severity::Warning, "approval", format!("Grants a token allowance to {spender}")));
            }
        }
        "setApprovalForAll" => {
            let operator = call.arg("operator").map(short_str).unwrap_or_default();
            if call.arg("approved") == Some("true") {
                risks.push(Risk::new(
                    Severity::Danger,
                    "approval_for_all",
                    format!("Gives {operator} control over ALL tokens of this collection"),
                ));
            } else {
                risks.push(Risk::new(Severity::Info, "revoke_approval", format!("Revokes operator {operator}")));
            }
        }
        "permit" => {
            risks.push(Risk::new(
                Severity::Warning,
                "permit",
                "Submits a signed permit, which grants a token allowance on behalf of the signer",
            ));
        }
        "transfer" | "transferFrom" | "safeTransferFrom" => {
            if let (Some(recipient), Some(from)) = (call.arg("to"), tx.from)
                && recipient.parse::<Address>().ok() != Some(from) {
                    risks.push(Risk::new(
                        Severity::Info,
                        "token_transfer",
                        format!("Sends tokens to {}", short_str(recipient)),
                    ));
                }
        }
        _ => {}
    }

    if !tx.input.is_empty() && call.function.is_none() && tx.to.is_some() {
        risks.push(Risk::new(
            Severity::Info,
            "unknown_function",
            format!("Calls an unrecognised function {}", call.selector.as_deref().unwrap_or("")),
        ));
    }

    let summary = summarize(tx, &call);
    Analysis { call, risks, summary }
}

fn summarize(tx: &TxFields, call: &CallInfo) -> String {
    let target = match tx.to {
        Some(to) => short(&to),
        None => "new contract".into(),
    };
    let eth = if tx.value.is_zero() { String::new() } else { format!(" · {} ETH", format_units(tx.value, 18)) };
    match (&call.function, &call.selector) {
        (Some(f), _) => {
            let args = call.args.iter().map(|a| match (a.value.parse::<Address>(), a.value.parse::<U256>()) {
                (Ok(addr), _) => short(&addr),
                (_, Ok(v)) if is_unlimited_amount(v) => "unlimited".to_string(),
                _ => a.value.clone(),
            });
            format!("{target} · {f}({}){eth}", args.collect::<Vec<_>>().join(", "))
        }
        (None, Some(sel)) => format!("{target} · {sel}{eth}"),
        (None, None) => format!("Transfer to {target}{eth}"),
    }
}

/// Formats an integer amount with `decimals` decimal places, trimming trailing zeros.
pub fn format_units(amount: U256, decimals: u8) -> String {
    let s = amount.to_string();
    let d = decimals as usize;
    let (int, frac) = if s.len() > d {
        (s[..s.len() - d].to_string(), s[s.len() - d..].to_string())
    } else {
        ("0".to_string(), format!("{}{}", "0".repeat(d - s.len()), s))
    };
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() { int } else { format!("{int}.{frac}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, hex};

    fn word_addr(a: Address) -> Vec<u8> {
        let mut w = vec![0u8; 12];
        w.extend_from_slice(a.as_slice());
        w
    }

    fn word_u256(v: U256) -> Vec<u8> {
        v.to_be_bytes::<32>().to_vec()
    }

    fn calldata(sig: &str, words: &[Vec<u8>]) -> Bytes {
        let mut d = selector_of(sig).to_vec();
        for w in words {
            d.extend_from_slice(w);
        }
        d.into()
    }

    const ME: Address = address!("3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE");
    const SPENDER: Address = address!("9e1F000000000000000000000000000000003c07");
    const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    #[test]
    fn known_selectors_match_the_standard_values() {
        assert_eq!(hex::encode(selector_of("approve(address,uint256)")), "095ea7b3");
        assert_eq!(hex::encode(selector_of("setApprovalForAll(address,bool)")), "a22cb465");
        assert_eq!(hex::encode(selector_of("transfer(address,uint256)")), "a9059cbb");
    }

    #[test]
    fn decodes_approve_arguments() {
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER), word_u256(U256::from(1000))]);
        let info = decode_call(&data);
        assert_eq!(info.function.as_deref(), Some("approve"));
        assert_eq!(info.arg("spender"), Some(SPENDER.to_checksum(None).as_str()));
        assert_eq!(info.arg("amount"), Some("1000"));
    }

    #[test]
    fn unlimited_approve_is_dangerous() {
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER), word_u256(U256::MAX)]);
        let tx = TxFields { from: Some(ME), to: Some(USDC), input: data, ..Default::default() };
        let a = analyze(&tx, &[], 1);
        assert!(a.risks.iter().any(|r| r.code == "unlimited_approval" && r.severity == Severity::Danger));
    }

    #[test]
    fn set_approval_for_all_true_is_dangerous_and_false_is_a_revoke() {
        let on = calldata("setApprovalForAll(address,bool)", &[word_addr(SPENDER), word_u256(U256::from(1))]);
        let off = calldata("setApprovalForAll(address,bool)", &[word_addr(SPENDER), word_u256(U256::ZERO)]);
        let codes = |input: Bytes| {
            analyze(&TxFields { to: Some(USDC), input, ..Default::default() }, &[], 1)
                .risks
                .into_iter()
                .map(|r| r.code)
                .collect::<Vec<_>>()
        };
        assert!(codes(on).contains(&"approval_for_all".to_string()));
        assert!(codes(off).contains(&"revoke_approval".to_string()));
    }

    #[test]
    fn chain_mismatch_is_flagged() {
        let tx = TxFields { to: Some(USDC), chain_id: Some(8453), ..Default::default() };
        assert!(analyze(&tx, &[], 1).risks.iter().any(|r| r.code == "chain_mismatch"));
    }

    #[test]
    fn eip7702_authorization_is_flagged() {
        let auth = Authorization { chain_id: U256::ZERO, address: SPENDER, nonce: 0, authority: Some(ME) };
        let a = analyze(&TxFields { to: Some(ME), ..Default::default() }, &[auth], 1);
        assert!(a.risks.iter().any(|r| r.code == "eip7702_delegation" && r.message.contains("any chain")));
    }

    #[test]
    fn unknown_selector_is_reported() {
        let tx = TxFields { to: Some(USDC), input: Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]), ..Default::default() };
        let a = analyze(&tx, &[], 1);
        assert_eq!(a.call.selector.as_deref(), Some("0xdeadbeef"));
        assert!(a.risks.iter().any(|r| r.code == "unknown_function"));
    }

    #[test]
    fn parses_request_objects_with_data_or_input() {
        let v = serde_json::json!({"from": ME.to_string(), "to": USDC.to_string(), "data": "0x095ea7b3", "value": "0x10"});
        let f = TxFields::from_request(&v);
        assert_eq!(f.from, Some(ME));
        assert_eq!(f.value, U256::from(16));
        assert_eq!(f.input.as_ref(), &[0x09, 0x5e, 0xa7, 0xb3]);
    }

    #[test]
    fn summary_calls_huge_amounts_unlimited() {
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER), word_u256(U256::MAX)]);
        let a = analyze(&TxFields { to: Some(USDC), input: data, ..Default::default() }, &[], 1);
        assert!(a.summary.ends_with(", unlimited)"), "{}", a.summary);
    }

    #[test]
    fn formats_units() {
        assert_eq!(format_units(U256::from(1_500_000u64), 6), "1.5");
        assert_eq!(format_units(U256::from(5u64), 3), "0.005");
        assert_eq!(format_units(U256::from(7u64), 0), "7");
    }

    #[test]
    fn decodes_a_signed_eip1559_transaction() {
        // A mainnet EIP-1559 transaction (from the alloy test suite).
        let raw = hex!("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8");
        let tx = decode_signed(&raw).unwrap();
        assert_eq!(tx.tx_type, 2);
        assert_eq!(tx.fields.chain_id, Some(1));
        assert_eq!(tx.fields.to, Some(address!("d9e1459a7a482635700cbc20bbaf52d495ab9c96")));
        assert_eq!(tx.hash, keccak256(raw));
        assert!(tx.fields.from.is_some());
    }

    fn risk_codes(a: &Analysis) -> Vec<&str> {
        a.risks.iter().map(|r| r.code.as_str()).collect()
    }

    #[test]
    fn effectively_unlimited_approval_is_dangerous() {
        // 2^200 is "unlimited" in every practical sense and in the summary.
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER), word_u256(U256::from(1) << 200)]);
        let a = analyze(&TxFields { from: Some(ME), to: Some(USDC), input: data, ..Default::default() }, &[], 1);
        assert!(risk_codes(&a).contains(&"unlimited_approval"), "{:?}", a.risks);
    }

    #[test]
    fn increase_allowance_by_zero_is_not_a_revoke() {
        let data = calldata("increaseAllowance(address,uint256)", &[word_addr(SPENDER), word_u256(U256::ZERO)]);
        let a = analyze(&TxFields { to: Some(USDC), input: data, ..Default::default() }, &[], 1);
        assert!(!risk_codes(&a).contains(&"revoke_approval"), "{:?}", a.risks);
    }

    #[test]
    fn truncated_calldata_is_flagged_not_called_a_revoke() {
        // approve(spender) with the amount word missing.
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER)]);
        let a = analyze(&TxFields { to: Some(USDC), input: data, ..Default::default() }, &[], 1);
        let codes = risk_codes(&a);
        assert!(!codes.contains(&"revoke_approval"), "{codes:?}");
        assert!(codes.contains(&"malformed_calldata"), "{codes:?}");
    }

    #[test]
    fn signature_without_replay_protection_is_flagged() {
        // Pre-EIP-155 deployment transaction of the deterministic deployment proxy (v = 27).
        let raw = hex!("f8a58085174876e800830186a08080b853604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf31ba02222222222222222222222222222222222222222222222222222222222222222a02222222222222222222222222222222222222222222222222222222222222222");
        let tx = decode_signed(&raw).unwrap();
        assert_eq!(tx.fields.chain_id, None);
        let codes: Vec<String> = signed_tx_risks(&tx).into_iter().map(|r| r.code).collect();
        assert!(codes.contains(&"no_replay_protection".to_string()), "{codes:?}");
    }

    #[test]
    fn garbage_is_not_a_transaction() {
        assert!(matches!(decode_signed(&[0x02, 0x01]), Err(DecodeError::Invalid(_))));
    }
}
