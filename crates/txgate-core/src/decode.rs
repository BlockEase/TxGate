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

use crate::safe::{analyze_exec, SafeExec};
use crate::text::Text;

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

impl Authorization {
    /// The `authorizationList` of a JSON-RPC transaction request. Entries whose
    /// signature cannot be read are kept with an unknown authority, so the
    /// delegation is still shown.
    pub fn list_from_request(obj: &Value) -> Vec<Self> {
        let Some(list) = obj.get("authorizationList").and_then(Value::as_array) else { return Vec::new() };
        list.iter()
            .filter_map(|v| {
                if let Ok(signed) = serde_json::from_value::<alloy_eips::eip7702::SignedAuthorization>(v.clone()) {
                    return Some(Self {
                        chain_id: signed.chain_id,
                        address: signed.address,
                        nonce: signed.nonce,
                        authority: signed.recover_authority().ok(),
                    });
                }
                let quantity = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok());
                Some(Self {
                    chain_id: quantity("chainId").unwrap_or_default(),
                    address: v.get("address")?.as_str()?.parse().ok()?,
                    nonce: quantity("nonce").map(|n| n.saturating_to()).unwrap_or_default(),
                    authority: None,
                })
            })
            .collect()
    }
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
    Bytes32,
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
    // Safe multisig. execTransaction's later arguments are dynamic; see crate::safe.
    KnownFn {
        signature: "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
        params: &[("to", Kind::Address), ("value", Kind::Uint)],
    },
    KnownFn { signature: "approveHash(bytes32)", params: &[("hashToApprove", Kind::Bytes32)] },
    KnownFn { signature: "multiSend(bytes)", params: &[] },
    KnownFn { signature: "addOwnerWithThreshold(address,uint256)", params: &[("owner", Kind::Address), ("threshold", Kind::Uint)] },
    KnownFn {
        signature: "removeOwner(address,address,uint256)",
        params: &[("prevOwner", Kind::Address), ("owner", Kind::Address), ("threshold", Kind::Uint)],
    },
    KnownFn {
        signature: "swapOwner(address,address,address)",
        params: &[("prevOwner", Kind::Address), ("oldOwner", Kind::Address), ("newOwner", Kind::Address)],
    },
    KnownFn { signature: "changeThreshold(uint256)", params: &[("threshold", Kind::Uint)] },
    KnownFn { signature: "enableModule(address)", params: &[("module", Kind::Address)] },
    KnownFn { signature: "disableModule(address,address)", params: &[("prevModule", Kind::Address), ("module", Kind::Address)] },
    KnownFn { signature: "setGuard(address)", params: &[("guard", Kind::Address)] },
    KnownFn { signature: "setModuleGuard(address)", params: &[("moduleGuard", Kind::Address)] },
    KnownFn { signature: "setFallbackHandler(address)", params: &[("handler", Kind::Address)] },
    KnownFn { signature: "changeMasterCopy(address)", params: &[("masterCopy", Kind::Address)] },
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
            Kind::Bytes32 => format!("0x{}", hex::encode(word)),
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
    /// English text, also kept for records written before `text` existed.
    pub message: String,
    /// The same message for translation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
}

impl Risk {
    /// A risk whose message is the translation `risk.<key>` with `params`.
    pub(crate) fn t<K: Into<String>, V: Into<String>>(severity: Severity, code: &str, key: &str, params: impl IntoIterator<Item = (K, V)>) -> Self {
        let text = Text::new(&format!("risk.{key}"), params);
        Self { severity, code: code.into(), message: text.english(), text: Some(text) }
    }

    /// Puts `prefix` (e.g. "Call 2: ") in front of the message.
    pub(crate) fn prefixed(mut self, prefix: Text) -> Self {
        self.message = format!("{}{}", prefix.english(), self.message);
        if let Some(t) = &mut self.text {
            t.prefix.insert(0, prefix);
        }
        self
    }
}

/// Static analysis of one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    pub call: CallInfo,
    pub risks: Vec<Risk>,
    /// One-line human summary for list views.
    pub summary: String,
    /// The decoded Safe transaction when this calls `execTransaction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe: Option<Box<SafeExec>>,
    /// The calls a smart account is asked to make (ERC-4337 / ERC-7579 `execute`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Box<crate::safe::SafeTxAnalysis>>,
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
        risks.push(Risk::t::<&str, &str>(Severity::Warning, "no_replay_protection", "no_replay_protection", []));
    }
    risks
}

