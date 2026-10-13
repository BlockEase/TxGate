//! Safe (Gnosis Safe) multisig support: SafeTx hashing, `execTransaction`
//! decoding, `multiSend` unpacking, signature recovery and risk flags.
//!
//! Everything here is pure. Reading a Safe's state from the chain lives in
//! [`crate::safe_state`]. See `docs/SAFE.md` for the background.

use alloy_primitives::{address, keccak256, Address, Bytes, B256, U256};
use alloy_sol_types::{sol, SolCall, SolValue};
use serde::{Deserialize, Serialize};

use crate::text::Text;
use crate::decode::{analyze, format_units, short, CallInfo, Risk, Severity, TxFields};

sol! {
    function execTransaction(address to, uint256 value, bytes data, uint8 operation, uint256 safeTxGas, uint256 baseGas, uint256 gasPrice, address gasToken, address refundReceiver, bytes signatures) returns (bool success);
    function multiSend(bytes transactions);
    function addOwnerWithThreshold(address owner, uint256 _threshold);
    function removeOwner(address prevOwner, address owner, uint256 _threshold);
    function swapOwner(address prevOwner, address oldOwner, address newOwner);
    function changeThreshold(uint256 _threshold);
    function enableModule(address module);
    function disableModule(address prevModule, address module);
    function setGuard(address guard);
    function setModuleGuard(address moduleGuard);
    function setFallbackHandler(address handler);
    function changeMasterCopy(address _masterCopy);
    function approveHash(bytes32 hashToApprove);
}

/// Official Safe libraries that are meant to be DELEGATECALLed (MultiSend,
/// MultiSendCallOnly, SignMessageLib; v1.1.1 – v1.5.0, canonical, EIP-155 and
/// zkSync deployments), from github.com/safe-global/safe-deployments.
const DELEGATECALL_LIBRARIES: &[(Address, &str)] = &[
    (address!("8D29bE29923b68abfDD21e541b9374737B49cdAD"), "MultiSend 1.1.1"),
    (address!("A238CBeb142c10Ef7Ad8442C6D1f9E89e07e7761"), "MultiSend 1.3.0"),
    (address!("998739BFdAAdde7C933B942a68053933098f9EDa"), "MultiSend 1.3.0"),
    (address!("0dFcccB95225ffB03c6FBB2559B530C2B7C8A912"), "MultiSend 1.3.0"),
    (address!("38869bf66a61cF6bDB996A6aE40D5853Fd43B526"), "MultiSend 1.4.1"),
    (address!("309D0B190FeCCa8e1D5D8309a16F7e3CB133E885"), "MultiSend 1.4.1"),
    (address!("218543288004CD07832472D464648173c77D7eB7"), "MultiSend 1.5.0"),
    (address!("40A2aCCbd92BCA938b02010E17A5b8929b49130D"), "MultiSendCallOnly 1.3.0"),
    (address!("A1dabEF33b3B82c7814B6D82A79e50F4AC44102B"), "MultiSendCallOnly 1.3.0"),
    (address!("f220D3b4DFb23C4ade8C88E526C1353AbAcbC38F"), "MultiSendCallOnly 1.3.0"),
    (address!("9641d764fc13c8B624c04430C7356C1C7C8102e2"), "MultiSendCallOnly 1.4.1"),
    (address!("0408EF011960d02349d50286D20531229BCef773"), "MultiSendCallOnly 1.4.1"),
    (address!("A83c336B20401Af773B6219BA5027174338D1836"), "MultiSendCallOnly 1.5.0"),
    (address!("A65387F16B013cf2Af4605Ad8aA5ec25a2cbA3a2"), "SignMessageLib 1.3.0"),
    (address!("98FFBBF51bb33A056B08ddf711f289936AafF717"), "SignMessageLib 1.3.0"),
    (address!("357147caf9C0cCa67DfA0CF5369318d8193c8407"), "SignMessageLib 1.3.0"),
    (address!("d53cd0aB83D845Ac265BE939c57F53AD838012c9"), "SignMessageLib 1.4.1"),
    (address!("Aca1ec0a1A575CDCCF1DC3d5d296202Eb6061888"), "SignMessageLib 1.4.1"),
    (address!("4FfeF8222648872B3dE295Ba1e49110E61f5b5aa"), "SignMessageLib 1.5.0"),
];

