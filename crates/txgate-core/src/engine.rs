//! The engine: everything between "an HTTP request arrived" and "a response is
//! returned", plus the approval queue the UI drives.
//!
//! * read calls are forwarded to the network's read upstream;
//! * `eth_estimateGas` is forwarded, decoded and remembered as a preview;
//! * sending calls are held until [`Engine::decide`] is called, then broadcast
//!   by the engine itself (so approval still works after the wallet gave up).
//!
//! Lock order is `gate` before `monitor`. Records are published (monitor,
//! storage, event) while the lock that owns them is held, so updates to one
//! record are never published out of order.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alloy_primitives::keccak256;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

use crate::config::{Config, ConfigError, LogMode, NetworkConfig, ResolvedNetwork};
use crate::decode::{analyze, decode_signed, signed_tx_risks, Authorization, TxFields};
use crate::enrich::enrich;
use crate::jsonrpc::{error_response, rejected_response, send_payload, with_id, Call, Category, SendPayload};
use crate::preview::PreviewBook;
use crate::record::{Decision, RequestRecord, Status, TxDetail, TxView};
use crate::safe_state::{review, SafeTxReport};
use crate::safe_watch::{check_safe, SafeAlert, WatchStatus};
use crate::simulate::{self, web_simulator_url, SimulationOutcome};
use crate::store::{HistoryQuery, Store};
use crate::upstream::{self, Upstream};

/// Number of recent requests kept in memory for the live monitor.
const MONITOR_CAPACITY: usize = 1000;

/// Request and response bodies larger than this are clipped in records (the
/// monitor, the log and UI events). Wallets and upstreams always get the full body.
const MAX_LOGGED_BODY: usize = 32 * 1024;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn elapsed_ms(since: Instant) -> Option<u64> {
    Some(since.elapsed().as_millis() as u64)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Event {
    /// A request was added to the monitor or its record changed.
    Request(Box<RequestRecord>),
    /// The set of held transactions changed.
    Pending(Vec<RequestRecord>),
    /// A wallet addressed a chain TxGate cannot serve: no such network, or
    /// its upstream cannot be resolved (e.g. a provider key is missing).
    Unroutable { chain_id: u64, client: String, message: String },
    Server(ServerStatus),
    /// The latest status of every watched Safe.
    SafeWatch(Vec<WatchStatus>),
    /// Something changed on a watched Safe.
    SafeAlert(SafeAlert),
    /// The request history was cleared; only held transactions remain.
    HistoryCleared,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub listening: bool,
    pub address: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("request {0} is not waiting for a decision")]
    NotPending(u64),
    #[error("request {0} not found")]
    NotFound(u64),
    #[error("request {0} has no transaction to simulate")]
    NothingToSimulate(u64),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}

struct Waiter {
    request_id: Value,
    tx: oneshot::Sender<Value>,
}

struct HeldTx {
    record: RequestRecord,
    /// The original JSON-RPC call, forwarded verbatim on approval.
    call: Call,
    dedupe_key: String,
    broadcast_url: String,
    waiters: HashMap<u64, Waiter>,
    received: Instant,
}

impl HeldTx {
    fn detail(&mut self) -> &mut TxDetail {
        self.record.detail.get_or_insert_with(TxDetail::default)
    }
}

#[derive(Default)]
struct Gate {
    held: HashMap<u64, HeldTx>,
    by_key: HashMap<String, u64>,
    next_waiter: u64,
}

impl Gate {
    /// Held transactions, oldest first.
    fn records(&self) -> Vec<RequestRecord> {
        let mut v: Vec<_> = self.held.values().map(|h| h.record.clone()).collect();
        v.sort_by_key(|r| r.id);
        v
    }
}

struct Inner {
    config: RwLock<Config>,
    config_path: Option<PathBuf>,
    store: Store,
    upstream: Arc<dyn Upstream>,
    events: broadcast::Sender<Event>,
    next_id: AtomicU64,
    monitor: Mutex<VecDeque<RequestRecord>>,
    previews: Mutex<PreviewBook>,
    gate: Mutex<Gate>,
    server: Mutex<ServerStatus>,
    safe_watch: Mutex<Vec<WatchStatus>>,
    /// Serialises Safe polls so snapshots are never compared twice.
    safe_poll: tokio::sync::Mutex<()>,
}

