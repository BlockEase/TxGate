// App shell: navigation, live state from engine events, and the approval dialog.

import { useCallback, useEffect, useRef, useState } from "react";
import logo from "./assets/logo.png";
import { api, errorText, onEngineEvent, type Config, type EngineEvent, type RequestRecord, type SafeAlert, type ServerStatus, type Snapshot, type WatchStatus } from "./api";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { About } from "./views/About";
import { History } from "./views/History";
import { Monitor } from "./views/Monitor";
import { Networks } from "./views/Networks";
import { Safe } from "./views/Safe";
import { Settings } from "./views/Settings";
import { setNativeCurrencies } from "./format";
import { I18nProvider, isLang, makeI18n, systemLang } from "./i18n";

type View = "monitor" | "history" | "safe" | "networks" | "settings" | "about";

const VIEWS: View[] = ["monitor", "history", "safe", "networks", "settings", "about"];
const MONITOR_LIMIT = 1000;

/** Updates or inserts `r` in a list kept newest-first by id. An update for a
 *  record that already scrolled out of a full list is dropped (it is in History). */
function upsertNewestFirst(list: RequestRecord[], r: RequestRecord): RequestRecord[] {
  const i = list.findIndex((x) => x.id <= r.id);
  if (i >= 0 && list[i].id === r.id) {
    const next = list.slice();
    next[i] = r;
    return next;
  }
  if (i < 0 && list.length >= MONITOR_LIMIT) return list;
  const at = i < 0 ? list.length : i;
  return [...list.slice(0, at), r, ...list.slice(at)].slice(0, MONITOR_LIMIT);
}

