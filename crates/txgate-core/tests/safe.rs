//! Safe multisig support through the engine: watching a Safe and reviewing a
//! SafeTx before signing it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use alloy_primitives::{address, Address, U256};
use alloy_sol_types::SolValue;
use serde_json::json;
use txgate_core::config::{Config, NetworkConfig, RpcSource, WatchedSafe};
use txgate_core::decode::Severity;
use txgate_core::engine::{Engine, Event};
use txgate_core::store::Store;
use txgate_core::upstream::fake::FakeUpstream;

const SAFE: Address = address!("849D52316331967b6fF1198e5E32A0eB168D039d");
const SINGLETON_1_3_0: Address = address!("d9Db270c1B5E3Bd161E8c8503c55cEABeE709552");

/// A chain with one Safe whose threshold the test can change.
fn chain(threshold: Arc<AtomicU64>) -> FakeUpstream {
    let up = FakeUpstream::new();
    let owners = vec![Address::repeat_byte(0xa), Address::repeat_byte(0xb), Address::repeat_byte(0xc)];
    up.on_fn("eth_call", move |req| {
        let data = req["params"][0]["data"].as_str().unwrap().to_string();
        let ret = match &data[..10] {
            "0xffa1ad74" => ("1.3.0".to_string(),).abi_encode_params(),
            "0xaffed0e0" => U256::from(10).abi_encode(),
            "0xe75235b8" => U256::from(threshold.load(Ordering::SeqCst)).abi_encode(),
            "0xa0e67e2b" => (owners.clone(),).abi_encode_params(),
            "0xcc2f8452" => (Vec::<Address>::new(), Address::with_last_byte(1)).abi_encode_params(),
            _ => vec![],
        };
        Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":format!("0x{}", hex::encode(ret))}))
    });
    up.on_fn("eth_getStorageAt", move |req| {
        let slot = req["params"][1].as_str().unwrap();
        let v = if slot == "0x0" { format!("0x{:0>64}", hex::encode(SINGLETON_1_3_0)) } else { "0x0".into() };
        Ok(json!({"jsonrpc":"2.0","id":req["id"],"result":v}))
    });
    up
}

fn engine(up: &FakeUpstream) -> Engine {
    let mut config = Config {
        networks: vec![NetworkConfig {
            chain_id: 1,
            name: "Mainnet".into(),
            read: RpcSource::Custom { url: "https://read.example".into() },
            broadcast: None,
        }],
        ..Config::default()
    };
    config.safe.watched = vec![WatchedSafe { chain_id: 1, address: SAFE, label: "Treasury".into() }];
    config.safe.service_enabled = false;
    Engine::new(config, None, Store::open_in_memory().unwrap(), Arc::new(up.clone())).unwrap()
}

#[tokio::test]
async fn watching_a_safe_alerts_when_its_configuration_changes() {
    let threshold = Arc::new(AtomicU64::new(2));
    let up = chain(threshold.clone());
    let engine = engine(&up);
    let mut events = engine.subscribe();

    let status = engine.poll_safes().await;
    assert_eq!(status[0].state.as_ref().unwrap().threshold, 2);
    assert!(engine.safe_alerts(10).unwrap().is_empty(), "the first check is the baseline");

    threshold.store(1, Ordering::SeqCst);
    engine.poll_safes().await;
    let alerts = engine.safe_alerts(10).unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].severity, Severity::Danger);
    assert!(alerts[0].message.contains("Threshold changed from 2 to 1"));
    assert_eq!(engine.safe_watch()[0].state.as_ref().unwrap().threshold, 1);

    let mut saw_alert = false;
    while let Ok(e) = events.try_recv() {
        saw_alert |= matches!(e, Event::SafeAlert(_));
    }
    assert!(saw_alert);
}

