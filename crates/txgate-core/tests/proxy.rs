//! End-to-end behaviour of the proxy through its public interface: the engine
//! plus (where connection handling matters) the real HTTP server.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use txgate_core::config::{Config, LogMode, NetworkConfig, RpcSource};
use txgate_core::engine::{Engine, Event};
use txgate_core::preview::PreviewMatch;
use txgate_core::record::{Decision, RequestRecord, Status, TxDetail};
use txgate_core::store::{HistoryQuery, Store};
use txgate_core::upstream::fake::FakeUpstream;

/// A signed mainnet EIP-1559 transaction to 0xd9e1459a7a482635700cbc20bbaf52d495ab9c96.
const RAW_TX: &str = "0x02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8";
const TX_TO: &str = "0xd9e1459a7a482635700cbc20bbaf52d495ab9c96";
const TX_DATA: &str = "0x1b55ba3a";
const READ_URL: &str = "https://read.example";
const SEND_URL: &str = "https://send.example";

fn config() -> Config {
    Config {
        networks: vec![NetworkConfig {
            chain_id: 1,
            name: "Mainnet".into(),
            read: RpcSource::Custom { url: READ_URL.into() },
            broadcast: Some(RpcSource::Custom { url: SEND_URL.into() }),
        }],
        ..Config::default()
    }
}

fn setup() -> (Engine, FakeUpstream) {
    let up = FakeUpstream::new();
    up.on("eth_blockNumber", json!("0x10"));
    up.on("eth_estimateGas", json!("0x5208"));
    up.on("eth_getCode", json!("0x"));
    up.on("eth_getTransactionCount", json!("0x2"));
    up.on_fn("eth_sendRawTransaction", |req| {
        Ok(json!({"jsonrpc": "2.0", "id": req["id"], "result": "0xhash-from-upstream"}))
    });
    let engine = Engine::new(config(), None, Store::open_in_memory().unwrap(), Arc::new(up.clone())).unwrap();
    (engine, up)
}

fn send_raw(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "eth_sendRawTransaction", "params": [RAW_TX]})
}

