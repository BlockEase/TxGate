// Live request stream with a detail panel.

import { useMemo, useState } from "react";
import type { Config, RequestRecord } from "../api";
import { RecordPanel } from "../components/RecordPanel";
import { RequestTable } from "../components/RequestTable";
import { walletUrl } from "../format";
import { t } from "../i18n";

export function Monitor({
  recent,
  config,
  selectedId,
  onSelect,
}: {
  recent: RequestRecord[];
  config: Config;
  selectedId: number | null;
  onSelect: (id: number | null) => void;
}) {
  const [txOnly, setTxOnly] = useState(false);
  const [frozen, setFrozen] = useState<RequestRecord[] | null>(null);

  // While paused, keep showing the frozen rows but with their latest state.
  const source = useMemo(() => {
    if (!frozen) return recent;
    const latest = new Map(recent.map((r) => [r.id, r]));
    return frozen.map((r) => latest.get(r.id) ?? r);
  }, [frozen, recent]);
  const rows = txOnly ? source.filter((r) => r.category !== "read") : source;
  const txCount = recent.filter((r) => r.category !== "read").length;
  const selected = selectedId === null ? null : recent.find((r) => r.id === selectedId) ?? null;
  const firstNet = config.networks[0];

  return (
    <>
      <div className="view-head">
        <h3>{t("monitor.title")}</h3>
        <span className="text-muted num">
          {t("monitor.counts", { recent: recent.length.toLocaleString("en-US"), tx: txCount })}
        </span>
        <div className="spacer" />
        <div className="seg" role="group" aria-label={t("monitor.filter")}>
          <button className="seg-opt" aria-pressed={!txOnly} onClick={() => setTxOnly(false)}>
            {t("monitor.all")}
          </button>
          <button className="seg-opt" aria-pressed={txOnly} onClick={() => setTxOnly(true)}>
            {t("monitor.tx_only")}
          </button>
        </div>
        <button className="btn btn-secondary" onClick={() => setFrozen(frozen ? null : recent)}>
          {frozen ? t("monitor.resume") : t("monitor.pause")}
        </button>
      </div>
      <div className="split">
        <div className="table-wrap">
          {rows.length > 0 ? (
            <RequestTable rows={rows} config={config} selectedId={selectedId} onSelect={onSelect} />
          ) : (
            <div className="empty">
              <h4>{txOnly && recent.length > 0 ? t("monitor.empty_tx") : t("monitor.empty")}</h4>
              <span>{t("monitor.empty_hint")}</span>
              {firstNet && <code>{walletUrl("127.0.0.1", config.listen.port, firstNet.chain_id)}</code>}
            </div>
          )}
        </div>
        {selected && <RecordPanel record={selected} config={config} onClose={() => onSelect(null)} />}
      </div>
    </>
  );
}