/// Cheap to clone handle to the shared engine state.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

struct Ctx {
    chain_id: u64,
    client: String,
    network: ResolvedNetwork,
}

impl Engine {
    /// Creates the engine. Requests left open by a previous run are marked abandoned.
    pub fn new(config: Config, config_path: Option<PathBuf>, store: Store, upstream: Arc<dyn Upstream>) -> Result<Self, EngineError> {
        store.abandon_open()?;
        let max_id = store.max_id()?;
        let (events, _) = broadcast::channel(1024);
        let window = config.preview_window_secs;
        Ok(Self(Arc::new(Inner {
            config: RwLock::new(config),
            config_path,
            store,
            upstream,
            events,
            next_id: AtomicU64::new(max_id + 1),
            monitor: Mutex::new(VecDeque::new()),
            previews: Mutex::new(PreviewBook::new(window)),
            gate: Mutex::new(Gate::default()),
            server: Mutex::new(ServerStatus::default()),
            safe_watch: Mutex::new(Vec::new()),
            safe_poll: tokio::sync::Mutex::new(()),
        })))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.events.subscribe()
    }

    fn emit(&self, e: Event) {
        // No subscribers is fine (e.g. headless tests).
        let _ = self.0.events.send(e);
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    pub fn config(&self) -> Config {
        self.0.config.read().clone()
    }

    /// Applies `change` to the current configuration, then validates, persists
    /// and activates the result. Concurrent updates are serialised.
    pub fn update_config(&self, change: impl FnOnce(&mut Config)) -> Result<Config, EngineError> {
        let mut current = self.0.config.write();
        let mut next = current.clone();
        change(&mut next);
        next.validate()?;
        if let Some(path) = &self.0.config_path {
            next.save(path)?;
        }
        self.0.previews.lock().set_window(next.preview_window_secs);
        *current = next.clone();
        drop(current);
        self.forget_unwatched_safes(&next);
        Ok(next)
    }

    /// Replaces the whole configuration (see [`Engine::update_config`]).
    pub fn set_config(&self, config: Config) -> Result<(), EngineError> {
        self.update_config(|c| *c = config).map(|_| ())
    }

    /// Checks that the upstream(s) of `network` answer `eth_chainId` with the
    /// network's chain id. Provider API keys come from the current configuration.
    pub async fn verify_network(&self, network: &NetworkConfig) -> Result<(), String> {
        let mut probe = self.config();
        probe.networks = vec![network.clone()];
        let resolved = probe.resolve(network.chain_id).map_err(|e| e.to_string())?;
        let mut urls = vec![("Read", resolved.read_url.clone())];
        if resolved.broadcast_url != resolved.read_url {
            urls.push(("Broadcast", resolved.broadcast_url.clone()));
        }
        for (label, url) in urls {
            let got = upstream::chain_id(self.0.upstream.as_ref(), &url)
                .await
                .map_err(|e| format!("{label} upstream: {e}"))?;
            if got != network.chain_id {
                return Err(format!(
                    "{label} upstream reports chain {got}, but this network is configured as chain {}",
                    network.chain_id
                ));
            }
        }
        Ok(())
    }

    pub fn set_server_status(&self, status: ServerStatus) {
        *self.0.server.lock() = status.clone();
        self.emit(Event::Server(status));
    }

    pub fn server_status(&self) -> ServerStatus {
        self.0.server.lock().clone()
    }

    // ------------------------------------------------------------------
    // Queries for the UI
    // ------------------------------------------------------------------

    /// Recent requests, newest first.
    pub fn recent(&self) -> Vec<RequestRecord> {
        self.0.monitor.lock().iter().cloned().collect()
    }

    /// Transactions waiting for a decision, oldest first.
    pub fn pending(&self) -> Vec<RequestRecord> {
        self.0.gate.lock().records()
    }

    pub fn history(&self, q: &HistoryQuery) -> Result<Vec<RequestRecord>, EngineError> {
        Ok(self.0.store.query(q)?)
    }

    /// Forgets logged requests, in the history and the live monitor, except
    /// transactions still waiting for a decision.
    pub fn clear_history(&self) -> Result<usize, EngineError> {
        // Holding the gate keeps a transaction that arrives meanwhile from being deleted.
        let gate = self.0.gate.lock();
        let held: Vec<u64> = gate.held.keys().copied().collect();
        self.0.monitor.lock().retain(|r| held.contains(&r.id));
        let removed = self.0.store.clear_requests(&held)?;
        drop(gate);
        self.emit(Event::HistoryCleared);
        Ok(removed)
    }

    pub fn get(&self, id: u64) -> Result<RequestRecord, EngineError> {
        if let Some(h) = self.0.gate.lock().held.get(&id) {
            return Ok(h.record.clone());
        }
        if let Some(r) = self.0.monitor.lock().iter().find(|r| r.id == id) {
            return Ok(r.clone());
        }
        self.0.store.get(id)?.ok_or(EngineError::NotFound(id))
    }

    // ------------------------------------------------------------------
    // Record bookkeeping
    // ------------------------------------------------------------------

    fn next_id(&self) -> u64 {
        self.0.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Publishes a record: monitor buffer, storage (per log mode) and event.
    fn put(&self, record: RequestRecord) {
        let mut monitor = self.0.monitor.lock();
        match monitor.iter_mut().find(|r| r.id == record.id) {
            Some(existing) => *existing = record.clone(),
            None => {
                monitor.push_front(record.clone());
                monitor.truncate(MONITOR_CAPACITY);
            }
        }
        self.persist_and_emit(record);
    }

    /// Storage and event only; the caller holds the lock that owns `record`.
    fn persist_and_emit(&self, record: RequestRecord) {
        let persist = record.category != Category::Read || self.0.config.read().logging.mode == LogMode::All;
        if persist && let Err(e) = self.0.store.upsert(&record) {
            tracing::error!("failed to persist request {}: {e}", record.id);
        }
        self.emit(Event::Request(Box::new(record)));
    }

    /// Publishes a held transaction and the pending list. Call with the gate locked.
    fn publish_held(&self, gate: &Gate, id: u64) {
        if let Some(h) = gate.held.get(&id) {
            self.put(h.record.clone());
        }
        self.emit(Event::Pending(gate.records()));
    }

    /// Applies `f` to the current version of a record, wherever it lives, and
    /// publishes the result.
    fn update_record(&self, id: u64, f: impl FnOnce(&mut RequestRecord)) {
        let mut f = Some(f);
        let mut apply = |r: &mut RequestRecord| {
            if let Some(f) = f.take() {
                f(r)
            }
        };
        {
            let mut gate = self.0.gate.lock();
            if let Some(h) = gate.held.get_mut(&id) {
                apply(&mut h.record);
                self.publish_held(&gate, id);
                return;
            }
        }
        let mut monitor = self.0.monitor.lock();
        let record = match monitor.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                apply(r);
                r.clone()
            }
            // Evicted from the monitor: update the stored copy.
            None => match self.0.store.get(id) {
                Ok(Some(mut r)) => {
                    apply(&mut r);
                    r
                }
                _ => return,
            },
        };
        self.persist_and_emit(record);
    }

