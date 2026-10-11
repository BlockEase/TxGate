// Side panel describing one request: decoded transaction for estimate/send
// calls, raw request and response for everything.

import { Fragment, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Config, RequestRecord } from "../api";
import { clock, copyText, dateTime, eth, gwei, networkName, shortAddr, STATUS_LABEL, STATUS_TAG, tenderlyReady } from "../format";
import { allRisks, Effects, Params, PreviewNote, RiskList, tokenSymbols, useSimulate } from "./TxParts";

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

  const title = tx ? `${tx.fields.to ? shortAddr(tx.fields.to) : "New contract"} · ${call?.function ?? call?.selector ?? "transfer"}` : record.method;
  const json = (v: unknown) => JSON.stringify(v, null, 2);

  return (
    <aside className="aside" aria-label="Request details">
      <div className="aside-head">
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="card-kicker num">
            {record.method} · {showDate ? dateTime(record.ts_ms) : clock(record.ts_ms)} · {networkName(config, record.chain_id)}
          </div>
          <h3>{title}</h3>
        </div>
        <button className="btn btn-ghost" onClick={onClose}>
          Close
        </button>
      </div>
      <div>
        <span className={STATUS_TAG[record.status]}>{STATUS_LABEL[record.status]}</span>
      </div>

      {tx && (
        <>
          <Facts
            rows={[
              ["Contract", tx.fields.to ?? "— (deploys a contract)"],
              ["From", tx.fields.from ?? "—"],
              ["Value", eth(tx.fields.value)],
              ["Nonce", tx.fields.nonce],
              ["Gas limit", tx.fields.gas_limit?.toLocaleString("en-US")],
              ["Max fee", tx.max_fee_per_gas && gwei(tx.max_fee_per_gas)],
              ["Type", tx.tx_type],
              ["Tx hash", tx.hash && <span className="mono">{tx.hash}</span>],
            ]}
          />
          {record.category === "send" && <PreviewNote preview={detail?.preview ?? null} />}
          <Params tx={tx} />
          <Effects outcome={detail?.simulation ?? null} onSimulate={simulate} busy={busy} canSimulate={tenderlyReady(config)} tokenSymbols={tokenSymbols(tx)} />
          {error && <span className="error-text">{error}</span>}
          <RiskList risks={allRisks(tx)} />
          {!tx.enrichment && <span className="fine">Checking addresses and nonce with the read upstream…</span>}
          {detail && detail.txs.length > 1 && <span className="text-muted">This bundle contains {detail.txs.length} transactions; the first one is shown.</span>}
          <div className="actions">
            <button className="btn btn-primary" onClick={() => openUrl(tx.simulator_url)}>
              Simulate in Tenderly
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
                {copied ? "Copied" : "Copy calldata"}
              </button>
            )}
          </div>
        </>
      )}
      {detail?.decode_error && <span className="error-text">Could not decode: {detail.decode_error}</span>}

      <Facts
        rows={[
          ["Client", record.client],
          ["Duration", record.duration_ms === null ? "—" : `${record.duration_ms.toLocaleString("en-US")} ms`],
        ]}
      />
      <div className="section">
        <h6>Request</h6>
        <pre className="code-block">{json(record.request)}</pre>
      </div>
      <div className="section">
        <h6>Response</h6>
        <pre className="code-block">{record.response ? json(record.response) : "Waiting…"}</pre>
      </div>
    </aside>
  );
}
