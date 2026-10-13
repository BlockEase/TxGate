// Networks: one local wallet URL per chain, each with a read and a broadcast upstream.

import { useState } from "react";
import { Copy, Plus } from "lucide-react";
import { api, errorText, type Config, type NetworkConfig, type ProviderId, type RpcSource, type Snapshot } from "../api";
import { copyText, walletUrl } from "../format";
import { t } from "../i18n";

type SourceKind = "custom" | ProviderId | "same";

interface Draft {
  original: number | null;
  name: string;
  chainId: string;
  readKind: SourceKind;
  readUrl: string;
  sendKind: SourceKind;
  sendUrl: string;
}

function toDraft(n: NetworkConfig | null): Draft {
  if (!n) return { original: null, name: "", chainId: "", readKind: "custom", readUrl: "", sendKind: "same", sendUrl: "" };
  const kind = (s: RpcSource): SourceKind => (s.kind === "custom" ? "custom" : s.provider);
  return {
    original: n.chain_id,
    name: n.name,
    chainId: String(n.chain_id),
    readKind: kind(n.read),
    readUrl: n.read.kind === "custom" ? n.read.url : "",
    sendKind: n.broadcast ? kind(n.broadcast) : "same",
    sendUrl: n.broadcast?.kind === "custom" ? n.broadcast.url : "",
  };
}

function toSource(kind: SourceKind, url: string): RpcSource | null {
  if (kind === "same") return null;
  if (kind === "custom") return { kind: "custom", url: url.trim() };
  return { kind: "provider", provider: kind };
}

export function describeSource(s: RpcSource | null | undefined, snapshot: Snapshot): string {
  if (!s) return t("networks.same_as_read");
  if (s.kind === "custom") {
    try {
      return new URL(s.url).host;
    } catch {
      return s.url;
    }
  }
  return snapshot.providers.find((p) => p.id === s.provider)?.name ?? s.provider;
}

