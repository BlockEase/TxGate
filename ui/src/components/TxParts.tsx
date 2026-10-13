// Building blocks for showing a decoded transaction: parameters, simulated
// effects, risk flags and the estimate/send comparison.

import { useState } from "react";
import { AlertTriangle, Info, ShieldAlert } from "lucide-react";
import { api, errorText, type PreviewMatch, type Risk, type SimulationOutcome, type TxView } from "../api";
import { clock, toBig } from "../format";
import { t, tn } from "../i18n";
import { Address, EngineText, WithAddresses } from "./Address";

/** Static risks plus chain-state risks, most severe first. */
export function allRisks(tx: TxView): Risk[] {
  const order = { danger: 0, warning: 1, info: 2 } as const;
  return [...tx.analysis.risks, ...(tx.enrichment?.risks ?? [])].sort((a, b) => order[a.severity] - order[b.severity]);
}

/** Risks of every transaction a request broadcasts, most severe first. In a
 *  bundle each risk names its transaction, so none of them goes unseen. */
export function requestRisks(txs: TxView[]): Risk[] {
  if (txs.length === 1) return allRisks(txs[0]);
  const order = { danger: 0, warning: 1, info: 2 } as const;
  return txs
    .flatMap((tx, i) =>
      allRisks(tx).map((r) =>
        r.text
          ? { ...r, text: { ...r.text, prefix: [{ key: "dialog.bundle_prefix", params: { n: String(i + 1) } }, ...(r.text.prefix ?? [])] } }
          : { ...r, message: t("dialog.bundle_risk", { n: i + 1, message: r.message }) },
      ),
    )
    .sort((a, b) => order[a.severity] - order[b.severity]);
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
  const tok = tx.enrichment?.token;
  return tok?.symbol ? { [tok.address.toLowerCase()]: tok.symbol } : {};
}

export function RiskList({ risks, title }: { risks: Risk[]; title?: string }) {
  if (risks.length === 0) return null;
  const serious = risks.some((r) => r.severity !== "info");
  const body = risks.map((r, i) => (
    <div key={i} className={`risk ${r.severity}`}>
      {r.severity === "danger" ? <ShieldAlert size={14} /> : r.severity === "warning" ? <AlertTriangle size={14} /> : <Info size={14} />}
      <span>
        <EngineText text={r.text} fallback={r.message} />
      </span>
    </div>
  ));
  if (!serious) return <div className="section">{body}</div>;
  return (
    <div className="attention" role="alert">
      <span className="attention-title">{title ?? t("risk.needs_attention")}</span>
      {body}
    </div>
  );
}

