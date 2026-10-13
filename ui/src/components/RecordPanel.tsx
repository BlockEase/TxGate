// Side panel describing one request: decoded transaction for estimate/send
// calls, raw request and response for everything.

import { Fragment, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Config, RequestRecord } from "../api";
import { clock, copyText, dateTime, eth, gwei, networkName, shortAddr, statusLabel, STATUS_TAG, tenderlyReady } from "../format";
import { SafeSection } from "./SafeParts";
import { allRisks, Authorizations, Effects, Params, PreviewNote, RiskList, tokenSymbols, useSimulate } from "./TxParts";
import { t } from "../i18n";

/** Label/value rows; rows whose value is null are left out. */
export function Facts({ rows, className = "kv" }: { rows: [string, ReactNode][]; className?: string }) {
  return (
    <div className={className}>
      {rows
        .filter(([, v]) => v !== null && v !== undefined && v !== "" && v !== false)
        .map(([k, v]) => (
          <Fragment key={k}>
            <span>{k}</span>
            <span>{v}</span>
          </Fragment>
        ))}
    </div>
  );
}

export function RecordPanel({
  record,
  config,
  onClose,
  showDate,
}: {
  record: RequestRecord;
  config: Config;
  onClose: () => void;
  showDate?: boolean;
}) {
  const { simulate, busy, error } = useSimulate(record.id);
  const [copied, setCopied] = useState(false);
  const detail = record.detail;
  const tx = detail?.txs[0];
  const call = tx?.analysis.call;

  const title = tx ? `${tx.fields.to ? shortAddr(tx.fields.to) : t("panel.new_contract")} · ${call?.function ?? call?.selector ?? t("panel.transfer")}` : record.method;
  const json = (v: unknown) => JSON.stringify(v, null, 2);

  return (
    <aside className="aside" aria-label={t("panel.label")}>
      <div className="aside-head">
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="card-kicker num">
            {record.method} · {showDate ? dateTime(record.ts_ms) : clock(record.ts_ms)} · {networkName(config, record.chain_id)}
          </div>
          <h3>{title}</h3>
        </div>
        <button className="btn btn-ghost" onClick={onClose}>
          {t("common.close")}
        </button>
      </div>
      <div>
        <span className={STATUS_TAG[record.status]}>{statusLabel(record.status)}</span>
      </div>

      {tx && (
        <>
          <Facts
            rows={[
              [t("panel.contract"), tx.fields.to ?? t("panel.deploys")],
              [t("panel.from"), tx.fields.from ?? "—"],
              [t("panel.value"), eth(tx.fields.value)],
              [t("panel.nonce"), tx.fields.nonce],
              [t("panel.gas_limit"), tx.fields.gas_limit?.toLocaleString("en-US")],
              [t("panel.max_fee"), tx.max_fee_per_gas && gwei(tx.max_fee_per_gas)],
              [t("panel.type"), tx.tx_type],
              [t("panel.tx_hash"), tx.hash && <span className="mono">{tx.hash}</span>],
            ]}
          />
          {record.category === "send" && <PreviewNote preview={detail?.preview ?? null} />}
          <Authorizations tx={tx} />
          <SafeSection tx={tx} />
          <Params tx={tx} />
          <Effects outcome={detail?.simulation ?? null} onSimulate={simulate} busy={busy} canSimulate={tenderlyReady(config)} tokenSymbols={tokenSymbols(tx)} />
          {error && <span className="error-text">{error}</span>}
          <RiskList risks={allRisks(tx)} />
          {!tx.enrichment && <span className="fine">{t("panel.checking")}</span>}
          {detail && detail.txs.length > 1 && <span className="text-muted">{t("panel.bundle", { count: detail.txs.length })}</span>}
          <div className="actions">
            <button className="btn btn-primary" onClick={() => openUrl(tx.simulator_url)}>
              {t("common.simulate_tenderly")}
            </button>
            {tx.fields.input !== "0x" && (
              <button
                className="btn btn-secondary"
                onClick={async () => {
                  await copyText(tx.fields.input);
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                }}
              >
                {copied ? t("common.copied") : t("panel.copy_calldata")}
              </button>
            )}
          </div>
        </>
      )}
      {detail?.decode_error && <span className="error-text">{t("panel.decode_error", { error: detail.decode_error })}</span>}

      <Facts
        rows={[
          [t("panel.client"), record.client],
          [t("panel.duration"), record.duration_ms === null ? "—" : `${record.duration_ms.toLocaleString("en-US")} ms`],
        ]}
      />
      <div className="section">
        <h6>{t("panel.request")}</h6>
        <pre className="code-block">{json(record.request)}</pre>
      </div>
      <div className="section">
        <h6>{t("panel.response")}</h6>
        <pre className="code-block">{record.response ? json(record.response) : t("panel.waiting")}</pre>
      </div>
    </aside>
  );
}
