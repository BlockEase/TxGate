// Sample data for running the UI in a plain browser (outside Tauri), used for
// layout work and screenshots. Never active inside the desktop app.

import type { AddressNote, Config, EngineEvent, RequestRecord, SafeAlert, SafeCheck, SafeExec, SafeTxReport, Snapshot, TxDetail, TxView, WatchStatus } from "./api";
import safeDump from "./mock-safe.json";

const ME = "0x3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE";
const USDC = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const ROUTER = "0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D";
const SPENDER = "0x9e1F000000000000000000000000000000003c07";
const now = Date.now();

const config: Config = {
  listen: { lan: false, port: 8545 },
  networks: [
    { chain_id: 1, name: "Ethereum Mainnet", read: { kind: "custom", url: "https://rpc.flashbots.net" }, broadcast: { kind: "custom", url: "https://rpc.flashbots.net" } },
    { chain_id: 8453, name: "Base", read: { kind: "provider", provider: "alchemy" }, broadcast: null },
  ],
  providers: { alchemy: "demo-key" },
  tenderly: { account: "me", project: "txgate", access_key: "x" },
  logging: { mode: "tx_only" },
  safe: {
    watched: [{ chain_id: 1, address: "0x849D52316331967b6fF1198e5E32A0eB168D039d", label: "Treasury" }],
    service_enabled: true,
    service_api_key: "",
    service_url: "",
    poll_secs: 60,
  },
  preview_window_secs: 600,
  // `#…lang=es` previews another language.
  language: window.location.hash.match(/lang=([\w-]+)/)?.[1] ?? null,
};

// Safe fixtures: real serde output of txgate-core (see safe_state.rs dump_for_ui).
const safeQueued = safeDump.queued as unknown as SafeTxReport;
const safeEvil = safeDump.evil as unknown as SafeTxReport;
const safeExec = safeDump.exec as unknown as SafeExec;
const safeCheck = safeDump.check as unknown as SafeCheck;
const SAFE = "0x849D52316331967b6fF1198e5E32A0eB168D039d";
const safeWatch: WatchStatus[] = [
  { chain_id: 1, address: SAFE, label: "Treasury", state: safeQueued.state, queue: [safeQueued, safeEvil], error: null, service_error: null, checked_ms: now - 12_000 },
];
const safeAlerts: SafeAlert[] = [
  { id: 3, ts_ms: now - 60_000, chain_id: 1, safe: SAFE, severity: "danger", kind: "proposed", message: `New transaction proposed by 0x964a…31E3 for nonce 2868: ${safeEvil.analysis.summary} · 2 danger flag(s)`, safe_tx_hash: safeEvil.hashes.safe_tx_hash, read: false },
  { id: 2, ts_ms: now - 3_600_000, chain_id: 1, safe: SAFE, severity: "warning", kind: "proposed", message: `New transaction proposed by 0x964a…31E3 for nonce 2867: ${safeQueued.analysis.summary}`, safe_tx_hash: safeQueued.hashes.safe_tx_hash, read: true },
  { id: 1, ts_ms: now - 86_400_000, chain_id: 1, safe: SAFE, severity: "info", kind: "executed", message: "Executed nonce 2866", safe_tx_hash: null, read: true },
];

function view(to: string, fn: string, sig: string, args: [string, string][], risks: TxView["analysis"]["risks"], extra?: Partial<TxView>): TxView {
  return {
    hash: null,
    tx_type: null,
    fields: { from: ME, to, value: "0x0", input: "0x095ea7b3" + "0".repeat(128), nonce: null, gas_limit: null, chain_id: null },
    max_fee_per_gas: null,
    max_priority_fee_per_gas: null,
    authorizations: [],
    analysis: { call: { selector: "0x095ea7b3", signature: sig, function: fn, args: args.map(([name, value]) => ({ name, value })) }, risks, summary: "" },
    enrichment: { risks: [], token: { address: to, symbol: "USDC", decimals: 6 }, formatted_args: [], account_nonce: 42, to_is_contract: true },
    simulator_url: "https://dashboard.tenderly.co/simulator/new?network=1",
    ...extra,
  };
}