pub fn short(addr: &Address) -> String {
    let s = addr.to_checksum(None);
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

/// An address for a text parameter: checksummed and in full, so the UI can
/// link it. Rendered text shortens it.
pub fn full(addr: &Address) -> String {
    addr.to_checksum(None)
}

fn full_str(addr: &str) -> String {
    addr.parse::<Address>().map(|a| full(&a)).unwrap_or_else(|_| addr.to_string())
}

/// Analyses a transaction.
///
/// `route_chain_id` is the chain the wallet addressed (from the URL path);
/// `authorizations` are EIP-7702 delegations carried by the transaction.
/// How many calls deep `analyze` unwraps Safe and smart-account calls. Real
/// transactions nest two or three levels; a crafted one could nest thousands
/// and overflow the stack.
const MAX_NESTING: u32 = 8;

thread_local! {
    static NESTING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Counts one level of `analyze` on this thread while alive.
struct NestingGuard;

impl NestingGuard {
    fn enter() -> (Self, u32) {
        let depth = NESTING.with(|n| {
            let d = n.get();
            n.set(d + 1);
            d
        });
        (Self, depth)
    }
}

impl Drop for NestingGuard {
    fn drop(&mut self) {
        NESTING.with(|n| n.set(n.get() - 1));
    }
}

pub fn analyze(tx: &TxFields, authorizations: &[Authorization], route_chain_id: u64) -> Analysis {
    let (_guard, depth) = NestingGuard::enter();
    let expand = depth < MAX_NESTING;
    let call = decode_call(&tx.input);
    let mut risks = Vec::new();

    if let Some(cid) = tx.chain_id
        && cid != route_chain_id {
            risks.push(Risk::t(
                Severity::Danger,
                "chain_mismatch",
                "chain_mismatch",
                [("signed", cid.to_string()), ("route", route_chain_id.to_string())],
            ));
        }

    for auth in authorizations {
        let any = if auth.chain_id.is_zero() { "_any_chain" } else { "" };
        let account = auth.authority.as_ref().map(full).unwrap_or_else(|| "?".into());
        let chain = auth.chain_id.to_string();
        if auth.address.is_zero() {
            risks.push(Risk::t(Severity::Info, "eip7702_revocation", &format!("eip7702_revocation{any}"), [("account", account), ("chain", chain)]));
            continue;
        }
        risks.push(Risk::t(
            Severity::Danger,
            "eip7702_delegation",
            &format!("eip7702_delegation{any}"),
            [("account", account), ("delegate", auth.address.to_checksum(None)), ("chain", chain)],
        ));
    }

    if tx.to.is_none() {
        risks.push(Risk::t::<&str, &str>(Severity::Info, "contract_creation", "contract_creation", []));
    }

    let func = call.function.as_deref().unwrap_or_default();
    let sig = call.signature.as_deref().unwrap_or_default();
    let expected_args = KNOWN_FNS.iter().find(|f| f.signature == sig).map_or(0, |f| f.params.len());
    if call.args.len() < expected_args {
        risks.push(Risk::t(Severity::Warning, "malformed_calldata", "malformed_calldata", [("signature", sig)]));
    }
    let is_permit2_approve = sig == "approve(address,address,uint160,uint48)";
    match func {
        "approve" | "increaseAllowance" => {
            let amount_key = if func == "approve" { "amount" } else { "addedValue" };
            let spender = call.arg("spender").map(full_str).unwrap_or_default();
            let amount = call.arg(amount_key);
            if is_permit2_approve {
                let token = call.arg("token").map(full_str).unwrap_or_default();
                risks.push(Risk::t(Severity::Warning, "permit2_approval", "permit2_approval", [("token", token), ("spender", spender.clone())]));
                if amount.is_some_and(is_unlimited) {
                    risks.push(Risk::t(Severity::Danger, "unlimited_approval", "unlimited_permit2", [("spender", spender)]));
                }
            } else if func == "approve" && amount == Some("0") {
                risks.push(Risk::t(Severity::Info, "revoke_approval", "revoke_allowance", [("spender", spender)]));
            } else if amount.is_some_and(is_unlimited) {
                risks.push(Risk::t(Severity::Danger, "unlimited_approval", "unlimited_approval", [("spender", spender)]));
            } else {
                risks.push(Risk::t(Severity::Warning, "approval", "approval", [("spender", spender)]));
            }
        }
        "setApprovalForAll" => {
            let operator = call.arg("operator").map(full_str).unwrap_or_default();
            if call.arg("approved") == Some("true") {
                risks.push(Risk::t(Severity::Danger, "approval_for_all", "approval_for_all", [("operator", operator)]));
            } else {
                risks.push(Risk::t(Severity::Info, "revoke_approval", "revoke_operator", [("operator", operator)]));
            }
        }
        "permit" => {
            risks.push(Risk::t::<&str, &str>(Severity::Warning, "permit", "permit", []));
        }
        "transfer" | "transferFrom" | "safeTransferFrom" => {
            if let (Some(recipient), Some(from)) = (call.arg("to"), tx.from)
                && recipient.parse::<Address>().ok() != Some(from) {
                    risks.push(Risk::t(Severity::Info, "token_transfer", "token_transfer", [("recipient", full_str(recipient))]));
                }
        }
        "approveHash" => {
            risks.push(Risk::t(Severity::Warning, "safe_approve_hash", "safe_approve_hash", [("hash", call.arg("hashToApprove").unwrap_or_default())]));
        }
        _ => {}
    }

    let safe = match tx.to {
        Some(to) if func == "execTransaction" && expand => analyze_exec(route_chain_id, to, &tx.input).map(Box::new),
        _ => None,
    };
    if let Some(exec) = &safe {
        risks.extend(exec.analysis.risks.iter().cloned());
    } else if func == "execTransaction" && expand {
        risks.push(Risk::t::<&str, &str>(Severity::Danger, "safe_malformed_exec", "safe_malformed_exec", []));
    }

    let account = match tx.to {
        Some(to) if call.function.is_none() && expand => crate::account::analyze_account_call(route_chain_id, to, &tx.input).map(Box::new),
        _ => None,
    };
    if !expand && (func == "execTransaction" || call.function.is_none()) && !tx.input.is_empty() {
        risks.push(Risk::t(Severity::Danger, "nesting_too_deep", "nesting_too_deep", [("depth", MAX_NESTING.to_string())]));
    }
    if let Some(a) = &account {
        risks.extend(a.risks.iter().cloned());
    } else if !tx.input.is_empty() && call.function.is_none() && tx.to.is_some() {
        risks.push(Risk::t(Severity::Info, "unknown_function", "unknown_function", [("selector", call.selector.clone().unwrap_or_default())]));
    }

    let summary = match (&safe, &account) {
        (Some(exec), _) => exec.analysis.summary.clone(),
        (None, Some(a)) => a.summary.clone(),
        (None, None) => summarize(tx, &call, route_chain_id),
    };
    Analysis { call, risks, summary, safe, account }
}

fn summarize(tx: &TxFields, call: &CallInfo, chain_id: u64) -> String {
    let target = match tx.to {
        Some(to) => short(&to),
        None => "new contract".into(),
    };
    let native = crate::providers::native_symbol(chain_id);
    let eth = if tx.value.is_zero() { String::new() } else { format!(" · {} {native}", format_units(tx.value, 18)) };
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
    fn risks_carry_a_translation_key_and_parameters() {
        let data = calldata("approve(address,uint256)", &[word_addr(SPENDER), word_u256(U256::MAX)]);
        let tx = TxFields { from: Some(ME), to: Some(USDC), input: data, ..Default::default() };
        let risk = analyze(&tx, &[], 1).risks.into_iter().find(|r| r.code == "unlimited_approval").unwrap();
        let text = risk.text.expect("translatable");
        assert_eq!(text.key, "risk.unlimited_approval");
        // The full address, so the UI can link it.
        assert_eq!(text.params.get("spender").map(String::as_str), Some(SPENDER.to_checksum(None).as_str()));
        assert_eq!(risk.message, format!("Unlimited allowance to {}: it can move all of this token at any time", SPENDER.to_checksum(None)));
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
    fn value_is_shown_in_the_chain_native_currency() {
        let tx = TxFields { to: Some(SPENDER), value: U256::from(15) * U256::from(10).pow(U256::from(17)), ..Default::default() };
        assert!(analyze(&tx, &[], 56).summary.ends_with("· 1.5 BNB"), "{}", analyze(&tx, &[], 56).summary);
        assert!(analyze(&tx, &[], 137).summary.ends_with("· 1.5 POL"));
        assert!(analyze(&tx, &[], 8453).summary.ends_with("· 1.5 ETH"), "L2s pay in ETH");
        assert!(analyze(&tx, &[], 999_999).summary.ends_with("· 1.5 ETH"), "unknown chains default to ETH");
    }

    #[test]
    fn chain_mismatch_is_flagged() {
        let tx = TxFields { to: Some(USDC), chain_id: Some(8453), ..Default::default() };
        assert!(analyze(&tx, &[], 1).risks.iter().any(|r| r.code == "chain_mismatch"));
    }

    #[test]
    fn eip7702_revocation_is_not_a_delegation() {
        // Delegating to the zero address clears the delegation (EIP-7702).
        let auth = Authorization { chain_id: U256::from(1), address: Address::ZERO, nonce: 0, authority: Some(ME) };
        let a = analyze(&TxFields { to: Some(ME), ..Default::default() }, &[auth], 1);
        let codes: Vec<_> = a.risks.iter().map(|r| (r.code.as_str(), r.severity)).collect();
        assert_eq!(codes, vec![("eip7702_revocation", Severity::Info)], "{:?}", a.risks);
    }

    #[test]
    fn unsigned_authorization_entries_are_still_shown() {
        let req = serde_json::json!({"authorizationList": [{"chainId": "0x0", "address": SPENDER.to_string(), "nonce": "0x5"}]});
        let list = Authorization::list_from_request(&req);
        assert_eq!(list, vec![Authorization { chain_id: U256::ZERO, address: SPENDER, nonce: 5, authority: None }]);
        let a = analyze(&TxFields::default(), &list, 1);
        assert!(a.risks.iter().any(|r| r.code == "eip7702_delegation" && r.message.contains("any chain")), "{:?}", a.risks);
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
    fn every_risk_of_a_real_safe_transaction_is_translatable() {
        use crate::safe::fixtures;
        let input = fixtures::exec_calldata(&fixtures::tx(), &fixtures::signatures());
        let a = analyze(&TxFields { to: Some(fixtures::SAFE), input, ..Default::default() }, &[], 1);
        assert!(!a.risks.is_empty());
        for r in &a.risks {
            let text = r.text.as_ref().unwrap_or_else(|| panic!("{} has no translation", r.code));
            assert_eq!(text.english(), r.message);
        }
    }

    #[test]
    fn safe_exec_transaction_is_expanded() {
        use crate::safe::fixtures;
        let mut t = fixtures::tx();
        t.to = Address::repeat_byte(0x96); // not an official library
        let input = fixtures::exec_calldata(&t, &fixtures::signatures());
        let a = analyze(&TxFields { from: Some(ME), to: Some(fixtures::SAFE), input, ..Default::default() }, &[], 1);
        assert_eq!(a.call.function.as_deref(), Some("execTransaction"));
        assert!(a.safe.is_some());
        assert!(risk_codes(&a).contains(&"safe_delegatecall"), "{:?}", a.risks);
        assert!(a.summary.starts_with("Safe 0x849D"), "{}", a.summary);
    }

    #[test]
    fn approve_hash_is_an_on_chain_signature() {
        let data = calldata("approveHash(bytes32)", &[vec![0xab; 32]]);
        let a = analyze(&TxFields { from: Some(ME), to: Some(USDC), input: data, ..Default::default() }, &[], 1);
        assert_eq!(a.call.arg("hashToApprove"), Some(format!("0x{}", "ab".repeat(32)).as_str()));
        assert!(risk_codes(&a).contains(&"safe_approve_hash"), "{:?}", a.risks);
    }

    #[test]
    fn garbage_is_not_a_transaction() {
        assert!(matches!(decode_signed(&[0x02, 0x01]), Err(DecodeError::Invalid(_))));
    }
}
