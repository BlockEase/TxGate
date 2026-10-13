// Refreshes the token labels shipped with the UI from the Uniswap Labs Default
// token list. The list is bundled rather than fetched at runtime, so TxGate
// never contacts a third party to label an address.
//
//   npm run tokens:update

import { writeFileSync } from "node:fs";

const SOURCE = "https://tokens.uniswap.org";
const OUT = new URL("../ui/src/data/token-list.json", import.meta.url);

const list = await (await fetch(SOURCE)).json();
const tokens = list.tokens
  // EVM addresses only; the list also carries non-EVM chains.
  .filter((t) => Number.isSafeInteger(t.chainId) && /^0x[0-9a-fA-F]{40}$/.test(t.address))
  .map((t) => [t.chainId, t.address, t.symbol, t.name])
  .sort((a, b) => a[0] - b[0] || a[1].toLowerCase().localeCompare(b[1].toLowerCase()));

const v = list.version;
writeFileSync(
  OUT,
  JSON.stringify({ name: list.name, version: `${v.major}.${v.minor}.${v.patch}`, timestamp: list.timestamp, source: SOURCE, tokens }, null, 0) + "\n",
);
console.log(`${tokens.length} tokens from ${list.name} ${v.major}.${v.minor}.${v.patch}`);
