// Request list shared by the live monitor and the history view.

import type { Config, RequestRecord } from "../api";
import { clock, dateTime, networkName, statusLabel, STATUS_TAG } from "../format";
import { t } from "../i18n";

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
          <th style={{ width: showDate ? 158 : 76 }}>{t("table.time")}</th>
          <th className="col-network" style={{ width: 110 }}>{t("table.network")}</th>
          <th className="col-method">{t("table.method")}</th>
          <th>{t("table.summary")}</th>
          <th style={{ width: 132 }}>{t("table.status")}</th>
          <th className="col-duration nowrap" style={{ width: 90, textAlign: "right" }}>{t("table.duration")}</th>
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
              <td className="text-muted num nowrap">{showDate ? dateTime(r.ts_ms) : clock(r.ts_ms)}</td>
              <td className="ellipsis col-network" title={networkName(config, r.chain_id)}>
                {networkName(config, r.chain_id)}
              </td>
              <td className="clip">
                <div className={`method ${hot ? "hot" : ""}`}>
                  <span className="method-name" title={r.method}>
                    {r.method}
                  </span>
                  {hot && (
                    <span className="tag tag-outline flag" title={r.category === "send" ? t("table.send_hint") : t("table.preview_hint")}>
                      {r.category === "send" ? t("table.flag_send") : t("table.flag_preview")}
                    </span>
                  )}
                </div>
              </td>
              <td className="ellipsis" title={r.summary}>
                {r.summary}
              </td>
              <td style={{ whiteSpace: "nowrap" }}>
                <span className={STATUS_TAG[r.status]}>{statusLabel(r.status)}</span>
              </td>
              <td className="text-muted num col-duration" style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                {r.duration_ms === null ? "—" : `${r.duration_ms.toLocaleString("en-US")} ms`}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
