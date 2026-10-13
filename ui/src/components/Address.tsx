// Every address in the UI goes through <Address>: it links to the network's
// block explorer and, on hover or focus, offers to open, copy or annotate it.
// Notes are the user's own, kept per network. Token contracts on the bundled
// Uniswap token list are labelled too. The card is English only.

import { createContext, Fragment, useContext, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorText, type AddressNote } from "../api";
import { copyText, shortAddr } from "../format";
import { t, type Text } from "../i18n";
import tokenList from "../data/token-list.json";

const ADDRESS = /^0x[0-9a-fA-F]{40}$/;
/** Full addresses inside a longer string (not part of a longer hex value). */
const ADDRESSES = /(?<![0-9a-zA-Z])(0x[0-9a-fA-F]{40})(?![0-9a-fA-F])/g;
const MAX_NOTE = 100;

// ---------------------------------------------------------------------------
// Explorers and notes, shared by every <Address>
// ---------------------------------------------------------------------------

let explorers = new Map<number, string>();
let notes = new Map<string, string>();
let notesVersion = 0;
const listeners = new Set<() => void>();

const noteKey = (chainId: number, address: string) => `${chainId}:${address.toLowerCase()}`;

/** Tokens of the Uniswap Labs Default list (scripts/update-token-list.mjs), by chain and address. */
const tokens = new Map((tokenList.tokens as [number, string, string, string][]).map(([chain, address, symbol, name]) => [noteKey(chain, address), { symbol, name }]));
const TOKEN_SOURCE = `${tokenList.name} token list`;

/** Block explorer base URLs by chain id, from the engine. */
export function setExplorers(list: [number, string][]): void {
  explorers = new Map(list);
}

/** Replaces all notes (from the snapshot or an engine event). */
export function setAddressNotes(list: AddressNote[]): void {
  notes = new Map(list.map((n) => [noteKey(n.chain_id, n.address), n.note]));
  notesVersion++;
  listeners.forEach((l) => l());
}

function useNotes(): number {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => notesVersion,
  );
}

export function explorerUrl(chainId: number | undefined, address: string): string | null {
  const base = chainId === undefined ? undefined : explorers.get(chainId);
  return base ? `${base}/address/${address}` : null;
}

/** The network addresses below it belong to. */
const ChainCtx = createContext<number | undefined>(undefined);

export function OnChain({ id, children }: { id: number; children: ReactNode }) {
  return <ChainCtx.Provider value={id}>{children}</ChainCtx.Provider>;
}

// ---------------------------------------------------------------------------
// <Address>
// ---------------------------------------------------------------------------

export function Address({
  address,
  chainId,
  full,
  label,
}: {
  address: string | null | undefined;
  /** Defaults to the surrounding <OnChain>. */
  chainId?: number;
  /** Show all 40 digits instead of 0x1234…abcd. */
  full?: boolean;
  /** Text to show instead of the address, e.g. a token symbol. */
  label?: string;
}) {
  const ctxChain = useContext(ChainCtx);
  const chain = chainId ?? ctxChain;
  useNotes();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (!address || !ADDRESS.test(address)) return <>{address || "—"}</>;
  const note = chain === undefined ? undefined : notes.get(noteKey(chain, address));
  const token = chain === undefined ? undefined : tokens.get(noteKey(chain, address));
  const url = explorerUrl(chain, address);
  const text = label ?? (full ? address : shortAddr(address));

  // Hovering opens the card after a moment; leaving the address and the card closes it.
  const show = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(true), 250);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    if (!editing) timer.current = window.setTimeout(() => setOpen(false), 200);
  };
  const close = () => {
    setEditing(false);
    setOpen(false);
  };

  return (
    <span className="addr" ref={anchor} onMouseEnter={show} onMouseLeave={hide}>
      {note ? <span className="addr-note">{note}</span> : token && !label && <span className="addr-token">{token.symbol}</span>}
      {url ? (
        <a
          className={`addr-link${full ? " mono" : ""}`}
          href={url}
          onClick={(e) => {
            e.preventDefault();
            openUrl(url).catch(() => window.open(url, "_blank", "noopener"));
          }}
          onFocus={() => setOpen(true)}
          aria-describedby={open ? "addr-card" : undefined}
        >
          {text}
        </a>
      ) : (
        <span className={`addr-text${full ? " mono" : ""}`} tabIndex={0} onFocus={() => setOpen(true)}>
          {text}
        </span>
      )}
      {open &&
        createPortal(
          <AddressCard
            anchor={anchor}
            address={address}
            chainId={chain}
            note={note}
            token={token}
            url={url}
            editing={editing}
            setEditing={setEditing}
            onEnter={() => window.clearTimeout(timer.current)}
            onLeave={hide}
            onClose={close}
          />,
          document.body,
        )}
    </span>
  );
}

