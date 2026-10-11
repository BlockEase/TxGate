// Everything stored in config.toml except networks: where TxGate listens,
// provider keys, Tenderly credentials and logging. One draft, one save.

import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Eye, EyeOff } from "lucide-react";
import { api, errorText, type Config, type ProviderId, type ServerStatus, type Snapshot } from "../api";
import { tenderlyReady, walletUrl } from "../format";

/** Trims text fields and drops empty provider keys. */
function normalise(c: Config): Config {
  const providers: Config["providers"] = {};
  for (const [id, key] of Object.entries(c.providers) as [ProviderId, string | undefined][]) {
    if (key?.trim()) providers[id] = key.trim();
  }
  const t = c.tenderly;
  return { ...c, providers, tenderly: { account: t.account.trim(), project: t.project.trim(), access_key: t.access_key.trim() } };
}

function validate(c: Config): string | null {
  const port = c.listen.port;
  if (!Number.isInteger(port) || port < 1 || port > 65535) return "Port must be between 1 and 65535.";
  if (!Number.isInteger(c.preview_window_secs) || c.preview_window_secs < 10) return "The estimate window must be at least 10 seconds.";
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

  const next = normalise(draft);
  const dirty = JSON.stringify(next) !== JSON.stringify(normalise(saved));
  const invalid = validate(next);
  const edit = (change: Partial<Config>) => {
    setDraft({ ...draft, ...change });
    setResult(null);
  };
  const lanHost = snapshot.lan_addresses[0] ?? "<this computer's IP>";
  const firstChain = saved.networks[0]?.chain_id ?? 1;

  const save = async () => {
    setSaving(true);
    // Networks are edited on their own page; never write back a stale copy.
    const toSave = { ...next, networks: snapshot.config.networks };
    try {
      const status = await api.saveConfig(toSave);
      onConfig(toSave);
      onServer(status);
      setDraft(toSave);
      setResult(status.error ? { ok: false, text: status.error } : { ok: true, text: `Saved. Listening on ${status.address}.` });
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
      .join(", ") + (ids.length > 3 ? ` and ${ids.length - 3} more` : "");

  return (
    <>
      <div className="page narrow">
        <h3 style={{ margin: 0, fontSize: 18 }}>Settings</h3>

        <section className="settings-section">
          <h5>Connections</h5>
          <div style={{ display: "grid", gridTemplateColumns: "1fr 140px", gap: 16, alignItems: "end" }}>
            <div className="field">
              <label>Accept connections from</label>
              <div className="seg" role="group">
                <button className="seg-opt" aria-pressed={!draft.listen.lan} onClick={() => edit({ listen: { ...draft.listen, lan: false } })}>
                  This computer only
                </button>
                <button className="seg-opt" aria-pressed={draft.listen.lan} onClick={() => edit({ listen: { ...draft.listen, lan: true } })}>
                  Local network
                </button>
              </div>
            </div>
            <div className="field">
              <label>Port</label>
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
                Any device on your local network can use this port, and with it your provider keys. Transactions from other devices still need your approval here.
              </span>
              <div className="field">
                <label>RPC URL for wallets on other devices</label>
                <input className="input num" readOnly value={walletUrl(lanHost, draft.listen.port || saved.listen.port, firstChain)} />
              </div>
            </div>
          )}
        </section>

        <section className="settings-section">
          <h5>Provider keys</h5>
          <span className="text-muted">Save a key once, then pick the provider as an upstream under Networks. For QuickNode or Chainstack, paste the endpoint URL there instead.</span>
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
                    placeholder={p.id === "infura" ? "Project ID" : "API key"}
                    value={draft.providers[p.id] ?? ""}
                    onChange={(e) => edit({ providers: { ...draft.providers, [p.id]: e.target.value } })}
                  />
                  <button className="btn btn-ghost btn-icon" aria-label={shown[p.id] ? "Hide key" : "Show key"} onClick={() => setShown({ ...shown, [p.id]: !shown[p.id] })}>
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
            Tenderly simulation
            {tenderlyReady(saved) ? <span className="tag tag-accent">Connected</span> : <span className="tag tag-neutral">Not set</span>}
          </h5>
          <span className="text-muted">
            With an access key, held transactions are simulated as they arrive and the approval dialog shows what you would lose, receive or approve. Without one,
            “Simulate in Tenderly” opens the web simulator instead.
          </span>
          <div className="grid2">
            <div className="field">
              <label>Account slug</label>
              <input className="input" spellCheck={false} value={draft.tenderly.account} onChange={(e) => edit({ tenderly: { ...draft.tenderly, account: e.target.value } })} />
            </div>
            <div className="field">
              <label>Project slug</label>
              <input className="input" spellCheck={false} value={draft.tenderly.project} onChange={(e) => edit({ tenderly: { ...draft.tenderly, project: e.target.value } })} />
            </div>
          </div>
          <div className="field">
            <label>Access key</label>
            <input
              className="input"
              type="password"
              autoComplete="off"
              value={draft.tenderly.access_key}
              onChange={(e) => edit({ tenderly: { ...draft.tenderly, access_key: e.target.value } })}
            />
            <span className="hint">
              The slugs are the two path segments of your dashboard URL: dashboard.tenderly.co/<i>account</i>/<i>project</i>.{" "}
              <button className="link-btn" onClick={() => openUrl("https://dashboard.tenderly.co/account/authorization")}>
                Create an access key
              </button>
              . Simulating sends the transaction's sender, target, value and calldata to Tenderly.
            </span>
          </div>
        </section>

        <section className="settings-section">
          <h5>Request log</h5>
          <div className="field">
            <div className="seg" role="group" aria-label="Request log">
              <button className="seg-opt" aria-pressed={draft.logging.mode === "tx_only"} onClick={() => edit({ logging: { mode: "tx_only" } })}>
                Transactions and gas estimates
              </button>
              <button className="seg-opt" aria-pressed={draft.logging.mode === "all"} onClick={() => edit({ logging: { mode: "all" } })}>
                Every request
              </button>
            </div>
            <span className="hint">The log is kept forever in {snapshot.db_path}.</span>
          </div>
          <div className="field" style={{ maxWidth: 360 }}>
            <label>Compare a transaction with gas estimates from the last</label>
            <div className="row">
              <input
                className="input num"
                inputMode="numeric"
                value={draft.preview_window_secs || ""}
                onChange={(e) => edit({ preview_window_secs: Number(e.target.value.replace(/\D/g, "")) })}
              />
              <span>seconds</span>
            </div>
          </div>
        </section>

        <p className="fine" style={{ margin: 0 }}>
          Settings and keys are stored in plain text in {snapshot.config_path}.
        </p>
      </div>

      <div className="save-bar">
        {invalid && dirty ? (
          <span className="error-text">{invalid}</span>
        ) : result ? (
          <span className={result.ok ? "ok-text" : "error-text"}>{result.text}</span>
        ) : (
          <span className="text-muted">{dirty ? "You have unsaved changes." : "All changes saved."}</span>
        )}
        <div className="spacer" />
        <button className="btn btn-ghost" disabled={!dirty || saving} onClick={() => edit(saved)}>
          Discard
        </button>
        <button className="btn btn-primary" disabled={!dirty || !!invalid || saving} onClick={save}>
          {saving ? "Saving…" : "Save settings"}
        </button>
      </div>
    </>
  );
}
