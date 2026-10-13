//! JSON-RPC envelope handling and method classification.

use alloy_primitives::Bytes;
use serde_json::{json, Value};

/// Methods that cause a transaction to be broadcast and therefore must be held for approval.
pub const SEND_METHODS: &[&str] = &[
    "eth_sendRawTransaction",
    "eth_sendRawTransactionSync",
    "eth_sendRawTransactionConditional",
    "eth_sendTransaction",
    "eth_sendBundle",
    "eth_sendPrivateTransaction",
    "eth_sendPrivateRawTransaction",
    "mev_sendBundle",
    "personal_sendTransaction",
    // ERC-4337: smart-account wallets submit signed UserOperations to a bundler,
    // often on the same provider endpoint.
    "eth_sendUserOperation",
];

/// Sending methods served by the read upstream once approved: an ERC-4337
/// bundler or a node holding the account. Broadcast upstreams (e.g. Flashbots
/// Protect) only relay signed transactions.
pub const READ_UPSTREAM_SENDS: &[&str] = &["eth_sendUserOperation", "personal_sendTransaction"];

/// Node-account methods and the positions of their secret parameters
/// (passwords, private keys). These are forwarded as they are but never logged.
pub const SECRET_PARAMS: &[(&str, &[usize])] = &[
    ("personal_sendTransaction", &[1]),
    ("personal_signTransaction", &[1]),
    ("personal_sign", &[2]),
    ("personal_unlockAccount", &[1]),
    ("personal_importRawKey", &[0, 1]),
    ("personal_newAccount", &[0]),
];

pub const ESTIMATE_METHOD: &str = "eth_estimateGas";

/// JSON-RPC error code returned to the wallet when the user rejects a transaction
/// (EIP-1193 "User Rejected Request").
pub const USER_REJECTED_CODE: i64 = 4001;
pub const REJECTED_MESSAGE: &str = "Rejected by TxGate";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Read,
    Estimate,
    Send,
}

pub fn classify(method: &str) -> Category {
    // Some upstreams may match method names regardless of case. A sending method
    // spelt differently is treated as sending, and `send_payload` refuses it.
    if SEND_METHODS.iter().any(|m| m.eq_ignore_ascii_case(method)) {
        Category::Send
    } else if method == ESTIMATE_METHOD {
        Category::Estimate
    } else {
        Category::Read
    }
}

/// A single JSON-RPC call, parsed leniently: unknown fields are ignored and the
/// original value is kept so it can be forwarded byte-for-byte equivalent.
#[derive(Debug, Clone)]
pub struct Call {
    pub id: Value,
    pub method: String,
    pub params: Value,
    pub raw: Value,
}

impl Call {
    pub fn parse(mut value: Value) -> Result<Self, Value> {
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        // Some clients omit the version; strict upstreams reject such requests.
        if let Some(obj) = value.as_object_mut()
            && !obj.contains_key("jsonrpc")
        {
            obj.insert("jsonrpc".into(), json!("2.0"));
        }
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Err(error_response(id, -32600, "Invalid Request"));
        };
        Ok(Self {
            id: id.clone(),
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Array(vec![])),
            raw: value,
        })
    }

    pub fn category(&self) -> Category {
        classify(&self.method)
    }

    /// The n-th positional parameter.
    pub fn param(&self, n: usize) -> Option<&Value> {
        self.params.as_array().and_then(|a| a.get(n))
    }
}

pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

pub fn rejected_response(id: Value) -> Value {
    error_response(id, USER_REJECTED_CODE, REJECTED_MESSAGE)
}

/// Rewrites the `id` of an upstream response so it matches the wallet's request.
/// Needed when one upstream response is fanned out to several waiting requests.
pub fn with_id(mut response: Value, id: &Value) -> Value {
    if let Some(obj) = response.as_object_mut() {
        obj.insert("id".into(), id.clone());
    }
    response
}

/// What a sending method asks to broadcast.
#[derive(Debug, Clone, PartialEq)]
pub enum SendPayload {
    /// One or more signed, RLP/EIP-2718 encoded transactions.
    Signed(Vec<Bytes>),
    /// An unsigned transaction object (`eth_sendTransaction`), signed by the node.
    Unsigned(Value),
    /// An ERC-4337 UserOperation, which `entry_point` executes on the smart account `sender`.
    UserOperation { op: Value, entry_point: Option<alloy_primitives::Address> },
}

/// Extracts the transaction(s) from a sending call.
pub fn send_payload(call: &Call) -> Result<SendPayload, String> {
    let first = call.param(0).ok_or("missing params[0]")?;
    let hex_tx = |v: &Value| -> Result<Bytes, String> {
        let s = v.as_str().ok_or("transaction is not a hex string")?;
        s.parse::<Bytes>().map_err(|e| format!("invalid transaction hex: {e}"))
    };
    match call.method.as_str() {
        "eth_sendRawTransaction" | "eth_sendRawTransactionSync" | "eth_sendRawTransactionConditional" | "eth_sendPrivateRawTransaction" => {
            Ok(SendPayload::Signed(vec![hex_tx(first)?]))
        }
        "eth_sendPrivateTransaction" => {
            let tx = first.get("tx").ok_or("missing params[0].tx")?;
            Ok(SendPayload::Signed(vec![hex_tx(tx)?]))
        }
        "eth_sendBundle" => {
            let txs = first.get("txs").and_then(Value::as_array).ok_or("missing params[0].txs")?;
            let txs = txs.iter().map(hex_tx).collect::<Result<Vec<_>, _>>()?;
            if txs.is_empty() {
                return Err("bundle has no transactions".into());
            }
            Ok(SendPayload::Signed(txs))
        }
        "mev_sendBundle" => {
            let mut txs = Vec::new();
            mev_bundle_txs(first, &hex_tx, &mut txs)?;
            if txs.is_empty() {
                return Err("bundle has no transactions".into());
            }
            Ok(SendPayload::Signed(txs))
        }
        "eth_sendTransaction" | "personal_sendTransaction" => Ok(SendPayload::Unsigned(first.clone())),
        "eth_sendUserOperation" => {
            first.get("sender").and_then(Value::as_str).ok_or("missing params[0].sender")?;
            let entry_point = call.param(1).and_then(Value::as_str).and_then(|s| s.parse().ok());
            Ok(SendPayload::UserOperation { op: first.clone(), entry_point })
        }
        other => Err(format!("{other} is not a sending method")),
    }
}

