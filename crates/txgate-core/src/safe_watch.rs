//! Watching Safes: configuration changes on-chain and newly proposed
//! transactions in the Safe Transaction Service.

use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

use crate::config::{SafeConfig, WatchedSafe};
use crate::decode::{short, Severity};
use crate::safe_service;
use crate::safe_state::{read_state, review_service_tx, SafeState, SafeTxReport};
use crate::text::Text;
use crate::upstream::Upstream;

/// Something the user should know about a watched Safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeAlert {
    pub id: u64,
    pub ts_ms: u64,
    pub chain_id: u64,
    pub safe: Address,
    pub severity: Severity,
    /// `config_change`, `executed` or `proposed`.
    pub kind: String,
    pub message: String,
    pub safe_tx_hash: Option<String>,
    pub read: bool,
    /// `message` for translation; absent on alerts stored before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
}

/// An alert before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub kind: &'static str,
    pub message: String,
    pub safe_tx_hash: Option<String>,
    pub text: Option<Text>,
}

impl Finding {
    fn new<const N: usize>(severity: Severity, kind: &'static str, key: &str, params: [(&str, String); N], safe_tx_hash: Option<String>) -> Self {
        let text = Text::new(&format!("alert.{key}"), params);
        Self { severity, kind, message: text.english(), safe_tx_hash, text: Some(text) }
    }

    fn config<const N: usize>(severity: Severity, key: &str, params: [(&str, String); N]) -> Self {
        Self::new(severity, "config_change", key, params, None)
    }
}

fn addr_list(list: &[Address]) -> String {
    list.iter().map(|a| a.to_checksum(None)).collect::<Vec<_>>().join(", ")
}

fn opt(a: &Option<Address>) -> String {
    a.map(|a| a.to_checksum(None)).unwrap_or_else(|| "—".into())
}

/// What changed between two readings of the same Safe.
pub fn diff_states(old: &SafeState, new: &SafeState) -> Vec<Finding> {
    let mut out = Vec::new();
    let added: Vec<Address> = new.owners.iter().filter(|o| !old.owners.contains(o)).copied().collect();
    let removed: Vec<Address> = old.owners.iter().filter(|o| !new.owners.contains(o)).copied().collect();
    if !added.is_empty() {
        out.push(Finding::config(Severity::Danger, "owners_added", [("owners", addr_list(&added))]));
    }
    if !removed.is_empty() {
        out.push(Finding::config(Severity::Warning, "owners_removed", [("owners", addr_list(&removed))]));
    }
    if new.threshold != old.threshold {
        let sev = if new.threshold < old.threshold { Severity::Danger } else { Severity::Warning };
        out.push(Finding::config(sev, "threshold_changed", [("old", old.threshold.to_string()), ("new", new.threshold.to_string())]));
    }
    if new.singleton != old.singleton {
        let sev = if new.singleton_name.is_some() { Severity::Warning } else { Severity::Danger };
        let params = [("old", opt(&old.singleton)), ("new", opt(&new.singleton)), ("name", new.singleton_name.clone().unwrap_or_default())];
        let key = if new.singleton_name.is_some() { "implementation_changed" } else { "implementation_changed_unofficial" };
        out.push(Finding::config(sev, key, params));
    }
    if new.guard != old.guard {
        out.push(Finding::config(Severity::Danger, "guard_changed", [("old", opt(&old.guard)), ("new", opt(&new.guard))]));
    }
    if new.module_guard != old.module_guard {
        out.push(Finding::config(Severity::Danger, "module_guard_changed", [("old", opt(&old.module_guard)), ("new", opt(&new.module_guard))]));
    }
    if new.fallback_handler != old.fallback_handler {
        out.push(Finding::config(Severity::Danger, "fallback_handler_changed", [("old", opt(&old.fallback_handler)), ("new", opt(&new.fallback_handler))]));
    }
    let enabled: Vec<Address> = new.modules.iter().filter(|m| !old.modules.contains(m)).copied().collect();
    let disabled: Vec<Address> = old.modules.iter().filter(|m| !new.modules.contains(m)).copied().collect();
    if !enabled.is_empty() {
        out.push(Finding::config(Severity::Danger, "modules_enabled", [("modules", addr_list(&enabled))]));
    }
    if !disabled.is_empty() {
        out.push(Finding::config(Severity::Info, "modules_disabled", [("modules", addr_list(&disabled))]));
    }
    if new.version != old.version {
        out.push(Finding::config(
            Severity::Warning,
            "version_changed",
            [("old", old.version.clone().unwrap_or_else(|| "?".into())), ("new", new.version.clone().unwrap_or_else(|| "?".into()))],
        ));
    }
    if new.nonce > old.nonce {
        let first = old.nonce;
        let last = new.nonce - alloy_primitives::U256::from(1);
        let key = if first == last { "executed_one" } else { "executed" };
        out.push(Finding::new(Severity::Info, "executed", key, [("first", first.to_string()), ("last", last.to_string())], None));
    }
    out
}