    fn update_detail(&self, id: u64, f: impl FnOnce(&mut TxDetail)) {
        self.update_record(id, |r| f(r.detail.get_or_insert_with(TxDetail::default)));
    }

    fn base_record(&self, ctx: &Ctx, call: &Call) -> RequestRecord {
        RequestRecord {
            id: self.next_id(),
            ts_ms: now_ms(),
            chain_id: ctx.chain_id,
            method: call.method.clone(),
            category: call.category(),
            client: ctx.client.clone(),
            summary: String::new(),
            status: Status::Forwarded,
            duration_ms: None,
            tx_hash: None,
            from: None,
            to: None,
            request: clip(&redact_passwords(call)),
            response: None,
            detail: None,
        }
    }

    // ------------------------------------------------------------------
    // Request handling
    // ------------------------------------------------------------------

    /// Handles one HTTP request body addressed to `/{route}`.
    /// Returns the JSON-RPC response body (object or batch array).
    pub async fn handle(&self, route: &str, client: String, body: Value) -> Value {
        let Some(chain_id) = parse_chain_id(route) else {
            return error_for_body(&body, -32000, crate::server::USAGE);
        };
        let resolved = self.0.config.read().resolve(chain_id);
        let ctx = match resolved {
            Ok(network) => Ctx { chain_id, client, network },
            Err(e) => {
                let message = e.to_string();
                self.emit(Event::Unroutable { chain_id, client, message: message.clone() });
                return error_for_body(&body, -32000, &format!("TxGate: {message}"));
            }
        };
        match body {
            Value::Array(items) if items.is_empty() => error_response(Value::Null, -32600, "Empty batch"),
            Value::Array(items) => {
                // Calls in a batch run concurrently: reads answer immediately while
                // sending calls wait for approval; the batch returns when all are done.
                let futs = items.into_iter().map(|item| self.handle_item(&ctx, item));
                Value::Array(futures::future::join_all(futs).await)
            }
            item => self.handle_item(&ctx, item).await,
        }
    }

