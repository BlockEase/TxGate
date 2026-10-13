// Typed bridge to the Rust shell. Types mirror the serde output of txgate-core.

import type { Text } from "./i18n";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Outside the desktop app (plain browser, dev builds only) the UI runs on sample data. */
const browserPreview = import.meta.env.DEV && !("__TAURI_INTERNALS__" in window);

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (browserPreview) {
    const { mockInvoke } = await import("./mock");
    return (await mockInvoke(cmd)) as T;
  }
  return tauriInvoke<T>(cmd, args);
}

export type Category = "read" | "estimate" | "send";
export type Status =
  | "forwarded"
  | "failed"
  | "invalid"
  | "pending"
  | "disconnected"
  | "approved"
  | "broadcast_failed"
  | "auto_approved"
  | "rejected"
  | "abandoned";

export type Severity = "info" | "warning" | "danger";
export type ProviderId = "alchemy" | "infura" | "ankr" | "drpc";

export interface RequestRecord {
  id: number;
  ts_ms: number;
  chain_id: number;
  method: string;
  category: Category;
  client: string;
  summary: string;
  status: Status;
  duration_ms: number | null;
  tx_hash: string | null;
  from: string | null;
  to: string | null;
  request: unknown;
  response: unknown | null;
  detail: TxDetail | null;
}

export interface TxFields {
  from: string | null;
  to: string | null;
  value: string; // hex quantity (alloy U256 serde)
  input: string;
  nonce: number | null;
  gas_limit: number | null;
  chain_id: number | null;
}

export interface Arg {
  name: string;
  value: string;
}

export interface Risk {
  severity: Severity;
  code: string;
  message: string;
  text?: Text;
}

export interface CallInfo {
  selector: string | null;
  signature: string | null;
  function: string | null;
  args: Arg[];
}

export interface Analysis {
  call: CallInfo;
  risks: Risk[];
  summary: string;
  /** Present when the transaction calls a Safe's execTransaction. */
  safe?: SafeExec;
  /** The calls a smart account is asked to make (ERC-4337 / ERC-7579 execute). */
  account?: SafeTxAnalysis;
}

// — Safe multisig (mirrors txgate-core safe*.rs) —

/** The fields a Safe owner signs. Quantities are hex strings. */
export interface SafeTx {
  to: string;
  value: string;
  data: string;
  /** 0 = CALL, 1 = DELEGATECALL */
  operation: number;
  safe_tx_gas: string;
  base_gas: string;
  gas_price: string;
  gas_token: string;
  refund_receiver: string;
  nonce: string;
}

export interface SafeHashes {
  domain_hash: string;
  message_hash: string;
  safe_tx_hash: string;
}

export interface InnerCall {
  operation: number;
  to: string;
  value: string;
  data: string;
  call: CallInfo;
  summary: string;
}

export interface SafeTxAnalysis {
  calls: InnerCall[];
  /** Name of the official MultiSend library when the SafeTx is a batch. */
  batch: string | null;
  risks: Risk[];
  summary: string;
}

export interface SafeExec {
  safe: string;
  tx: SafeTx;
  signatures: string;
  analysis: SafeTxAnalysis;
}

export interface SafeState {
  address: string;
  version: string | null;
  nonce: string;
  threshold: number;
  owners: string[];
  singleton: string | null;
  singleton_name: string | null;
  guard: string | null;
  fallback_handler: string | null;
  modules: string[];
}

export type SignatureKind = "eip712" | "eth_sign" | "approved_hash" | "contract";

export interface CheckedSignature {
  kind: SignatureKind;
  signer: string | null;
  error: string | null;
  is_owner: boolean;
  valid: boolean;
  note: string | null;
  note_text?: Text;
}

export interface SafeCheck {
  state: SafeState | null;
  hashes: SafeHashes | null;
  signatures: CheckedSignature[];
  valid_signatures: number;
  risks: Risk[];
}

export interface SafeTxReport {
  chain_id: number;
  safe: string;
  tx: SafeTx;
  state: SafeState | null;
  hashes: SafeHashes;
  analysis: SafeTxAnalysis;
  service: { reported_hash: string; proposer: string | null; submission_date: string | null; origin: string | null } | null;
  confirmations: CheckedSignature[];
  risks: Risk[];
}

export interface WatchStatus {
  chain_id: number;
  address: string;
  label: string;
  state: SafeState | null;
  queue: SafeTxReport[];
  error: string | null;
  service_error: string | null;
  checked_ms: number;
  error_text?: Text;
  service_error_text?: Text;
}

export interface SafeAlert {
  id: number;
  ts_ms: number;
  chain_id: number;
  safe: string;
  severity: Severity;
  kind: "config_change" | "executed" | "proposed";
  message: string;
  safe_tx_hash: string | null;
  read: boolean;
  text?: Text;
}

export interface WatchedSafe {
  chain_id: number;
  address: string;
  label: string;
}

export interface SafeConfig {
  watched: WatchedSafe[];
  service_enabled: boolean;
  service_api_key: string;
  service_url: string;
  poll_secs: number;
}

export interface TokenInfo {
  address: string;
  symbol: string | null;
  decimals: number | null;
}

export interface Enrichment {
  risks: Risk[];
  token: TokenInfo | null;
  formatted_args: [string, string][];
  account_nonce: number | null;
  to_is_contract: boolean | null;
  /** execTransaction: hashes at the Safe's nonce and verified signatures. */
  safe?: SafeCheck;
  /** approveHash: the transaction the approved hash stands for. */
  safe_approval?: SafeTxReport;
}

