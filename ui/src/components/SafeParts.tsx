// Building blocks for Safe multisig transactions: the hashes a signer compares
// with the hardware wallet, the calls the Safe would make, and who signed.

import { Fragment, useState } from "react";
import type { CheckedSignature, InnerCall, SafeHashes, SafeState, SafeTxReport, TxView } from "../api";
import { copyText, eth, shortAddr, toBig } from "../format";
import { Facts } from "./RecordPanel";
import { RiskList } from "./TxParts";
import { t, tt } from "../i18n";

/** A hash split into groups of four, as hardware wallets show it. */
export function HashLine({ label, hash, primary }: { label: string; hash: string; primary?: boolean }) {
  const [copied, setCopied] = useState(false);
  const hex = hash.replace(/^0x/, "");
  const groups = hex.match(/.{1,4}/g) ?? [];
  return (
    <div className={`hash-line${primary ? " primary" : ""}`}>
      <span className="hash-label">{label}</span>
      <button
        className="hash-value mono"
        title={t("common.copy")}
        aria-label={t("safe.copy_label", { label })}
        onClick={async () => {
          await copyText(hash);
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        }}
      >
        <span className="hash-prefix">0x</span>
        {groups.map((g, i) => (
          <span key={i}>{g}</span>
        ))}
      </button>
      <span className="fine hash-copied" aria-live="polite">
        {copied ? t("common.copied") : ""}
      </span>
    </div>
  );
}

export function SafeHashBlock({ hashes, nonce }: { hashes: SafeHashes; nonce?: string }) {
  return (
    <div className="section">
      <h6>{nonce !== undefined ? t("safe.compare_nonce", { nonce: toBig(nonce).toString() }) : t("safe.compare")}</h6>
      <span className="text-muted">{t("safe.compare_hint")}</span>
      <div className="hash-block">
        <HashLine label={t("safe.domain_hash")} hash={hashes.domain_hash} />
        <HashLine label={t("safe.message_hash")} hash={hashes.message_hash} />
        <HashLine label={t("safe.safe_tx_hash")} hash={hashes.safe_tx_hash} primary />
      </div>
    </div>
  );
}

function CallRow({ c, index, numbered }: { c: InnerCall; index: number; numbered: boolean }) {
  const delegate = c.operation === 1;
  const fn = c.call.function ?? c.call.selector;
  return (
    <div className="safe-call">
      <div className="row" style={{ flexWrap: "wrap" }}>
        {numbered && <span className="text-muted num">{index + 1}.</span>}
        <span className={delegate ? "tag tag-outline" : "tag tag-neutral"}>{delegate ? t("safe.delegatecall") : t("safe.call")}</span>
        <span className="mono" title={c.to}>
          {c.to}
        </span>
      </div>
      <div className="args num">
        {fn && (
          <>
            <span>{t("safe.function")}</span>
            <span className="mono">{c.call.signature ?? fn}</span>
          </>
        )}
        {c.call.args.map((a) => (
          <Fragment key={a.name}>
            <span>{a.name}</span>
            <span className="mono">{a.value}</span>
          </Fragment>
        ))}
        {toBig(c.value) > 0n && (
          <>
            <span>{t("panel.value")}</span>
            <span>{eth(c.value)}</span>
          </>
        )}
        {!fn && toBig(c.value) === 0n && (
          <>
            <span>{t("safe.data")}</span>
            <span className="text-muted">{t("safe.none")}</span>
          </>
        )}
      </div>
    </div>
  );
}

export function SafeCalls({ calls, batch }: { calls: InnerCall[]; batch: string | null }) {
  return (
    <div className="section">
      <h6>{batch ? t("safe.does_batch", { count: calls.length, library: batch }) : t("safe.does")}</h6>
      {calls.map((c, i) => (
        <CallRow key={i} c={c} index={i} numbered={calls.length > 1} />
      ))}
    </div>
  );
}


