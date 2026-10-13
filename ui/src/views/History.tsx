// Persisted request log with filters.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  errorText,
  type Category,
  type Config,
  type HistoryQuery,
  type RequestRecord,
  type Status,
} from "../api";
import { RecordPanel } from "../components/RecordPanel";
import { RequestTable } from "../components/RequestTable";
import { STATUSES, statusLabel } from "../format";
import { t } from "../i18n";

const PAGE = 200;

export function History({
  config,
  refreshKey,
}: {
  config: Config;
  refreshKey: number;
}) {
  const [text, setText] = useState("");
  const [address, setAddress] = useState("");
  const [chain, setChain] = useState<string>("");
  const [category, setCategory] = useState<Category | "">("");
  const [status, setStatus] = useState<Status | "">("");
  const [rows, setRows] = useState<RequestRecord[]>([]);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<RequestRecord | null>(null);
  // `#history-confirm` opens the confirmation (previews and screenshots).
  const [confirming, setConfirming] = useState(() =>
    window.location.hash.includes("confirm"),
  );
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState<number | null>(null);

  const query = useCallback(
    (beforeId?: number): HistoryQuery => ({
      text: text.trim() || undefined,
      address: address.trim() || undefined,
      chain_id: chain ? Number(chain) : undefined,
      category: category || undefined,
      status: status || undefined,
      limit: PAGE,
      before_id: beforeId,
    }),
    [text, address, chain, category, status],
  );

  // Filters the rows on screen were loaded with; a live refresh under the same
  // filters keeps older pages the user already loaded.
  const loadedFor = useRef("");

  const load = useCallback(async () => {
    const q = query();
    const key = JSON.stringify(q);
    try {
      const r = await api.history(q);
      const sameFilters = loadedFor.current === key;
      loadedFor.current = key;
      setRows((prev) => {
        if (!sameFilters || r.length < PAGE) return r;
        const oldest = r[r.length - 1].id;
        return [...r, ...prev.filter((x) => x.id < oldest)];
      });
      if (!sameFilters || r.length < PAGE) setMore(r.length === PAGE);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [query]);

  useEffect(() => {
    const timer = setTimeout(load, 200);
    return () => clearTimeout(timer);
  }, [load, refreshKey]);

  // Keep the open panel in sync with fresh data.
  useEffect(() => {
    if (selected) {
      const fresh = rows.find((r) => r.id === selected.id);
      if (fresh && fresh !== selected) setSelected(fresh);
    }
  }, [rows, selected]);

  const clearAll = async () => {
    setClearing(true);
    setError(null);
    try {
      const n = await api.clearHistory();
      setSelected(null);
      setConfirming(false);
      setCleared(n);
      await load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setClearing(false);
    }
  };

  const loadMore = async () => {
    try {
      const r = await api.history(query(rows[rows.length - 1]?.id));
      setRows([...rows, ...r]);
      setMore(r.length === PAGE);
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <>
      <div
        className="view-head"
        style={{ flexDirection: "column", alignItems: "stretch", gap: 12 }}
      >
        <div className="row">
          <h3>{t("history.title")}</h3>
          <span className="spacer" />
          {!confirming && (
            <button
              className="btn btn-ghost"
              onClick={() => {
                setCleared(null);
                setConfirming(true);
              }}
            >
              {t("history.clear")}
            </button>
          )}
        </div>
        <div className="filters">
          <input
            className="input"
           
            placeholder={t("history.search")}
            title={t("history.search_hint")}
            aria-label={t("history.search_hint")}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
          <input
            className="input"
           
            placeholder={t("history.address")}
            title={t("history.address_hint")}
            aria-label={t("history.address_hint")}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
          />
          <select
            className="input"
           
            value={chain}
            onChange={(e) => setChain(e.target.value)}
            aria-label={t("table.network")}
          >
            <option value="">{t("history.all_networks")}</option>
            {config.networks.map((n) => (
              <option key={n.chain_id} value={n.chain_id}>
                {n.name}
              </option>
            ))}
          </select>
          <select
            className="input"
           
            value={category}
            onChange={(e) => setCategory(e.target.value as Category | "")}
            aria-label={t("history.kind")}
          >
            <option value="">{t("history.all_kinds")}</option>
            <option value="send">{t("history.kind_send")}</option>
            <option value="estimate">{t("history.kind_estimate")}</option>
            <option value="read">{t("history.kind_read")}</option>
          </select>
          <select
            className="input"
           
            value={status}
            onChange={(e) => setStatus(e.target.value as Status | "")}
            aria-label={t("table.status")}
          >
            <option value="">{t("history.any_status")}</option>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {statusLabel(s)}
              </option>
            ))}
          </select>
        </div>
      </div>
      {confirming && (
        <div
          className="attention clear-confirm"
          role="alertdialog"
          aria-labelledby="clear-title"
        >
          <span id="clear-title" className="attention-title">
            {t("history.confirm_title")}
          </span>
          <span>
            {t("history.confirm_body")}
          </span>
          <div className="actions">
            <button
              className="btn btn-secondary"
              disabled={clearing}
              onClick={clearAll}
            >
              {clearing ? t("history.deleting") : t("history.delete")}
            </button>
            <button
              className="btn btn-ghost"
              disabled={clearing}
              onClick={() => setConfirming(false)}
              autoFocus
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
      {cleared !== null && !confirming && (
        <div className="clear-confirm text-muted" role="status">
          {cleared === 1 ? t("history.deleted_one") : t("history.deleted", { count: cleared.toLocaleString("en-US") })}
        </div>
      )}
      <div className="split">
        <div className="table-wrap">
          {error && (
            <div className="error-text" style={{ padding: 20 }}>
              {error}
            </div>
          )}
          {rows.length > 0 ? (
            <>
              <RequestTable
                rows={rows}
                config={config}
                selectedId={selected?.id ?? null}
                onSelect={(id) =>
                  setSelected(rows.find((r) => r.id === id) ?? null)
                }
                showDate
              />
              {more && (
                <div style={{ padding: 16, textAlign: "center" }}>
                  <button className="btn btn-secondary" onClick={loadMore}>
                    {t("history.load_more")}
                  </button>
                </div>
              )}
            </>
          ) : (
            <div className="empty">
              <h4>{t("history.empty")}</h4>
              <span>
                {t("history.empty_hint")}{" "}
                {config.logging.mode === "tx_only" ? t("history.empty_reads") : ""}
              </span>
            </div>
          )}
        </div>
        {selected && (
          <RecordPanel
            record={selected}
            config={config}
            onClose={() => setSelected(null)}
            showDate
          />
        )}
      </div>
    </>
  );
}
