// Building blocks for showing a decoded transaction: parameters, simulated
// effects, risk flags and the estimate/send comparison.

import { useState } from "react";
import { AlertTriangle, Info, ShieldAlert } from "lucide-react";
import { api, errorText, type PreviewMatch, type Risk, type SimulationOutcome, type TxView } from "../api";
import { clock, shortAddr } from "../format";

/** Static risks plus chain-state risks, most severe first. */
export function allRisks(tx: TxView): Risk[] {
  const order = { danger: 0, warning: 1, info: 2 } as const;
  return [...tx.analysis.risks, ...(tx.enrichment?.risks ?? [])].sort((a, b) => order[a.severity] - order[b.severity]);
}

/** Runs a Tenderly simulation for a request; the result arrives as a record update. */
export function useSimulate(id: number) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const simulate = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.simulate(id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return { simulate, busy, error };
}

/** Known token symbols by lower-case address, from chain-state enrichment. */
export function tokenSymbols(tx: TxView): Record<string, string> {
  const t = tx.enrichment?.token;
  return t?.symbol ? { [t.address.toLowerCase()]: t.symbol } : {};
}

export function RiskList({ risks, title }: { risks: Risk[]; title?: string }) {
  if (risks.length === 0) return null;
  const serious = risks.some((r) => r.severity !== "info");
  const body = risks.map((r, i) => (
    <div key={i} className={`risk ${r.severity}`}>
      {r.severity === "danger" ? <ShieldAlert size={14} /> : r.severity === "warning" ? <AlertTriangle size={14} /> : <Info size={14} />}
      <span>{r.message}</span>
    </div>
  ));
  if (!serious) return <div className="section">{body}</div>;
  return (
    <div className="attention" role="alert">
      <span className="attention-title">{title ?? "Needs attention"}</span>
      {body}
    </div>
  );
}

export function Params({ tx }: { tx: TxView }) {
  const call = tx.analysis.call;
  const formatted = new Map(tx.enrichment?.formatted_args ?? []);
  if (!call.selector) return null;
  return (
    <div className="section">
      <h6>Call · {call.function ?? call.selector}</h6>
      {call.signature ? <span className="fine mono">{call.signature}</span> : <span className="fine">Unrecognised function. Only the selector is known.</span>}
      {call.args.length > 0 && (
        <div className="args num">
          {call.args.map((a) => (
            <ArgRow key={a.name} name={a.name} value={formatted.get(a.name) ?? a.value} raw={a.value} />
          ))}
        </div>
      )}
    </div>
  );
}

function ArgRow({ name, value, raw }: { name: string; value: string; raw: string }) {
  return (
    <>
      <span>{name}</span>
      <span title={raw}>{value}</span>
    </>
  );
}

export function Effects({
  outcome,
  onSimulate,
  busy,
  canSimulate,
  tokenSymbols = {},
}: {
  outcome: SimulationOutcome | null;
  onSimulate?: () => void;
  busy?: boolean;
  canSimulate: boolean;
  tokenSymbols?: Record<string, string>;
}) {
  return (
    <div className="section">
      <h6>
        Effects after execution
        {outcome && onSimulate && (
          <button className="btn btn-ghost" style={{ marginLeft: "auto", fontSize: 12 }} onClick={onSimulate} disabled={busy}>
            {busy ? "Simulating…" : "Simulate again"}
          </button>
        )}
      </h6>
      {!outcome && (
        <div className="row">
          {canSimulate ? (
            <>
              <span className="text-muted" style={{ flex: 1 }}>
                {busy ? "Simulating with Tenderly…" : "Not simulated yet."}
              </span>
              {onSimulate && (
                <button className="btn btn-secondary" onClick={onSimulate} disabled={busy}>
                  Simulate
                </button>
              )}
            </>
          ) : (
            <span className="text-muted">Add Tenderly credentials in Settings to see balance and approval changes here.</span>
          )}
        </div>
      )}
      {outcome?.status === "error" && <span className="error-text">Simulation failed: {outcome.message}</span>}
      {outcome?.status === "ok" && <SimulationView outcome={outcome} symbols={tokenSymbols} />}
    </div>
  );
}

function SimulationView({ outcome, symbols }: { outcome: Extract<SimulationOutcome, { status: "ok" }>; symbols: Record<string, string> }) {
  const r = outcome.result;
  const moves = r.asset_changes.filter((c) => c.direction !== "other");
  return (
    <>
      {!r.success && <span className="error-text">Reverts: {r.error ?? "execution reverted"}. Nothing would change on-chain except the gas fee.</span>}
      {r.success && moves.length === 0 && r.approvals.length === 0 && <span className="text-muted">No token or ETH balance changes for the sender.</span>}
      {moves.map((c, i) => (
        <div className="effect" key={`m${i}`}>
          <span className="kind">{c.direction === "out" ? "Lose" : "Receive"}</span>
          <span className="amt">{c.amount}</span>
          <span>{c.symbol}</span>
          <span className="note">{c.dollar_value ? `≈ $${Number(c.dollar_value).toLocaleString("en-US", { maximumFractionDigits: 2 })}` : ""}</span>
        </div>
      ))}
      {r.approvals.map((a, i) => (
        <div className="effect" key={`a${i}`}>
          <span className="kind">Approve</span>
          <span className="amt">{approvalAmount(a.amount)}</span>
          <span title={a.token_address ?? ""}>{(a.token_address && symbols[a.token_address.toLowerCase()]) || shortAddr(a.token_address)}</span>
          <span className="note">to {shortAddr(a.spender)}</span>
        </div>
      ))}
      <span className="fine">
        Tenderly simulation{r.block_number ? ` at block ${r.block_number.toLocaleString("en-US")}` : ""} · {clock(outcome.ts_ms)}
        {r.gas_used ? ` · ${r.gas_used.toLocaleString("en-US")} gas` : ""}
      </span>
    </>
  );
}

function approvalAmount(raw: string): string {
  if (raw === "all") return "All tokens";
  try {
    return BigInt(raw) >= 2n ** 159n ? "Unlimited" : BigInt(raw).toLocaleString("en-US");
  } catch {
    return raw;
  }
}

export function PreviewNote({ preview }: { preview: PreviewMatch | null }) {
  if (!preview) return null;
  if (preview.kind === "matched") {
    return <span className="text-muted">Matches the eth_estimateGas preview from {clock(preview.at_ms)}.</span>;
  }
  if (preview.kind === "no_preview") {
    return <span className="text-muted">The wallet did not preview this transaction with eth_estimateGas.</span>;
  }
  return (
    <div className="attention" role="alert">
      <span className="attention-title">Signed transaction differs from the preview</span>
      <span className="note-accent">
        At {clock(preview.at_ms)} the wallet estimated a different transaction. The dApp front-end may have been tampered with.
      </span>
      {preview.diffs.map((d) => (
        <div className="diff" key={d.field}>
          <span className="text-muted">{d.field}</span>
          <span />
          <span className="text-muted">preview</span>
          <span className="mono">{d.previewed}</span>
          <span className="text-muted">signed</span>
          <span className="mono">{d.signed}</span>
        </div>
      ))}
    </div>
  );
}