const drainTx = view(
  USDC,
  "approve",
  "approve(address,uint256)",
  [
    ["spender", SPENDER],
    ["amount", (2n ** 256n - 1n).toString()],
  ],
  [{ severity: "danger", code: "unlimited_approval", message: `Unlimited allowance to ${SPENDER}: it can move all of this token at any time`, text: { key: "risk.unlimited_approval", params: { spender: SPENDER } } }],
  {
    hash: "0x6f1e3d0c9a3b2e1f00c0ffee5a1e2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c",
    tx_type: "EIP-1559",
    max_fee_per_gas: "18400000000",
    fields: { from: ME, to: USDC, value: "0x0", input: "0x095ea7b3" + "0".repeat(128), nonce: 42, gas_limit: 46109, chain_id: 1 },
    enrichment: {
      risks: [{ severity: "danger", code: "grantee_is_eoa", message: `Approval goes to ${SPENDER} — a plain account (EOA) with no contract code`, text: { key: "risk.grantee_is_eoa", params: { address: SPENDER } } }],
      token: { address: USDC, symbol: "USDC", decimals: 6 },
      formatted_args: [["amount", "Unlimited USDC"]],
      account_nonce: 42,
      to_is_contract: true,
    },
  },
);

const pendingDetail: TxDetail = {
  txs: [drainTx],
  preview: { kind: "mismatch", request_id: 3, at_ms: now - 40_000, diffs: [{ field: "to", previewed: ROUTER, signed: USDC }] },
  decode_error: null,
  dedupe_key: drainTx.hash,
  attempts: 1,
  wallet_connections: 1,
  simulation: {
    status: "ok",
    ts_ms: now - 5_000,
    result: {
      success: true,
      error: null,
      gas_used: 46109,
      block_number: 21643052,
      asset_changes: [],
      approvals: [{ token_address: USDC, owner: ME, spender: SPENDER, amount: (2n ** 256n - 1n).toString() }],
      simulation_id: "sim",
    },
  },
};

let nextId = 1;
function rec(partial: Partial<RequestRecord>): RequestRecord {
  const id = nextId++;
  return {
    id,
    ts_ms: now - (40 - id) * 3000,
    chain_id: 1,
    method: "eth_blockNumber",
    category: "read",
    client: "127.0.0.1",
    summary: "",
    status: "forwarded",
    duration_ms: 20 + ((id * 37) % 140),
    tx_hash: null,
    from: null,
    to: null,
    request: { jsonrpc: "2.0", id, method: partial.method ?? "eth_blockNumber", params: [] },
    response: { jsonrpc: "2.0", id, result: "0x14a3f2c" },
    detail: null,
    ...partial,
  };
}