export interface Authorization {
  chain_id: string;
  address: string;
  nonce: number;
  authority: string | null;
}

export interface TxView {
  hash: string | null;
  tx_type: string | null;
  fields: TxFields;
  max_fee_per_gas: string | null;
  max_priority_fee_per_gas: string | null;
  authorizations: Authorization[];
  analysis: Analysis;
  enrichment: Enrichment | null;
  simulator_url: string;
}

export interface FieldDiff {
  field: string;
  previewed: string;
  signed: string;
}

export type PreviewMatch =
  | { kind: "matched"; request_id: number; at_ms: number }
  | { kind: "mismatch"; request_id: number; at_ms: number; diffs: FieldDiff[] }
  | { kind: "no_preview" };

export interface AssetChange {
  direction: "in" | "out" | "other";
  symbol: string;
  token_address: string | null;
  amount: string;
  from: string | null;
  to: string | null;
  dollar_value: string | null;
}

export interface ApprovalChange {
  token_address: string | null;
  owner: string | null;
  spender: string | null;
  amount: string;
}

export interface SimulationResult {
  success: boolean;
  error: string | null;
  gas_used: number | null;
  block_number: number | null;
  asset_changes: AssetChange[];
  approvals: ApprovalChange[];
  simulation_id: string | null;
}

export type SimulationOutcome =
  | { status: "ok"; result: SimulationResult; ts_ms: number }
  | { status: "error"; message: string; ts_ms: number };

export interface TxDetail {
  txs: TxView[];
  preview: PreviewMatch | null;
  decode_error: string | null;
  dedupe_key: string | null;
  attempts: number;
  wallet_connections: number;
  simulation: SimulationOutcome | null;
}

export type RpcSource = { kind: "custom"; url: string } | { kind: "provider"; provider: ProviderId };

export interface NetworkConfig {
  chain_id: number;
  name: string;
  read: RpcSource;
  broadcast?: RpcSource | null;
}

export interface Config {
  listen: { lan: boolean; port: number };
  networks: NetworkConfig[];
  providers: Partial<Record<ProviderId, string>>;
  tenderly: { account: string; project: string; access_key: string };
  logging: { mode: "tx_only" | "all" };
  safe: SafeConfig;
  preview_window_secs: number;
  /** Interface language; absent follows the system. */
  language?: string | null;
}

export interface ServerStatus {
  listening: boolean;
  address: string | null;
  error: string | null;
}

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  chains: number[];
}

export interface Snapshot {
  config: Config;
  recent: RequestRecord[];
  pending: RequestRecord[];
  server: ServerStatus;
  intercepted_methods: string[];
  providers: ProviderInfo[];
  known_chains: { chain_id: number; name: string }[];
  config_path: string;
  db_path: string;
  lan_addresses: string[];
  startup_warning: string | null;
  safe_watch: WatchStatus[];
  safe_alerts: SafeAlert[];
}

export interface HistoryQuery {
  chain_id?: number;
  category?: Category;
  status?: Status;
  address?: string;
  text?: string;
  limit?: number;
  /** Only records older than this id (paging). */
  before_id?: number;
}

export type EngineEvent =
  | { type: "request"; payload: RequestRecord }
  | { type: "pending"; payload: RequestRecord[] }
  | { type: "unroutable"; payload: { chain_id: number; client: string; message: string } }
  | { type: "server"; payload: ServerStatus }
  | { type: "safe_watch"; payload: WatchStatus[] }
  | { type: "safe_alert"; payload: SafeAlert }
  | { type: "history_cleared" };

export const api = {
  snapshot: () => invoke<Snapshot>("get_snapshot"),
  decide: (id: number, approve: boolean) => invoke<RequestRecord>("decide", { id, approve }),
  simulate: (id: number) => invoke<SimulationOutcome>("simulate", { id }),
  history: (query: HistoryQuery) => invoke<RequestRecord[]>("query_history", { query }),
  /** Deletes the request history except held transactions; returns how many requests were deleted. */
  clearHistory: () => invoke<number>("clear_history"),
  setUiLanguage: (language: string) => invoke<void>("set_ui_language", { language }),
  saveConfig: (config: Config) => invoke<ServerStatus>("save_config", { config }),
  saveNetwork: (network: NetworkConfig, originalChainId: number | null) =>
    invoke<Config>("save_network", { network, originalChainId }),
  deleteNetwork: (chainId: number) => invoke<Config>("delete_network", { chainId }),
  restartListener: () => invoke<ServerStatus>("restart_listener"),
  safeReview: (chainId: number, safe: string, input: unknown) => invoke<SafeTxReport>("safe_review", { chainId, safe, input }),
  safeQueue: (chainId: number, safe: string) => invoke<SafeTxReport[]>("safe_queue", { chainId, safe }),
  safePoll: () => invoke<WatchStatus[]>("safe_poll"),
  safeAlertsRead: () => invoke<SafeAlert[]>("safe_alerts_read"),
};

export async function onEngineEvent(handler: (e: EngineEvent) => void): Promise<UnlistenFn> {
  if (browserPreview) {
    const { mockListen } = await import("./mock");
    return mockListen(handler);
  }
  return listen<EngineEvent>("txgate://event", (e) => handler(e.payload));
}

/** Tauri commands reject with a plain string; normalise anything else. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
