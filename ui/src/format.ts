// Display helpers shared across views.

import type { Config, Status } from "./api";
import { t } from "./i18n";

export function shortAddr(addr: string | null | undefined): string {
  if (!addr) return "—";
  return addr.length > 12 ? `${addr.slice(0, 6)}…${addr.slice(-4)}` : addr;
}

const pad = (n: number) => String(n).padStart(2, "0");

export function clock(ts: number): string {
  const d = new Date(ts);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function dateTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${clock(ts)}`;
}

/** Converts a hex or decimal integer string to BigInt (0 on failure). */
export function toBig(v: string | number | null | undefined): bigint {
  if (v === null || v === undefined || v === "") return 0n;
  try {
    return BigInt(v);
  } catch {
    return 0n;
  }
}

/** Formats an integer amount with `decimals`, keeping up to `maxFrac` fraction digits. */
export function formatUnits(v: bigint, decimals: number, maxFrac = 6): string {
  const neg = v < 0n;
  const abs = neg ? -v : v;
  const base = 10n ** BigInt(decimals);
  const int = abs / base;
  let frac = (abs % base).toString().padStart(decimals, "0").slice(0, maxFrac).replace(/0+$/, "");
  const intStr = int.toLocaleString("en-US");
  if (frac === "" && abs % base !== 0n) frac = "0".repeat(maxFrac - 1) + "1"; // tiny non-zero amount
  return `${neg ? "-" : ""}${intStr}${frac ? "." + frac : ""}`;
}

let nativeCurrencies = new Map<number, string>();

/** Native currencies other than ETH, from the engine (set once from the snapshot). */
export function setNativeCurrencies(list: [number, string][]): void {
  nativeCurrencies = new Map(list);
}

/** A native-currency amount (wei) in the chain's own unit: ETH, BNB, POL, … */
export function eth(v: string | null | undefined, chainId: number): string {
  return `${formatUnits(toBig(v), 18)} ${nativeCurrencies.get(chainId) ?? "ETH"}`;
}

export function gwei(v: string | null | undefined): string {
  if (!v) return "—";
  return `${formatUnits(toBig(v), 9, 3)} gwei`;
}

export function elapsed(fromMs: number, nowMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - fromMs) / 1000));
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)}:${pad(m % 60)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

export function networkName(config: Config | undefined, chainId: number): string {
  return config?.networks.find((n) => n.chain_id === chainId)?.name ?? t("format.chain", { id: chainId });
}

export const STATUSES: Status[] = ["forwarded", "failed", "invalid", "pending", "disconnected", "approved", "broadcast_failed", "auto_approved", "rejected", "abandoned"];

export function statusLabel(s: Status): string {
  return t(`status.${s}`);
}

export const STATUS_TAG: Record<Status, string> = {
  forwarded: "tag tag-neutral",
  failed: "tag tag-outline",
  invalid: "tag tag-neutral",
  pending: "tag tag-outline",
  disconnected: "tag tag-outline",
  approved: "tag tag-accent",
  broadcast_failed: "tag tag-outline",
  auto_approved: "tag tag-accent",
  rejected: "tag tag-neutral",
  abandoned: "tag tag-neutral",
};

export function walletUrl(host: string, port: number, chainId: number): string {
  return `http://${host}:${port}/${chainId}`;
}

export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

/** In-app simulation needs all three Tenderly settings. */
export function tenderlyReady(config: Config): boolean {
  const t = config.tenderly;
  return Boolean(t.account && t.project && t.access_key);
}