export function Networks({ snapshot, onConfig }: { snapshot: Snapshot; onConfig: (c: Config) => void }) {
  const config = snapshot.config;
  const [draft, setDraft] = useState<Draft | null>(config.networks[0] ? toDraft(config.networks[0]) : null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const port = config.listen.port;

  const edit = (n: NetworkConfig | null) => {
    setDraft(toDraft(n));
    setError(null);
    setSaved(null);
  };

  const update = (d: Draft) => {
    setDraft(d);
    setSaved(null);
  };

  const chainId = Number(draft?.chainId);
  const keyed = (p: ProviderId) => Boolean(config.providers[p]?.trim());
  const providerOptions = snapshot.providers.filter((p) => p.chains.includes(chainId));

  const save = async () => {
    if (!draft) return;
    if (!Number.isInteger(chainId) || chainId <= 0) return setError(t("networks.err_chain_id"));
    if (!draft.name.trim()) return setError(t("networks.err_name"));
    const read = toSource(draft.readKind, draft.readUrl);
    if (!read) return setError(t("networks.err_read"));
    if (read.kind === "custom" && !read.url) return setError(t("networks.err_read_url"));
    const broadcast = toSource(draft.sendKind, draft.sendUrl);
    if (broadcast?.kind === "custom" && !broadcast.url) return setError(t("networks.err_send_url"));
    if (draft.original !== chainId && config.networks.some((n) => n.chain_id === chainId)) {
      return setError(t("networks.err_exists", { id: chainId }));
    }
    setSaving(true);
    setError(null);
    try {
      const network: NetworkConfig = { chain_id: chainId, name: draft.name.trim(), read, broadcast };
      const next = await api.saveNetwork(network, draft.original);
      onConfig(next);
      setDraft({ ...draft, original: chainId });
      setSaved(t("networks.saved"));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (draft?.original === null || draft?.original === undefined) return;
    try {
      const next = await api.deleteNetwork(draft.original);
      onConfig(next);
      edit(next.networks[0] ?? null);
    } catch (e) {
      setError(errorText(e));
    }
  };

  const sourcePicker = (label: string, kind: SourceKind, url: string, allowSame: boolean, set: (k: SourceKind, u: string) => void) => (
    <div className="field">
      <label>{label}</label>
      <div className="row">
        <select className="input" style={{ flex: "0 0 170px" }} value={kind} onChange={(e) => set(e.target.value as SourceKind, url)}>
          {allowSame && <option value="same">{t("networks.same_as_read")}</option>}
          <option value="custom">{t("networks.custom_url")}</option>
          {providerOptions.map((p) => (
            <option key={p.id} value={p.id} disabled={!keyed(p.id)}>
              {p.name}
              {keyed(p.id) ? "" : " (no key saved)"}
            </option>
          ))}
        </select>
        {kind === "custom" && <input className="input" placeholder="https://…" value={url} onChange={(e) => set(kind, e.target.value)} />}
      </div>
    </div>
  );

  return (
    <div className="page">
      <div className="page-head">
        <h3>{t("nav.networks")}</h3>
        <span className="text-muted" style={{ flex: 1 }}>
          {t("networks.intro")}
        </span>
        <button className="btn btn-primary" onClick={() => edit(null)}>
          <Plus size={15} /> {t("networks.add")}
        </button>
      </div>
      {config.networks.length > 0 ? (
        <table className="table" style={{ fontSize: 13 }}>
          <thead>
            <tr>
              <th>{t("table.network")}</th>
              <th>{t("networks.chain_id")}</th>
              <th>{t("networks.wallet_url")}</th>
              <th>{t("networks.read")}</th>
              <th>{t("networks.broadcast")}</th>
            </tr>
          </thead>
          <tbody>
            {config.networks.map((n) => (
              <tr key={n.chain_id} className={draft?.original === n.chain_id ? "hot" : ""} onClick={() => edit(n)}>
                <td>{n.name}</td>
                <td className="num">{n.chain_id}</td>
                <td className="num">{walletUrl("127.0.0.1", port, n.chain_id)}</td>
                <td>{describeSource(n.read, snapshot)}</td>
                <td className={n.broadcast ? "" : "text-muted"}>{describeSource(n.broadcast, snapshot)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="empty">
          <h4>{t("networks.empty")}</h4>
          <span>{t("networks.empty_hint")}</span>
        </div>
      )}

      {draft && (
        <div className="card editor">
          <div className="card-kicker">{draft.original === null ? t("networks.new") : t("networks.edit", { name: draft.name })}</div>
          {draft.original === null && (
            <div className="field">
              <label>{t("networks.known")}</label>
              <select
                className="input"
                value=""
                onChange={(e) => {
                  const k = snapshot.known_chains.find((c) => String(c.chain_id) === e.target.value);
                  if (k) update({ ...draft, name: k.name, chainId: String(k.chain_id) });
                }}
              >
                <option value="">{t("networks.choose")}</option>
                {snapshot.known_chains
                  .filter((k) => !config.networks.some((n) => n.chain_id === k.chain_id))
                  .map((k) => (
                    <option key={k.chain_id} value={k.chain_id}>
                      {k.name} · {k.chain_id}
                    </option>
                  ))}
              </select>
            </div>
          )}
          <div className="grid2">
            <div className="field">
              <label>{t("networks.name")}</label>
              <input className="input" value={draft.name} onChange={(e) => update({ ...draft, name: e.target.value })} />
            </div>
            <div className="field">
              <label>{t("networks.chain_id_hint")}</label>
              <input className="input num" inputMode="numeric" value={draft.chainId} onChange={(e) => update({ ...draft, chainId: e.target.value.replace(/\D/g, "") })} />
            </div>
            {sourcePicker(t("networks.read_label"), draft.readKind, draft.readUrl, false, (k, u) =>
              update({ ...draft, readKind: k, readUrl: u }),
            )}
            {sourcePicker(t("networks.broadcast_label"), draft.sendKind, draft.sendUrl, true, (k, u) => update({ ...draft, sendKind: k, sendUrl: u }))}
          </div>
          {[draft.readUrl, draft.sendUrl].some((u) => u.includes("relay.flashbots.net")) && (
            <p className="note-accent" style={{ margin: 0, fontSize: 12 }}>
              {t("networks.relay_warning")}
            </p>
          )}
          {Number.isInteger(chainId) && chainId > 0 && (
            <div className="field">
              <label>{t("networks.wallet_url")}</label>
              <div className="row">
                <input className="input num" readOnly value={walletUrl("127.0.0.1", port, chainId)} />
                <button className="btn btn-secondary" onClick={() => copyText(walletUrl("127.0.0.1", port, chainId))}>
                  <Copy size={14} /> {t("common.copy")}
                </button>
              </div>
            </div>
          )}
          {error && <span className="error-text">{error}</span>}
          {saved && !error && <span className="ok-text">{saved}</span>}
          <div className="actions">
            <button className="btn btn-primary" onClick={save} disabled={saving}>
              {saving ? t("networks.checking") : t("networks.save")}
            </button>
            {draft.original !== null && (
              <button className="btn btn-secondary" onClick={remove}>
                {t("networks.remove")}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
