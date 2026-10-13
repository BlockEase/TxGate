//! Safe checks that need chain state, read from the user's own upstream:
//! configuration, the hash at the current nonce, and who actually signed.

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::{sol, SolCall};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::decode::{short, Risk, Severity};
use crate::safe::{
    analyze_safe_tx, parse_signatures, safe_tx_hashes, singleton_name, uses_legacy_domain, SafeExec, SafeHashes, SafeSignature, SafeTx,
    SafeTxAnalysis, SignatureKind,
};
use crate::safe_service::ServiceTx;
use crate::text::Text;
use crate::upstream::{rpc_result, Upstream};

sol! {
    function VERSION() external view returns (string);
    function nonce() external view returns (uint256);
    function getThreshold() external view returns (uint256);
    function getOwners() external view returns (address[]);
    function getModulesPaginated(address start, uint256 pageSize) external view returns (address[] array, address next);
    function approvedHashes(address owner, bytes32 hash) external view returns (uint256);
}

/// `keccak256("guard_manager.guard.address")`
const GUARD_SLOT: &str = "0x4a204f620c8c5ccdca3fd54d003badd85ba500436a431f0cbda4f558c93c34c8";
/// `keccak256("module_manager.module_guard.address")`: Safe v1.5.0's guard for
/// module transactions (empty on older versions).
pub(crate) const MODULE_GUARD_SLOT: &str = "0xb104e0b93118902c651344349b610029d694cfdec91c589c91ebafbcd0289947";
/// `keccak256("fallback_manager.handler.address")`
const FALLBACK_HANDLER_SLOT: &str = "0x6c9a6c4a39284e37ed1cf53d337577d14212a4870fb976a4366c693b939918d5";
/// Start and end marker of the Safe's linked lists.
const SENTINEL: Address = Address::with_last_byte(1);

/// A Safe's configuration as reported by the read upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeState {
    pub address: Address,
    pub version: Option<String>,
    pub nonce: U256,
    pub threshold: u64,
    pub owners: Vec<Address>,
    /// Implementation contract (storage slot 0).
    pub singleton: Option<Address>,
    /// Name of the official deployment `singleton` matches, if any.
    pub singleton_name: Option<String>,
    pub guard: Option<Address>,
    /// Safe v1.5.0+: the guard that checks transactions executed by modules.
    #[serde(default)]
    pub module_guard: Option<Address>,
    pub fallback_handler: Option<Address>,
    pub modules: Vec<Address>,
}

impl SafeState {
    pub fn is_owner(&self, a: &Address) -> bool {
        self.owners.contains(a)
    }
}

async fn call<C: SolCall>(up: &dyn Upstream, url: &str, to: &Address, c: C) -> Result<C::Return, String> {
    let data = format!("0x{}", hex::encode(c.abi_encode()));
    let v = rpc_result(up, url, "eth_call", json!([{"to": to.to_string(), "data": data}, "latest"]))
        .await
        .map_err(|e| e.to_string())?;
    let bytes = hex::decode(v.as_str().unwrap_or_default().trim_start_matches("0x")).map_err(|e| e.to_string())?;
    C::abi_decode_returns(&bytes).map_err(|e| e.to_string())
}

async fn storage_address(up: &dyn Upstream, url: &str, at: &Address, slot: &str) -> Option<Address> {
    let v = rpc_result(up, url, "eth_getStorageAt", json!([at.to_string(), slot, "latest"])).await.ok()?;
    let word = hex::decode(format!("{:0>64}", v.as_str()?.trim_start_matches("0x"))).ok()?;
    (word.len() == 32).then(|| Address::from_slice(&word[12..]))
}

/// Reads the configuration of the Safe at `safe`. Fails when it does not answer like a Safe.
pub async fn read_state(up: &dyn Upstream, url: &str, safe: Address) -> Result<SafeState, String> {
    let nonce = call(up, url, &safe, nonceCall {}).await?;
    let threshold = call(up, url, &safe, getThresholdCall {}).await?;
    let owners = call(up, url, &safe, getOwnersCall {}).await?;
    let version = call(up, url, &safe, VERSIONCall {}).await.ok();
    let modules = call(up, url, &safe, getModulesPaginatedCall { start: SENTINEL, pageSize: U256::from(50) })
        .await
        .map(|r| r.array)
        .unwrap_or_default();
    let singleton = storage_address(up, url, &safe, "0x0").await;
    let nonzero = |a: Option<Address>| a.filter(|a| !a.is_zero());
    Ok(SafeState {
        address: safe,
        version,
        nonce,
        threshold: threshold.saturating_to(),
        owners,
        singleton_name: singleton.as_ref().and_then(singleton_name).map(str::to_string),
        singleton,
        guard: nonzero(storage_address(up, url, &safe, GUARD_SLOT).await),
        module_guard: nonzero(storage_address(up, url, &safe, MODULE_GUARD_SLOT).await),
        fallback_handler: nonzero(storage_address(up, url, &safe, FALLBACK_HANDLER_SLOT).await),
        modules,
    })
}