    async fn handle_item(&self, ctx: &Ctx, item: Value) -> Value {
        let call = match Call::parse(item) {
            Ok(c) => c,
            Err(resp) => return resp,
        };
        match call.category() {
            Category::Read => self.handle_read(ctx, call).await,
            Category::Estimate => self.handle_estimate(ctx, call).await,
            Category::Send => self.handle_send(ctx, call).await,
        }
    }

    async fn forward(&self, url: &str, call: &Call) -> (Value, Status) {
        match self.0.upstream.call(url, &call.raw).await {
            Ok(resp) => (resp, Status::Forwarded),
            Err(e) => (error_response(call.id.clone(), -32603, &format!("TxGate: {e}")), Status::Failed),
        }
    }

    /// Sends an approved call to the broadcast upstream.
    async fn broadcast(&self, url: &str, call: &Call) -> (Value, Status) {
        let (resp, status) = self.forward(url, call).await;
        let accepted = status == Status::Forwarded && resp.get("error").is_none();
        (resp, if accepted { Status::Approved } else { Status::BroadcastFailed })
    }

    async fn handle_read(&self, ctx: &Ctx, call: Call) -> Value {
        let started = Instant::now();
        let mut record = self.base_record(ctx, &call);
        record.summary = read_summary(&call);
        let (resp, status) = self.forward(&ctx.network.read_url, &call).await;
        record.status = status;
        record.duration_ms = elapsed_ms(started);
        record.response = Some(clip(&resp));
        self.put(record);
        resp
    }

    async fn handle_estimate(&self, ctx: &Ctx, call: Call) -> Value {
        let started = Instant::now();
        let mut record = self.base_record(ctx, &call);
        let fields = call.param(0).map(TxFields::from_request).unwrap_or_default();
        let auths = call.param(0).map(Authorization::list_from_request).unwrap_or_default();
        let view = tx_view(ctx.chain_id, None, fields.clone(), &auths);
        record.summary = view.analysis.summary.clone();
        record.from = fields.from.map(|a| a.to_checksum(None));
        record.to = fields.to.map(|a| a.to_checksum(None));
        record.detail = Some(TxDetail { txs: vec![view.clone()], ..Default::default() });
        self.0.previews.lock().record(record.id, record.ts_ms, ctx.chain_id, &fields, &auths);

        let (resp, status) = self.forward(&ctx.network.read_url, &call).await;
        record.status = status;
        record.duration_ms = elapsed_ms(started);
        record.response = Some(clip(&resp));
        let id = record.id;
        self.put(record);
        self.spawn_enrichment(id, ctx.chain_id, ctx.network.read_url.clone(), vec![view], false);
        resp
    }