/// Official Safe implementations (singletons / master copies).
pub const SINGLETONS: &[(Address, &str)] = &[
    (address!("b6029EA3B2c51D09a50B53CA8012FeEB05bDa35A"), "Safe 1.0.0"),
    (address!("34CfAC646f301356fAa8B21e94227e3583Fe3F5F"), "Safe 1.1.1"),
    (address!("6851D6fDFAfD08c0295C392436245E5bc78B0185"), "Safe 1.2.0"),
    (address!("d9Db270c1B5E3Bd161E8c8503c55cEABeE709552"), "Safe 1.3.0"),
    (address!("69f4D1788e39c87893C980c06EdF4b7f686e2938"), "Safe 1.3.0"),
    (address!("B00ce5CCcdEf57e539ddcEd01DF43a13855d9910"), "Safe 1.3.0"),
    (address!("3E5c63644E683549055b9Be8653de26E0B4CD36E"), "SafeL2 1.3.0"),
    (address!("fb1bffC9d739B8D520DaF37dF666da4C687191EA"), "SafeL2 1.3.0"),
    (address!("1727c2c531cf966f902E5927b98490fDFb3b2b70"), "SafeL2 1.3.0"),
    (address!("41675C099F32341bf84BFc5382aF534df5C7461a"), "Safe 1.4.1"),
    (address!("C35F063962328aC65cED5D4c3fC5dEf8dec68dFa"), "Safe 1.4.1"),
    (address!("29fcB43b46531BcA003ddC8FCB67FFE91900C762"), "SafeL2 1.4.1"),
    (address!("610fcA2e0279Fa1F8C00c8c2F71dF522AD469380"), "SafeL2 1.4.1"),
    (address!("Ff51A5898e281Db6DfC7855790607438dF2ca44b"), "Safe 1.5.0"),
    (address!("Edd160fEBBD92E350D4D398fb636302fccd67C7e"), "SafeL2 1.5.0"),
];

pub fn singleton_name(addr: &Address) -> Option<&'static str> {
    SINGLETONS.iter().find(|(a, _)| a == addr).map(|(_, n)| *n)
}

/// Official fallback handlers (safe-deployments, safe-modules-deployments and
/// CoW Protocol's composable-cow, whose ExtensibleFallbackHandler is Safe's own
/// code and is used by Safes placing CoW TWAP / conditional orders).
/// A fallback handler answers EIP-1271 signature checks for the Safe.
pub const FALLBACK_HANDLERS: &[(Address, &str)] = &[
    (address!("017062a1dE2FE6b99BE3d9d37841FeD19F573804"), "CompatibilityFallbackHandler 1.3.0"),
    (address!("2f870a80647BbC554F3a0EBD093f11B4d2a7492A"), "CompatibilityFallbackHandler 1.3.0"),
    (address!("f48f2B2d2a534e402487b3ee7C18c33Aec0Fe5e4"), "CompatibilityFallbackHandler 1.3.0"),
    (address!("9301E98DD367135f21bdF66f342A249c9D5F9069"), "CompatibilityFallbackHandler 1.4.1"),
    (address!("fd0732Dc9E303f09fCEf3a7388Ad10A83459Ec99"), "CompatibilityFallbackHandler 1.4.1"),
    (address!("3EfCBb83A4A7AfcB4F68D501E2c2203a38be77f4"), "CompatibilityFallbackHandler 1.5.0"),
    (address!("85a8ca358D388530ad0fB95D0cb89Dd44Fc242c3"), "ExtensibleFallbackHandler 1.5.0"),
    (address!("2f55e8b20D0B9FEFA187AA7d00B6Cbe563605bF5"), "ExtensibleFallbackHandler (CoW Protocol)"),
    (address!("a581c4A4DB7175302464fF3C06380BC3270b4037"), "Safe4337Module 0.2.0"),
    (address!("75cf11467937ce3F2f357CE24ffc3DBF8fD5c226"), "Safe4337Module 0.3.0"),
];

/// Official Safe modules (safe-modules-deployments).
pub const OFFICIAL_MODULES: &[(Address, &str)] = &[
    (address!("a581c4A4DB7175302464fF3C06380BC3270b4037"), "Safe4337Module 0.2.0"),
    (address!("75cf11467937ce3F2f357CE24ffc3DBF8fD5c226"), "Safe4337Module 0.3.0"),
    (address!("1Fb403834C911eB98d56E74F5182b0d64C3b3b4D"), "AllowanceModule 0.1.0"),
    (address!("CFbFaC74C26F8647cBDb8c5caf80BB5b32E43134"), "AllowanceModule 0.1.0"),
    (address!("AA46724893dedD72658219405185Fb0Fc91e091C"), "AllowanceModule 0.1.1"),
    (address!("4Aa5Bf7D840aC607cb5BD3249e6Af6FC86C04897"), "SocialRecoveryModule 0.1.0"),
];