fn risk<const N: usize>(severity: Severity, code: &str, key: &str, params: [(&str, String); N]) -> Risk {
    Risk::t(severity, code, key, params)
}

/// A signature note in English and for translation.
fn note<const N: usize>(key: &str, params: [(&str, String); N]) -> Option<Text> {
    Some(Text::new(&format!("signature.{key}"), params))
}

/// Risks that follow from a Safe's configuration alone.
pub fn state_risks(state: &SafeState) -> Vec<Risk> {
    let mut risks = Vec::new();
    match (&state.singleton, &state.singleton_name) {
        (Some(_), Some(_)) => {}
        (Some(s), None) => risks.push(risk(
            Severity::Danger,
            "safe_unknown_implementation",
            "safe_unofficial_implementation",
            [("implementation", s.to_checksum(None))],
        )),
        (None, _) => risks.push(risk(Severity::Warning, "safe_unknown_implementation", "safe_unreadable_implementation", [])),
    }
    if !state.modules.is_empty() {
        let list: Vec<String> = state.modules.iter().map(short).collect();
        risks.push(risk(
            Severity::Warning,
            "safe_modules_enabled",
            "safe_modules_enabled",
            [("modules", list.join(", "))],
        ));
    }
    if let Some(g) = state.guard {
        risks.push(risk(Severity::Info, "safe_guard", "safe_guard", [("guard", short(&g))]));
    }
    if let Some(h) = state.fallback_handler
        && crate::safe::fallback_handler_name(&h).is_none()
    {
        risks.push(risk(Severity::Warning, "safe_unknown_fallback_handler", "safe_unknown_fallback_handler", [("handler", h.to_checksum(None))]));
    }
    if let Some(g) = state.module_guard {
        risks.push(risk(Severity::Info, "safe_module_guard", "safe_module_guard", [("guard", short(&g))]));
    }
    risks
}

/// A signature with its verdict against the Safe's owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedSignature {
    #[serde(flatten)]
    pub signature: SafeSignature,
    pub is_owner: bool,
    /// Counts towards the threshold.
    pub valid: bool,
    /// English note, also kept for records written before `note_text` existed.
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_text: Option<Text>,
}

impl CheckedSignature {
    fn new(signature: SafeSignature, is_owner: bool, valid: bool, note: Option<Text>) -> Self {
        Self { signature, is_owner, valid, note: note.as_ref().map(Text::english), note_text: note }
    }
}

/// Result of checking an `execTransaction` against chain state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeCheck {
    pub state: Option<SafeState>,
    /// Hashes at the Safe's current nonce, computed by TxGate.
    pub hashes: Option<SafeHashes>,
    pub signatures: Vec<CheckedSignature>,
    pub valid_signatures: usize,
    pub risks: Vec<Risk>,
}