export function Signatures({ title, sigs, threshold }: { title: string; sigs: CheckedSignature[]; threshold?: number }) {
  if (sigs.length === 0) return null;
  const valid = sigs.filter((s) => s.valid).length;
  return (
    <div className="section">
      <h6>
        {title}
        {threshold !== undefined && (
          <span className={valid >= threshold ? "tag tag-accent" : "tag tag-outline"}>
            {t("safe.valid_of", { valid, threshold })}
          </span>
        )}
      </h6>
      <div className="sig-grid">
        {sigs.map((s, i) => (
          <Fragment key={i}>
            <span className={s.valid ? "ok-text" : "error-text"}>{s.valid ? t("safe.valid") : t("safe.invalid")}</span>
            <span className="mono" title={s.signer ?? ""}>
              {s.signer ?? t("safe.unrecoverable")}
            </span>
            <span className="fine">
              {t(`safe.sig_kind.${s.kind}`)}
              {!s.is_owner && s.signer ? ` · ${t("safe.not_owner")}` : ""}
              {s.note ? ` · ${tt(s.note_text, s.note)}` : ""}
            </span>
          </Fragment>
        ))}
      </div>
    </div>
  );
}

export function SafeStateFacts({ state }: { state: SafeState }) {
  return (
    <Facts
      className="kv kv-wide"
      rows={[
        [t("safe.safe"), <span className="mono">{state.address}</span>],
        [t("safe.version"), state.version ?? t("safe.unknown")],
        [t("safe.owners"), t("safe.must_sign", { threshold: state.threshold, owners: state.owners.length })],
        [t("panel.nonce"), toBig(state.nonce).toString()],
        [t("safe.implementation"), state.singleton_name ?? <span className="error-text">{t("safe.not_official", { address: state.singleton ?? t("safe.unknown") })}</span>],
        [t("safe.guard"), state.guard && <span className="mono">{state.guard}</span>],
        [t("safe.modules"), state.modules.length > 0 && state.modules.map(shortAddr).join(", ")],
      ]}
    />
  );
}

/** Safe details of a decoded `execTransaction` or `approveHash` request. */
export function SafeSection({ tx }: { tx: TxView }) {
  const exec = tx.analysis.safe;
  const check = tx.enrichment?.safe;
  const approval = tx.enrichment?.safe_approval;
  if (!exec && !approval) return null;
  if (approval) {
    return (
      <div className="section">
        <h6>{t("safe.approves")}</h6>
        <SafeReportBody report={approval} showRisks={false} />
      </div>
    );
  }
  if (!exec) return null;
  return (
    <>
      <SafeCalls calls={exec.analysis.calls} batch={exec.analysis.batch} />
      {check?.state && <SafeStateFacts state={check.state} />}
      {check?.hashes && <SafeHashBlock hashes={check.hashes} nonce={check.state?.nonce} />}
      {check && <Signatures title={t("safe.sigs_in_tx")} sigs={check.signatures} threshold={check.state?.threshold} />}
      {!tx.enrichment && <span className="fine">{t("safe.reading")}</span>}
    </>
  );
}

/** Everything about a reviewed SafeTx. */
export function SafeReportBody({ report, showRisks = true }: { report: SafeTxReport; showRisks?: boolean }) {
  const svc = report.service;
  return (
    <>
      {showRisks && <RiskList risks={sortRisks(report.risks)} />}
      <SafeCalls calls={report.analysis.calls} batch={report.analysis.batch} />
      {toBig(report.tx.gas_price) > 0n && (
        <Facts
          rows={[
            [t("safe.gas_price"), toBig(report.tx.gas_price).toString()],
            [t("safe.gas_token"), report.tx.gas_token],
            [t("safe.refund_to"), report.tx.refund_receiver],
          ]}
        />
      )}
      <SafeHashBlock hashes={report.hashes} nonce={report.tx.nonce} />
      {report.state && <SafeStateFacts state={report.state} />}
      {svc && (
        <Facts
          className="kv kv-wide"
          rows={[
            [t("safe.proposed_by"), svc.proposer && <span className="mono">{svc.proposer}</span>],
            [t("safe.proposed_at"), svc.submission_date?.replace("T", " ").slice(0, 19)],
            [t("safe.service_note"), svc.origin && <span className="text-muted">{svc.origin}</span>],
          ]}
        />
      )}
      <Signatures title={t("safe.sigs_collected")} sigs={report.confirmations} threshold={report.state?.threshold} />
    </>
  );
}

export function sortRisks<T extends { severity: "info" | "warning" | "danger" }>(risks: T[]): T[] {
  const order = { danger: 0, warning: 1, info: 2 } as const;
  return [...risks].sort((a, b) => order[a.severity] - order[b.severity]);
}
