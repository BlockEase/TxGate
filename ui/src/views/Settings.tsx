// Everything stored in config.toml except networks: where TxGate listens,
// provider keys, Tenderly credentials and logging. One draft, one save.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Eye, EyeOff } from "lucide-react";
import { api, errorText, type Config, type ProviderId, type ServerStatus, type Snapshot } from "../api";
import { tenderlyReady, walletUrl } from "../format";
import { isLang, LANGUAGES, systemLang, t } from "../i18n";

/** Trims text fields and drops empty provider keys. */
function normalise(c: Config): Config {
  const providers: Config["providers"] = {};
  for (const [id, key] of Object.entries(c.providers) as [ProviderId, string | undefined][]) {
    if (key?.trim()) providers[id] = key.trim();
  }
  const ten = c.tenderly;
  return {
    ...c,
    providers,
    tenderly: { account: ten.account.trim(), project: ten.project.trim(), access_key: ten.access_key.trim() },
    safe: { ...c.safe, service_api_key: c.safe.service_api_key.trim(), service_url: c.safe.service_url.trim() },
  };
}

function validate(c: Config): string | null {
  const port = c.listen.port;
  if (!Number.isInteger(port) || port < 1 || port > 65535) return t("settings.err_port");
  if (!Number.isInteger(c.preview_window_secs) || c.preview_window_secs < 10) return t("settings.err_window");
  if (!Number.isInteger(c.safe.poll_secs) || c.safe.poll_secs < 15) return t("settings.err_poll");
  if (c.safe.service_url && !/^https?:\/\//.test(c.safe.service_url)) return t("settings.err_service_url");
  return null;
}

export function Settings({
  snapshot,
  onConfig,
  onServer,
}: {
  snapshot: Snapshot;
  onConfig: (c: Config) => void;
  onServer: (s: ServerStatus) => void;
}) {
  const saved = snapshot.config;
  const [draft, setDraft] = useState<Config>(saved);
  const [shown, setShown] = useState<Partial<Record<ProviderId, boolean>>>({});
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  // The language is saved on its own as soon as it is picked.
  const next = { ...normalise(draft), language: saved.language };
  const dirty = JSON.stringify(next) !== JSON.stringify(normalise(saved));
  const invalid = validate(next);
  const edit = (change: Partial<Config>) => {
    setDraft({ ...draft, ...change });
    setResult(null);
  };
  const lanHost = snapshot.lan_addresses[0] ?? t("settings.this_ip");
  const firstChain = saved.networks[0]?.chain_id ?? 1;

  const save = async () => {
    setSaving(true);
    // Networks and watched Safes are edited on their own pages; never write back a stale copy.
    const toSave = { ...next, networks: snapshot.config.networks, safe: { ...next.safe, watched: snapshot.config.safe.watched }, language: snapshot.config.language };
    try {
      const status = await api.saveConfig(toSave);
      onConfig(toSave);
      onServer(status);
      setDraft(toSave);
      setResult(status.error ? { ok: false, text: status.error } : { ok: true, text: t("settings.saved", { address: status.address ?? "" }) });
    } catch (e) {
      setResult({ ok: false, text: errorText(e) });
    } finally {
      setSaving(false);
    }
  };

  const chainNames = (ids: number[]) =>
    ids
      .map((id) => snapshot.known_chains.find((c) => c.chain_id === id)?.name ?? String(id))
      .slice(0, 3)
      .join(", ") + (ids.length > 3 ? ` ${t("settings.and_more", { count: ids.length - 3 })}` : "");

  const setLanguage = async (language: string) => {
    const config = { ...snapshot.config, language: language || null };
    try {
      await api.saveConfig(config);
      onConfig(config);
    } catch (e) {
      setResult({ ok: false, text: errorText(e) });
    }
  };

  return (
    <>
      <div className="page narrow">
        <h3 style={{ margin: 0, fontSize: 18 }}>{t("settings.title")}</h3>

        <section className="settings-section">
          <h5>{t("settings.language")}</h5>
          <div className="field" style={{ maxWidth: 360 }}>
            <select
              className="input"
              aria-label={t("settings.language")}
              value={isLang(saved.language) ? saved.language : ""}
              onChange={(e) => setLanguage(e.target.value)}
            >
              <option value="">{t("settings.language_system", { name: LANGUAGES.find((l) => l.code === systemLang())?.name ?? "English" })}</option>
              {LANGUAGES.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.name}
                </option>
              ))}
            </select>
            <span className="hint">{t("settings.language_hint")}</span>
          </div>
        </section>

        <section className="settings-section">
          <h5>{t("settings.connections")}</h5>
          <div style={{ display: "grid", gridTemplateColumns: "1fr 140px", gap: 16, alignItems: "end" }}>
            <div className="field">
              <label>{t("settings.accept_from")}</label>
              <div className="seg" role="group">
                <button className="seg-opt" aria-pressed={!draft.listen.lan} onClick={() => edit({ listen: { ...draft.listen, lan: false } })}>
                  {t("settings.this_computer")}
                </button>
                <button className="seg-opt" aria-pressed={draft.listen.lan} onClick={() => edit({ listen: { ...draft.listen, lan: true } })}>
                  {t("settings.local_network")}
                </button>
              </div>
            </div>
            <div className="field">
              <label>{t("settings.port")}</label>
              <input
                className="input num"
                inputMode="numeric"
                value={draft.listen.port || ""}
                onChange={(e) => edit({ listen: { ...draft.listen, port: Number(e.target.value.replace(/\D/g, "")) } })}
              />
            </div>
          </div>
          {draft.listen.lan && (
            <div className="attention">
              <span className="note-accent">
                {t("settings.lan_warning")}
              </span>
              <div className="field">
                <label>{t("settings.lan_url")}</label>
                <input className="input num" readOnly value={walletUrl(lanHost, draft.listen.port || saved.listen.port, firstChain)} />
              </div>
            </div>
          )}
        </section>

        <section className="settings-section">
          <h5>{t("settings.provider_keys")}</h5>
          <span className="text-muted">{t("settings.provider_hint")}</span>
          <div className="key-grid">
            {snapshot.providers.map((p) => (
              <div className="key-row" key={p.id}>
                <label htmlFor={`key-${p.id}`}>{p.name}</label>
                <div className="row">
                  <input
                    id={`key-${p.id}`}
                    className="input"
                    type={shown[p.id] ? "text" : "password"}
                    autoComplete="off"
                    spellCheck={false}
                    placeholder={p.id === "infura" ? t("settings.project_id") : t("settings.api_key")}
                    value={draft.providers[p.id] ?? ""}
                    onChange={(e) => edit({ providers: { ...draft.providers, [p.id]: e.target.value } })}
                  />
                  <button className="btn btn-ghost btn-icon" aria-label={shown[p.id] ? t("settings.hide_key") : t("settings.show_key")} onClick={() => setShown({ ...shown, [p.id]: !shown[p.id] })}>
                    {shown[p.id] ? <EyeOff size={15} /> : <Eye size={15} />}
                  </button>
                </div>
                <span className="fine">{chainNames(p.chains)}</span>
              </div>
            ))}
          </div>
        </section>

        <section className="settings-section">
          <h5>
            {t("settings.tenderly")}
            {tenderlyReady(saved) ? <span className="tag tag-accent">{t("settings.connected")}</span> : <span className="tag tag-neutral">{t("settings.not_set")}</span>}
          </h5>
          <span className="text-muted">{t("settings.tenderly_hint")}</span>
          <div className="grid2">
            <div className="field">
              <label>{t("settings.account_slug")}</label>
              <input className="input" spellCheck={false} value={draft.tenderly.account} onChange={(e) => edit({ tenderly: { ...draft.tenderly, account: e.target.value } })} />
            </div>
            <div className="field">
              <label>{t("settings.project_slug")}</label>
              <input className="input" spellCheck={false} value={draft.tenderly.project} onChange={(e) => edit({ tenderly: { ...draft.tenderly, project: e.target.value } })} />
            </div>
          </div>
          <div className="field">
            <label>{t("settings.access_key")}</label>
            <input
              className="input"
              type="password"
              autoComplete="off"
              value={draft.tenderly.access_key}
              onChange={(e) => edit({ tenderly: { ...draft.tenderly, access_key: e.target.value } })}
            />
            <span className="hint">
              {t("settings.slug_hint")}{" "}
              <button className="link-btn" onClick={() => openUrl("https://dashboard.tenderly.co/account/authorization")}>
                {t("settings.create_key")}
              </button>
              . {t("settings.tenderly_privacy")}
            </span>
          </div>
        </section>

        <section className="settings-section">
          <h5>
            {t("settings.safe_service")}
            {draft.safe.service_enabled ? <span className="tag tag-accent">{t("settings.on")}</span> : <span className="tag tag-neutral">{t("settings.off")}</span>}
          </h5>
          <span className="text-muted">{t("settings.safe_service_hint")}</span>
          <label className="check">
            <input
              type="checkbox"
              checked={draft.safe.service_enabled}
              onChange={(e) => edit({ safe: { ...draft.safe, service_enabled: e.target.checked } })}
            />
            <span className="box" />
            {t("settings.use_service")}
          </label>
          <div className="grid2">
            <div className="field">
              <label>{t("settings.service_key")}</label>
              <input
                className="input"
                type="password"
                autoComplete="off"
                value={draft.safe.service_api_key}
                onChange={(e) => edit({ safe: { ...draft.safe, service_api_key: e.target.value } })}
              />
              <span className="hint">
                {t("settings.service_key_hint")}{" "}
                <button className="link-btn" onClick={() => openUrl("https://developer.safe.global")}>
                  {t("settings.get_key")}
                </button>
              </span>
            </div>
            <div className="field">
              <label>{t("settings.service_url")}</label>
              <input
                className="input"
                spellCheck={false}
                placeholder="https://api.safe.global/tx-service/<chain>"
                value={draft.safe.service_url}
                onChange={(e) => edit({ safe: { ...draft.safe, service_url: e.target.value } })}
              />
            </div>
          </div>
          <div className="field" style={{ maxWidth: 360 }}>
            <label>{t("settings.poll")}</label>
            <div className="row">
              <input
                className="input num"
                inputMode="numeric"
                value={draft.safe.poll_secs || ""}
                onChange={(e) => edit({ safe: { ...draft.safe, poll_secs: Number(e.target.value.replace(/\D/g, "")) } })}
              />
              <span>{t("settings.seconds")}</span>
            </div>
          </div>
        </section>

        <section className="settings-section">
          <h5>{t("settings.request_log")}</h5>
          <div className="field">
            <div className="seg" role="group" aria-label={t("settings.request_log")}>
              <button className="seg-opt" aria-pressed={draft.logging.mode === "tx_only"} onClick={() => edit({ logging: { mode: "tx_only" } })}>
                {t("settings.log_tx")}
              </button>
              <button className="seg-opt" aria-pressed={draft.logging.mode === "all"} onClick={() => edit({ logging: { mode: "all" } })}>
                {t("settings.log_all")}
              </button>
            </div>
            <span className="hint">{t("settings.log_path", { path: snapshot.db_path })}</span>
          </div>
          <div className="field" style={{ maxWidth: 360 }}>
            <label>{t("settings.window")}</label>
            <div className="row">
              <input
                className="input num"
                inputMode="numeric"
                value={draft.preview_window_secs || ""}
                onChange={(e) => edit({ preview_window_secs: Number(e.target.value.replace(/\D/g, "")) })}
              />
              <span>{t("settings.seconds")}</span>
            </div>
          </div>
        </section>

        <p className="fine" style={{ margin: 0 }}>
          {t("settings.config_path", { path: snapshot.config_path })}
        </p>
      </div>

      <div className="save-bar">
        {invalid && dirty ? (
          <span className="error-text">{invalid}</span>
        ) : result ? (
          <span className={result.ok ? "ok-text" : "error-text"}>{result.text}</span>
        ) : (
          <span className="text-muted">{dirty ? t("settings.unsaved") : t("settings.all_saved")}</span>
        )}
        <div className="spacer" />
        <button className="btn btn-ghost" disabled={!dirty || saving} onClick={() => edit(saved)}>
          {t("settings.discard")}
        </button>
        <button className="btn btn-primary" disabled={!dirty || !!invalid || saving} onClick={save}>
          {saving ? t("settings.saving") : t("settings.save")}
        </button>
      </div>
    </>
  );
}