/// Checks an `execTransaction` sent by `sender`: reads the Safe, computes the
/// hash at its current nonce and verifies every signature against the owners.
pub async fn check_exec(up: &dyn Upstream, url: &str, chain_id: u64, exec: &SafeExec, sender: Option<Address>) -> SafeCheck {
    let state = match read_state(up, url, exec.safe).await {
        Ok(s) => s,
        Err(e) => {
            return SafeCheck {
                risks: vec![risk(
                    Severity::Warning,
                    "safe_unreadable",
                    "safe_unreadable_exec",
                    [("safe", short(&exec.safe)), ("error", e)],
                )],
                ..Default::default()
            };
        }
    };
    let mut tx = exec.tx.clone();
    tx.nonce = state.nonce;
    let hashes = safe_tx_hashes(chain_id, exec.safe, state.version.as_deref(), &tx);
    let mut risks = state_risks(&state);

    let mut signatures = Vec::new();
    let mut seen = Vec::new();
    for (i, sig) in parse_signatures(&exec.signatures, hashes.safe_tx_hash).into_iter().enumerate() {
        let n = i + 1;
        let is_owner = sig.signer.is_some_and(|s| state.is_owner(&s));
        let mut note: Option<Text> = None;
        let mut valid = is_owner;
        if let Some(signer) = sig.signer {
            if seen.contains(&signer) {
                valid = false;
                note = self::note("duplicate", []);
            } else if seen.last().is_some_and(|last| signer < *last) {
                // The Safe walks signatures in ascending signer order and reverts (GS026) otherwise.
                note = self::note("out_of_order", []);
                if !risks.iter().any(|r| r.code == "safe_signature_order") {
                    risks.push(risk(
                        Severity::Warning,
                        "safe_signature_order",
                        "safe_signature_order",
                        [("n", n.to_string())],
                    ));
                }
            }
            seen.push(signer);
            if !is_owner {
                let key = match sig.kind {
                    SignatureKind::Eip712 | SignatureKind::EthSign => "safe_signer_not_owner",
                    _ => "safe_claimed_signer_not_owner",
                };
                risks.push(risk(Severity::Danger, "safe_signer_not_owner", key, [("n", n.to_string()), ("signer", signer.to_checksum(None))]));
            }
        }
        if valid {
            match sig.kind {
                SignatureKind::ApprovedHash if sig.signer != sender => {
                    let approved = call(up, url, &exec.safe, approvedHashesCall { owner: sig.signer.unwrap_or_default(), hash: hashes.safe_tx_hash })
                        .await
                        .map(|v| !v.is_zero());
                    valid = approved == Ok(true);
                    note = self::note(if valid { "approved_on_chain" } else { "not_approved_on_chain" }, []);
                }
                SignatureKind::ApprovedHash => note = self::note("sender", []),
                SignatureKind::Contract => {
                    note = self::note("contract", []);
                    risks.push(risk(
                        Severity::Info,
                        "safe_contract_signature",
                        "safe_contract_signature",
                        [("n", n.to_string()), ("signer", short(&sig.signer.unwrap_or_default()))],
                    ));
                }
                _ => {}
            }
        }
        if let Some(e) = &sig.error {
            note = self::note("invalid", [("error", e.clone())]);
        }
        signatures.push(CheckedSignature::new(sig, is_owner, valid, note));
    }
    let valid_signatures = signatures.iter().filter(|s| s.valid).count();
    if (valid_signatures as u64) < state.threshold {
        risks.push(risk(
            Severity::Warning,
            "safe_insufficient_signatures",
            "safe_insufficient_signatures",
            [("valid", valid_signatures.to_string()), ("threshold", state.threshold.to_string()), ("nonce", state.nonce.to_string())],
        ));
    }
    SafeCheck { state: Some(state), hashes: Some(hashes), signatures, valid_signatures, risks }
}

// ---------------------------------------------------------------------------
// Reviewing a SafeTx before it is signed
// ---------------------------------------------------------------------------

/// What the Transaction Service said about a transaction (untrusted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub reported_hash: B256,
    pub proposer: Option<Address>,
    pub submission_date: Option<String>,
    pub origin: Option<String>,
}

/// Everything a signer should compare before signing a SafeTx.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeTxReport {
    pub chain_id: u64,
    pub safe: Address,
    pub tx: SafeTx,
    pub state: Option<SafeState>,
    /// Computed by TxGate from the raw fields.
    pub hashes: SafeHashes,
    pub analysis: SafeTxAnalysis,
    pub service: Option<ServiceInfo>,
    /// Signatures the service holds, verified against `hashes`.
    pub confirmations: Vec<CheckedSignature>,
    pub risks: Vec<Risk>,
}