    async fn handle_send(&self, ctx: &Ctx, call: Call) -> Value {
        let received = Instant::now();
        let mut record = self.base_record(ctx, &call);

        let payload = match send_payload(&call) {
            Ok(p) => p,
            Err(e) => {
                let resp = error_response(call.id.clone(), -32602, &format!("TxGate: {e}"));
                record.status = Status::Invalid;
                record.summary = e;
                record.response = Some(clip(&resp));
                self.put(record);
                return resp;
            }
        };

        let (mut detail, dedupe_key) = describe_send(ctx.chain_id, &payload);
        if let Some(first) = detail.txs.first() {
            detail.preview = Some(self.0.previews.lock().check(now_ms(), ctx.chain_id, &first.fields, &first.authorizations));
            record.from = first.fields.from.map(|a| a.to_checksum(None));
            record.to = first.fields.to.map(|a| a.to_checksum(None));
            record.tx_hash = first.hash.clone();
        }
        record.summary = send_summary(&detail);

        // The same signed transaction was approved before: forward without asking
        // again. Unsigned requests carry no nonce and could be replayed verbatim,
        // so they are always held.
        let signed = matches!(payload, SendPayload::Signed(_));
        if signed && let Ok(Some(Decision::Approved)) = self.0.store.decision(&dedupe_key) {
            let (resp, status) = self.broadcast(&ctx.network.broadcast_url, &call).await;
            record.status = if status == Status::Approved { Status::AutoApproved } else { status };
            record.detail = Some(detail);
            record.duration_ms = elapsed_ms(received);
            record.response = Some(clip(&resp));
            self.put(record);
            return resp;
        }

        let (tx, rx) = oneshot::channel();
        let waiter = Waiter { request_id: call.id.clone(), tx };
        let (guard, is_new) = {
            let mut gate = self.0.gate.lock();
            let waiter_id = gate.next_waiter;
            gate.next_waiter += 1;
            let existing = gate.by_key.get(&dedupe_key).copied();
            let (held_id, is_new) = match existing.and_then(|id| gate.held.get_mut(&id)) {
                // The wallet retried a transaction that is already held: join it.
                Some(held) => {
                    held.waiters.insert(waiter_id, waiter);
                    let connections = held.waiters.len();
                    held.record.status = Status::Pending;
                    let d = held.detail();
                    d.attempts += 1;
                    d.wallet_connections = connections;
                    (held.record.id, false)
                }
                None => {
                    detail.attempts = 1;
                    detail.wallet_connections = 1;
                    record.status = Status::Pending;
                    record.detail = Some(detail.clone());
                    let held_id = record.id;
                    gate.by_key.insert(dedupe_key.clone(), held_id);
                    gate.held.insert(
                        held_id,
                        HeldTx {
                            record,
                            call: call.clone(),
                            dedupe_key,
                            broadcast_url: ctx.network.broadcast_url.clone(),
                            waiters: HashMap::from([(waiter_id, waiter)]),
                            received,
                        },
                    );
                    (held_id, true)
                }
            };
            self.publish_held(&gate, held_id);
            (WaiterGuard { engine: self.clone(), held_id, waiter_id }, is_new)
        };
        if is_new {
            self.spawn_enrichment(guard.held_id, ctx.chain_id, ctx.network.read_url.clone(), detail.txs, true);
        }

        let resp = rx.await.unwrap_or_else(|_| error_response(call.id.clone(), -32603, "TxGate: request dropped"));
        drop(guard);
        resp
    }

