// Modal shown while transactions are held. One transaction at a time, ordered
// by sender and nonce; approval requires an explicit acknowledgement.

import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { api, errorText, type Config, type RequestRecord } from "../api";
import { elapsed, eth, gwei, networkName, tenderlyReady } from "../format";
import { AccountSection, SafeSection } from "./SafeParts";
import { Authorizations, Effects, Params, PreviewNote, requestRisks, RiskList, tokenSymbols, useSimulate } from "./TxParts";
import { t } from "../i18n";
import { Address, OnChain } from "./Address";

/** Held transactions in the order they should be decided: per sender by nonce. */
export function orderQueue(pending: RequestRecord[]): RequestRecord[] {
  const nonce = (r: RequestRecord) => r.detail?.txs[0]?.fields.nonce ?? Number.MAX_SAFE_INTEGER;
  return [...pending].sort((a, b) => {
    const sa = `${a.chain_id}:${(a.from ?? "").toLowerCase()}`;
    const sb = `${b.chain_id}:${(b.from ?? "").toLowerCase()}`;
    if (sa === sb) return nonce(a) - nonce(b) || a.id - b.id;
    return a.id - b.id;
  });
}

/** A lower nonce from the same sender that is still held blocks this one. */
function blockingNonce(record: RequestRecord, pending: RequestRecord[]): number | null {
  const mine = record.detail?.txs[0]?.fields.nonce;
  if (mine === null || mine === undefined || !record.from) return null;
  const lower = pending
    .filter((p) => p.id !== record.id && p.chain_id === record.chain_id && p.from?.toLowerCase() === record.from?.toLowerCase())
    .map((p) => p.detail?.txs[0]?.fields.nonce)
    .filter((n): n is number => typeof n === "number" && n < mine);
  return lower.length ? Math.min(...lower) : null;
}

