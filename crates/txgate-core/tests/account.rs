//! Smart-account calls are unwrapped so the calls inside get the usual checks.
//! Fixtures are the callData of real mainnet UserOperations (EntryPoint v0.7),
//! one per account interface; expected values were decoded with `cast`.

use alloy_primitives::{address, Address, Bytes};
use txgate_core::decode::{analyze, Severity, TxFields};

fn fixture(selector: &str) -> Bytes {
    let path = format!("{}/tests/fixtures/account/{selector}.hex", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
}

fn as_account(account: Address, selector: &str) -> txgate_core::decode::Analysis {
    analyze(&TxFields { from: Some(account), to: Some(account), input: fixture(selector), ..Default::default() }, &[], 1)
}

const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

#[test]
fn a_safe_4337_multisend_hides_an_unlimited_approval() {
    // executeUserOpWithErrorString: DELEGATECALL into Safe's MultiSendCallOnly 1.4.1,
    // whose first call is approve(0x5514…a964, 2^256-1).
    let a = as_account(address!("d44cc583c6CBab61b2428eAdD4686aC07BA3A374"), "0x541d63c8");
    let calls = &a.account.as_ref().expect("unwrapped").calls;
    assert!(calls.len() >= 2, "{calls:?}");
    assert!(a.risks.iter().any(|r| r.code == "unlimited_approval" && r.severity == Severity::Danger), "{:?}", a.risks);
    assert!(!a.risks.iter().any(|r| r.code == "safe_delegatecall"), "the official MultiSend is not flagged: {:?}", a.risks);
    // The wrapper itself is understood; only calls inside may be unknown.
    assert!(!a.risks.iter().any(|r| r.code == "unknown_function" && !r.message.starts_with("Call ")), "{:?}", a.risks);
}

#[test]
fn batches_are_unwrapped() {
    // executeBatch((address,uint256,bytes)[]): approve USDC, then call the spender.
    let a = as_account(address!("285E0f5DF3E9aa2826cA6267E1bde0415a04ee3D"), "0x34fcd5be");
    let calls = &a.account.as_ref().unwrap().calls;
    assert_eq!(calls.iter().map(|c| c.to).collect::<Vec<_>>(), [USDC, address!("4cD00E387622C35bDDB9b4c962C136462338BC31")]);
    assert_eq!(calls[0].call.function.as_deref(), Some("approve"));
    assert!(a.risks.iter().any(|r| r.code == "approval" && r.message.starts_with("Call 1: ")), "{:?}", a.risks);

    // execute4337Ops((address,uint256,bytes)[]): a USDC transfer.
    let a = as_account(address!("077C9e08c7FF0d87e8D7A63719F738D7681F52b3"), "0x26da7d88");
    let calls = &a.account.as_ref().unwrap().calls;
    assert_eq!((calls.len(), calls[0].to, calls[0].call.function.as_deref()), (1, USDC, Some("transfer")));
}

#[test]
fn single_calls_are_unwrapped() {
    // execute(address,uint256,bytes): sends 0.0045 ETH, no call data.
    let a = as_account(address!("f7c5a499d3a81d13f191cce06e0c2b64f1ff57e3"), "0xb61d27f6");
    let c = &a.account.as_ref().unwrap().calls[0];
    assert_eq!((c.to, c.value.to_string()), (address!("7106b14387D31E9A288f635A4E8a20cC98532030"), "4543816084586180".to_string()));
    assert!(c.data.is_empty());

    // ERC-7579 execute(bytes32 mode, bytes): single call, packed target|value|data.
    let a = as_account(address!("BADA90b67D30074E9866A168d34CeA69737b928F"), "0xe9ae5c53");
    let c = &a.account.as_ref().unwrap().calls[0];
    assert_eq!((c.to, c.call.function.as_deref()), (address!("5afe3855358e112b5647b952709e6165e1c1eeee"), Some("transfer")));
}

#[test]
fn a_delegatecall_from_a_smart_account_is_dangerous() {
    use alloy_sol_types::{sol, SolCall};
    sol! { function execute(bytes32 mode, bytes executionCalldata); }
    // ERC-7579 call type 0xff: DELEGATECALL, packed target|data.
    let mut mode = [0u8; 32];
    mode[0] = 0xff;
    let target = address!("9e1F000000000000000000000000000000003c07");
    let packed = [target.as_slice(), &[0x12, 0x34, 0x56, 0x78]].concat();
    let input = executeCall { mode: mode.into(), executionCalldata: packed.into() }.abi_encode();
    let account = address!("BADA90b67D30074E9866A168d34CeA69737b928F");
    let a = analyze(&TxFields { to: Some(account), input: input.into(), ..Default::default() }, &[], 1);
    assert!(a.risks.iter().any(|r| r.code == "account_delegatecall" && r.severity == Severity::Danger), "{:?}", a.risks);
}

#[test]
fn deeply_nested_account_calls_do_not_crash_the_analysis() {
    use alloy_sol_types::{sol, SolCall};
    sol! { function execute(address dest, uint256 value, bytes func); }
    // A dApp can wrap execute(execute(execute(...))) thousands of times; the
    // analysis runs on every estimate and must not overflow the stack.
    let account = address!("BADA90b67D30074E9866A168d34CeA69737b928F");
    let mut input: Vec<u8> = vec![0x12, 0x34, 0x56, 0x78];
    for _ in 0..5_000 {
        input = executeCall { dest: account, value: alloy_primitives::U256::ZERO, func: input.into() }.abi_encode();
    }
    let input_len = input.len();
    let a = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024) // the default for spawned threads
        .spawn(move || analyze(&TxFields { to: Some(account), input: input.into(), ..Default::default() }, &[], 1))
        .unwrap()
        .join()
        .expect("analysis must not overflow the stack");
    assert!(a.risks.iter().any(|r| r.code == "nesting_too_deep"), "{} bytes: {:?}", input_len, a.risks.iter().map(|r| &r.code).collect::<Vec<_>>());
}