/// Signed transactions of a MEV-Share bundle. Body entries are a signed
/// transaction ({tx}), the hash of another bundle ({hash}) or a whole nested
/// bundle ({bundle}), whose transactions are broadcast as well.
fn mev_bundle_txs(bundle: &Value, hex_tx: &dyn Fn(&Value) -> Result<Bytes, String>, out: &mut Vec<Bytes>) -> Result<(), String> {
    let body = bundle.get("body").and_then(Value::as_array).ok_or("missing bundle body")?;
    for entry in body {
        if let Some(tx) = entry.get("tx") {
            out.push(hex_tx(tx)?);
        } else if let Some(nested) = entry.get("bundle") {
            mev_bundle_txs(nested, hex_tx, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(v: Value) -> Call {
        Call::parse(v).unwrap()
    }

    #[test]
    fn classifies_methods() {
        assert_eq!(classify("eth_sendRawTransaction"), Category::Send);
        assert_eq!(classify("eth_sendBundle"), Category::Send);
        assert_eq!(classify("eth_estimateGas"), Category::Estimate);
        assert_eq!(classify("eth_call"), Category::Read);
    }

    #[test]
    fn missing_jsonrpc_version_is_filled_in_for_the_upstream() {
        let c = call(json!({"id": 1, "method": "eth_chainId"}));
        assert_eq!(c.raw["jsonrpc"], "2.0");
    }

    #[test]
    fn request_without_method_is_invalid() {
        let err = Call::parse(json!({"jsonrpc":"2.0","id":7})).unwrap_err();
        assert_eq!(err["error"]["code"], -32600);
        assert_eq!(err["id"], 7);
    }

    #[test]
    fn extracts_raw_transaction_variants() {
        let raw = json!({"id":1,"method":"eth_sendRawTransaction","params":["0x01ab"]});
        assert_eq!(send_payload(&call(raw)).unwrap(), SendPayload::Signed(vec![Bytes::from(vec![1, 0xab])]));

        let private = json!({"id":1,"method":"eth_sendPrivateTransaction","params":[{"tx":"0x02"}]});
        assert_eq!(send_payload(&call(private)).unwrap(), SendPayload::Signed(vec![Bytes::from(vec![2])]));

        let bundle = json!({"id":1,"method":"eth_sendBundle","params":[{"txs":["0x01","0x02"],"blockNumber":"0x1"}]});
        let SendPayload::Signed(txs) = send_payload(&call(bundle)).unwrap() else { panic!() };
        assert_eq!(txs.len(), 2);

        let unsigned = json!({"id":1,"method":"eth_sendTransaction","params":[{"from":"0x1"}]});
        assert!(matches!(send_payload(&call(unsigned)).unwrap(), SendPayload::Unsigned(_)));
    }

    #[test]
    fn mev_bundles_include_transactions_of_nested_bundles() {
        // MEV-Share bodies can nest whole bundles; their transactions are broadcast too.
        let nested = json!({"id":1,"method":"mev_sendBundle","params":[{"body":[
            {"tx":"0x01","canRevert":false},
            {"hash":"0xabc"},
            {"bundle":{"body":[{"tx":"0x02"},{"bundle":{"body":[{"tx":"0x03"}]}}]}}
        ]}]});
        let SendPayload::Signed(txs) = send_payload(&call(nested)).unwrap() else { panic!() };
        assert_eq!(txs, vec![Bytes::from(vec![1]), Bytes::from(vec![2]), Bytes::from(vec![3])]);
    }

    #[test]
    fn a_send_method_in_other_letter_case_is_never_forwarded_unchecked() {
        let shouting = call(json!({"id":1,"method":"ETH_SENDRAWTRANSACTION","params":["0x01"]}));
        assert_eq!(shouting.category(), Category::Send);
        assert!(send_payload(&shouting).is_err(), "refused instead of forwarded");
    }

    #[test]
    fn rejects_malformed_send_params() {
        let bad = json!({"id":1,"method":"eth_sendRawTransaction","params":[42]});
        assert!(send_payload(&call(bad)).is_err());
        let empty = json!({"id":1,"method":"eth_sendBundle","params":[{"txs":[]}]});
        assert!(send_payload(&call(empty)).is_err());
    }

    #[test]
    fn rejected_response_uses_4001() {
        let r = rejected_response(json!(3));
        assert_eq!(r["error"]["code"], 4001);
        assert_eq!(r["error"]["message"], "Rejected by TxGate");
    }
}