const reads = ["eth_blockNumber", "eth_getBalance", "eth_call", "eth_chainId", "eth_getTransactionCount", "eth_gasPrice", "eth_feeHistory"];
const recent: RequestRecord[] = [];
for (let i = 0; i < 9; i++) recent.push(rec({ method: reads[i % reads.length], summary: i % 3 === 1 ? `${ME.slice(0, 10)}…f0bE, latest` : "", chain_id: i % 5 === 4 ? 8453 : 1 }));
// A dApp asking what an approval would do, without a sender.
const callView = view(USDC, "approve", "approve(address,uint256)", [["spender", SPENDER], ["amount", "1000000"]], [], { enrichment: null, fields: { from: null, to: USDC, value: "0x0", input: "0x095ea7b3" + "0".repeat(128), nonce: null, gas_limit: null, chain_id: null } });
for (const r of recent) {
  if (r.method === "eth_call") {
    r.request = { jsonrpc: "2.0", id: r.id, method: "eth_call", params: [{ to: USDC, data: callView.fields.input }, "latest"] };
    r.summary = "{…}, latest";
    r.to = USDC;
    r.detail = { ...pendingDetail, txs: [callView], preview: null, simulation: null };
  }
}
const swapView = view(ROUTER, "swapExactTokensForETH", "swapExactTokensForETH(uint256,uint256,address[],address,uint256)", [["amountIn", "1000000000"]], []);
recent.push(
  rec({
    method: "eth_estimateGas",
    category: "estimate",
    summary: "0x7a25…488D · swapExactTokensForETH(1000000000, …)",
    from: ME,
    to: ROUTER,
    request: { jsonrpc: "2.0", id: 10, method: "eth_estimateGas", params: [{ from: ME, to: ROUTER, data: "0x18cbafe5000000000000000000000000000000000000000000000000000000003b9aca00…" }] },
    response: { jsonrpc: "2.0", id: 10, result: "0x2c4f1" },
    detail: { ...pendingDetail, txs: [swapView], preview: null, simulation: null },
  }),
);
const held = rec({
  method: "eth_sendRawTransaction",
  category: "send",
  status: "pending",
  duration_ms: null,
  summary: "0xA0b8…eB48 · approve(0x9e1F…3c07, unlimited)",
  from: ME,
  to: USDC,
  tx_hash: drainTx.hash,
  response: null,
  detail: pendingDetail,
});
held.ts_ms = now - 23_000;
recent.push(held);
const safeView: TxView = {
  ...drainTx,
  hash: "0xcf5ce1b87453dabdeed8921c6ad5088d9cf3424b0cae9588821558d58f670d42",
  fields: { ...drainTx.fields, to: SAFE, from: "0xeD5Ddb58F3131274597EE59c98A073ce3C5d1cb9", nonce: 7 },
  analysis: {
    call: { selector: "0x6a761202", signature: "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)", function: "execTransaction", args: [] },
    risks: safeExec.analysis.risks,
    summary: safeExec.analysis.summary,
    safe: safeExec,
  },
  enrichment: { risks: safeCheck.risks, token: null, formatted_args: [], account_nonce: 7, to_is_contract: true, safe: safeCheck },
};
const heldSafe = rec({
  method: "eth_sendRawTransaction",
  category: "send",
  status: "pending",
  duration_ms: null,
  summary: safeExec.analysis.summary,
  from: safeView.fields.from,
  to: SAFE,
  tx_hash: safeView.hash,
  response: null,
  detail: { ...pendingDetail, txs: [safeView], preview: { kind: "matched", request_id: 9, at_ms: now - 30_000 }, simulation: null },
});
heldSafe.ts_ms = now - 8_000;
recent.push(heldSafe);
// Modelled on mainnet EIP-7702 transaction 0x5b7e…ff4b: a relayer pays the gas
// for account 0x4de2…b2f3, which delegates itself to 0x84D0…1F40.
const RELAYER = "0x333333320bb4a0345a7a102c49c72b37647aa088";
const DELEGATED = "0x4de23f3f0fb3318287378adbde030cf61714b2f3";
const delegationView: TxView = {
  ...view(DELEGATED, "", "", [], [], {}),
  hash: "0x5b7ea1021ceb559be5544593b9abf3cf1f44d71712a3411c19b601b5c40aff4b",
  tx_type: "EIP-7702 (set code)",
  fields: { from: RELAYER, to: DELEGATED, value: "0x0", input: "0xc4d66de8" + "0".repeat(64), nonce: 1180, gas_limit: 120000, chain_id: 1 },
  authorizations: [{ chain_id: "0x1", address: "0x84D05511614272694D3a9cebE896514DBDE51F40", nonce: 561020, authority: DELEGATED }],
  analysis: {
    call: { selector: "0xc4d66de8", signature: null, function: null, args: [] },
    risks: [
      { severity: "danger", code: "eip7702_delegation", message: "EIP-7702: delegates account 0x4DE23f3f0fB3318287378AdbdE030cf61714b2f3 to contract 0x84D05511614272694D3a9cebE896514DBDE51F40 on chain 1. That contract will fully control the account", text: { key: "risk.eip7702_delegation", params: { account: "0x4DE23f3f0fB3318287378AdbdE030cf61714b2f3", delegate: "0x84D05511614272694D3a9cebE896514DBDE51F40", chain: "1" } } },
      { severity: "info", code: "unknown_function", message: "Calls an unrecognised function 0xc4d66de8", text: { key: "risk.unknown_function", params: { selector: "0xc4d66de8" } } },
    ],
    summary: "0x4DE2…b2f3 · 0xc4d66de8",
  },
  enrichment: { risks: [], token: null, formatted_args: [], account_nonce: 1180, to_is_contract: true },
};
const heldDelegation = rec({
  method: "eth_sendRawTransaction",
  category: "send",
  status: "pending",
  duration_ms: null,
  summary: delegationView.analysis.summary,
  from: RELAYER,
  to: DELEGATED,
  tx_hash: delegationView.hash,
  response: null,
  detail: { ...pendingDetail, txs: [delegationView], preview: null, simulation: null },
});
// A bundle whose harmless first transaction is followed by a delegation.
const heldBundle = rec({
  method: "eth_sendBundle",
  category: "send",
  status: "pending",
  duration_ms: null,
  summary: `Bundle of 2 · ${swapView.analysis.summary}`,
  from: ME,
  to: ROUTER,
  response: null,
  detail: { ...pendingDetail, txs: [{ ...swapView, fields: { ...swapView.fields, from: ME, nonce: 42 } }, delegationView], preview: null, simulation: null },
});
recent.reverse();