pub fn official_module_name(addr: &Address) -> Option<&'static str> {
    OFFICIAL_MODULES.iter().find(|(a, _)| a == addr).map(|(_, n)| *n)
}

pub fn fallback_handler_name(addr: &Address) -> Option<&'static str> {
    FALLBACK_HANDLERS.iter().find(|(a, _)| a == addr).map(|(_, n)| *n)
}

pub fn delegatecall_library(addr: &Address) -> Option<&'static str> {
    DELEGATECALL_LIBRARIES.iter().find(|(a, _)| a == addr).map(|(_, n)| *n)
}

/// The fields an owner signs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeTx {
    pub to: Address,
    pub value: U256,
    pub data: Bytes,
    /// 0 = CALL, 1 = DELEGATECALL.
    pub operation: u8,
    pub safe_tx_gas: U256,
    pub base_gas: U256,
    pub gas_price: U256,
    pub gas_token: Address,
    pub refund_receiver: Address,
    pub nonce: U256,
}

fn json_uint(v: &serde_json::Value, keys: &[&str]) -> Result<U256, String> {
    use serde_json::Value;
    match keys.iter().find_map(|k| v.get(*k)) {
        Some(Value::Number(n)) => n.as_u64().map(U256::from).ok_or_else(|| format!("invalid {}", keys[0])),
        // Decimal or 0x-prefixed hex.
        Some(Value::String(s)) if s.trim().is_empty() => Ok(U256::ZERO),
        Some(Value::String(s)) => s.trim().parse().map_err(|_| format!("invalid {}", keys[0])),
        Some(Value::Null) | None => Ok(U256::ZERO),
        _ => Err(format!("invalid {}", keys[0])),
    }
}

fn json_address(v: &serde_json::Value, key: &str, required: bool) -> Result<Address, String> {
    match v.get(key).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.parse().map_err(|_| format!("invalid {key}")),
        None if required => Err(format!("missing {key}")),
        None => Ok(Address::ZERO),
    }
}

impl SafeTx {
    /// Reads the fields as named in the EIP-712 message, the Safe Transaction
    /// Service and Safe{Wallet}: `to, value, data, operation, safeTxGas,
    /// baseGas (or dataGas), gasPrice, gasToken, refundReceiver, nonce`.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let data = match v.get("data").and_then(serde_json::Value::as_str).map(str::trim) {
            Some(s) if !s.is_empty() && s != "0x" => s.parse().map_err(|_| "invalid data".to_string())?,
            _ => Bytes::new(),
        };
        let operation = json_uint(v, &["operation"])?;
        Ok(Self {
            to: json_address(v, "to", true)?,
            value: json_uint(v, &["value"])?,
            data,
            operation: u8::try_from(operation).map_err(|_| "invalid operation".to_string())?,
            safe_tx_gas: json_uint(v, &["safeTxGas"])?,
            base_gas: json_uint(v, &["baseGas", "dataGas"])?,
            gas_price: json_uint(v, &["gasPrice"])?,
            gas_token: json_address(v, "gasToken", false)?,
            refund_receiver: json_address(v, "refundReceiver", false)?,
            nonce: json_uint(v, &["nonce"])?,
        })
    }
}

/// The three hashes a signer may be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeHashes {
    pub domain_hash: B256,
    pub message_hash: B256,
    pub safe_tx_hash: B256,
}

fn domain_typehash() -> B256 {
    keccak256("EIP712Domain(uint256 chainId,address verifyingContract)")
}

fn legacy_domain_typehash() -> B256 {
    keccak256("EIP712Domain(address verifyingContract)")
}

fn safe_tx_typehash() -> B256 {
    keccak256(
        "SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)",
    )
}

/// Whether a Safe of this version omits the chain id from its EIP-712 domain
/// (v1.0.0 – v1.2.0). Unknown versions use the current format.
pub fn uses_legacy_domain(version: Option<&str>) -> bool {
    let Some(v) = version else { return false };
    let mut parts = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    matches!((parts.next().flatten(), parts.next().flatten()), (Some(1), Some(minor)) if minor < 3)
}