/// Reviews `tx` for the Safe at `safe`. `state` is read from the chain when not given.
pub async fn review_tx(up: &dyn Upstream, url: &str, chain_id: u64, safe: Address, tx: SafeTx, state: Option<SafeState>) -> SafeTxReport {
    let analysis = analyze_safe_tx(chain_id, safe, &tx);
    let mut risks = analysis.risks.clone();
    let state = match state {
        Some(s) => Some(s),
        None => match read_state(up, url, safe).await {
            Ok(s) => Some(s),
            Err(e) => {
                risks.push(risk(
                    Severity::Warning,
                    "safe_unreadable",
                    "safe_unreadable_review",
                    [("safe", short(&safe)), ("error", e)],
                ));
                None
            }
        },
    };
    if let Some(st) = &state {
        risks.extend(state_risks(st));
        if tx.nonce < st.nonce {
            risks.push(risk(
                Severity::Warning,
                "safe_nonce_used",
                "safe_nonce_used",
                [("nonce", tx.nonce.to_string()), ("current", st.nonce.to_string())],
            ));
        } else if tx.nonce > st.nonce {
            let last = tx.nonce - U256::from(1);
            let key = if last == st.nonce { "safe_nonce_queued_one" } else { "safe_nonce_queued" };
            risks.push(risk(Severity::Info, "safe_nonce_queued", key, [("current", st.nonce.to_string()), ("last", last.to_string())]));
        }
    }
    let hashes = safe_tx_hashes(chain_id, safe, state.as_ref().and_then(|s| s.version.as_deref()), &tx);
    SafeTxReport { chain_id, safe, tx, state, hashes, analysis, service: None, confirmations: Vec::new(), risks }
}

/// Reviews a transaction fetched from the Transaction Service: re-hashes it
/// and verifies every confirmation the service lists.
pub async fn review_service_tx(up: &dyn Upstream, url: &str, chain_id: u64, stx: &ServiceTx, state: Option<SafeState>) -> SafeTxReport {
    let mut report = review_tx(up, url, chain_id, stx.safe, stx.tx.clone(), state).await;
    let hash = report.hashes.safe_tx_hash;
    if stx.reported_hash != hash {
        report.risks.push(risk(
            Severity::Danger,
            "safe_service_hash_mismatch",
            "safe_service_hash_mismatch",
            [("reported", stx.reported_hash.to_string()), ("computed", hash.to_string())],
        ));
    }
    let mut counted = Vec::new();
    for c in &stx.confirmations {
        let Some(sig) = parse_signatures(&c.signature, hash).into_iter().next() else { continue };
        let is_owner = report.state.as_ref().is_none_or(|s| s.is_owner(&c.owner));
        let matches = sig.signer == Some(c.owner);
        let repeated = counted.contains(&c.owner);
        let valid = matches && is_owner && sig.kind != SignatureKind::Contract && !repeated;
        if valid {
            counted.push(c.owner);
        }
        let note = if repeated {
            note("repeated", [])
        } else if !matches {
            report.risks.push(risk(
                Severity::Danger,
                "safe_bad_confirmation",
                "safe_bad_confirmation",
                [("owner", c.owner.to_checksum(None))],
            ));
            note("wrong_transaction", [])
        } else if !is_owner {
            note("not_owner", [])
        } else if sig.kind == SignatureKind::Contract {
            note("contract", [])
        } else {
            None
        };
        report.confirmations.push(CheckedSignature::new(SafeSignature { signer: Some(c.owner), ..sig }, is_owner, valid, note));
    }
    report.service = Some(ServiceInfo {
        reported_hash: stx.reported_hash,
        proposer: stx.proposer,
        submission_date: stx.submission_date.clone(),
        origin: stx.origin.clone(),
    });
    report
}

fn json_chain_id(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => {
            let s = s.trim();
            match s.strip_prefix("0x") {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => s.parse().ok(),
            }
        }
        _ => None,
    }
}