interface Toast {
  id: number;
  text: string;
}

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [recent, setRecent] = useState<RequestRecord[]>([]);
  const [pending, setPending] = useState<RequestRecord[]>([]);
  const [server, setServer] = useState<ServerStatus | null>(null);
  const [safeWatch, setSafeWatch] = useState<WatchStatus[]>([]);
  const [safeAlerts, setSafeAlerts] = useState<SafeAlert[]>([]);
  // The URL hash selects the initial view (handy for previews and screenshots).
  const [view, setView] = useState<View>(() => (VIEWS.find((v) => window.location.hash.startsWith(`#${v}`)) ?? "monitor"));
  const [selectedId, setSelectedId] = useState<number | null>(() => {
    const m = window.location.hash.match(/select=(\d+)/);
    return m ? Number(m[1]) : null;
  });
  const [deferred, setDeferred] = useState(() => window.location.hash.includes("later"));
  const [historyKey, setHistoryKey] = useState(0);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const knownPending = useRef<Set<number>>(new Set());
  const unroutableAt = useRef<Map<number, number>>(new Map());

  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const toast = useCallback(
    (text: string, ms = 6000) => {
      const id = Date.now() + Math.random();
      setToasts((t) => [...t.slice(-3), { id, text }]);
      setTimeout(() => dismiss(id), ms);
    },
    [dismiss],
  );

  // Language shown now; read through a ref by event handlers set up once.
  const lang = isLang(snapshot?.config.language) ? snapshot.config.language : systemLang();
  const i18n = makeI18n(lang);
  const i18nRef = useRef(i18n);
  i18nRef.current = i18n;
  useEffect(() => {
    document.documentElement.lang = lang;
    api.setUiLanguage(lang).catch(() => {});
  }, [lang]);

  const pendingLoaded = useRef(false);

  const applyPending = useCallback((list: RequestRecord[]) => {
    // A transaction arriving after start-up re-opens a deferred dialog.
    if (pendingLoaded.current && list.some((r) => !knownPending.current.has(r.id))) setDeferred(false);
    pendingLoaded.current = true;
    knownPending.current = new Set(list.map((r) => r.id));
    setPending(list);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const handle = (e: EngineEvent) => {
      switch (e.type) {
        case "request": {
          const r = e.payload;
          setRecent((prev) => upsertNewestFirst(prev, r));
          if (r.category !== "read") setHistoryKey((k) => k + 1);
          break;
        }
        case "pending":
          applyPending(e.payload);
          break;
        case "server":
          setServer(e.payload);
          break;
        case "safe_watch":
          setSafeWatch(e.payload);
          break;
        case "safe_alert": {
          const a = e.payload;
          setSafeAlerts((prev) => [a, ...prev.filter((x) => x.id !== a.id)]);
          if (a.severity !== "info") {
            const { t, tt } = i18nRef.current;
            toast(t("app.toast_safe_alert", { safe: `${a.safe.slice(0, 6)}…${a.safe.slice(-4)}`, message: tt(a.text, a.message) }), 15_000);
          }
          break;
        }
        case "history_cleared":
          // Held transactions stay; everything else is gone from the live list too.
          api.snapshot().then((s) => setRecent(s.recent), () => {});
          setHistoryKey((k) => k + 1);
          break;
        case "unroutable": {
          // Wallets poll every few seconds; say it once a minute per chain.
          const { chain_id, client, message } = e.payload;
          const last = unroutableAt.current.get(chain_id) ?? 0;
          if (Date.now() - last > 60_000) {
            unroutableAt.current.set(chain_id, Date.now());
            toast(i18nRef.current.t("app.toast_unroutable", { client, chain: chain_id, message }), 15_000);
          }
          break;
        }
      }
    };
    // Subscribe before taking the snapshot so no event falls in between.
    onEngineEvent(handle)
      .then((u) => {
        unlisten = u;
        return api.snapshot();
      })
      .then((s) => {
        setNativeCurrencies(s.native_currencies);
        setSnapshot(s);
        setRecent(s.recent);
        applyPending(s.pending);
        setServer(s.server);
        setSafeWatch(s.safe_watch);
        setSafeAlerts(s.safe_alerts);
        if (s.startup_warning) toast(s.startup_warning, 60_000);
      })
      .catch((e) => setLoadError(errorText(e)));
    return () => unlisten?.();
  }, [applyPending, toast]);

  const defer = useCallback(() => setDeferred(true), []);
  const setConfig = useCallback((config: Config) => setSnapshot((s) => (s ? { ...s, config } : s)), []);

  if (loadError) {
    return (
      <div className="empty" style={{ height: "100%", justifyContent: "center" }}>
        <h4>{i18n.t("app.load_error")}</h4>
        <span>{loadError}</span>
      </div>
    );
  }
  if (!snapshot) return null;
  const config = snapshot.config;

  const unreadAlerts = safeAlerts.filter((a) => !a.read).length;
  const { t } = i18n;
  const nav: { id: View; label: string; badge?: string }[] = [
    { id: "monitor", label: t("nav.requests"), badge: pending.length ? t("nav.held", { count: pending.length }) : undefined },
    { id: "history", label: t("nav.history") },
    { id: "safe", label: t("nav.safe"), badge: unreadAlerts ? t("nav.new", { count: unreadAlerts }) : undefined },
    { id: "networks", label: t("nav.networks"), badge: String(config.networks.length) },
    { id: "settings", label: t("nav.settings") },
    { id: "about", label: t("nav.about") },
  ];

  return (
    <I18nProvider lang={lang}>
    <div className="app">
      <nav className="sidebar" aria-label={t("nav.main")}>
        <div className="brand">
          <img src={logo} width={24} height={24} alt="" /> TxGate
        </div>
        {nav.map((n, i) => (
          <div key={n.id} style={{ display: "contents" }}>
            {i === 3 && <div className="nav-gap" />}
            <button
              className="nav-item"
              aria-current={view === n.id ? "page" : undefined}
              onClick={() => {
                setView(n.id);
                if (n.id === "monitor" && pending.length) setDeferred(false);
              }}
            >
              <span>{n.label}</span>
              {n.badge && <span className="badge">{n.badge}</span>}
            </button>
          </div>
        ))}
        <div className="sidebar-foot">
          <div className="row">
            <span className={`dot ${server?.listening ? "on" : "off"}`} />
            <span>{server?.listening ? t("app.listening") : t("app.not_listening")}</span>
          </div>
          <span className="text-muted num" style={{ fontSize: 12, overflowWrap: "anywhere" }}>
            {server?.address ?? `${config.listen.lan ? "0.0.0.0" : "127.0.0.1"}:${config.listen.port}`} · {t(config.networks.length === 1 ? "app.network_count_one" : "app.network_count", { count: config.networks.length })}
          </span>
          {server?.error && (
            <>
              <span className="error-text">{server.error}</span>
              <button className="btn btn-secondary" onClick={() => api.restartListener().then(setServer)}>
                {t("app.try_again")}
              </button>
            </>
          )}
        </div>
      </nav>

      <main className="main">
        {view === "monitor" && <Monitor recent={recent} config={config} selectedId={selectedId} onSelect={setSelectedId} />}
        {view === "history" && <History config={config} refreshKey={historyKey} />}
        {view === "safe" && <Safe snapshot={snapshot} watch={safeWatch} alerts={safeAlerts} onConfig={setConfig} onAlerts={setSafeAlerts} />}
        {view === "networks" && <Networks snapshot={snapshot} onConfig={setConfig} />}
        {view === "settings" && <Settings snapshot={snapshot} onConfig={setConfig} onServer={setServer} />}
        {view === "about" && <About snapshot={snapshot} />}
      </main>

      {pending.length > 0 && !deferred && <ApprovalDialog pending={pending} config={config} onDefer={defer} onNotice={(text) => toast(text)} />}

      <div className="toasts" aria-live="polite">
        {toasts.map((x) => (
          <div className="toast" key={x.id} onClick={() => dismiss(x.id)} title={t("app.dismiss")}>
            {x.text}
          </div>
        ))}
      </div>
    </div>
    </I18nProvider>
  );
}
