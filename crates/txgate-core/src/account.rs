//! Smart accounts (ERC-4337 / ERC-7579): the calls an account is asked to make.
//!
//! A UserOperation's `callData` (or an owner's direct call) usually wraps the
//! real calls in the account's `execute` function. Unwrapping them lets every
//! inner call get the usual checks; otherwise an unlimited approval inside a
//! batch would pass unnoticed. The interfaces below are the ones seen on
//! mainnet EntryPoint v0.7 traffic.

use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{sol, SolCall};

use crate::decode::{analyze, short, Risk, Severity, TxFields};
use crate::safe::{analyze_safe_tx, InnerCall, SafeTx, SafeTxAnalysis};
use crate::text::Text;

mod simple {
    alloy_sol_types::sol! {
        /// SimpleAccount, Coinbase Smart Wallet, Biconomy and others.
        function execute(address dest, uint256 value, bytes func);
    }
}

mod batch {
    alloy_sol_types::sol! {
        struct Call { address target; uint256 value; bytes data; }
        function executeBatch(Call[] calls);
        function execute4337Ops(Call[] calls);
    }
}

mod batch_arrays {
    alloy_sol_types::sol! {
        function executeBatch(address[] dest, uint256[] value, bytes[] func);
    }
}

mod batch_no_value {
    alloy_sol_types::sol! {
        /// SimpleAccount for EntryPoint v0.6.
        function executeBatch(address[] dest, bytes[] func);
    }
}

mod safe_module {
    alloy_sol_types::sol! {
        /// Safe4337Module: the Safe executes `data` with `operation` (1 = DELEGATECALL).
        function executeUserOp(address to, uint256 value, bytes data, uint8 operation);
        function executeUserOpWithErrorString(address to, uint256 value, bytes data, uint8 operation);
    }
}

sol! {
    /// ERC-7579 (Kernel v3, Nexus, Safe7579, …).
    function execute(bytes32 mode, bytes executionCalldata);
    struct Execution { address target; uint256 value; bytes callData; }
}

type Entry = (u8, Address, U256, Bytes);

/// ERC-7579 `executionCalldata` for the call type in `mode[0]`.
fn erc7579_calls(mode: &[u8; 32], data: &[u8]) -> Option<Vec<Entry>> {
    match mode[0] {
        // Single: abi.encodePacked(target, value, callData).
        0x00 if data.len() >= 52 => {
            Some(vec![(0, Address::from_slice(&data[..20]), U256::from_be_slice(&data[20..52]), Bytes::copy_from_slice(&data[52..]))])
        }
        // Batch: abi.encode(Execution[]).
        0x01 => {
            let list = <alloy_sol_types::sol_data::Array<Execution> as alloy_sol_types::SolType>::abi_decode(data).ok()?;
            Some(list.into_iter().map(|e| (0, e.target, e.value, e.callData)).collect())
        }
        // Delegatecall: abi.encodePacked(target, callData).
        0xff if data.len() >= 20 => Some(vec![(1, Address::from_slice(&data[..20]), U256::ZERO, Bytes::copy_from_slice(&data[20..]))]),
        _ => None,
    }
}

/// The calls `input` asks a smart account to make, with the interface name.
fn unwrap(input: &[u8]) -> Option<(Vec<Entry>, &'static str)> {
    if let Ok(c) = simple::executeCall::abi_decode(input) {
        return Some((vec![(0, c.dest, c.value, c.func)], "execute"));
    }
    if let Ok(c) = batch::executeBatchCall::abi_decode(input) {
        return Some((c.calls.into_iter().map(|c| (0, c.target, c.value, c.data)).collect(), "executeBatch"));
    }
    if let Ok(c) = batch::execute4337OpsCall::abi_decode(input) {
        return Some((c.calls.into_iter().map(|c| (0, c.target, c.value, c.data)).collect(), "execute4337Ops"));
    }
    if let Ok(c) = batch_arrays::executeBatchCall::abi_decode(input)
        && c.dest.len() == c.value.len()
        && c.dest.len() == c.func.len()
    {
        let calls = c.dest.into_iter().zip(c.value).zip(c.func).map(|((to, v), d)| (0, to, v, d)).collect();
        return Some((calls, "executeBatch"));
    }
    if let Ok(c) = batch_no_value::executeBatchCall::abi_decode(input)
        && c.dest.len() == c.func.len()
    {
        return Some((c.dest.into_iter().zip(c.func).map(|(to, d)| (0, to, U256::ZERO, d)).collect(), "executeBatch"));
    }
    if let Ok(c) = executeCall::abi_decode(input) {
        return erc7579_calls(&c.mode.0, &c.executionCalldata).map(|calls| (calls, "execute"));
    }
    None
}

/// Decodes a call to a smart account at `account` and checks every call it
/// would make. `None` when `input` is not a known smart-account interface.
pub fn analyze_account_call(chain_id: u64, account: Address, input: &[u8]) -> Option<SafeTxAnalysis> {
    // The Safe4337Module runs the call as a Safe transaction: reuse the Safe
    // checks (official MultiSend batches, library allowlist).
    let safe_op = safe_module::executeUserOpCall::abi_decode(input)
        .map(|c| (c.to, c.value, c.data, c.operation))
        .or_else(|_| safe_module::executeUserOpWithErrorStringCall::abi_decode(input).map(|c| (c.to, c.value, c.data, c.operation)));
    if let Ok((to, value, data, operation)) = safe_op {
        let tx = SafeTx { to, value, data, operation, ..Default::default() };
        return Some(analyze_safe_tx(chain_id, account, &tx));
    }

    let (entries, function) = unwrap(input)?;
    let numbered = entries.len() > 1;
    let mut risks = Vec::new();
    let mut calls = Vec::new();
    for (i, (operation, to, value, data)) in entries.into_iter().enumerate() {
        let prefix = |r: Risk| if numbered { r.prefixed(Text::new("prefix.call", [("n", (i + 1).to_string())])) } else { r };
        if operation == 1 {
            risks.push(prefix(Risk::t(Severity::Danger, "account_delegatecall", "account_delegatecall", [("target", to.to_checksum(None))])));
        }
        let inner = analyze(&TxFields { from: Some(account), to: Some(to), value, input: data.clone(), ..Default::default() }, &[], chain_id);
        risks.extend(inner.risks.into_iter().map(prefix));
        calls.push(InnerCall { operation, to, value, data, call: inner.call, summary: inner.summary });
    }
    let summary = match calls.as_slice() {
        [c] => format!("{} → {}", short(&account), c.summary),
        list => {
            let names: Vec<String> = list.iter().map(|c| c.call.function.clone().or_else(|| c.call.selector.clone()).unwrap_or_else(|| crate::providers::native_symbol(chain_id).into())).collect();
            format!("{} → {function} of {}: {}", short(&account), list.len(), names.join(", "))
        }
    };
    let batch = numbered.then(|| function.to_string());
    Some(SafeTxAnalysis { calls, batch, risks, summary })
}