/// Reviews a SafeTx given as EIP-712 typed data (`{domain, message}`), as a
/// Transaction Service record, or as a plain object of the SafeTx fields.
pub async fn review(up: &dyn Upstream, url: &str, chain_id: u64, safe: Address, input: &Value) -> Result<SafeTxReport, String> {
    let message = input.get("message").filter(|m| m.is_object()).unwrap_or(input);
    let mut report = if message.get("safeTxHash").is_some() {
        let stx = ServiceTx::from_json(message).map_err(|e| e.to_string())?;
        let mut r = review_service_tx(up, url, chain_id, &stx, None).await;
        if stx.safe != safe {
            r.risks.push(risk(
                Severity::Danger,
                "safe_domain_mismatch",
                "safe_wrong_safe",
                [("owner_safe", stx.safe.to_checksum(None)), ("safe", safe.to_checksum(None))],
            ));
        }
        r
    } else {
        let tx = SafeTx::from_json(message)?;
        review_tx(up, url, chain_id, safe, tx, None).await
    };
    if let Some(domain) = input.get("domain") {
        if let Some(cid) = domain.get("chainId").and_then(json_chain_id)
            && cid != chain_id
        {
            report.risks.push(risk(
                Severity::Danger,
                "safe_domain_mismatch",
                "safe_wrong_chain",
                [("typed_chain", cid.to_string()), ("chain", chain_id.to_string())],
            ));
        }
        let contract = domain.get("verifyingContract").and_then(Value::as_str).and_then(|s| s.parse::<Address>().ok());
        if let Some(c) = contract
            && c != safe
        {
            report.risks.push(risk(
                Severity::Danger,
                "safe_domain_mismatch",
                "safe_typed_data_other_safe",
                [("typed_safe", c.to_checksum(None)), ("safe", safe.to_checksum(None))],
            ));
        }
        if domain.get("chainId").is_none() && !report.state.as_ref().is_some_and(|s| uses_legacy_domain(s.version.as_deref())) {
            report.risks.push(risk(Severity::Warning, "safe_domain_mismatch", "safe_domain_no_chain_id", []));
        }
    }
    Ok(report)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::safe::fixtures::*;
    use crate::safe::{analyze_exec, SINGLETONS};
    use crate::upstream::fake::FakeUpstream;
    use alloy_primitives::{Address, U256};
    use alloy_sol_types::SolValue;
    use serde_json::json;

    /// A fake chain where `SAFE` is the real v1.3.0 Safe at `nonce`.
    pub(crate) fn chain(nonce: u64, singleton: Address, modules: Vec<Address>) -> FakeUpstream {
        let up = FakeUpstream::new();
        up.on_fn("eth_call", move |req| {
            let data = req["params"][0]["data"].as_str().unwrap().to_string();
            let ret = match &data[..10] {
                "0xffa1ad74" => ("1.3.0".to_string(),).abi_encode_params(),
                "0xaffed0e0" => U256::from(nonce).abi_encode(),
                "0xe75235b8" => U256::from(3).abi_encode(),
                "0xa0e67e2b" => (OWNERS.to_vec(),).abi_encode_params(),
                "0xcc2f8452" => (modules.clone(), Address::with_last_byte(1)).abi_encode_params(),
                "0x7d832974" => U256::ZERO.abi_encode(),
                other => panic!("unexpected call {other}"),
            };
            Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":format!("0x{}", hex::encode(ret))}))
        });
        up.on_fn("eth_getStorageAt", move |req| {
            let slot = req["params"][1].as_str().unwrap();
            let value = if slot == "0x0" { format!("0x{:0>64}", hex::encode(singleton)) } else { format!("0x{}", "0".repeat(64)) };
            Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":value}))
        });
        up
    }

    fn codes(c: &SafeCheck) -> Vec<&str> {
        c.risks.iter().map(|r| r.code.as_str()).collect()
    }

    fn exec() -> crate::safe::SafeExec {
        analyze_exec(1, SAFE, &exec_calldata(&tx(), &signatures())).unwrap()
    }

    #[tokio::test]
    async fn verifies_a_fully_signed_transaction() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let c = check_exec(&up, "u", 1, &exec(), Some(EXECUTOR)).await;
        let state = c.state.as_ref().unwrap();
        assert_eq!(state.version.as_deref(), Some("1.3.0"));
        assert_eq!(state.threshold, 3);
        assert_eq!(c.hashes.unwrap().safe_tx_hash, SAFE_TX_HASH);
        assert_eq!(c.valid_signatures, 3);
        assert!(c.risks.iter().all(|r| r.severity != crate::decode::Severity::Danger), "{:?}", c.risks);
        assert!(!codes(&c).contains(&"safe_signature_order"), "executed on-chain, so correctly ordered");
    }

    #[tokio::test]
    async fn signatures_for_another_nonce_are_not_from_owners() {
        let up = chain(2868, SINGLETONS[3].0, vec![]);
        let c = check_exec(&up, "u", 1, &exec(), Some(EXECUTOR)).await;
        assert!(codes(&c).contains(&"safe_signer_not_owner"), "{:?}", c.risks);
        assert!(codes(&c).contains(&"safe_insufficient_signatures"), "{:?}", c.risks);
    }

    #[tokio::test]
    async fn approved_hash_of_another_owner_must_be_on_chain() {
        // Sent by someone other than the owner who "approved": approvedHashes() is 0.
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let c = check_exec(&up, "u", 1, &exec(), Some(Address::repeat_byte(9))).await;
        assert_eq!(c.valid_signatures, 2);
    }

    #[tokio::test]
    async fn signers_out_of_ascending_order_make_the_transaction_revert() {
        // The Safe requires signers sorted by address; swapping two entries reverts (GS026).
        let sigs = signatures();
        let mut swapped = sigs.to_vec();
        swapped[..65].copy_from_slice(&sigs[65..130]);
        swapped[65..130].copy_from_slice(&sigs[..65]);
        let e = analyze_exec(1, SAFE, &exec_calldata(&tx(), &swapped.into())).unwrap();
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let c = check_exec(&up, "u", 1, &e, Some(EXECUTOR)).await;
        assert!(codes(&c).contains(&"safe_signature_order"), "{:?}", c.risks);
        assert!(c.signatures.iter().any(|s| s.note.as_deref().is_some_and(|n| n.contains("order"))));
    }

    #[tokio::test]
    async fn repeated_service_confirmations_count_once() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let mut doc = crate::safe_service::tests::service_json();
        let c = doc["confirmations"][0].clone();
        doc["confirmations"] = json!([c.clone(), c]);
        let r = review(&up, "u", 1, SAFE, &doc).await.unwrap();
        assert_eq!(r.confirmations.iter().filter(|c| c.valid).count(), 1);
    }

    #[tokio::test]
    async fn a_safe_150_module_guard_is_read() {
        // Safe v1.5.0 keeps a separate guard for module transactions.
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let module_guard = Address::repeat_byte(0x4d);
        up.on_fn("eth_getStorageAt", move |req| {
            let value = match req["params"][1].as_str().unwrap() {
                "0x0" => format!("0x{:0>64}", hex::encode(SINGLETONS[3].0)),
                MODULE_GUARD_SLOT => format!("0x{:0>64}", hex::encode(module_guard)),
                _ => format!("0x{}", "0".repeat(64)),
            };
            Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":value}))
        });
        let state = read_state(&up, "u", SAFE).await.unwrap();
        assert_eq!(state.module_guard, Some(module_guard));
        assert!(state_risks(&state).iter().any(|r| r.code == "safe_module_guard"), "{:?}", state_risks(&state));
    }

    #[test]
    fn an_unknown_fallback_handler_is_flagged() {
        use alloy_primitives::address;
        let mut state = SafeState { singleton: Some(SINGLETONS[3].0), singleton_name: Some("Safe 1.3.0".into()), ..Default::default() };
        let codes = |s: &SafeState| state_risks(s).into_iter().map(|r| (r.code, r.severity)).collect::<Vec<_>>();
        // Official handlers, as used by real Safes: CompatibilityFallbackHandler 1.5.0
        // and the Safe4337Module v0.3.0.
        for official in [
            address!("3EfCBb83A4A7AfcB4F68D501E2c2203a38be77f4"),
            address!("75cf11467937ce3F2f357CE24ffc3DBF8fD5c226"),
            address!("2f55e8b20D0B9FEFA187AA7d00B6Cbe563605bF5"),
        ] {
            state.fallback_handler = Some(official);
            assert!(codes(&state).is_empty(), "{official}: {:?}", codes(&state));
        }
        state.fallback_handler = Some(Address::repeat_byte(0x66));
        assert_eq!(codes(&state), vec![("safe_unknown_fallback_handler".to_string(), crate::decode::Severity::Warning)]);
    }

    #[tokio::test]
    async fn unofficial_implementation_and_modules_are_flagged() {
        let up = chain(2867, Address::repeat_byte(0xba), vec![Address::repeat_byte(0x33)]);
        let c = check_exec(&up, "u", 1, &exec(), Some(EXECUTOR)).await;
        assert!(codes(&c).contains(&"safe_unknown_implementation"), "{:?}", c.risks);
        assert!(codes(&c).contains(&"safe_modules_enabled"), "{:?}", c.risks);
    }

    /// The SafeTx as the `message` of EIP-712 typed data.
    fn message_json(t: &crate::safe::SafeTx) -> serde_json::Value {
        json!({
            "to": t.to.to_string(), "value": t.value.to_string(), "data": t.data.to_string(),
            "operation": t.operation, "safeTxGas": t.safe_tx_gas.to_string(), "baseGas": t.base_gas.to_string(),
            "gasPrice": t.gas_price.to_string(), "gasToken": t.gas_token.to_string(),
            "refundReceiver": t.refund_receiver.to_string(), "nonce": t.nonce.to_string(),
        })
    }

    fn report_codes(r: &SafeTxReport) -> Vec<&str> {
        r.risks.iter().map(|r| r.code.as_str()).collect()
    }

    #[tokio::test]
    async fn reviews_a_transaction_before_signing() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let r = review(&up, "u", 1, SAFE, &json!({"message": message_json(&tx()), "domain": {"chainId": 1, "verifyingContract": SAFE.to_string()}})).await.unwrap();
        assert_eq!(r.hashes.safe_tx_hash, SAFE_TX_HASH);
        assert_eq!(r.analysis.calls.len(), 2);
        assert!(!report_codes(&r).iter().any(|c| c.starts_with("safe_nonce") || *c == "safe_domain_mismatch"), "{:?}", r.risks);
    }

    #[tokio::test]
    async fn a_used_nonce_is_flagged() {
        let up = chain(2900, SINGLETONS[3].0, vec![]);
        let r = review(&up, "u", 1, SAFE, &message_json(&tx())).await.unwrap();
        assert!(report_codes(&r).contains(&"safe_nonce_used"), "{:?}", r.risks);
    }

    #[tokio::test]
    async fn typed_data_for_another_chain_or_safe_is_flagged() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let doc = json!({"message": message_json(&tx()), "domain": {"chainId": "0x2105", "verifyingContract": SAFE.to_string()}});
        let r = review(&up, "u", 1, SAFE, &doc).await.unwrap();
        assert!(report_codes(&r).contains(&"safe_domain_mismatch"), "{:?}", r.risks);
    }

    #[tokio::test]
    async fn service_transactions_are_rehashed_and_their_confirmations_verified() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let r = review(&up, "u", 1, SAFE, &crate::safe_service::tests::service_json()).await.unwrap();
        assert_eq!(r.confirmations.len(), 1);
        assert!(r.confirmations[0].valid);
        assert!(!report_codes(&r).contains(&"safe_service_hash_mismatch"));

        let mut tampered = crate::safe_service::tests::service_json();
        tampered["to"] = json!(Address::repeat_byte(0x66).to_string());
        let r = review(&up, "u", 1, SAFE, &tampered).await.unwrap();
        assert!(report_codes(&r).contains(&"safe_service_hash_mismatch"), "{:?}", r.risks);
        assert!(!r.confirmations[0].valid);
    }

    #[tokio::test]
    async fn an_address_that_is_not_a_safe_is_reported() {
        let up = FakeUpstream::new(); // every call fails
        let c = check_exec(&up, "u", 1, &exec(), None).await;
        assert!(c.state.is_none());
        assert!(codes(&c).contains(&"safe_unreadable"), "{:?}", c.risks);
    }
}

