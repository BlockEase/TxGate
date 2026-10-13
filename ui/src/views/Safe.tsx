// Safe multisig: check a transaction before signing it, and watch Safes for
// configuration changes and new proposals.

import { useState } from "react";
import { AlertTriangle, Info, ShieldAlert } from "lucide-react";
import { api, errorText, type Config, type SafeAlert, type SafeTxReport, type Snapshot, type WatchStatus } from "../api";
import { dateTime, networkName, toBig } from "../format";
import { SafeReportBody } from "../components/SafeParts";
import { t, tt } from "../i18n";

function worst(r: SafeTxReport): "danger" | "warning" | "info" | null {
  if (r.risks.some((x) => x.severity === "danger")) return "danger";
  if (r.risks.some((x) => x.severity === "warning")) return "warning";
  return r.risks.length ? "info" : null;
}

function SeverityIcon({ severity }: { severity: "danger" | "warning" | "info" | null }) {
  if (severity === "danger") return <ShieldAlert size={14} className="note-accent" aria-label={t("severity.danger")} />;
  if (severity === "warning") return <AlertTriangle size={14} className="note-accent" aria-label={t("severity.warning")} />;
  return <Info size={14} className="text-muted" aria-label={t("severity.info")} />;
}

/** Queued transactions; each expands into the full review. */
function Queue({ reports }: { reports: SafeTxReport[] }) {
  // `#safe-open` expands the most dangerous transaction (previews and screenshots).
  const [open, setOpen] = useState<string | null>(() =>
    window.location.hash.includes("open") ? (reports.find((r) => worst(r) === "danger") ?? reports[0])?.hashes.safe_tx_hash ?? null : null,
  );
  if (reports.length === 0) return <span className="text-muted">{t("safepage.queue_empty")}</span>;
  return (
    <div className="safe-list" style={{ gap: 6 }}>
      {reports.map((r) => {
        const id = r.hashes.safe_tx_hash;
        const signed = r.confirmations.filter((c) => c.valid).length;
        return (
          <div key={id} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <button className="queue-row" aria-expanded={open === id} onClick={() => setOpen(open === id ? null : id)}>
              <span className="num">{t("safepage.nonce", { nonce: toBig(r.tx.nonce).toString() })}</span>
              <span style={{ overflowWrap: "anywhere" }}>
                <SeverityIcon severity={worst(r)} /> {r.analysis.summary}
              </span>
              <span className="text-muted num">
                {t("safepage.signed", { signed, threshold: r.state?.threshold ?? "?" })}
              </span>
            </button>
            {open === id && (
              <div className="queue-detail">
                <SafeReportBody report={r} />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function CheckBeforeSigning({ config, watch }: { config: Config; watch: WatchStatus[] }) {
  const [chainId, setChainId] = useState(config.networks[0]?.chain_id ?? 1);
  const [safe, setSafe] = useState(watch[0]?.address ?? "");
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState<"check" | "queue" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<SafeTxReport | null>(null);
  const [queue, setQueue] = useState<SafeTxReport[] | null>(null);

  const run = async (kind: "check" | "queue") => {
    setBusy(kind);
    setError(null);
    try {
      if (kind === "check") {
        let parsed: unknown;
        try {
          parsed = JSON.parse(input);
        } catch {
          throw new Error(t("safepage.bad_json"));
        }
        setReport(await api.safeReview(chainId, safe, parsed));
        setQueue(null);
      } else {
        setQueue(await api.safeQueue(chainId, safe));
        setReport(null);
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="settings-section">
      <h5>{t("safepage.check_title")}</h5>
      <span className="text-muted">{t("safepage.check_intro")}</span>
      <div style={{ display: "grid", gridTemplateColumns: "200px 1fr", gap: 14 }}>
        <div className="field">
          <label htmlFor="safe-net">{t("table.network")}</label>
          <select id="safe-net" className="input" value={chainId} onChange={(e) => setChainId(Number(e.target.value))}>
            {config.networks.map((n) => (
              <option key={n.chain_id} value={n.chain_id}>
                {n.name}
              </option>
            ))}
          </select>
        </div>
        <div className="field">
          <label htmlFor="safe-addr">{t("safepage.address")}</label>
          <input id="safe-addr" className="input mono" list="watched-safes" spellCheck={false} placeholder="0x…" value={safe} onChange={(e) => setSafe(e.target.value)} />
          <datalist id="watched-safes">
            {watch.map((w) => (
              <option key={`${w.chain_id}:${w.address}`} value={w.address}>
                {w.label}
              </option>
            ))}
          </datalist>
        </div>
      </div>
      <div className="field">
        <label htmlFor="safe-input">{t("safepage.tx_to_sign")}</label>
        <textarea
          id="safe-input"
          className="input"
          spellCheck={false}
          placeholder={'{"domain": {"chainId": 1, "verifyingContract": "0x…"}, "message": {"to": "0x…", "value": "0", "data": "0x…", "operation": 0, "nonce": 12, …}}'}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <span className="hint">
          {t("safepage.input_hint")}
        </span>
      </div>
      <div className="actions">
        <button className="btn btn-primary" disabled={!safe.trim() || !input.trim() || busy !== null} onClick={() => run("check")}>
          {busy === "check" ? t("safepage.checking") : t("safepage.check")}
        </button>
        <button className="btn btn-secondary" disabled={!safe.trim() || busy !== null} onClick={() => run("queue")}>
          {busy === "queue" ? t("safepage.loading") : t("safepage.load_queue")}
        </button>
        {error && <span className="error-text">{error}</span>}
      </div>
      {report && (
        <div className="queue-detail">
          <SafeReportBody report={report} />
        </div>
      )}
      {queue && <Queue reports={queue} />}
    </section>
  );
}

function AddSafe({ config, onSaved }: { config: Config; onSaved: (c: Config) => void }) {
  const [chainId, setChainId] = useState(config.networks[0]?.chain_id ?? 1);
  const [address, setAddress] = useState("");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const add = async () => {
    const addr = address.trim();
    if (!/^0x[0-9a-fA-F]{40}$/.test(addr)) return setError(t("safepage.err_address"));
    if (config.safe.watched.some((w) => w.chain_id === chainId && w.address.toLowerCase() === addr.toLowerCase()))
      return setError(t("safepage.err_watched"));
    setBusy(true);
    setError(null);
    const next: Config = { ...config, safe: { ...config.safe, watched: [...config.safe.watched, { chain_id: chainId, address: addr, label: label.trim() }] } };
    try {
      await api.saveConfig(next);
      onSaved(next);
      setAddress("");
      setLabel("");
      await api.safePoll();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div style={{ display: "grid", gridTemplateColumns: "160px 1fr 160px auto", gap: 10, alignItems: "end" }}>
      <div className="field">
        <label htmlFor="watch-net">{t("table.network")}</label>
        <select id="watch-net" className="input" value={chainId} onChange={(e) => setChainId(Number(e.target.value))}>
          {config.networks.map((n) => (
            <option key={n.chain_id} value={n.chain_id}>
              {n.name}
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor="watch-addr">{t("safepage.address")}</label>
        <input id="watch-addr" className="input mono" spellCheck={false} placeholder="0x…" value={address} onChange={(e) => setAddress(e.target.value)} />
      </div>
      <div className="field">
        <label htmlFor="watch-label">{t("safepage.name")}</label>
        <input id="watch-label" className="input" value={label} onChange={(e) => setLabel(e.target.value)} />
      </div>
      <button className="btn btn-secondary" disabled={busy || !address.trim()} onClick={add}>
        {busy ? t("safepage.adding") : t("safepage.watch")}
      </button>
      {error && <span className="error-text" style={{ gridColumn: "1 / -1" }}>{error}</span>}
    </div>
  );
}

function Watched({ config, watch, onConfig }: { config: Config; watch: WatchStatus[]; onConfig: (c: Config) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const statusOf = (chainId: number, address: string) =>
    watch.find((w) => w.chain_id === chainId && w.address.toLowerCase() === address.toLowerCase());

  const remove = async (chainId: number, address: string) => {
    const next: Config = {
      ...config,
      safe: { ...config.safe, watched: config.safe.watched.filter((w) => !(w.chain_id === chainId && w.address.toLowerCase() === address.toLowerCase())) },
    };
    setError(null);
    try {
      await api.saveConfig(next);
      onConfig(next);
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <section className="settings-section">
      <h5>
        {t("safepage.watched")}
        <span className="spacer" />
        {config.safe.watched.length > 0 && (
          <button
            className="btn btn-ghost"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await api.safePoll();
              } catch (e) {
                setError(errorText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? t("safepage.checking") : t("safepage.check_now")}
          </button>
        )}
      </h5>
      <span className="text-muted">
        {t("safepage.watched_intro", { secs: config.safe.poll_secs })}
      </span>
      {error && <span className="error-text">{error}</span>}
      <AddSafe config={config} onSaved={onConfig} />
      <div className="safe-list">
        {config.safe.watched.map((w) => {
          const s = statusOf(w.chain_id, w.address);
          const st = s?.state;
          return (
            <div className="safe-item" key={`${w.chain_id}:${w.address}`}>
              <div className="safe-item-head">
                <h5>{w.label || t("safe.safe")}</h5>
                <span className="mono">{w.address}</span>
                <span className="text-muted">{networkName(config, w.chain_id)}</span>
                <span className="spacer" />
                <button className="btn btn-ghost" onClick={() => remove(w.chain_id, w.address)}>
                  {t("safepage.stop")}
                </button>
              </div>
              {!s && <span className="text-muted">{t("safepage.not_checked")}</span>}
              {s?.error && <span className="error-text">{tt(s.error_text, s.error)}</span>}
              {st && (
                <span className="num">
                  {st.version ? `Safe ${st.version}` : t("safe.safe")} · {t("safepage.owners", { threshold: st.threshold, owners: st.owners.length })} ·{" "}
                  {t("safepage.nonce_lower", { nonce: toBig(st.nonce).toString() })}
                  {st.singleton_name ? "" : ` · ${t("safepage.unofficial")}`}
                  {st.modules.length ? ` · ${t("safepage.modules", { count: st.modules.length })}` : ""}
                  <span className="fine"> · {t("safepage.checked", { time: dateTime(s.checked_ms) })}</span>
                </span>
              )}
              {s?.service_error && <span className="fine">{t("safepage.queue_unavailable", { error: tt(s.service_error_text, s.service_error) })}</span>}
              {st && !s.service_error && <Queue reports={s.queue} />}
            </div>
          );
        })}
        {config.safe.watched.length === 0 && <span className="text-muted">{t("safepage.watched_empty")}</span>}
      </div>
    </section>
  );
}

function Alerts({ config, alerts, onRead }: { config: Config; alerts: SafeAlert[]; onRead: () => void }) {
  const unread = alerts.filter((a) => !a.read).length;
  return (
    <section className="settings-section">
      <h5>
        {t("safepage.alerts")}
        {unread > 0 && <span className="tag tag-accent">{t("nav.new", { count: unread })}</span>}
        <span className="spacer" />
        {unread > 0 && (
          <button className="btn btn-ghost" onClick={onRead}>
            {t("safepage.mark_read")}
          </button>
        )}
      </h5>
      {alerts.length === 0 && <span className="text-muted">{t("safepage.alerts_empty")}</span>}
      <div>
        {[...alerts]
          .sort((a, b) => b.id - a.id)
          .map((a) => (
            <div className={`alert-row${a.read ? "" : " unread"}`} key={a.id}>
              <span className="fine num">{dateTime(a.ts_ms)}</span>
              <SeverityIcon severity={a.severity} />
              <span style={{ overflowWrap: "anywhere" }}>
                <span className="mono">{a.safe.slice(0, 6)}…{a.safe.slice(-4)}</span> <span className="text-muted">{t("safepage.on_network", { network: networkName(config, a.chain_id) })}</span>{" "}
                {tt(a.text, a.message)}
              </span>
            </div>
          ))}
      </div>
    </section>
  );
}

export function Safe({
  snapshot,
  watch,
  alerts,
  onConfig,
  onAlerts,
}: {
  snapshot: Snapshot;
  watch: WatchStatus[];
  alerts: SafeAlert[];
  onConfig: (c: Config) => void;
  onAlerts: (a: SafeAlert[]) => void;
}) {
  const config = snapshot.config;
  return (
    <div className="page narrow">
      <h3 style={{ margin: 0, fontSize: 18 }}>{t("nav.safe")}</h3>
      <span className="text-muted">{t("safepage.intro")}</span>
      {config.networks.length === 0 ? (
        <span className="error-text">{t("safepage.no_network")}</span>
      ) : (
        <>
          <CheckBeforeSigning config={config} watch={watch} />
          <Watched config={config} watch={watch} onConfig={onConfig} />
          <Alerts config={config} alerts={alerts} onRead={() => api.safeAlertsRead().then(onAlerts)} />
        </>
      )}
    </div>
  );
}
