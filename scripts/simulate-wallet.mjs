#!/usr/bin/env node
// Sends wallet-like JSON-RPC traffic to a running TxGate, for manual testing.
//
//   node scripts/simulate-wallet.mjs [reads|estimate|send|batch|all] [url]
//
// `send` submits an old, already-mined mainnet transaction: approving it is
// harmless (the upstream answers "nonce too low"), rejecting returns 4001.

const scenario = process.argv[2] ?? "all";
const url = process.argv[3] ?? "http://127.0.0.1:8545/1";

const RAW_TX =
  "0x02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8";
const USDC = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const SENDER = "0x3f5CE5FBFe3E9af3971dD833D26bA9b5C936f0bE";
const SPENDER = "0x9e1F000000000000000000000000000000003c07";

let id = 1;
const call = (method, params = []) => ({ jsonrpc: "2.0", id: id++, method, params });

async function post(body, timeoutMs = 10 * 60_000) {
  const started = Date.now();
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs),
  });
  const json = await res.json();
  console.log(`${Array.isArray(body) ? "batch" : body.method} (${Date.now() - started} ms) →`, JSON.stringify(json));
  return json;
}

// approve(SPENDER, type(uint256).max) on USDC
const approveData = "0x095ea7b3" + SPENDER.slice(2).toLowerCase().padStart(64, "0") + "f".repeat(64);

const scenarios = {
  reads: async () => {
    await post(call("eth_chainId"));
    await post(call("eth_blockNumber"));
    await post(call("eth_getBalance", [SENDER, "latest"]));
  },
  estimate: async () => {
    await post(call("eth_estimateGas", [{ from: SENDER, to: USDC, data: approveData }]));
  },
  send: async () => {
    console.log("Sending a raw transaction; approve or reject it in TxGate…");
    await post(call("eth_sendRawTransaction", [RAW_TX]));
  },
  batch: async () => {
    console.log("Sending a batch with a read and a raw transaction…");
    await post([call("eth_blockNumber"), call("eth_sendRawTransaction", [RAW_TX])]);
  },
};

const run = scenario === "all" ? ["reads", "estimate", "send"] : [scenario];
for (const s of run) {
  if (!scenarios[s]) {
    console.error(`Unknown scenario ${s}. Use reads, estimate, send, batch or all.`);
    process.exit(1);
  }
  await scenarios[s]();
}