const addressNotes: AddressNote[] = [{ chain_id: 1, address: ME, note: "My hot wallet" }];

export const mockSnapshot: Snapshot = {
  config,
  recent,
  // `#monitor-safetx` previews the dialog for a held Safe transaction.
  // `#monitor-bundle` previews the dialog for a held two-transaction bundle.
  // `#monitor-7702` previews the dialog for a held EIP-7702 transaction.
  pending: window.location.hash.includes("bundle") ? [heldBundle] : window.location.hash.includes("safetx") ? [heldSafe] : window.location.hash.includes("7702") ? [heldDelegation] : [held, heldSafe],
  server: { listening: true, address: "127.0.0.1:8545", error: null },
  intercepted_methods: ["eth_sendRawTransaction", "eth_sendRawTransactionSync", "eth_sendRawTransactionConditional", "eth_sendTransaction", "eth_sendBundle", "eth_sendPrivateTransaction"],
  providers: [
    { id: "alchemy", name: "Alchemy", chains: [1, 11155111, 8453, 42161, 10, 137] },
    { id: "infura", name: "Infura", chains: [1, 11155111, 8453, 42161, 10, 137] },
    { id: "ankr", name: "Ankr", chains: [1, 8453, 42161, 10, 137] },
    { id: "drpc", name: "dRPC", chains: [1, 8453, 42161, 10, 137] },
  ],
  version: "0.1.1",
  native_currencies: [[56, "BNB"], [137, "POL"], [43114, "AVAX"], [100, "xDAI"]],
  explorers: [[1, "https://etherscan.io"], [8453, "https://basescan.org"], [11155111, "https://sepolia.etherscan.io"]],
  address_notes: addressNotes,
  known_chains: [
    { chain_id: 1, name: "Ethereum Mainnet" },
    { chain_id: 8453, name: "Base" },
    { chain_id: 11155111, name: "Sepolia" },
    { chain_id: 42161, name: "Arbitrum One" },
    { chain_id: 10, name: "OP Mainnet" },
    { chain_id: 137, name: "Polygon PoS" },
  ],
  config_path: "~/Library/Application Support/app.txgate.desktop/config.toml",
  db_path: "~/Library/Application Support/app.txgate.desktop/txgate.sqlite",
  lan_addresses: ["192.168.1.20"],
  startup_warning: null,
  safe_watch: safeWatch,
  safe_alerts: safeAlerts,
};

/** Minimal command handler so every view renders in a browser. */
export async function mockInvoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "set_address_note": {
      const { chainId, address, note } = args as { chainId: number; address: string; note: string };
      const i = addressNotes.findIndex((n) => n.chain_id === chainId && n.address.toLowerCase() === address.toLowerCase());
      if (i >= 0) addressNotes.splice(i, 1);
      if (note.trim()) addressNotes.push({ chain_id: chainId, address, note: note.trim().slice(0, 100) });
      return [...addressNotes];
    }
    case "get_snapshot":
      return mockSnapshot;
    case "check_update":
      // `#about-uptodate` previews the "latest version" answer.
      return window.location.hash.includes("uptodate")
        ? { status: "up_to_date", current: "0.1.1", latest: "0.1.1" }
        : { status: "available", current: "0.1.1", latest: "0.2.0", url: "https://github.com/BlockEase/TxGate/releases/tag/v0.2.0" };
    case "set_ui_language":
      return null;
    case "clear_history": {
      const before = recent.length;
      const held = recent.filter((r) => r.status === "pending" || r.status === "disconnected");
      recent.splice(0, recent.length, ...held);
      return before - held.length;
    }
    case "query_history":
      return recent.filter((r) => r.category !== "read");
    case "save_config":
    case "restart_listener":
      return mockSnapshot.server;
    case "safe_review":
      return safeEvil;
    case "safe_queue":
      return [safeQueued, safeEvil];
    case "safe_poll":
      return safeWatch;
    case "safe_alerts_read":
      return safeAlerts.map((a) => ({ ...a, read: true }));
    default:
      throw `"${cmd}" is not available in the browser preview`;
  }
}

export function mockListen(_handler: (e: EngineEvent) => void): Promise<() => void> {
  return Promise.resolve(() => {});
}
