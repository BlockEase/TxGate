// Request list shared by the live monitor and the history view.

import type { Config, RequestRecord } from "../api";
import { clock, dateTime, networkName, STATUS_LABEL, STATUS_TAG } from "../format";

const PREVIEW_HINT =
  "The wallet estimated gas for this transaction, which it usually does right before asking you to sign. TxGate compares it with the transaction that follows.";
const SEND_HINT = "A transaction to broadcast. TxGate holds it until you approve it.";

export function RequestTable({
  rows,
  config,
  selectedId,
  onSelect,
  showDate,
}: {
  rows: RequestRecord[];
  config: Config;
  selectedId: number | null;
  onSelect: (id: number) => void;
  showDate?: boolean;
}) {
  return (
    <table className="table" style={{ fontSize: 13 }}>
      <thead>
        <tr>
          <th style={{ width: showDate ? 150 : 76 }}>Time</th>
          <th style={{ width: 96 }}>Network</th>
          <th style={{ width: 236 }}>Method</th>
          <th>Summary</th>
          <th style={{ width: 140 }}>Status</th>
          <th style={{ width: 70, textAlign: "right" }}>Time taken</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => {
          const hot = r.category !== "read";
          const cls = [r.id === selectedId ? "sel" : hot ? "hot" : ""].join(" ");
          return (
            <tr
              key={r.id}
              className={cls}
              onClick={() => onSelect(r.id)}
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === "Enter") onSelect(r.id);
              }}
            >
              <td className="text-muted num">{showDate ? dateTime(r.ts_ms) : clock(r.ts_ms)}</td>
              <td className="ellipsis">{networkName(config, r.chain_id)}</td>
              <td>
                <div className={`method ${hot ? "hot" : ""}`}>
                  <span>{r.method}</span>
                  {hot && (
                    <span className="tag tag-outline flag" title={r.category === "send" ? SEND_HINT : PREVIEW_HINT}>
                      {r.category === "send" ? "Transaction" : "Preview"}
                    </span>
                  )}
                </div>
              </td>
              <td className="ellipsis" title={r.summary}>
                {r.summary}
              </td>
              <td style={{ whiteSpace: "nowrap" }}>
                <span className={STATUS_TAG[r.status]}>{STATUS_LABEL[r.status]}</span>
              </td>
              <td className="text-muted num" style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                {r.duration_ms === null ? "—" : `${r.duration_ms.toLocaleString("en-US")} ms`}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
