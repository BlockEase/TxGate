// What TxGate protects against, what it cannot see, and how to connect a wallet.

import type { Snapshot } from "../api";
import { walletUrl } from "../format";
import { t, tn } from "../i18n";

export function About({ snapshot }: { snapshot: Snapshot }) {
  const port = snapshot.config.listen.port;
  return (
    <div className="page">
      <div className="prose">
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
