// Modal shown while transactions are held. One transaction at a time, ordered
// by sender and nonce; approval requires an explicit acknowledgement.

import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { api, errorText, type Config, type RequestRecord } from "../api";
import { elapsed, eth, gwei, networkName, shortAddr, tenderlyReady } from "../format";
import { allRisks, Effects, Params, PreviewNote, RiskList, tokenSymbols, useSimulate } from "./TxParts";

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
  }, [currentId]);

  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
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
  const tx = detail?.txs[0];
  const disconnected = current.status === "disconnected";
  const blockedBy = blockingNonce(current, pending);
  const risks = tx ? allRisks(tx) : [];
  if (detail?.decode_error) {
    risks.unshift({
      severity: "danger",
      code: "undecodable",
      message: `TxGate could not decode this transaction: ${detail.decode_error}`,
    });
  }

  const decide = async (approve: boolean) => {
    setBusy(approve ? "approve" : "reject");
    setError(null);
    try {
      const result = await api.decide(current.id, approve);
      setIndex(0);
      if (result.status === "broadcast_failed") {
        onNotice(`Broadcast failed: ${upstreamError(result.response)}. The wallet received the same error.`);
      } else if (result.status === "approved" && result.tx_hash) {
        onNotice(`Approved and broadcast ${result.tx_hash.slice(0, 10)}…`);
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-labelledby="approval-title">
      <div className="dialog approval">
        <div className="approval-head">
          <div className="dialog-title" id="approval-title">
            {current.method === "eth_sendBundle" ? "Bundle held for approval" : "Transaction held for approval"}
          </div>
          <span className="num note-accent">{disconnected ? "Wallet stopped waiting" : `Wallet waiting ${elapsed(current.ts_ms, now)}`}</span>
        </div>
        {queue.length > 1 && (
          <div className="queue-nav">
            <button
              className="btn btn-ghost btn-icon"
              style={{ width: 28, height: 28 }}
              aria-label="Previous"
              disabled={pos === 0}
              onClick={() => setIndex(pos - 1)}
            >
              <ChevronLeft size={16} />
            </button>
            <span className="num">
              {pos + 1} of {queue.length} held
            </span>
            <button
              className="btn btn-ghost btn-icon"
              style={{ width: 28, height: 28 }}
              aria-label="Next"
              disabled={pos >= queue.length - 1}
              onClick={() => setIndex(pos + 1)}
            >
              <ChevronRight size={16} />
            </button>
          </div>
        )}
        <span className="text-muted">
          {current.method} from {current.client}. Nothing is sent to any upstream until you approve.
          {detail && detail.attempts > 1 ? ` The wallet has submitted it ${detail.attempts} times.` : ""}
        </span>

        {tx && (
          <div
            className="kv kv4"
            style={{
              borderTop: "1px solid var(--color-divider)",
              paddingTop: 12,
            }}
          >
            <span>Network</span>
            <span>
              {networkName(config, current.chain_id)} · {current.chain_id}
            </span>
            <span>Nonce</span>
            <span>{tx.fields.nonce ?? "—"}</span>
            <span>Signer</span>
            <span title={tx.fields.from ?? ""}>{shortAddr(tx.fields.from)}</span>
            <span>Max fee</span>
            <span>{gwei(tx.max_fee_per_gas)}</span>
            <span>Calls</span>
            <span className="span3" title={tx.fields.to ?? ""}>
              {tx.fields.to ? tx.fields.to : "Deploys a new contract"}
              {tx.analysis.call.function ? ` · ${tx.analysis.call.function}` : tx.analysis.call.selector ? ` · ${tx.analysis.call.selector}` : ""}
            </span>
            <span>Sends</span>
            <span className="span3">{eth(tx.fields.value)}</span>
            {tx.tx_type && (
              <>
                <span>Type</span>
                <span className="span3">{tx.tx_type}</span>
              </>
            )}
          </div>
        )}

        {blockedBy !== null && (
          <RiskList
            title="Approve in nonce order"
            risks={[
              {
                severity: "warning",
                code: "nonce_order",
                message: `Nonce ${blockedBy} from the same account is also held. Decide it first, or this transaction will wait in the mempool.`,
              },
            ]}
          />
        )}
        <RiskList risks={risks} />
        <PreviewNote preview={detail?.preview ?? null} />
        {tx && (
          <Effects
            outcome={detail?.simulation ?? null}
            onSimulate={sim.simulate}
            busy={sim.busy}
            canSimulate={tenderlyReady(config)}
            tokenSymbols={tokenSymbols(tx)}
          />
        )}
        {tx && <Params tx={tx} />}
        {detail && detail.txs.length > 1 && (
          <span className="note-accent">
            This bundle has {detail.txs.length} transactions. Only the first is shown in detail; approving broadcasts all of them.
          </span>
        )}
        {disconnected && (
          <div className="note-accent">
            The wallet stopped waiting and may show this transaction as failed. The signed transaction is still held here: approving broadcasts it as usual,
            discarding means it is never sent.
          </div>
        )}
        <div className="approval-foot">
          {(error ?? sim.error) && <span className="error-text">{error ?? sim.error}</span>}
          <label className="check">
            <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
            <span className="box" />I have checked the details above and I started this transaction
          </label>
          <div className="dialog-actions" style={{ marginTop: 0, alignItems: "center" }}>
            {tx && (
              <button className="btn btn-ghost" style={{ marginRight: "auto" }} onClick={() => openUrl(tx.simulator_url)}>
                Simulate in Tenderly
              </button>
            )}
            <button className="btn btn-ghost" onClick={onDefer} style={tx ? undefined : { marginRight: "auto" }}>
              Review later
            </button>
            <button className="btn btn-secondary" onClick={() => decide(false)} disabled={busy !== null}>
              {busy === "reject" ? "Rejecting…" : disconnected ? "Discard transaction" : "Reject"}
            </button>
            <button className="btn btn-primary" onClick={() => decide(true)} disabled={!ack || busy !== null}>
              {busy === "approve" ? "Broadcasting…" : "Approve and broadcast"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** The upstream's error message from a JSON-RPC error response. */
function upstreamError(response: unknown): string {
  const err = (response as { error?: { message?: string } } | null)?.error;
  return err?.message ?? "the upstream did not accept the transaction";
}
