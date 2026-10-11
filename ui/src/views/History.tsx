// Persisted request log with filters.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, type Category, type Config, type HistoryQuery, type RequestRecord, type Status } from "../api";
import { RecordPanel } from "../components/RecordPanel";
import { RequestTable } from "../components/RequestTable";
import { STATUS_LABEL } from "../format";

const PAGE = 200;

export function History({ config, refreshKey }: { config: Config; refreshKey: number }) {
  const [text, setText] = useState("");
  const [address, setAddress] = useState("");
  const [chain, setChain] = useState<string>("");
  const [category, setCategory] = useState<Category | "">("");
  const [status, setStatus] = useState<Status | "">("");
  const [rows, setRows] = useState<RequestRecord[]>([]);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<RequestRecord | null>(null);

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
    const t = setTimeout(load, 200);
    return () => clearTimeout(t);
  }, [load, refreshKey]);

  // Keep the open panel in sync with fresh data.
  useEffect(() => {
    if (selected) {
      const fresh = rows.find((r) => r.id === selected.id);
      if (fresh && fresh !== selected) setSelected(fresh);
    }
  }, [rows, selected]);

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
      <div className="view-head" style={{ flexWrap: "wrap" }}>
        <h3>History</h3>
        <input className="input" style={{ width: 220 }} placeholder="Search method, summary or hash" value={text} onChange={(e) => setText(e.target.value)} />
        <input className="input" style={{ width: 200 }} placeholder="Address (from or to)" value={address} onChange={(e) => setAddress(e.target.value)} />
        <select className="input" style={{ width: 150 }} value={chain} onChange={(e) => setChain(e.target.value)} aria-label="Network">
          <option value="">All networks</option>
          {config.networks.map((n) => (
            <option key={n.chain_id} value={n.chain_id}>
              {n.name}
            </option>
          ))}
        </select>
        <select className="input" style={{ width: 150 }} value={category} onChange={(e) => setCategory(e.target.value as Category | "")} aria-label="Kind">
          <option value="">All kinds</option>
          <option value="send">Sending</option>
          <option value="estimate">Gas estimates</option>
          <option value="read">Reads</option>
        </select>
        <select className="input" style={{ width: 170 }} value={status} onChange={(e) => setStatus(e.target.value as Status | "")} aria-label="Status">
          <option value="">Any status</option>
          {(Object.keys(STATUS_LABEL) as Status[]).map((s) => (
            <option key={s} value={s}>
              {STATUS_LABEL[s]}
            </option>
          ))}
        </select>
      </div>
      <div className="split">
        <div className="table-wrap">
          {error && <div className="error-text" style={{ padding: 20 }}>{error}</div>}
          {rows.length > 0 ? (
            <>
              <RequestTable rows={rows} config={config} selectedId={selected?.id ?? null} onSelect={(id) => setSelected(rows.find((r) => r.id === id) ?? null)} showDate />
              {more && (
                <div style={{ padding: 16, textAlign: "center" }}>
                  <button className="btn btn-secondary" onClick={loadMore}>
                    Load older requests
                  </button>
                </div>
              )}
            </>
          ) : (
            <div className="empty">
              <h4>Nothing logged yet</h4>
              <span>
                Sending calls and gas estimates are saved here permanently.{" "}
                {config.logging.mode === "tx_only" ? "Reads are only kept in the live monitor; change that in Settings." : ""}
              </span>
            </div>
          )}
        </div>
        {selected && <RecordPanel record={selected} config={config} onClose={() => setSelected(null)} showDate />}
      </div>
    </>
  );
}