    fn spawn_enrichment(&self, id: u64, chain_id: u64, read_url: String, views: Vec<TxView>, auto_simulate: bool) {
        if views.is_empty() {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            let safe_cfg = engine.0.config.read().safe.clone();
            let mut enrichments = Vec::new();
            for v in &views {
                enrichments.push(enrich(engine.0.upstream.as_ref(), &read_url, chain_id, &safe_cfg, &v.fields, &v.analysis).await);
            }
            engine.update_detail(id, |d| {
                for (view, e) in d.txs.iter_mut().zip(enrichments) {
                    view.enrichment = Some(e);
                }
            });
            let tenderly_ready = engine.0.config.read().tenderly.is_configured();
            if auto_simulate && tenderly_ready {
                let _ = engine.simulate(id).await;
            }
        });
    }

    // ------------------------------------------------------------------
    // Decisions
    // ------------------------------------------------------------------

    /// Approves (broadcasts) or rejects a held transaction and answers every
    /// wallet connection still waiting on it. Returns the final record; after
    /// an approval its status tells whether the upstream accepted the broadcast.
    pub async fn decide(&self, id: u64, decision: Decision) -> Result<RequestRecord, EngineError> {
        let held = {
            let mut gate = self.0.gate.lock();
            let held = gate.held.remove(&id).ok_or(EngineError::NotPending(id))?;
            gate.by_key.remove(&held.dedupe_key);
            self.emit(Event::Pending(gate.records()));
            held
        };
        // The stored decision only drives auto-approval of a resend; failing to
        // store it must not lose the decision on this transaction.
        if let Err(e) = self.0.store.record_decision(&held.dedupe_key, decision, now_ms()) {
            tracing::error!("failed to remember decision for request {id}: {e}");
        }

        let (response, status) = match decision {
            Decision::Rejected => (rejected_response(held.call.id.clone()), Status::Rejected),
            Decision::Approved => self.broadcast(&held.broadcast_url, &held.call).await,
        };
        for w in held.waiters.into_values() {
            let r = match decision {
                Decision::Rejected => rejected_response(w.request_id),
                Decision::Approved => with_id(response.clone(), &w.request_id),
            };
            let _ = w.tx.send(r);
        }
        let duration_ms = elapsed_ms(held.received);
        // Update the latest copy: enrichment may have landed during the broadcast.
        self.update_record(id, |r| {
            r.status = status;
            r.response = Some(clip(&response));
            r.duration_ms = duration_ms;
            if let Some(d) = r.detail.as_mut() {
                d.wallet_connections = 0;
            }
        });
        self.get(id)
    }

    // ------------------------------------------------------------------
    // Safe multisig
    // ------------------------------------------------------------------

    /// Latest status of every watched Safe.
    pub fn safe_watch(&self) -> Vec<WatchStatus> {
        self.0.safe_watch.lock().clone()
    }

    /// Stored Safe alerts, newest first.
    pub fn safe_alerts(&self, limit: u32) -> Result<Vec<SafeAlert>, EngineError> {
        Ok(self.0.store.safe_alerts(limit)?)
    }

    pub fn mark_safe_alerts_read(&self) -> Result<(), EngineError> {
        Ok(self.0.store.mark_safe_alerts_read()?)
    }

    /// Checks every watched Safe once: on-chain configuration against the last
    /// snapshot, and the Transaction Service queue. New findings are stored and
    /// emitted as alerts.
    fn forget_unwatched_safes(&self, config: &Config) {
        let watched: Vec<_> = config.safe.watched.iter().map(|w| (w.chain_id, w.address)).collect();
        if let Err(e) = self.0.store.retain_safe_snapshots(&watched) {
            tracing::error!("cannot prune Safe snapshots: {e}");
        }
    }

    pub async fn poll_safes(&self) -> Vec<WatchStatus> {
        let _running = self.0.safe_poll.lock().await;
        let config = self.config();
        let mut statuses = Vec::new();
        for watched in &config.safe.watched {
            let read_url = match config.resolve(watched.chain_id) {
                Ok(n) => n.read_url,
                Err(e) => {
                    let mut status = WatchStatus::new(watched);
                    status.set_error(crate::text::Text::new(
                        "watch.chain_unreadable",
                        [("chain", watched.chain_id.to_string()), ("error", e.to_string())],
                    ));
                    statuses.push(status);
                    continue;
                }
            };
            let prev = match self.0.store.safe_snapshot(watched.chain_id, &watched.address) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("cannot read Safe snapshot: {e}");
                    None
                }
            };
            let outcome = check_safe(self.0.upstream.as_ref(), &read_url, &config.safe, watched, prev).await;
            if let Some((state, seen)) = &outcome.snapshot
                && let Err(e) = self.0.store.save_safe_snapshot(watched.chain_id, state, seen, now_ms())
            {
                tracing::error!("cannot store Safe snapshot: {e}");
            }
            for f in &outcome.findings {
                match self.0.store.add_safe_alert(now_ms(), watched.chain_id, &watched.address, f) {
                    Ok(alert) => self.emit(Event::SafeAlert(alert)),
                    Err(e) => tracing::error!("cannot store Safe alert: {e}"),
                }
            }
            statuses.push(outcome.status);
        }
        // A Safe removed while this poll ran must not keep the snapshot just saved.
        self.forget_unwatched_safes(&self.config());
        *self.0.safe_watch.lock() = statuses.clone();
        self.emit(Event::SafeWatch(statuses.clone()));
        statuses
    }

    /// Reviews a SafeTx before signing (typed data, Transaction Service JSON or
    /// plain fields), reading the Safe from the network's read upstream.
    pub async fn review_safe_tx(&self, chain_id: u64, safe: alloy_primitives::Address, input: Value) -> Result<SafeTxReport, String> {
        let read_url = self.0.config.read().resolve(chain_id).map_err(|e| e.to_string())?.read_url;
        review(self.0.upstream.as_ref(), &read_url, chain_id, safe, &input).await
    }

    /// Queued transactions of a Safe from the Transaction Service, verified locally.
    pub async fn safe_queue(&self, chain_id: u64, safe: alloy_primitives::Address) -> Result<Vec<SafeTxReport>, String> {
        let config = self.config();
        let read_url = config.resolve(chain_id).map_err(|e| e.to_string())?.read_url;
        let up = self.0.upstream.as_ref();
        let state = crate::safe_state::read_state(up, &read_url, safe).await?;
        let list = crate::safe_service::queued(up, &config.safe, chain_id, safe, state.nonce).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for stx in &list {
            let mut report = crate::safe_state::review_service_tx(up, &read_url, chain_id, stx, Some(state.clone())).await;
            if stx.safe != safe {
                report.risks.push(crate::decode::Risk::t(
                    crate::decode::Severity::Danger,
                    "safe_domain_mismatch",
                    "safe_service_other_safe",
                    [("other", stx.safe.to_checksum(None)), ("safe", safe.to_checksum(None))],
                ));
            }
            out.push(report);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Simulation
    // ------------------------------------------------------------------

    /// Simulates the (first) transaction of a request with Tenderly and stores the result.
    pub async fn simulate(&self, id: u64) -> Result<SimulationOutcome, EngineError> {
        let record = self.get(id)?;
        let tx = record
            .detail
            .and_then(|d| d.txs.into_iter().next())
            .ok_or(EngineError::NothingToSimulate(id))?;
        let tenderly = self.0.config.read().tenderly.clone();
        let outcome = match simulate::simulate(&tenderly, record.chain_id, &tx.fields, tx.entry_point).await {
            Ok(result) => SimulationOutcome::Ok { result, ts_ms: now_ms() },
            Err(e) => SimulationOutcome::Error { message: e.to_string(), ts_ms: now_ms() },
        };
        let stored = outcome.clone();
        self.update_detail(id, move |d| d.simulation = Some(stored));
        Ok(outcome)
    }
}

/// Keeps a wallet connection registered while its HTTP request is open. When
/// the request future is dropped (the wallet disconnected) the waiter is
/// removed, and a transaction with no remaining waiters becomes `Disconnected`.
/// After a decision the transaction is no longer held and dropping is a no-op.
struct WaiterGuard {
    engine: Engine,
    held_id: u64,
    waiter_id: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let mut gate = self.engine.0.gate.lock();
        let Some(held) = gate.held.get_mut(&self.held_id) else { return };
        if held.waiters.remove(&self.waiter_id).is_none() {
            return;
        }
        let remaining = held.waiters.len();
        if remaining == 0 {
            held.record.status = Status::Disconnected;
        }
        held.detail().wallet_connections = remaining;
        self.engine.publish_held(&gate, self.held_id);
    }
}