const same = (a: string | null | undefined, b: string | null | undefined) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** EIP-7702 authorizations: which accounts hand their code to which contract. */
export function Authorizations({ tx }: { tx: TxView }) {
  const list = tx.authorizations;
  if (list.length === 0) return null;
  return (
    <div className="section">
      <h6>{list.length === 1 ? t("auth.title_one") : t("auth.title", { count: list.length })}</h6>
      <span className="text-muted">{t("auth.explain")}</span>
      {list.map((a, i) => {
        const revokes = toBig(a.address) === 0n;
        const anyChain = toBig(a.chain_id) === 0n;
        const isSender = same(a.authority, tx.fields.from);
        return (
          <div className="safe-call" key={i}>
            <div className="row" style={{ flexWrap: "wrap" }}>
              <span className={revokes ? "tag tag-neutral" : "tag tag-outline"}>{revokes ? t("auth.removes") : t("auth.delegates")}</span>
              {anyChain && <span className="tag tag-outline">{t("auth.any_chain")}</span>}
            </div>
            <div className="args num">
              <span>{t("auth.account")}</span>
              <span>
                {a.authority ? <Address address={a.authority} full /> : <span className="error-text">{t("auth.unknown")}</span>}
                {isSender && <span className="text-muted"> · {t("auth.is_sender")}</span>}
                {a.authority && !isSender && <span className="text-muted"> · {t("auth.not_sender")}</span>}
              </span>
              {!revokes && (
                <>
                  <span>{t("auth.delegate_to")}</span>
                  <span><Address address={a.address} full /></span>
                </>
              )}
              <span>{t("auth.valid_on")}</span>
              <span className={anyChain ? "error-text" : undefined}>
                {anyChain ? t("auth.every_chain") : t("format.chain", { id: toBig(a.chain_id).toString() })}
              </span>
              <span>{t("panel.nonce")}</span>
              <span>{a.nonce}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function Params({ tx }: { tx: TxView }) {
  const call = tx.analysis.call;
  const formatted = new Map(tx.enrichment?.formatted_args ?? []);
  if (!call.selector) return null;
  return (
    <div className="section">
      <h6>{t("call.title", { name: call.function ?? call.selector })}</h6>
      {call.signature ? <span className="fine mono">{call.signature}</span> : <span className="fine">{t("call.unknown")}</span>}
      {call.args.length > 0 && (
        <div className="args num">
          {call.args.map((a) => (
            <ArgRow key={a.name} name={a.name} value={localAmount(formatted.get(a.name) ?? a.value)} raw={a.value} />
          ))}
        </div>
      )}
    </div>
  );
}

/** The engine formats unlimited amounts as "Unlimited SYMBOL". */
function localAmount(v: string): string {
  return v === "Unlimited" || v.startsWith("Unlimited ") ? t("effects.unlimited") + v.slice("Unlimited".length) : v;
}

function ArgRow({ name, value, raw }: { name: string; value: string; raw: string }) {
  return (
    <>
      <span>{name}</span>
      <span title={raw}>{/^0x[0-9a-fA-F]{40}$/.test(raw) ? <Address address={raw} full /> : value}</span>
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
        {t("effects.title")}
        {outcome && onSimulate && (
          <button className="btn btn-ghost" style={{ marginLeft: "auto", fontSize: 12 }} onClick={onSimulate} disabled={busy}>
            {busy ? t("effects.simulating") : t("effects.again")}
          </button>
        )}
      </h6>
      {!outcome && (
        <div className="row">
          {canSimulate ? (
            <>
              <span className="text-muted" style={{ flex: 1 }}>
                {busy ? t("effects.simulating_tenderly") : t("effects.not_yet")}
              </span>
              {onSimulate && (
                <button className="btn btn-secondary" onClick={onSimulate} disabled={busy}>
                  {t("effects.simulate")}
                </button>
              )}
            </>
          ) : (
            <span className="text-muted">{t("effects.no_credentials")}</span>
          )}
        </div>
      )}
      {outcome?.status === "error" && <span className="error-text">{t("effects.failed", { error: outcome.message })}</span>}
      {outcome?.status === "ok" && <SimulationView outcome={outcome} symbols={tokenSymbols} />}
    </div>
  );
}

function SimulationView({ outcome, symbols }: { outcome: Extract<SimulationOutcome, { status: "ok" }>; symbols: Record<string, string> }) {
  const r = outcome.result;
  const moves = r.asset_changes.filter((c) => c.direction !== "other");
  return (
    <>
      {!r.success && <span className="error-text">{t("effects.reverts", { error: r.error ?? "execution reverted" })}</span>}
      {r.success && moves.length === 0 && r.approvals.length === 0 && <span className="text-muted">{t("effects.no_changes")}</span>}
      {moves.map((c, i) => (
        <div className="effect" key={`m${i}`}>
          <span className="kind">{c.direction === "out" ? t("effects.lose") : t("effects.receive")}</span>
          <span className="amt">{c.amount}</span>
          <span>{c.token_address ? <Address address={c.token_address} label={c.symbol} /> : c.symbol}</span>
          <span className="note">{c.dollar_value ? `≈ $${Number(c.dollar_value).toLocaleString("en-US", { maximumFractionDigits: 2 })}` : ""}</span>
        </div>
      ))}
      {r.approvals.map((a, i) => (
        <div className="effect" key={`a${i}`}>
          <span className="kind">{t("effects.approve")}</span>
          <span className="amt">{approvalAmount(a.amount)}</span>
          <span>
            <Address address={a.token_address} label={(a.token_address && symbols[a.token_address.toLowerCase()]) || undefined} />
          </span>
          <span className="note">{tn("effects.to", { address: <Address address={a.spender} /> })}</span>
        </div>
      ))}
      <span className="fine">
        {r.block_number ? t("effects.footer_block", { block: r.block_number.toLocaleString("en-US") }) : t("effects.footer")} · {clock(outcome.ts_ms)}
        {r.gas_used ? ` · ${t("effects.gas", { gas: r.gas_used.toLocaleString("en-US") })}` : ""}
      </span>
    </>
  );
}

function approvalAmount(raw: string): string {
  if (raw === "all") return t("effects.all_tokens");
  try {
    return BigInt(raw) >= 2n ** 159n ? t("effects.unlimited") : BigInt(raw).toLocaleString("en-US");
  } catch {
    return raw;
  }
}


export function PreviewNote({ preview }: { preview: PreviewMatch | null }) {
  if (!preview) return null;
  if (preview.kind === "matched") {
    return <span className="text-muted">{t("preview.matched", { time: clock(preview.at_ms) })}</span>;
  }
  if (preview.kind === "no_preview") {
    return <span className="text-muted">{t("preview.none")}</span>;
  }
  return (
    <div className="attention" role="alert">
      <span className="attention-title">{t("preview.mismatch_title")}</span>
      <span className="note-accent">{t("preview.mismatch_body", { time: clock(preview.at_ms) })}</span>
      {preview.diffs.map((d) => (
        <div className="diff" key={d.field}>
          <span className="text-muted" style={{ gridColumn: "1 / -1" }}>
            {t(`preview.field.${d.field}`)}
          </span>
          <span className="text-muted">{t("preview.previewed")}</span>
          <span className="mono">
            <WithAddresses text={d.previewed} full />
          </span>
          <span className="text-muted">{t("preview.signed")}</span>
          <span className="mono">
            <WithAddresses text={d.signed} full />
          </span>
        </div>
      ))}
    </div>
  );
}
