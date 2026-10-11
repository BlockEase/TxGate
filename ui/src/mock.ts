// Sample data for running the UI in a plain browser (outside Tauri), used for
// layout work and screenshots. Never active inside the desktop app.

import type { Config, EngineEvent, RequestRecord, Snapshot, TxDetail, TxView } from "./api";

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
  preview_window_secs: 600,
};

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
  [{ severity: "danger", code: "unlimited_approval", message: "Unlimited allowance to 0x9e1F…3c07: it can move all of this token at any time" }],
  {
    hash: "0x6f1e3d0c9a3b2e1f00c0ffee5a1e2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c",
    tx_type: "EIP-1559",
    max_fee_per_gas: "18400000000",
    fields: { from: ME, to: USDC, value: "0x0", input: "0x095ea7b3" + "0".repeat(128), nonce: 42, gas_limit: 46109, chain_id: 1 },
    enrichment: {
      risks: [{ severity: "danger", code: "grantee_is_eoa", message: "Approval goes to 0x9e1F…3c07 — a plain account (EOA) with no contract code" }],
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
recent.reverse();

export const mockSnapshot: Snapshot = {
  config,
  recent,
  pending: [held],
  server: { listening: true, address: "127.0.0.1:8545", error: null },
  intercepted_methods: ["eth_sendRawTransaction", "eth_sendRawTransactionSync", "eth_sendRawTransactionConditional", "eth_sendTransaction", "eth_sendBundle", "eth_sendPrivateTransaction"],
  providers: [
    { id: "alchemy", name: "Alchemy", chains: [1, 11155111, 8453, 42161, 10, 137] },
    { id: "infura", name: "Infura", chains: [1, 11155111, 8453, 42161, 10, 137] },
    { id: "ankr", name: "Ankr", chains: [1, 8453, 42161, 10, 137] },
    { id: "drpc", name: "dRPC", chains: [1, 8453, 42161, 10, 137] },
  ],
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
};

/** Minimal command handler so every view renders in a browser. */
export async function mockInvoke(cmd: string): Promise<unknown> {
  switch (cmd) {
    case "get_snapshot":
      return mockSnapshot;
    case "query_history":
      return recent.filter((r) => r.category !== "read");
    case "save_config":
    case "restart_listener":
      return mockSnapshot.server;
    default:
      throw `"${cmd}" is not available in the browser preview`;
  }
}

export function mockListen(_handler: (e: EngineEvent) => void): Promise<() => void> {
  return Promise.resolve(() => {});
}