/// The latest view of one watched Safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchStatus {
    pub chain_id: u64,
    pub address: Address,
    pub label: String,
    pub state: Option<SafeState>,
    /// Queued transactions from the Transaction Service, verified locally.
    pub queue: Vec<SafeTxReport>,
    /// The Safe could not be read.
    pub error: Option<String>,
    /// The Transaction Service could not be read (or is turned off).
    pub service_error: Option<String>,
    pub checked_ms: u64,
    /// `error` and `service_error` for translation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_text: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_error_text: Option<Text>,
}

impl WatchStatus {
    pub fn new(watched: &WatchedSafe) -> Self {
        Self {
            chain_id: watched.chain_id,
            address: watched.address,
            label: watched.label.clone(),
            state: None,
            queue: Vec::new(),
            error: None,
            service_error: None,
            checked_ms: crate::engine::now_ms(),
            error_text: None,
            service_error_text: None,
        }
    }

    pub fn set_error(&mut self, text: Text) {
        self.error = Some(text.english());
        self.error_text = Some(text);
    }

    fn set_service_error(&mut self, text: Text) {
        self.service_error = Some(text.english());
        self.service_error_text = Some(text);
    }
}

/// Outcome of checking one Safe.
pub struct CheckOutcome {
    pub status: WatchStatus,
    pub findings: Vec<Finding>,
    /// New snapshot to store: the state and the queued hashes already alerted on.
    pub snapshot: Option<(SafeState, Vec<String>)>,
}

/// Reads a watched Safe and its queue, comparing with the previous snapshot.
/// The first check of a Safe sets the baseline: no configuration alerts, but
/// every queued transaction is reported.
pub async fn check_safe(
    up: &dyn Upstream,
    read_url: &str,
    cfg: &SafeConfig,
    watched: &WatchedSafe,
    prev: Option<(SafeState, Vec<String>)>,
) -> CheckOutcome {
    let mut status = WatchStatus::new(watched);
    let state = match read_state(up, read_url, watched.address).await {
        Ok(s) => s,
        Err(e) => {
            status.set_error(Text::new("watch.unreadable", [("error", e)]));
            return CheckOutcome { status, findings: Vec::new(), snapshot: None };
        }
    };
    let (prev_state, prev_seen) = match prev {
        Some((s, seen)) => (Some(s), seen),
        None => (None, Vec::new()),
    };
    let mut findings = prev_state.as_ref().map(|old| diff_states(old, &state)).unwrap_or_default();

    let mut seen = prev_seen.clone();
    if !cfg.service_enabled {
        status.set_service_error(Text::plain("watch.service_off"));
    } else {
        match safe_service::queued(up, cfg, watched.chain_id, watched.address, state.nonce).await {
            Ok(list) => {
                seen.clear();
                for stx in &list {
                    if stx.safe != watched.address {
                        // A tampered service answer: never show it as this Safe's queue.
                        findings.push(Finding::config(
                            Severity::Danger,
                            "service_other_safe",
                            [("nonce", stx.tx.nonce.to_string()), ("other", stx.safe.to_checksum(None))],
                        ));
                        continue;
                    }
                    let report = review_service_tx(up, read_url, watched.chain_id, stx, Some(state.clone())).await;
                    let hash = report.hashes.safe_tx_hash.to_string();
                    if !prev_seen.contains(&hash) {
                        findings.push(proposal_finding(&report));
                    }
                    seen.push(hash);
                    status.queue.push(report);
                }
            }
            Err(e) => status.set_service_error(e.text()),
        }
    }
    status.state = Some(state.clone());
    CheckOutcome { status, findings, snapshot: Some((state, seen)) }
}