/// Waits until the engine holds `n` transactions.
async fn wait_pending(engine: &Engine, n: usize) -> Vec<RequestRecord> {
    for _ in 0..200 {
        let p = engine.pending();
        if p.len() == n {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {n} pending, got {}", engine.pending().len());
}

fn detail(r: &RequestRecord) -> TxDetail {
    r.detail.clone().unwrap()
}

#[tokio::test]
async fn read_calls_are_forwarded_to_the_read_upstream() {
    let (engine, up) = setup();
    let resp = engine.handle("1", "127.0.0.1".into(), json!({"jsonrpc":"2.0","id":5,"method":"eth_blockNumber","params":[]})).await;
    assert_eq!(resp["result"], "0x10");
    assert_eq!(up.calls_to("eth_blockNumber")[0].0, READ_URL);
    let recent = engine.recent();
    assert_eq!(recent[0].method, "eth_blockNumber");
    assert_eq!(recent[0].status, Status::Forwarded);
    // Default log mode only persists transaction-related calls.
    assert!(engine.history(&HistoryQuery::default()).unwrap().is_empty());
}

#[tokio::test]
async fn log_mode_all_persists_reads() {
    let (engine, _) = setup();
    let mut c = engine.config();
    c.logging.mode = LogMode::All;
    engine.set_config(c).unwrap();
    engine.handle("1", "x".into(), json!({"id":1,"method":"eth_blockNumber"})).await;
    assert_eq!(engine.history(&HistoryQuery::default()).unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_chain_is_refused_and_reported() {
    let (engine, up) = setup();
    let mut events = engine.subscribe();
    let resp = engine.handle("137", "127.0.0.1".into(), json!({"id":1,"method":"eth_chainId"})).await;
    assert!(resp["error"]["message"].as_str().unwrap().contains("137"));
    assert!(up.calls().is_empty());
    let Event::Unroutable { chain_id: 137, message, .. } = events.recv().await.unwrap() else { panic!("expected Unroutable") };
    assert!(message.contains("not configured"), "{message}");
}

#[tokio::test]
async fn a_configured_chain_with_a_missing_key_is_reported_as_such() {
    let (engine, _) = setup();
    let mut c = engine.config();
    c.networks.push(NetworkConfig {
        chain_id: 8453,
        name: "Base".into(),
        read: RpcSource::Provider { provider: txgate_core::providers::Provider::Alchemy },
        broadcast: None,
    });
    engine.set_config(c).unwrap();
    let mut events = engine.subscribe();
    engine.handle("8453", "w".into(), json!({"id":1,"method":"eth_chainId"})).await;
    let Event::Unroutable { message, .. } = events.recv().await.unwrap() else { panic!("expected Unroutable") };
    assert!(message.contains("no API key saved for Alchemy"), "{message}");
}

#[tokio::test]
async fn unsigned_transactions_always_need_a_fresh_approval() {
    // Without a nonce the same eth_sendTransaction can be replayed verbatim,
    // so approving it once must not approve every future copy.
    let (engine, up) = setup();
    up.on("eth_sendTransaction", json!("0xok"));
    let send = json!({"jsonrpc":"2.0","id":1,"method":"eth_sendTransaction","params":[{"from": "0x3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE", "to": TX_TO, "value": "0xde0b6b3a7640000"}]});

    let e = engine.clone();
    let s = send.clone();
    let first = tokio::spawn(async move { e.handle("1", "w".into(), s).await });
    let id = wait_pending(&engine, 1).await[0].id;
    engine.decide(id, Decision::Approved).await.unwrap();
    first.await.unwrap();

    let e = engine.clone();
    let second = tokio::spawn(async move { e.handle("1", "w".into(), send).await });
    let held = wait_pending(&engine, 1).await;
    assert_eq!(held[0].status, Status::Pending);
    assert_eq!(up.calls_to("eth_sendTransaction").len(), 1, "the copy must not be broadcast without approval");
    engine.decide(held[0].id, Decision::Rejected).await.unwrap();
    assert_eq!(second.await.unwrap()["error"]["code"], 4001);
}

#[tokio::test]
async fn send_is_held_until_approved_then_broadcast() {
    let (engine, up) = setup();
    let e = engine.clone();
    let wallet = tokio::spawn(async move { e.handle("1", "127.0.0.1".into(), send_raw(9)).await });

    let pending = wait_pending(&engine, 1).await;
    assert!(up.calls_to("eth_sendRawTransaction").is_empty(), "must not reach any upstream before approval");
    assert_eq!(pending[0].status, Status::Pending);
    assert_eq!(pending[0].to.as_deref().map(str::to_lowercase).as_deref(), Some(TX_TO));

    let record = engine.decide(pending[0].id, Decision::Approved).await.unwrap();
    assert_eq!(record.status, Status::Approved);
    let resp = wallet.await.unwrap();
    assert_eq!(resp["result"], "0xhash-from-upstream");
    assert_eq!(resp["id"], 9);
    assert_eq!(up.calls_to("eth_sendRawTransaction")[0].0, SEND_URL);
    assert!(engine.pending().is_empty());
}

#[tokio::test]
async fn refused_broadcast_reaches_the_wallet_and_is_recorded() {
    let (engine, up) = setup();
    up.on_fn("eth_sendRawTransaction", |req| {
        Ok(json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32000, "message": "nonce too low"}}))
    });
    let e = engine.clone();
    let wallet = tokio::spawn(async move { e.handle("1", "127.0.0.1".into(), send_raw(4)).await });

    let id = wait_pending(&engine, 1).await[0].id;
    let record = engine.decide(id, Decision::Approved).await.unwrap();
    assert_eq!(record.status, Status::BroadcastFailed);
    let resp = wallet.await.unwrap();
    assert_eq!(resp["error"]["message"], "nonce too low");
    assert_eq!(resp["id"], 4);
    assert_eq!(engine.history(&HistoryQuery::default()).unwrap()[0].status, Status::BroadcastFailed);
}

#[tokio::test]
async fn rejected_send_returns_4001_and_is_never_broadcast() {
    let (engine, up) = setup();
    let e = engine.clone();
    let wallet = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(3)).await });
    let pending = wait_pending(&engine, 1).await;
    engine.decide(pending[0].id, Decision::Rejected).await.unwrap();
    let resp = wallet.await.unwrap();
    assert_eq!(resp["error"]["code"], 4001);
    assert_eq!(resp["id"], 3);
    assert!(up.calls_to("eth_sendRawTransaction").is_empty());

    // A rejected transaction sent again asks again.
    let e = engine.clone();
    let _again = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(4)).await });
    wait_pending(&engine, 1).await;
}