/// Computes the hashes the Safe at `safe` on `chain_id` checks signatures against.
pub fn safe_tx_hashes(chain_id: u64, safe: Address, version: Option<&str>, tx: &SafeTx) -> SafeHashes {
    let domain_hash = if uses_legacy_domain(version) {
        keccak256((legacy_domain_typehash(), safe).abi_encode())
    } else {
        keccak256((domain_typehash(), U256::from(chain_id), safe).abi_encode())
    };
    let message_hash = keccak256(
        (
            safe_tx_typehash(),
            tx.to,
            tx.value,
            keccak256(&tx.data),
            U256::from(tx.operation),
            tx.safe_tx_gas,
            tx.base_gas,
            tx.gas_price,
            tx.gas_token,
            tx.refund_receiver,
            tx.nonce,
        )
            .abi_encode(),
    );
    let mut preimage = Vec::with_capacity(66);
    preimage.extend_from_slice(&[0x19, 0x01]);
    preimage.extend_from_slice(domain_hash.as_slice());
    preimage.extend_from_slice(message_hash.as_slice());
    SafeHashes { domain_hash, message_hash, safe_tx_hash: keccak256(preimage) }
}

// ---------------------------------------------------------------------------
// Decoding and static analysis
// ---------------------------------------------------------------------------

/// One call the Safe would make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InnerCall {
    /// 0 = CALL, 1 = DELEGATECALL.
    pub operation: u8,
    pub to: Address,
    pub value: U256,
    pub data: Bytes,
    pub call: CallInfo,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeTxAnalysis {
    /// The calls the Safe makes: the batch entries of a MultiSend, otherwise the SafeTx itself.
    pub calls: Vec<InnerCall>,
    /// The SafeTx is a delegatecall into an official MultiSend library.
    pub batch: Option<String>,
    /// Safe-level risks plus the risks of every inner call.
    pub risks: Vec<Risk>,
    pub summary: String,
}

/// A decoded `execTransaction` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeExec {
    pub safe: Address,
    /// The signed fields. `nonce` is not part of the call; it is the Safe's
    /// nonce at execution time and stays zero until read from the chain.
    pub tx: SafeTx,
    pub signatures: Bytes,
    pub analysis: SafeTxAnalysis,
}

/// Decodes `execTransaction` calldata sent to the Safe at `safe`.
pub fn analyze_exec(chain_id: u64, safe: Address, input: &[u8]) -> Option<SafeExec> {
    let c = execTransactionCall::abi_decode(input).ok()?;
    let tx = SafeTx {
        to: c.to,
        value: c.value,
        data: c.data,
        operation: c.operation,
        safe_tx_gas: c.safeTxGas,
        base_gas: c.baseGas,
        gas_price: c.gasPrice,
        gas_token: c.gasToken,
        refund_receiver: c.refundReceiver,
        nonce: U256::ZERO,
    };
    let analysis = analyze_safe_tx(chain_id, safe, &tx);
    Some(SafeExec { safe, tx, signatures: c.signatures, analysis })
}

fn risk<const N: usize>(severity: Severity, code: &str, key: &str, params: [(&str, String); N]) -> Risk {
    Risk::t(severity, code, key, params)
}

/// Unpacks `multiSend(bytes)`: each entry is
/// `operation (1) ‖ to (20) ‖ value (32) ‖ dataLength (32) ‖ data`.
fn unpack_multisend(data: &[u8]) -> Option<Vec<(u8, Address, U256, Bytes)>> {
    let packed = multiSendCall::abi_decode(data).ok()?.transactions;
    let mut out = Vec::new();
    let mut rest: &[u8] = &packed;
    while !rest.is_empty() {
        if rest.len() < 85 {
            return None;
        }
        let operation = rest[0];
        let to = Address::from_slice(&rest[1..21]);
        let value = U256::from_be_slice(&rest[21..53]);
        let len = U256::from_be_slice(&rest[53..85]).saturating_to::<usize>();
        let end = 85usize.checked_add(len).filter(|e| *e <= rest.len())?;
        out.push((operation, to, value, Bytes::copy_from_slice(&rest[85..end])));
        rest = &rest[end..];
    }
    Some(out)
}

