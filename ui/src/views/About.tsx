// What TxGate protects against, what it cannot see, and how to connect a wallet.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorText, type Snapshot, type UpdateCheck } from "../api";
import { walletUrl } from "../format";
import { t, tn } from "../i18n";

const REPOSITORY = "https://github.com/BlockEase/TxGate";

/** The running version and a check against GitHub releases. GitHub is only
 *  contacted when the user clicks the button, never automatically. */
function Updates({ version }: { version: string }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<UpdateCheck | null>(null);
  const [error, setError] = useState<string | null>(null);

  const check = async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.checkUpdate());
    } catch (e) {
      setResult(null);
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="update">
      <div className="row">
        <span className="num">{t("update.version", { version })}</span>
        <button className="btn btn-secondary" onClick={check} disabled={busy}>
          {busy ? t("update.checking") : t("update.check")}
        </button>
        <span className="spacer" />
        <button className="link-btn" onClick={() => openUrl(REPOSITORY)}>
          github.com/BlockEase/TxGate
        </button>
      </div>
      {error && <span className="error-text">{t("update.error", { error })}</span>}
      {result?.status === "up_to_date" && <span className="text-muted">{t("update.up_to_date")}</span>}
      {result?.status === "no_release" && <span className="text-muted">{t("update.no_release")}</span>}
      {result?.status === "available" && (
        <div className="attention" role="status">
          <span className="attention-title">{t("update.available", { latest: result.latest, current: result.current })}</span>
          <div className="actions">
            <button className="btn btn-primary" onClick={() => openUrl(result.url)}>
              {t("update.download")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export function About({ snapshot }: { snapshot: Snapshot }) {
  const port = snapshot.config.listen.port;
  return (
    <div className="page">
      <div className="prose">
        <Updates version={snapshot.version} />

        <h3 style={{ fontSize: 22 }}>{t("about.title")}</h3>
        <p>{t("about.intro")}</p>

        <h4>{t("about.held_title")}</h4>
        <p>{t("about.held_intro")}</p>
        <ul>
          {snapshot.intercepted_methods.map((m) => (
            <li key={m}>
              <code className="mono">{m}</code>
            </li>
          ))}
        </ul>
        <p>{t("about.held_rest")}</p>

        <h4>{t("about.cannot_see")}</h4>
        <div className="attention">
          <span className="note-accent">
            {t("about.signatures")}
          </span>
        </div>
        <ul>
          <li>{t("about.limit_decide")}</li>
          <li>{t("about.limit_direct")}</li>
          <li>{t("about.limit_simulation")}</li>
          <li>{t("about.limit_safe")}</li>
        </ul>

        <h4>{t("about.connect")}</h4>
        <ol>
          <li>{t("about.connect_1")}</li>
          <li>{tn("about.connect_2", { url: <code className="mono">{walletUrl("127.0.0.1", port, 1)}</code> })}</li>
          <li>{t("about.connect_3")}</li>
        </ol>

        <h4>{t("about.data")}</h4>
        <ul>
          <li>{tn("about.data_config", { path: <code className="mono">{snapshot.config_path}</code> })}</li>
          <li>{tn("about.data_log", { path: <code className="mono">{snapshot.db_path}</code> })}</li>
        </ul>
      </div>
    </div>
  );
}