#[tokio::test]
async fn previously_approved_transaction_is_auto_approved() {
    let (engine, up) = setup();
    let e = engine.clone();
    let first = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(1)).await });
    let pending = wait_pending(&engine, 1).await;
    engine.decide(pending[0].id, Decision::Approved).await.unwrap();
    first.await.unwrap();

    let resp = engine.handle("1", "w".into(), send_raw(2)).await;
    assert_eq!(resp["result"], "0xhash-from-upstream");
    assert_eq!(up.calls_to("eth_sendRawTransaction").len(), 2);
    assert_eq!(engine.recent()[0].status, Status::AutoApproved);
}

#[tokio::test]
async fn duplicate_submissions_join_one_held_transaction() {
    let (engine, _) = setup();
    let e1 = engine.clone();
    let e2 = engine.clone();
    let a = tokio::spawn(async move { e1.handle("1", "w".into(), send_raw(1)).await });
    wait_pending(&engine, 1).await;
    let b = tokio::spawn(async move { e2.handle("1", "w".into(), send_raw(2)).await });
    for _ in 0..200 {
        if detail(&engine.pending()[0]).attempts == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let held = engine.pending();
    assert_eq!(held.len(), 1);
    assert_eq!(detail(&held[0]).wallet_connections, 2);

    engine.decide(held[0].id, Decision::Rejected).await.unwrap();
    assert_eq!(a.await.unwrap()["id"], 1);
    assert_eq!(b.await.unwrap()["id"], 2);
}

#[tokio::test]
async fn batch_answers_reads_and_waits_for_the_send() {
    let (engine, _) = setup();
    let e = engine.clone();
    let batch = json!([
        {"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]},
        send_raw(2),
    ]);
    let wallet = tokio::spawn(async move { e.handle("1", "w".into(), batch).await });
    let pending = wait_pending(&engine, 1).await;
    // The read part was already forwarded while the send waits.
    assert!(engine.recent().iter().any(|r| r.method == "eth_blockNumber" && r.status == Status::Forwarded));
    engine.decide(pending[0].id, Decision::Approved).await.unwrap();
    let resp = wallet.await.unwrap();
    assert_eq!(resp[0]["result"], "0x10");
    assert_eq!(resp[1]["result"], "0xhash-from-upstream");
}

#[tokio::test]
async fn send_is_compared_with_the_preceding_estimate() {
    let (engine, _) = setup();
    let sender = {
        let tx = txgate_core::decode::decode_signed(&hex_bytes(RAW_TX)).unwrap();
        tx.fields.from.unwrap().to_string()
    };
    let estimate = json!({"id":1,"method":"eth_estimateGas","params":[{"from": sender, "to": TX_TO, "data": TX_DATA}]});
    assert_eq!(engine.handle("1", "w".into(), estimate).await["result"], "0x5208");

    let e = engine.clone();
    let _wallet = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(2)).await });
    let pending = wait_pending(&engine, 1).await;
    assert!(matches!(detail(&pending[0]).preview, Some(PreviewMatch::Matched { .. })));

}

#[tokio::test]
async fn send_differing_from_the_only_preview_is_a_mismatch() {
    let (engine, _) = setup();
    let sender = txgate_core::decode::decode_signed(&hex_bytes(RAW_TX)).unwrap().fields.from.unwrap().to_string();
    let other = json!({"id":1,"method":"eth_estimateGas","params":[{"from": sender, "to": TX_TO, "data": "0xdeadbeef"}]});
    engine.handle("1", "w".into(), other).await;

    let e = engine.clone();
    let _wallet = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(2)).await });
    let pending = wait_pending(&engine, 1).await;
    let Some(PreviewMatch::Mismatch { diffs, .. }) = detail(&pending[0]).preview else { panic!("expected mismatch") };
    assert_eq!(diffs[0].field, "data");
}

