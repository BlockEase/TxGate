//! Correlates `eth_estimateGas` previews with the transaction later sent.
//!
//! Wallets usually estimate gas right before asking the user to sign. If the
//! transaction that arrives for broadcast differs from what was previewed, the
//! dApp front-end (or something between it and the wallet) may have swapped it.

use std::collections::VecDeque;

use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

use crate::decode::TxFields;

/// Upper bound on remembered previews; old ones fall out first.
const MAX_PREVIEWS: usize = 512;

#[derive(Debug, Clone)]
struct Preview {
    request_id: u64,
    at_ms: u64,
    chain_id: u64,
    from: Option<Address>,
    to: Option<Address>,
    value: U256,
    input: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub previewed: String,
    pub signed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewMatch {
    /// The sent transaction is identical (to / value / data) to a recent preview.
    Matched { request_id: u64, at_ms: u64 },
    /// The sender previewed something else recently: possible front-end tampering.
    Mismatch { request_id: u64, at_ms: u64, diffs: Vec<FieldDiff> },
    /// No preview from this sender within the window.
    NoPreview,
}

#[derive(Debug)]
pub struct PreviewBook {
    window_ms: u64,
    previews: VecDeque<Preview>,
}

impl PreviewBook {
    pub fn new(window_secs: u64) -> Self {
        Self { window_ms: window_secs * 1000, previews: VecDeque::new() }
    }

    pub fn set_window(&mut self, window_secs: u64) {
        self.window_ms = window_secs * 1000;
    }

    pub fn record(&mut self, request_id: u64, at_ms: u64, chain_id: u64, tx: &TxFields) {
        if self.previews.len() == MAX_PREVIEWS {
            self.previews.pop_front();
        }
        self.previews.push_back(Preview {
            request_id,
            at_ms,
            chain_id,
            from: tx.from,
            to: tx.to,
            value: tx.value,
            input: tx.input.clone(),
        });
    }

    /// Compares a transaction about to be broadcast with recent previews.
    pub fn check(&self, now_ms: u64, chain_id: u64, tx: &TxFields) -> PreviewMatch {
        let candidates: Vec<&Preview> = self
            .previews
            .iter()
            .rev()
            .filter(|p| p.chain_id == chain_id)
            .filter(|p| now_ms.saturating_sub(p.at_ms) <= self.window_ms)
            // A preview without `from` could belong to anyone; keep it as a candidate.
            .filter(|p| p.from.is_none() || p.from == tx.from)
            .collect();

        if let Some(p) = candidates.iter().find(|p| p.to == tx.to && p.value == tx.value && p.input == tx.input) {
            return PreviewMatch::Matched { request_id: p.request_id, at_ms: p.at_ms };
        }
        // Only previews explicitly from this sender count as evidence of tampering.
        let Some(p) = candidates.iter().find(|p| p.from.is_some()) else {
            return PreviewMatch::NoPreview;
        };
        let mut diffs = Vec::new();
        let fmt_to = |a: Option<Address>| a.map(|a| a.to_checksum(None)).unwrap_or_else(|| "(contract creation)".into());
        if p.to != tx.to {
            diffs.push(FieldDiff { field: "to".into(), previewed: fmt_to(p.to), signed: fmt_to(tx.to) });
        }
        if p.value != tx.value {
            diffs.push(FieldDiff { field: "value".into(), previewed: p.value.to_string(), signed: tx.value.to_string() });
        }
        if p.input != tx.input {
            diffs.push(FieldDiff { field: "data".into(), previewed: p.input.to_string(), signed: tx.input.to_string() });
        }
        PreviewMatch::Mismatch { request_id: p.request_id, at_ms: p.at_ms, diffs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    const ME: Address = address!("3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE");
    const ROUTER: Address = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");
    const EVIL: Address = address!("9e1F000000000000000000000000000000003c07");

    fn tx(to: Address, data: &[u8]) -> TxFields {
        TxFields { from: Some(ME), to: Some(to), input: Bytes::copy_from_slice(data), ..Default::default() }
    }

    #[test]
    fn identical_send_matches_preview() {
        let mut book = PreviewBook::new(600);
        book.record(7, 1_000, 1, &tx(ROUTER, &[1, 2]));
        assert_eq!(book.check(5_000, 1, &tx(ROUTER, &[1, 2])), PreviewMatch::Matched { request_id: 7, at_ms: 1_000 });
    }

    #[test]
    fn different_target_is_a_mismatch_with_diffs() {
        let mut book = PreviewBook::new(600);
        book.record(7, 1_000, 1, &tx(ROUTER, &[1, 2]));
        let PreviewMatch::Mismatch { diffs, .. } = book.check(2_000, 1, &tx(EVIL, &[9])) else { panic!("expected mismatch") };
        let fields: Vec<_> = diffs.iter().map(|d| d.field.as_str()).collect();
        assert_eq!(fields, ["to", "data"]);
    }

    #[test]
    fn previews_outside_the_window_or_chain_are_ignored() {
        let mut book = PreviewBook::new(10);
        book.record(1, 0, 1, &tx(ROUTER, &[1]));
        book.record(2, 50_000, 8453, &tx(ROUTER, &[1]));
        assert_eq!(book.check(60_000, 1, &tx(ROUTER, &[1])), PreviewMatch::NoPreview);
    }

    #[test]
    fn previews_from_other_senders_are_ignored() {
        let mut book = PreviewBook::new(600);
        let mut other = tx(ROUTER, &[1]);
        other.from = Some(EVIL);
        book.record(1, 0, 1, &other);
        assert_eq!(book.check(1_000, 1, &tx(ROUTER, &[1])), PreviewMatch::NoPreview);
    }

    #[test]
    fn latest_exact_match_wins_over_older_mismatch() {
        let mut book = PreviewBook::new(600);
        book.record(1, 0, 1, &tx(EVIL, &[9]));
        book.record(2, 100, 1, &tx(ROUTER, &[1]));
        assert!(matches!(book.check(200, 1, &tx(ROUTER, &[1])), PreviewMatch::Matched { request_id: 2, .. }));
    }
}