/// An alert for a queued transaction TxGate has not seen before.
pub fn proposal_finding(report: &SafeTxReport) -> Finding {
    let worst = report.risks.iter().map(|r| r.severity).max().unwrap_or(Severity::Info);
    let severity = worst.max(Severity::Warning);
    let proposer = report.service.as_ref().and_then(|s| s.proposer);
    let flagged = report.risks.iter().filter(|r| r.severity == Severity::Danger).count();
    let key = format!("proposed{}{}", if proposer.is_some() { "_by" } else { "" }, if flagged > 0 { "_flagged" } else { "" });
    let params = [
        ("proposer", proposer.as_ref().map(short).unwrap_or_default()),
        ("nonce", report.tx.nonce.to_string()),
        ("summary", report.analysis.summary.clone()),
        ("flags", flagged.to_string()),
    ];
    Finding::new(severity, "proposed", &key, params, Some(report.hashes.safe_tx_hash.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    mod polling {
        use super::super::*;
        use crate::config::{SafeConfig, WatchedSafe};
        use crate::safe::fixtures::SAFE;
        use crate::safe::SINGLETONS;
        use crate::safe_state::tests::chain;
        use serde_json::json;

        fn watched() -> WatchedSafe {
            WatchedSafe { chain_id: 1, address: SAFE, label: "Treasury".into() }
        }

        fn with_queue() -> crate::upstream::fake::FakeUpstream {
            let up = chain(2867, SINGLETONS[3].0, vec![]);
            up.on_get("https://api.safe.global/tx-service/eth/api/v2/safes/", json!({"results": [crate::safe_service::tests::service_json()]}));
            up
        }

        #[tokio::test]
        async fn first_check_alerts_on_queued_transactions_only() {
            let up = with_queue();
            let out = check_safe(&up, "u", &SafeConfig::default(), &watched(), None).await;
            assert_eq!(out.status.queue.len(), 1);
            assert_eq!(out.findings.len(), 1);
            assert_eq!(out.findings[0].kind, "proposed");
            assert!(out.findings[0].message.contains("by 0x964a"), "{}", out.findings[0].message);
            let (state, seen) = out.snapshot.unwrap();
            assert_eq!(state.threshold, 3);
            assert_eq!(seen.len(), 1);
        }

        #[tokio::test]
        async fn later_checks_report_only_what_changed() {
            let up = with_queue();
            let first = check_safe(&up, "u", &SafeConfig::default(), &watched(), None).await.snapshot.unwrap();
            let again = check_safe(&up, "u", &SafeConfig::default(), &watched(), Some(first.clone())).await;
            assert!(again.findings.is_empty(), "{:?}", again.findings);

            let mut old = first;
            old.0.threshold = 4;
            let changed = check_safe(&up, "u", &SafeConfig::default(), &watched(), Some(old)).await;
            assert_eq!(changed.findings.len(), 1);
            assert!(changed.findings[0].message.contains("Threshold changed from 4 to 3"));
        }

        #[tokio::test]
        async fn transactions_of_another_safe_are_not_queued() {
            let up = chain(2867, SINGLETONS[3].0, vec![]);
            let mut other = crate::safe_service::tests::service_json();
            other["safe"] = json!(Address::repeat_byte(0x77).to_checksum(None));
            up.on_get("https://api.safe.global/tx-service/eth/api/v2/safes/", json!({"results": [other]}));
            let out = check_safe(&up, "u", &SafeConfig::default(), &watched(), None).await;
            assert!(out.status.queue.is_empty());
            assert_eq!(out.findings.len(), 1);
            assert_eq!(out.findings[0].severity, Severity::Danger);
            assert!(out.findings[0].message.contains("another Safe"), "{}", out.findings[0].message);
        }

        #[tokio::test]
        async fn service_failure_keeps_what_was_seen() {
            let up = chain(2867, SINGLETONS[3].0, vec![]); // no service documents: 404
            let prev = (SafeState { address: SAFE, ..Default::default() }, vec!["0xaa".to_string()]);
            let prev = Some((check_safe(&up, "u", &SafeConfig::default(), &watched(), Some(prev.clone())).await.snapshot.unwrap().0, prev.1));
            let out = check_safe(&up, "u", &SafeConfig::default(), &watched(), prev).await;
            assert!(out.status.service_error.is_some());
            assert_eq!(out.snapshot.unwrap().1, vec!["0xaa".to_string()]);
        }

        #[tokio::test]
        async fn status_messages_are_translatable() {
            let up = chain(2867, SINGLETONS[3].0, vec![]);
            let cfg = SafeConfig { service_enabled: false, ..Default::default() };
            let out = check_safe(&up, "u", &cfg, &watched(), None).await;
            let text = out.status.service_error_text.expect("translatable");
            assert_eq!(text.key, "watch.service_off");
            assert_eq!(out.status.service_error, Some(text.english()));

            let other = WatchedSafe { chain_id: 999_999, ..watched() };
            let out = check_safe(&up, "u", &SafeConfig::default(), &other, None).await;
            assert_eq!(out.status.service_error_text.map(|t| t.key).as_deref(), Some("watch.service_unsupported"));

            let broken = crate::upstream::fake::FakeUpstream::new();
            let out = check_safe(&broken, "u", &SafeConfig::default(), &watched(), None).await;
            assert_eq!(out.status.error_text.map(|t| t.key).as_deref(), Some("watch.unreadable"));
        }

        #[tokio::test]
        async fn unreadable_safe_is_an_error_status() {
            let up = crate::upstream::fake::FakeUpstream::new();
            let out = check_safe(&up, "u", &SafeConfig::default(), &watched(), None).await;
            assert!(out.status.error.is_some());
            assert!(out.snapshot.is_none());
        }
    }

    fn state() -> SafeState {
        SafeState {
            address: Address::repeat_byte(1),
            version: Some("1.4.1".into()),
            nonce: U256::from(5),
            threshold: 2,
            owners: vec![Address::repeat_byte(0xa), Address::repeat_byte(0xb), Address::repeat_byte(0xc)],
            singleton: Some(crate::safe::SINGLETONS[9].0),
            singleton_name: Some("Safe 1.4.1".into()),
            ..Default::default()
        }
    }

    fn severities(f: &[Finding]) -> Vec<(Severity, &str)> {
        f.iter().map(|f| (f.severity, f.kind)).collect()
    }

    #[test]
    fn unchanged_safe_has_no_findings() {
        assert!(diff_states(&state(), &state()).is_empty());
    }

    #[test]
    fn owner_swap_and_lower_threshold_are_dangerous() {
        let mut new = state();
        new.owners[2] = Address::repeat_byte(0xee);
        new.threshold = 1;
        let f = diff_states(&state(), &new);
        assert_eq!(
            severities(&f),
            vec![(Severity::Danger, "config_change"), (Severity::Warning, "config_change"), (Severity::Danger, "config_change")]
        );
        assert!(f[0].message.contains(&Address::repeat_byte(0xee).to_checksum(None)));
    }

    #[test]
    fn implementation_replaced_by_unknown_contract_is_dangerous() {
        let mut new = state();
        new.singleton = Some(Address::repeat_byte(0xba));
        new.singleton_name = None;
        let f = diff_states(&state(), &new);
        assert_eq!(severities(&f), vec![(Severity::Danger, "config_change")]);
    }

    #[test]
    fn a_module_guard_change_is_dangerous() {
        // Safe v1.5.0: removing the module guard lets modules act unchecked.
        let mut old = state();
        old.module_guard = Some(Address::repeat_byte(0x4d));
        let f = diff_states(&old, &state());
        assert_eq!(severities(&f), vec![(Severity::Danger, "config_change")]);
        assert!(f[0].message.contains("Module guard"), "{}", f[0].message);
    }

    #[test]
    fn executed_nonces_are_reported() {
        let mut new = state();
        new.nonce = U256::from(7);
        let f = diff_states(&state(), &new);
        assert_eq!(f[0].kind, "executed");
        assert_eq!(f[0].message, "Executed nonces 5–6");
    }
}