export function ApprovalDialog({
  pending,
  config,
  onDefer,
  onNotice,
}: {
  pending: RequestRecord[];
  config: Config;
  onDefer: () => void;
  /** Reports the outcome of a decision once the dialog has moved on. */
  onNotice: (text: string) => void;
}) {
  const queue = useMemo(() => orderQueue(pending), [pending]);
  const [index, setIndex] = useState(0);
  /** Which transaction of a bundle is shown in detail. */
  const [txIndex, setTxIndex] = useState(0);
  const [ack, setAck] = useState(false);
  const [busy, setBusy] = useState<"approve" | "reject" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  // The queue can shrink while the dialog is open (decisions, other devices).
  const pos = Math.max(0, Math.min(index, queue.length - 1));
  const current = queue[pos];
  const currentId = current?.id;
  const sim = useSimulate(currentId ?? 0);

  // Each transaction needs its own acknowledgement.
  useEffect(() => {
    setAck(false);
    setError(null);
    setBusy(null);
    setTxIndex(0);
  }, [currentId]);

  // Chain-state checks finish after the dialog opens and can add warnings
  // (an approval to an account without code, a signer who is not an owner…).
  // An acknowledgement given before a new warning appeared no longer counts.
  const riskKey = requestRisks(current?.detail?.txs ?? [])
        .filter((r) => r.severity !== "info")
        .map((r) => `${r.severity}:${r.code}:${r.message}`)
        .join("|");
  const seenRisks = useRef(riskKey);
  useEffect(() => {
    const before = new Set(seenRisks.current.split("|"));
    const added = riskKey.split("|").some((r) => r && !before.has(r));
    seenRisks.current = riskKey;
    if (added) setAck(false);
  }, [riskKey]);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDefer();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDefer]);

  if (!current) return null;
  const detail = current.detail;
  const txCount = detail?.txs.length ?? 0;
  const shown = Math.min(txIndex, Math.max(0, txCount - 1));
  const tx = detail?.txs[shown];
  const disconnected = current.status === "disconnected";
  const blockedBy = blockingNonce(current, pending);
  const risks = requestRisks(detail?.txs ?? []);
  if (detail?.decode_error) {
    risks.unshift({
      severity: "danger",
      code: "undecodable",
      message: t("dialog.undecodable", { error: detail.decode_error }),
    });
  }

  const decide = async (approve: boolean) => {
    setBusy(approve ? "approve" : "reject");
    setError(null);
    try {
      const result = await api.decide(current.id, approve);
      setIndex(0);
      if (result.status === "broadcast_failed") {
        onNotice(t("dialog.broadcast_failed", { error: upstreamError(result.response) }));
      } else if (result.status === "approved" && result.tx_hash) {
        onNotice(t("dialog.approved", { hash: `${result.tx_hash.slice(0, 10)}…` }));
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-labelledby="approval-title">
      <OnChain id={current.chain_id}>
      <div className="dialog approval">
        <div className="approval-head">
          <div className="dialog-title" id="approval-title">
            {current.method === "eth_sendBundle" ? t("dialog.title_bundle") : t("dialog.title")}
          </div>
          <span className="num note-accent">{disconnected ? t("dialog.wallet_left") : t("dialog.wallet_waiting", { time: elapsed(current.ts_ms, now) })}</span>
        </div>
        {queue.length > 1 && (
          <div className="queue-nav">
            <button
              className="btn btn-ghost btn-icon"
              style={{ width: 28, height: 28 }}
              aria-label={t("dialog.previous")}
              disabled={pos === 0}
              onClick={() => setIndex(pos - 1)}
            >
              <ChevronLeft size={16} />
            </button>
            <span className="num">
              {t("dialog.position", { n: pos + 1, total: queue.length })}
            </span>
            <button
              className="btn btn-ghost btn-icon"
              style={{ width: 28, height: 28 }}
              aria-label={t("dialog.next")}
              disabled={pos >= queue.length - 1}
              onClick={() => setIndex(pos + 1)}
            >
              <ChevronRight size={16} />
            </button>
          </div>
        )}
        <span className="text-muted">
          {t("dialog.intro", { method: current.method, client: current.client })}
          {detail && detail.attempts > 1 ? ` ${t("dialog.attempts", { count: detail.attempts })}` : ""}
        </span>

        {txCount > 1 && (
          <div className="queue-nav">
            <button className="btn btn-ghost btn-icon" style={{ width: 28, height: 28 }} aria-label={t("dialog.previous_tx")} disabled={shown === 0} onClick={() => setTxIndex(shown - 1)}>
              <ChevronLeft size={16} />
            </button>
            <span className="num">{t("dialog.bundle_position", { n: shown + 1, total: txCount })}</span>
            <button className="btn btn-ghost btn-icon" style={{ width: 28, height: 28 }} aria-label={t("dialog.next_tx")} disabled={shown >= txCount - 1} onClick={() => setTxIndex(shown + 1)}>
              <ChevronRight size={16} />
            </button>
          </div>
        )}
        {tx && (
          <div
            className="kv kv4"
            style={{
              borderTop: "1px solid var(--color-divider)",
              paddingTop: 12,
            }}
          >
            {/* Always sender, then recipient, then amount. */}
            <span>{t("panel.from")}</span>
            <span className="span3">
              <Address address={tx.fields.from} full />
            </span>
            <span>{t("panel.to")}</span>
            <span className="span3">
              {tx.fields.to ? <Address address={tx.fields.to} full /> : t("risk.contract_creation")}
              {tx.analysis.call.function ? ` · ${tx.analysis.call.function}` : tx.analysis.call.selector ? ` · ${tx.analysis.call.selector}` : ""}
            </span>
            <span>{t("panel.value")}</span>
            <span className="span3">{eth(tx.fields.value, current.chain_id)}</span>
            <span>{t("table.network")}</span>
            <span>
              {networkName(config, current.chain_id)} · {current.chain_id}
            </span>
            <span>{t("panel.nonce")}</span>
            <span>{tx.fields.nonce ?? "—"}</span>
            <span>{t("panel.max_fee")}</span>
            <span>{gwei(tx.max_fee_per_gas)}</span>
            {tx.tx_type && (
              <>
                <span>{t("panel.type")}</span>
                <span>{tx.tx_type}</span>
              </>
            )}
          </div>
        )}

        {blockedBy !== null && (
          <RiskList
            title={t("dialog.nonce_order_title")}
            risks={[
              {
                severity: "warning",
                code: "nonce_order",
                message: t("dialog.nonce_order", { nonce: blockedBy }),
              },
            ]}
          />
        )}
        <RiskList risks={risks} />
        {shown === 0 && <PreviewNote preview={detail?.preview ?? null} />}
        {tx && shown === 0 && (
          <Effects
            outcome={detail?.simulation ?? null}
            onSimulate={sim.simulate}
            busy={sim.busy}
            canSimulate={tenderlyReady(config)}
            tokenSymbols={tokenSymbols(tx)}
          />
        )}
        {tx && <Authorizations tx={tx} />}
        {tx && <AccountSection tx={tx} chainId={current.chain_id} />}
        {tx && <SafeSection tx={tx} chainId={current.chain_id} />}
        {tx && <Params tx={tx} />}
        {txCount > 1 && <span className="note-accent">{t("dialog.bundle", { count: txCount })}</span>}
        {disconnected && (
          <div className="note-accent">
            {t("dialog.disconnected")}
          </div>
        )}
        <div className="approval-foot">
          {(error ?? sim.error) && <span className="error-text">{error ?? sim.error}</span>}
          <label className="check">
            <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
            <span className="box" />
            {t("dialog.ack")}
          </label>
          <div className="dialog-actions" style={{ marginTop: 0, alignItems: "center" }}>
            {tx && (
              <button className="btn btn-ghost" style={{ marginRight: "auto" }} onClick={() => openUrl(tx.simulator_url)}>
                {t("common.simulate_tenderly")}
              </button>
            )}
            <button className="btn btn-ghost" onClick={onDefer} style={tx ? undefined : { marginRight: "auto" }}>
              {t("dialog.later")}
            </button>
            <button className="btn btn-secondary" onClick={() => decide(false)} disabled={busy !== null}>
              {busy === "reject" ? t("dialog.rejecting") : disconnected ? t("dialog.discard") : t("dialog.reject")}
            </button>
            <button className="btn btn-primary" onClick={() => decide(true)} disabled={!ack || busy !== null}>
              {busy === "approve" ? t("dialog.broadcasting") : t("dialog.approve")}
            </button>
          </div>
        </div>
      </div>
      </OnChain>
    </div>
  );
}

/** The upstream's error message from a JSON-RPC error response. */
function upstreamError(response: unknown): string {
  const err = (response as { error?: { message?: string } } | null)?.error;
  return err?.message ?? t("dialog.not_accepted");
}