/// Risks of the Safe calling its own configuration functions.
fn self_call_risk(input: &[u8]) -> Option<Risk> {
    let a = |x: &Address| short(x);
    if let Ok(c) = addOwnerWithThresholdCall::abi_decode(input) {
        return Some(risk(Severity::Warning, "safe_config_change", "safe_add_owner", [("owner", a(&c.owner)), ("threshold", c._threshold.to_string())]));
    }
    if let Ok(c) = removeOwnerCall::abi_decode(input) {
        return Some(risk(Severity::Warning, "safe_config_change", "safe_remove_owner", [("owner", a(&c.owner)), ("threshold", c._threshold.to_string())]));
    }
    if let Ok(c) = swapOwnerCall::abi_decode(input) {
        return Some(risk(Severity::Warning, "safe_config_change", "safe_swap_owner", [("old", a(&c.oldOwner)), ("new", a(&c.newOwner))]));
    }
    if let Ok(c) = changeThresholdCall::abi_decode(input) {
        let sev = if c._threshold <= U256::from(1) { Severity::Danger } else { Severity::Warning };
        return Some(risk(sev, "safe_config_change", "safe_change_threshold", [("threshold", c._threshold.to_string())]));
    }
    if let Ok(c) = enableModuleCall::abi_decode(input) {
        return Some(risk(Severity::Danger, "safe_config_change", "safe_enable_module", [("module", a(&c.module))]));
    }
    if let Ok(c) = disableModuleCall::abi_decode(input) {
        return Some(risk(Severity::Warning, "safe_config_change", "safe_disable_module", [("module", a(&c.module))]));
    }
    if let Ok(c) = setGuardCall::abi_decode(input) {
        return Some(risk(Severity::Danger, "safe_config_change", "safe_set_guard", [("guard", a(&c.guard))]));
    }
    if let Ok(c) = setModuleGuardCall::abi_decode(input) {
        return Some(risk(Severity::Danger, "safe_config_change", "safe_set_module_guard", [("guard", a(&c.moduleGuard))]));
    }
    if let Ok(c) = setFallbackHandlerCall::abi_decode(input) {
        return Some(risk(Severity::Danger, "safe_config_change", "safe_set_fallback_handler", [("handler", a(&c.handler))]));
    }
    if let Ok(c) = changeMasterCopyCall::abi_decode(input) {
        return Some(risk(Severity::Danger, "safe_config_change", "safe_change_implementation", [("implementation", a(&c._masterCopy))]));
    }
    None
}

/// Decodes and checks what a SafeTx would do when executed by the Safe at `safe`.
pub fn analyze_safe_tx(chain_id: u64, safe: Address, tx: &SafeTx) -> SafeTxAnalysis {
    let mut risks = Vec::new();
    let mut batch = None;
    let mut entries = vec![(tx.operation, tx.to, tx.value, tx.data.clone())];

    if tx.operation == 1 {
        match delegatecall_library(&tx.to) {
            Some(lib) if lib.starts_with("MultiSend") => match unpack_multisend(&tx.data) {
                Some(list) => {
                    risks.push(risk(Severity::Info, "safe_batch", "safe_batch", [("count", list.len().to_string()), ("library", lib.to_string())]));
                    batch = Some(lib.to_string());
                    entries = list;
                }
                None => risks.push(risk(Severity::Danger, "safe_malformed_batch", "safe_malformed_batch", [("library", lib.to_string())])),
            },
            Some(lib) => risks.push(risk(Severity::Info, "safe_library", "safe_library", [("library", lib.to_string())])),
            None => {} // Flagged with the entries below.
        }
    } else if tx.operation > 1 {
        risks.push(risk(Severity::Danger, "safe_invalid_operation", "safe_invalid_operation", [("operation", tx.operation.to_string())]));
    }

    let numbered = entries.len() > 1;
    let mut calls = Vec::new();
    for (i, (operation, to, value, data)) in entries.into_iter().enumerate() {
        let prefix = |r: Risk| if numbered { r.prefixed(Text::new("prefix.call", [("n", (i + 1).to_string())])) } else { r };
        let is_top_level_library = batch.is_none() && operation == 1 && delegatecall_library(&to).is_some();
        if operation == 1 && !is_top_level_library {
            risks.push(prefix(risk(Severity::Danger, "safe_delegatecall", "safe_delegatecall", [("target", to.to_checksum(None))])));
        }
        if to == safe && operation == 0 {
            match self_call_risk(&data) {
                Some(r) => risks.push(prefix(r)),
                None if !data.is_empty() => {
                    risks.push(prefix(risk(Severity::Warning, "safe_self_call", "safe_self_call", [])));
                }
                None => {}
            }
        }
        let inner = analyze(&TxFields { from: Some(safe), to: Some(to), value, input: data.clone(), ..Default::default() }, &[], chain_id);
        risks.extend(inner.risks.into_iter().map(prefix));
        calls.push(InnerCall { operation, to, value, data, call: inner.call, summary: inner.summary });
    }

    if !tx.gas_price.is_zero() {
        let in_token = if tx.gas_token.is_zero() { "eth" } else { "token" };
        let to = if tx.refund_receiver.is_zero() { "executor" } else { "receiver" };
        risks.push(risk(
            Severity::Warning,
            "safe_gas_refund",
            &format!("safe_gas_refund_{in_token}_{to}"),
            [
                ("token", short(&tx.gas_token)),
                ("receiver", short(&tx.refund_receiver)),
                ("gas_price", tx.gas_price.to_string()),
                ("base_gas", tx.base_gas.to_string()),
                ("native", crate::providers::native_symbol(chain_id).to_string()),
            ],
        ));
    }

    let summary = match calls.as_slice() {
        [c] if batch.is_none() => {
            let op = if c.operation == 1 { "DELEGATECALL " } else { "" };
            format!("Safe {} → {op}{}", short(&safe), c.summary)
        }
        list => {
            let names: Vec<String> = list
                .iter()
                .map(|c| c.call.function.clone().or_else(|| c.call.selector.clone()).unwrap_or_else(|| format!("{} {}", format_units(c.value, 18), crate::providers::native_symbol(chain_id))))
                .collect();
            format!("Safe {} → batch of {}: {}", short(&safe), list.len(), names.join(", "))
        }
    };
    SafeTxAnalysis { calls, batch, risks, summary }
}