/// Decodes what a sending call would broadcast. Returns the detail and the key
/// that identifies the same submission across wallet retries.
/// The request as logged: node account passwords are replaced.
fn redact_passwords(call: &Call) -> Value {
    let mut raw = call.raw.clone();
    if crate::jsonrpc::PASSWORD_METHODS.contains(&call.method.as_str())
        && let Some(p) = raw.get_mut("params").and_then(Value::as_array_mut).and_then(|a| a.get_mut(1))
    {
        *p = Value::String("<redacted>".into());
    }
    raw
}

fn describe_send(chain_id: u64, payload: &SendPayload) -> (TxDetail, String) {
    let mut detail = TxDetail::default();
    let key = match payload {
        SendPayload::Signed(raws) => {
            let mut hashes = Vec::new();
            for raw in raws {
                match decode_signed(raw) {
                    Ok(tx) => {
                        let hash = tx.hash.to_string();
                        let extra_risks = signed_tx_risks(&tx);
                        let mut view = tx_view(chain_id, Some(hash.clone()), tx.fields, &tx.authorizations);
                        view.analysis.risks.extend(extra_risks);
                        view.tx_type = Some(tx.tx_type_name);
                        view.max_fee_per_gas = Some(tx.max_fee_per_gas.to_string());
                        view.max_priority_fee_per_gas = tx.max_priority_fee_per_gas.map(|v| v.to_string());
                        detail.txs.push(view);
                        hashes.push(hash);
                    }
                    Err(e) => {
                        hashes.push(keccak256(raw).to_string());
                        detail.decode_error = Some(e.to_string());
                    }
                }
            }
            if hashes.len() == 1 { hashes.remove(0) } else { format!("bundle:{}", hashes.join(",")) }
        }
        SendPayload::UserOperation { op, entry_point } => {
            // The smart account calls itself with `callData`; its own code then
            // performs the actual calls.
            let mut fields = TxFields::from_request(&serde_json::json!({
                "from": op.get("sender"),
                "to": op.get("sender"),
                "data": op.get("callData"),
                "nonce": op.get("nonce"),
            }));
            fields.nonce = None; // A UserOperation nonce is not the account's transaction nonce.
            // EntryPoint v0.8: the bundler applies this EIP-7702 delegation first.
            let auths = op
                .get("eip7702Auth")
                .map(|a| Authorization::list_from_request(&serde_json::json!({ "authorizationList": [a] })))
                .unwrap_or_default();
            let mut view = tx_view(chain_id, None, fields, &auths);
            view.tx_type = Some("ERC-4337 UserOperation".into());
            view.entry_point = *entry_point;
            view.simulator_url = web_simulator_url(chain_id, &view.fields, *entry_point);
            detail.txs.push(view);
            format!("userop:{chain_id}:{}", keccak256(op.to_string().as_bytes()))
        }
        SendPayload::Unsigned(obj) => {
            detail.txs.push(tx_view(chain_id, None, TxFields::from_request(obj), &Authorization::list_from_request(obj)));
            format!("unsigned:{chain_id}:{}", keccak256(obj.to_string().as_bytes()))
        }
    };
    detail.dedupe_key = Some(key.clone());
    (detail, key)
}