#[tokio::test]
async fn watching_a_safe_again_starts_from_a_fresh_baseline() {
    let threshold = Arc::new(AtomicU64::new(2));
    let up = chain(threshold.clone());
    let engine = engine(&up);
    engine.poll_safes().await;

    let watched = engine.config().safe.watched;
    engine.update_config(|c| c.safe.watched.clear()).unwrap();
    engine.poll_safes().await;
    threshold.store(1, Ordering::SeqCst); // changed while nobody was watching
    engine.update_config(|c| c.safe.watched = watched).unwrap();
    engine.poll_safes().await;
    assert!(engine.safe_alerts(10).unwrap().is_empty(), "{:?}", engine.safe_alerts(10).unwrap());
}

#[tokio::test]
async fn reviews_a_safe_transaction_on_a_configured_network() {
    let up = chain(Arc::new(AtomicU64::new(2)));
    let engine = engine(&up);
    let message = json!({"to": Address::repeat_byte(0x99).to_string(), "value": "0", "data": "0x", "operation": 1, "nonce": 10});
    let report = engine.review_safe_tx(1, SAFE, json!({"message": message, "domain": {"chainId": 1, "verifyingContract": SAFE.to_string()}})).await.unwrap();
    assert!(report.risks.iter().any(|r| r.code == "safe_delegatecall"));
    assert_eq!(report.state.unwrap().nonce, U256::from(10));

    assert!(engine.review_safe_tx(137, SAFE, message).await.is_err(), "chain 137 is not configured");
}

#[tokio::test]
async fn a_queue_entry_of_another_safe_is_flagged() {
    let up = chain(Arc::new(AtomicU64::new(2)));
    let mut engine_config = engine(&up).config();
    engine_config.safe.service_enabled = true;
    let engine = Engine::new(engine_config, None, Store::open_in_memory().unwrap(), Arc::new(up.clone())).unwrap();
    let other = Address::repeat_byte(0x77);
    up.on_get(
        "https://api.safe.global/tx-service/eth/api/v2/safes/",
        json!({"results": [{
            "safe": other.to_checksum(None), "to": other.to_checksum(None), "value": "0", "data": null, "operation": 0,
            "safeTxGas": "0", "baseGas": "0", "gasPrice": "0",
            "gasToken": "0x0000000000000000000000000000000000000000", "refundReceiver": "0x0000000000000000000000000000000000000000",
            "nonce": "10", "safeTxHash": format!("0x{}", "11".repeat(32)), "confirmations": []
        }]}),
    );
    let queue = engine.safe_queue(1, SAFE).await.unwrap();
    assert!(queue[0].risks.iter().any(|r| r.code == "safe_domain_mismatch" && r.severity == Severity::Danger), "{:?}", queue[0].risks);
}

/// Against the real mainnet and Safe Transaction Service:
/// `cargo test -p txgate-core --test safe -- --ignored`
#[tokio::test]
#[ignore = "needs network access"]
async fn live_mainnet_safe_is_read_and_its_queue_verified() {
    let mut config = Config::default();
    // Flashbots Protect does not serve eth_call; read from a public node.
    config.networks[0].read = RpcSource::Custom { url: "https://ethereum-rpc.publicnode.com".into() };
    config.safe.watched = vec![WatchedSafe { chain_id: 1, address: SAFE, label: "Safe{DAO}".into() }];
    let engine = Engine::new(config, None, Store::open_in_memory().unwrap(), Arc::new(txgate_core::upstream::HttpUpstream::new())).unwrap();
    let status = engine.poll_safes().await.remove(0);
    let state = status.state.unwrap_or_else(|| panic!("{:?}", status.error));
    assert_eq!(state.singleton_name.as_deref(), Some("Safe 1.3.0"));
    assert!(state.threshold >= 1 && !state.owners.is_empty());
    for r in &status.queue {
        assert!(r.risks.iter().all(|r| r.code != "safe_service_hash_mismatch"), "{:?}", r.risks);
        println!("{} {} {:?}", r.tx.nonce, r.analysis.summary, r.risks.iter().map(|r| &r.code).collect::<Vec<_>>());
    }
    println!("{state:?} service: {:?}", status.service_error);
}