/// The `safeTxHash` argument of `approveHash(bytes32)`, if `input` is that call.
pub fn approve_hash_arg(input: &[u8]) -> Option<B256> {
    approveHashCall::abi_decode(input).ok().map(|c| c.hashToApprove)
}

/// Whether `input` calls `execTransaction`.
pub fn is_exec_transaction(input: &[u8]) -> bool {
    input.starts_with(&execTransactionCall::SELECTOR)
}

// ---------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureKind {
    /// ECDSA over the EIP-712 hash (`eth_signTypedData`).
    Eip712,
    /// ECDSA over the EIP-191 prefixed hash (`eth_sign` / `personal_sign`).
    EthSign,
    /// `approveHash` on-chain, or the executor itself is the owner.
    ApprovedHash,
    /// EIP-1271 signature of a contract owner; not verified by TxGate.
    Contract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeSignature {
    pub kind: SignatureKind,
    /// The owner this signature counts for, when it can be determined.
    pub signer: Option<Address>,
    pub error: Option<String>,
}

/// Splits the `signatures` argument of `execTransaction` and recovers each
/// signer against `safe_tx_hash`.
pub fn parse_signatures(sigs: &[u8], safe_tx_hash: B256) -> Vec<SafeSignature> {
    // Contract signatures keep their data after the 65-byte entries; their
    // offsets tell where the entries end.
    let mut entries_end = sigs.len();
    let mut out = Vec::new();
    let mut i = 0;
    while (i + 1) * 65 <= entries_end {
        let e = &sigs[i * 65..(i + 1) * 65];
        let r = &e[..32];
        let s = &e[32..64];
        let v = e[64];
        let as_address = Address::from_slice(&r[12..]);
        let sig = match v {
            0 => {
                let offset = U256::from_be_slice(s).saturating_to::<usize>();
                if offset >= (i + 1) * 65 {
                    entries_end = entries_end.min(offset);
                }
                SafeSignature { kind: SignatureKind::Contract, signer: Some(as_address), error: None }
            }
            1 => SafeSignature { kind: SignatureKind::ApprovedHash, signer: Some(as_address), error: None },
            _ => {
                let (kind, v) = if v > 30 { (SignatureKind::EthSign, v - 4) } else { (SignatureKind::Eip712, v) };
                let mut raw = [0u8; 65];
                raw[..64].copy_from_slice(&e[..64]);
                raw[64] = v;
                let recovered = alloy_primitives::Signature::from_raw_array(&raw).and_then(|sig| match kind {
                    SignatureKind::EthSign => sig.recover_address_from_msg(safe_tx_hash.as_slice()),
                    _ => sig.recover_address_from_prehash(&safe_tx_hash),
                });
                match recovered {
                    Ok(a) => SafeSignature { kind, signer: Some(a), error: None },
                    Err(err) => SafeSignature { kind, signer: None, error: Some(err.to_string()) },
                }
            }
        };
        out.push(sig);
        i += 1;
    }
    out
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! A real mainnet Safe transaction (Safe v1.3.0, nonce 2867), as returned by
    //! the Safe Transaction Service and executed on-chain.

    use super::*;
    use alloy_primitives::{address, b256, bytes};

    pub const SAFE: Address = address!("849D52316331967b6fF1198e5E32A0eB168D039d");
    pub const SAFE_TX_HASH: B256 = b256!("a15d670d17621751bb88f493f8b305c5bde4eeaa7aa4f9875da4d9bd2fad39b0");
    pub const MULTI_SEND_CALL_ONLY: Address = address!("40A2aCCbd92BCA938b02010E17A5b8929b49130D");
    pub const OWNERS: [Address; 6] = [
        address!("964aE5aB627f1d536d5d5796734B7a44AD6531E3"),
        address!("eD5Ddb58F3131274597EE59c98A073ce3C5d1cb9"),
        address!("1B0C638616Ed79dB430Edbf549ad9512FF4a8ed1"),
        address!("4e288c2aBC77a4D7c5583cdC9792731220a088cF"),
        address!("baB04858C4eBBb226DD12B38639e59450820536F"),
        address!("e9eB7DA58f6B5CE5b0a6cFD778A2fa726203AAD5"),
    ];
    /// The executor; its own signature is the "approved by sender" kind.
    pub const EXECUTOR: Address = address!("eD5Ddb58F3131274597EE59c98A073ce3C5d1cb9");

    pub fn tx() -> SafeTx {
        SafeTx {
            to: MULTI_SEND_CALL_ONLY,
            value: U256::ZERO,
            data: bytes!("8d80ff0a00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000112000a7cb434f96f65972d46a5c1a64a9654dc9959b200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000024aa9224cd0000000000000000000000000000000000000000000000000000000000000000005afe3855358e112b5647b952709e6165e1c1eeee00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000044a9059cbb000000000000000000000000458cd345b4c05e8df39d0a07220feb4ec19f5e6f0000000000000000000000000000000000000000002042923c813833870000000000000000000000000000000000"),
            operation: 1,
            nonce: U256::from(2867),
            ..Default::default()
        }
    }

    pub fn exec_calldata(t: &SafeTx, signatures: &Bytes) -> Bytes {
        execTransactionCall {
            to: t.to,
            value: t.value,
            data: t.data.clone(),
            operation: t.operation,
            safeTxGas: t.safe_tx_gas,
            baseGas: t.base_gas,
            gasPrice: t.gas_price,
            gasToken: t.gas_token,
            refundReceiver: t.refund_receiver,
            signatures: signatures.clone(),
        }
        .abi_encode()
        .into()
    }

    pub fn signatures() -> Bytes {
        bytes!("37b9079718c3e23fccf78990c8b27bae9779348026a97ac5a37a89f0164de9aa3926ae0f500c351808be62a838d6f54b1fa7e4072bdbc6ca15e7f25f068c171d1ba51db4a84de84081833627336ff5d1b49a4acc0f60164ab705cd71f1ff672e7367b50680602f230ca982ca95050142211a662f52c4aa8890c5e737459ba6a5281c000000000000000000000000ed5ddb58f3131274597ee59c98a073ce3c5d1cb9000000000000000000000000000000000000000000000000000000000000000001")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn computes_the_safe_tx_hash_of_a_real_transaction() {
        let h = safe_tx_hashes(1, SAFE, Some("1.3.0"), &tx());
        assert_eq!(h.safe_tx_hash, SAFE_TX_HASH);
    }

    #[test]
    fn legacy_safes_hash_without_a_chain_id() {
        // Expected values from getTransactionHash() and domainSeparator() of the
        // real Safe 1.1.1 at 0x78605Df79524164911C144801f41e9811B7DB73D on mainnet.
        let tx = SafeTx {
            to: MULTI_SEND_CALL_ONLY,
            value: U256::from(123),
            data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            operation: 1,
            safe_tx_gas: U256::from(7),
            base_gas: U256::from(8),
            gas_price: U256::from(9),
            gas_token: Address::ZERO,
            refund_receiver: Address::with_last_byte(1),
            nonce: U256::from(42),
        };
        let h = safe_tx_hashes(1, address!("78605Df79524164911C144801f41e9811B7DB73D"), Some("1.1.1"), &tx);
        assert_eq!(h.domain_hash, alloy_primitives::b256!("86d4a49a7f13091d2c7b763df034af5fc8df3ff3a541e0b74187bec88256c1fb"));
        assert_eq!(h.safe_tx_hash, alloy_primitives::b256!("9fe29328d023532af34b1d0be0f7dfa2ca388afd25d5b8e56d958bb7ca5cd657"));
    }

    #[test]
    fn recovers_the_owners_who_signed() {
        let sigs = parse_signatures(&signatures(), SAFE_TX_HASH);
        let found: Vec<_> = sigs.iter().map(|s| (s.kind, s.signer)).collect();
        assert_eq!(
            found,
            vec![
                (SignatureKind::Eip712, Some(OWNERS[3])),
                (SignatureKind::Eip712, Some(OWNERS[0])),
                (SignatureKind::ApprovedHash, Some(EXECUTOR)),
            ]
        );
    }

    fn codes(risks: &[Risk]) -> Vec<&str> {
        risks.iter().map(|r| r.code.as_str()).collect()
    }

    #[test]
    fn gas_refunds_are_named_in_the_chain_currency() {
        let tx = SafeTx { to: OWNERS[0], gas_price: U256::from(1), ..Default::default() };
        let refund = |chain| analyze_safe_tx(chain, SAFE, &tx).risks.into_iter().find(|r| r.code == "safe_gas_refund").unwrap().message;
        assert!(refund(56).contains("in BNB"), "{}", refund(56));
        assert!(refund(1).contains("in ETH"), "{}", refund(1));
    }

    #[test]
    fn decodes_exec_transaction_and_unpacks_the_batch() {
        let input = exec_calldata(&tx(), &signatures());
        let exec = analyze_exec(1, SAFE, &input).expect("is execTransaction");
        assert_eq!(exec.tx.to, MULTI_SEND_CALL_ONLY);
        assert_eq!(exec.tx.operation, 1);
        assert_eq!(exec.signatures, signatures());
        // withdraw(0) on one contract, then transfer() of SAFE tokens.
        let fns: Vec<_> = exec.analysis.calls.iter().map(|c| c.call.selector.clone().unwrap()).collect();
        assert_eq!(fns, vec!["0xaa9224cd", "0xa9059cbb"]);
        assert_eq!(exec.analysis.calls[1].call.function.as_deref(), Some("transfer"));
        // A delegatecall into the official MultiSendCallOnly is normal batching.
        assert!(codes(&exec.analysis.risks).contains(&"safe_batch"));
        assert!(!codes(&exec.analysis.risks).contains(&"safe_delegatecall"));
    }

    #[test]
    fn delegatecall_to_an_unknown_contract_is_dangerous() {
        // The Bybit pattern: a "transfer" delegatecalled into an attacker contract.
        let t = SafeTx { to: Address::repeat_byte(0x96), operation: 1, data: Bytes::from_static(&[0xa9, 0x05, 0x9c, 0xbb]), ..Default::default() };
        let a = analyze_safe_tx(1, SAFE, &t);
        let r = a.risks.iter().find(|r| r.code == "safe_delegatecall").expect("flagged");
        assert_eq!(r.severity, Severity::Danger);
    }

    #[test]
    fn delegatecall_inside_a_batch_is_dangerous() {
        let mut packed = vec![1u8];
        packed.extend_from_slice(Address::repeat_byte(0x96).as_slice());
        packed.extend_from_slice(&[0u8; 32]);
        packed.extend_from_slice(&U256::ZERO.to_be_bytes::<32>());
        let batch = multiSendCall { transactions: packed.into() }.abi_encode();
        let multi_send = address!("38869bf66a61cF6bDB996A6aE40D5853Fd43B526");
        let t = SafeTx { to: multi_send, operation: 1, data: batch.into(), ..Default::default() };
        let a = analyze_safe_tx(1, SAFE, &t);
        assert_eq!(a.calls.len(), 1);
        assert!(codes(&a.risks).contains(&"safe_delegatecall"), "{:?}", a.risks);
    }

    #[test]
    fn calls_inside_the_safe_get_the_usual_checks() {
        let approve = hex::decode(format!("095ea7b3{:0>64}{}", hex::encode(Address::repeat_byte(0x55)), "f".repeat(64))).unwrap();
        let t = SafeTx { to: Address::repeat_byte(0x11), data: approve.into(), ..Default::default() };
        assert!(codes(&analyze_safe_tx(1, SAFE, &t).risks).contains(&"unlimited_approval"));
    }

    #[test]
    fn enabling_a_module_is_dangerous() {
        let data = enableModuleCall { module: Address::repeat_byte(0x77) }.abi_encode();
        let t = SafeTx { to: SAFE, data: data.into(), ..Default::default() };
        let a = analyze_safe_tx(1, SAFE, &t);
        let r = a.risks.iter().find(|r| r.code == "safe_config_change").expect("flagged");
        assert_eq!(r.severity, Severity::Danger);
    }

    #[test]
    fn gas_refunds_are_flagged() {
        let t = SafeTx { to: Address::repeat_byte(0x11), gas_price: U256::from(1), ..Default::default() };
        assert!(codes(&analyze_safe_tx(1, SAFE, &t).risks).contains(&"safe_gas_refund"));
    }

    #[test]
    fn signatures_over_another_hash_recover_to_strangers() {
        let sigs = parse_signatures(&signatures(), B256::repeat_byte(7));
        assert!(!OWNERS.contains(&sigs[0].signer.unwrap()));
    }
}