function AddressCard({
  anchor,
  address,
  chainId,
  note,
  token,
  url,
  editing,
  setEditing,
  onEnter,
  onLeave,
  onClose,
}: {
  anchor: React.RefObject<HTMLSpanElement | null>;
  address: string;
  chainId: number | undefined;
  note: string | undefined;
  token: { symbol: string; name: string } | undefined;
  url: string | null;
  editing: boolean;
  setEditing: (v: boolean) => void;
  onEnter: () => void;
  onLeave: () => void;
  onClose: () => void;
}) {
  const card = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [copied, setCopied] = useState(false);
  const [draft, setDraft] = useState(note ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Below the address, or above it when there is no room; always inside the window.
  useLayoutEffect(() => {
    const a = anchor.current?.getBoundingClientRect();
    const c = card.current?.getBoundingClientRect();
    if (!a || !c) return;
    const below = a.bottom + 6 + c.height <= window.innerHeight - 8;
    setPos({
      left: Math.max(8, Math.min(a.left, window.innerWidth - c.width - 8)),
      top: below ? a.bottom + 6 : Math.max(8, a.top - c.height - 6),
    });
  }, [anchor, editing, error]);

  // Escape closes the card (and not the dialog behind it); a click elsewhere does too.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopImmediatePropagation();
        onClose();
      }
    };
    const onDown = (e: MouseEvent) => {
      if (!card.current?.contains(e.target as Node) && !anchor.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [anchor, onClose]);

  const save = async (value: string) => {
    if (chainId === undefined) return;
    setSaving(true);
    setError(null);
    try {
      setAddressNotes(await api.setAddressNote(chainId, address, value));
      setEditing(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  const host = url ? new URL(url).host : null;
  return (
    <div
      ref={card}
      id="addr-card"
      className="addr-card"
      role="dialog"
      aria-label="Address"
      style={pos ? { left: pos.left, top: pos.top } : { left: -9999, top: 0 }}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
    >
      {note && !editing && <span className="addr-card-note">{note}</span>}
      {token && (
        <span className="addr-card-token">
          {token.name} ({token.symbol}) <span className="fine">· {TOKEN_SOURCE}</span>
        </span>
      )}
      <span className="addr-card-address mono">{address}</span>
      {editing ? (
        <form
          className="addr-card-edit"
          onSubmit={(e) => {
            e.preventDefault();
            save(draft);
          }}
        >
          <input
            className="input"
            autoFocus
            maxLength={MAX_NOTE}
            placeholder="e.g. My cold wallet"
            aria-label={`Note for this address on chain ${chainId}`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <span className="fine">Only you see this note. It applies on chain {chainId} only.</span>
          {error && <span className="error-text">{error}</span>}
          <div className="addr-card-actions">
            {note && (
              <button type="button" className="btn btn-ghost" disabled={saving} onClick={() => save("")}>
                Remove note
              </button>
            )}
            <button type="button" className="btn btn-ghost" style={{ marginLeft: "auto" }} onClick={() => setEditing(false)}>
              Cancel
            </button>
            <button type="submit" className="btn btn-primary" disabled={saving}>
              Save note
            </button>
          </div>
        </form>
      ) : (
        <div className="addr-card-actions">
          {url ? (
            <button className="btn btn-ghost" onClick={() => openUrl(url).catch(() => window.open(url, "_blank", "noopener"))}>
              Open in {host}
            </button>
          ) : (
            <span className="fine">No block explorer known for {chainId === undefined ? "this network" : `chain ${chainId}`}</span>
          )}
          <button
            className="btn btn-ghost"
            onClick={async () => {
              await copyText(address);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            }}
          >
            {copied ? "Copied" : "Copy address"}
          </button>
          {chainId !== undefined && (
            <button
              className="btn btn-ghost"
              onClick={() => {
                setDraft(note ?? "");
                setEditing(true);
              }}
            >
              {note ? "Edit note" : "Add note"}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Text with addresses in it
// ---------------------------------------------------------------------------

/** A string in which every full address becomes an <Address>. */
export function WithAddresses({ text, full }: { text: string; full?: boolean }) {
  const parts = text.split(ADDRESSES);
  return (
    <>
      {parts.map((p, i) => (i % 2 === 1 ? <Address key={i} address={p} full={full} /> : <Fragment key={i}>{p}</Fragment>))}
    </>
  );
}

/** An engine message in the current language, with its addresses linked. */
export function EngineText({ text, fallback }: { text: Text | null | undefined; fallback: string }) {
  if (!text) return <WithAddresses text={fallback} />;
  return (
    <>
      {(text.prefix ?? []).map((p, i) => (
        <EngineText key={i} text={p} fallback="" />
      ))}
      {t(text.key)
        .split(/(\{\w+\})/)
        .map((part, i) => {
          const name = part.match(/^\{(\w+)\}$/)?.[1];
          const value = name ? text.params?.[name] : undefined;
          return value !== undefined ? <WithAddresses key={i} text={value} /> : <Fragment key={i}>{part}</Fragment>;
        })}
    </>
  );
}