#[cfg(test)]
mod dump_for_ui {
    use super::tests::chain;
    use super::*;
    use crate::safe::fixtures::*;
    use crate::safe::SINGLETONS;

    #[tokio::test]
    #[ignore]
    async fn dump() {
        let up = chain(2867, SINGLETONS[3].0, vec![]);
        let queued = review(&up, "u", 1, SAFE, &crate::safe_service::tests::service_json()).await.unwrap();
        // The Bybit pattern: a delegatecall dressed as a transfer.
        let mut evil = crate::safe_service::tests::service_json();
        evil["to"] = json!("0x96221423681A6d52E184D440a8eFCEbB105C7242");
        evil["data"] = json!("0xa9059cbb000000000000000000000000bdd077f651ebe7f7b3ce16fe5f2b025be29695160000000000000000000000000000000000000000000000000000000000000000");
        evil["nonce"] = json!("2868");
        evil["confirmations"] = json!([]);
        let evil_report = review(&up, "u", 1, SAFE, &evil).await.unwrap();
        let exec = crate::safe::analyze_exec(1, SAFE, &exec_calldata(&tx(), &signatures())).unwrap();
        let check = check_exec(&up, "u", 1, &exec, Some(EXECUTOR)).await;
        let out = json!({"queued": queued, "evil": evil_report, "exec": exec, "check": check});
        std::fs::write(std::env::var("DUMP").unwrap(), serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }
}