#[tokio::test]
async fn enrichment_is_attached_to_held_transactions() {
    let (engine, _) = setup();
    let e = engine.clone();
    let _wallet = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(1)).await });
    wait_pending(&engine, 1).await;
    for _ in 0..200 {
        if detail(&engine.pending()[0]).txs[0].enrichment.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("enrichment never arrived");
}

#[tokio::test]
async fn wallet_disconnect_keeps_the_transaction_and_approval_still_broadcasts() {
    let (engine, up) = setup();
    let server = txgate_core::server::start(engine.clone(), &txgate_core::config::ListenConfig { lan: false, port: 0 })
        .await
        .unwrap();
    let url = format!("http://{}/1", server.addr);

    let client = reqwest::Client::builder().timeout(Duration::from_millis(300)).build().unwrap();
    let res = client.post(&url).json(&send_raw(7)).send().await;
    assert!(res.is_err(), "wallet should time out while the transaction is held");

    let mut status = Status::Pending;
    for _ in 0..200 {
        status = engine.pending()[0].status;
        if status == Status::Disconnected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(status, Status::Disconnected);

    let id = engine.pending()[0].id;
    let record = engine.decide(id, Decision::Approved).await.unwrap();
    assert_eq!(record.status, Status::Approved);
    assert_eq!(up.calls_to("eth_sendRawTransaction").len(), 1);
    server.stop().await;
}

#[tokio::test]
async fn a_wallet_retry_after_disconnect_rejoins_the_held_transaction() {
    let (engine, up) = setup();
    let e = engine.clone();
    let first = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(1)).await });
    let id = wait_pending(&engine, 1).await[0].id;
    first.abort();
    for _ in 0..200 {
        if engine.pending()[0].status == Status::Disconnected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.pending()[0].status, Status::Disconnected);

    let e = engine.clone();
    let retry = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(2)).await });
    for _ in 0..200 {
        if engine.pending()[0].status == Status::Pending {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let held = engine.pending();
    assert_eq!(held.len(), 1, "the retry joins instead of creating a second item");
    assert_eq!(held[0].id, id);
    assert_eq!(held[0].status, Status::Pending);
    assert_eq!(detail(&held[0]).wallet_connections, 1);

    engine.decide(id, Decision::Approved).await.unwrap();
    let resp = retry.await.unwrap();
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"], "0xhash-from-upstream");
    assert_eq!(up.calls_to("eth_sendRawTransaction").len(), 1);
}

#[tokio::test]
async fn http_server_routes_by_chain_id() {
    let (engine, _) = setup();
    let server = txgate_core::server::start(engine.clone(), &txgate_core::config::ListenConfig { lan: false, port: 0 })
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let ok: Value = client
        .post(format!("http://{}/1", server.addr))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["result"], "0x10");

    let bad: Value = client
        .post(format!("http://{}/1", server.addr))
        .body("not json")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bad["error"]["code"], -32700);
    server.stop().await;
}

#[tokio::test]
async fn http_server_refuses_websites_but_serves_wallet_extensions() {
    let (engine, up) = setup();
    let server = txgate_core::server::start(engine.clone(), &txgate_core::config::ListenConfig { lan: false, port: 0 })
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let post = |origin: &'static str| {
        client
            .post(format!("http://{}/1", server.addr))
            .header("origin", origin)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}))
            .send()
    };

    let refused = post("https://some-site.example").await.unwrap();
    assert_eq!(refused.status(), 403);
    assert!(up.calls_to("eth_blockNumber").is_empty());

    let ok: Value = post("chrome-extension://abcdef").await.unwrap().json().await.unwrap();
    assert_eq!(ok["result"], "0x10");
    server.stop().await;
}