fn send_summary(detail: &TxDetail) -> String {
    match detail.txs.as_slice() {
        [] => format!("Undecodable transaction ({})", detail.decode_error.as_deref().unwrap_or("no data")),
        [t] => t.analysis.summary.clone(),
        [t, ..] => format!("Bundle of {} · {}", detail.txs.len(), t.analysis.summary),
    }
}

fn tx_view(chain_id: u64, hash: Option<String>, fields: TxFields, auths: &[Authorization]) -> TxView {
    let analysis = analyze(&fields, auths, chain_id);
    TxView {
        hash,
        tx_type: None,
        simulator_url: web_simulator_url(chain_id, &fields, None),
        fields,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        authorizations: auths.to_vec(),
        analysis,
        enrichment: None,
        entry_point: None,
    }
}

/// Shortens a long string to its head and tail, on character boundaries.
fn abbreviate(s: &str, head: usize, tail: usize) -> String {
    let n = s.chars().count();
    if n <= head + tail + 6 {
        return s.to_string();
    }
    let start: String = s.chars().take(head).collect();
    let end: String = s.chars().skip(n - tail).collect();
    format!("{start}…{end}")
}

/// Short description of a read call's parameters for the monitor list.
fn read_summary(call: &Call) -> String {
    let s = match &call.params {
        Value::Array(a) => a
            .iter()
            .map(|p| match p {
                Value::String(s) => abbreviate(s, 10, 4),
                Value::Object(_) => "{…}".into(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    s.chars().take(160).collect()
}

/// Chain id from the URL path: decimal (`/1`) or hex (`/0x1`).
fn parse_chain_id(route: &str) -> Option<u64> {
    let s = route.trim_matches('/');
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Replaces an oversized body by a short marker with the beginning of its JSON.
fn clip(v: &Value) -> Value {
    let text = v.to_string();
    if text.len() <= MAX_LOGGED_BODY {
        return v.clone();
    }
    serde_json::json!({
        "txgate_clipped": true,
        "size_bytes": text.len(),
        "preview": text.chars().take(2000).collect::<String>(),
    })
}

/// Builds an error response mirroring the shape (single or batch) of `body`.
fn error_for_body(body: &Value, code: i64, message: &str) -> Value {
    let id = |v: &Value| v.get("id").cloned().unwrap_or(Value::Null);
    match body {
        Value::Array(items) => Value::Array(items.iter().map(|i| error_response(id(i), code, message)).collect()),
        other => error_response(id(other), code, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_on_char_boundaries() {
        assert_eq!(abbreviate("0x1234", 10, 4), "0x1234");
        assert_eq!(abbreviate("0xabcdef0123456789abcdef", 10, 4), "0xabcdef01…cdef");
        // Multi-byte characters must not panic.
        assert_eq!(abbreviate(&"é".repeat(30), 10, 4), format!("{}…{}", "é".repeat(10), "é".repeat(4)));
    }
}