#[tokio::test]
async fn huge_responses_reach_the_wallet_but_are_clipped_in_the_log() {
    let (engine, up) = setup();
    let big = "ab".repeat(1_000_000);
    up.on("eth_getLogs", json!(big));
    let resp = engine.handle("1", "x".into(), json!({"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[{}]})).await;
    assert_eq!(resp["result"].as_str().unwrap().len(), 2_000_000, "the wallet gets the full answer");

    let logged = serde_json::to_string(&engine.recent()[0]).unwrap();
    assert!(logged.len() < 100_000, "monitor copy is {} bytes", logged.len());
    assert_eq!(engine.recent()[0].response.as_ref().unwrap()["txgate_clipped"], true);
}

#[tokio::test]
async fn http_server_supports_cors_for_allowed_origins() {
    let (engine, _) = setup();
    let server = txgate_core::server::start(engine.clone(), &txgate_core::config::ListenConfig { lan: false, port: 0 })
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://{}/1", server.addr);

    let pre = client
        .request(reqwest::Method::OPTIONS, &url)
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .send()
        .await
        .unwrap();
    assert!(pre.status().is_success(), "preflight returned {}", pre.status());
    assert_eq!(pre.headers()["access-control-allow-origin"], "http://localhost:5173");

    let post = client
        .post(&url)
        .header("origin", "http://localhost:5173")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(post.headers()["access-control-allow-origin"], "http://localhost:5173");

    let remote = client
        .request(reqwest::Method::OPTIONS, &url)
        .header("origin", "https://some-site.example")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert!(remote.headers().get("access-control-allow-origin").is_none());
    server.stop().await;
}

#[tokio::test]
async fn http_server_answers_json_everywhere_and_accepts_hex_chain_ids() {
    let (engine, _) = setup();
    let server = txgate_core::server::start(engine.clone(), &txgate_core::config::ListenConfig { lan: false, port: 0 })
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let body = json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]});

    let hex: Value = client.post(format!("http://{}/0x1", server.addr)).json(&body).send().await.unwrap().json().await.unwrap();
    assert_eq!(hex["result"], "0x10");

    let lost = client.post(format!("http://{}/1/extra", server.addr)).json(&body).send().await.unwrap();
    assert_eq!(lost.status(), 404);
    let lost: Value = lost.json().await.unwrap();
    assert!(lost["error"]["message"].as_str().unwrap().contains("/<chainId>"));
    server.stop().await;
}

#[tokio::test]
async fn the_listener_can_be_restarted_on_the_same_port_at_once() {
    use txgate_core::config::ListenConfig;
    let (engine, _) = setup();
    let first = txgate_core::server::start(engine.clone(), &ListenConfig { lan: false, port: 0 }).await.unwrap();
    let port = first.addr.port();
    // Hold an open connection, as a wallet waiting on a held transaction would.
    let _conn = tokio::net::TcpStream::connect(first.addr).await.unwrap();
    first.stop().await;
    let again = txgate_core::server::start(engine.clone(), &ListenConfig { lan: false, port }).await;
    assert!(again.is_ok(), "restart failed: {:?}", again.err());
    again.unwrap().stop().await;
    let lan = txgate_core::server::start(engine, &ListenConfig { lan: true, port }).await;
    assert!(lan.is_ok(), "switching to LAN failed: {:?}", lan.err());
}

#[tokio::test]
async fn the_monitor_stays_in_order_when_an_old_held_transaction_is_decided() {
    let (engine, _) = setup();
    let e = engine.clone();
    let wallet = tokio::spawn(async move { e.handle("1", "w".into(), send_raw(1)).await });
    let id = wait_pending(&engine, 1).await[0].id;
    // Enough reads to push the held transaction out of the live monitor.
    for i in 0..1100 {
        engine.handle("1", "w".into(), json!({"jsonrpc":"2.0","id":i,"method":"eth_blockNumber","params":[]})).await;
    }
    engine.decide(id, Decision::Approved).await.unwrap();
    wallet.await.unwrap();

    let ids: Vec<u64> = engine.recent().iter().map(|r| r.id).collect();
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "monitor must stay newest-first");
    assert!(ids.len() <= 1000);
    // The decision is still recorded.
    assert_eq!(engine.get(id).unwrap().status, Status::Approved);
}

fn hex_bytes(s: &str) -> Vec<u8> {
    hex::decode(s.trim_start_matches("0x")).unwrap()
}
