# TxGate

**English** | [简体中文](docs/README.zh-CN.md) | [Español](docs/README.es.md) | [Русский](docs/README.ru.md) | [Português](docs/README.pt.md)

Your wallet's RPC TxGate: inspect every request, approve every transaction before it's broadcast.

TxGate is a desktop app (macOS, Windows, Linux) that runs a local Ethereum JSON-RPC endpoint. Point your wallet at it instead of a public RPC:

- **Reads** (`eth_call`, `eth_getBalance`, …) are forwarded to the upstream you choose and shown live.
- **`eth_estimateGas`** is highlighted: it usually means the wallet is about to ask you to sign. TxGate decodes the call and flags risky patterns.
- **Sending methods** (`eth_sendRawTransaction`, `eth_sendRawTransactionSync`, `eth_sendRawTransactionConditional`, `eth_sendTransaction`, `eth_sendBundle`, `eth_sendPrivateTransaction`, `eth_sendPrivateRawTransaction`, `mev_sendBundle`, `personal_sendTransaction`, `eth_sendUserOperation`) are **held**. Nothing reaches the network until you approve the transaction in TxGate.

What the approval dialog shows:

- The decoded transaction: signer, target, function, arguments and value.
- Risk flags, for example:
  - unlimited approvals
  - `setApprovalForAll`
  - Permit/Permit2 calls
  - approvals to accounts without contract code
  - EIP-7702 delegations
  - chain-id mismatches
  - nonce gaps
- A comparison with the gas estimate the wallet made earlier. If the two differ, the dApp front-end may have swapped the transaction.
- Simulated balance and approval changes, when a Tenderly access key is configured.

The interface, risk messages, Safe alerts and desktop notifications are available in English, 简体中文, Español, Русский and Português. TxGate follows the system language until you pick one under **Settings → Language**.

### Safe multisig

A Safe moves funds with one on-chain `execTransaction`, after the owners have signed off-chain. TxGate covers both halves (details in [docs/SAFE.md](docs/SAFE.md)):

- **Held `execTransaction`**: TxGate unpacks the Safe transaction, including MultiSend batches, and runs the usual checks on every inner call. A `DELEGATECALL` to anything other than Safe's own MultiSend / SignMessageLib contracts is flagged as dangerous; this is how the Bybit Safe was taken over. TxGate also reads the Safe from your own upstream, computes the hash at its current nonce and verifies every signature against the owners and the threshold. It flags an unofficial implementation, enabled modules and self-calls that change owners, the threshold, the guard, modules or the fallback handler.
- **Held `approveHash`**: TxGate looks the hash up in the Safe Transaction Service, recomputes it from the returned fields and shows what you are approving.
- **Before you sign** (Safe multisig page): paste the EIP-712 typed data, or pick a queued transaction. TxGate decodes it and shows the domain hash, message hash and Safe transaction hash to compare with your hardware wallet's screen.
- **Watched Safes**: TxGate polls each Safe and alerts you, with a desktop notification, when owners, threshold, implementation, guard, fallback handler or modules change, and when someone proposes a new transaction. Proposals from the Transaction Service are re-hashed locally, so a tampered service is detected rather than trusted.

![Safe multisig page](docs/screenshots/safe.png)

## Screenshots

The approval dialog. This transaction grants an unlimited USDC allowance to an account with no contract code, and the signed transaction targets a different contract from the one the wallet previewed with `eth_estimateGas`:

![Approval dialog holding a transaction](docs/screenshots/approval.png)

The live request monitor, with a gas estimate (Preview) opened in the detail panel:

![Request monitor with detail panel](docs/screenshots/requests.png)

Networks: each chain gets its own local RPC URL and separate read and broadcast upstreams:

![Networks page](docs/screenshots/networks.png)

Settings: listen address, provider keys, Tenderly simulation and the request log:

![Settings page](docs/screenshots/settings.png)

## Limits

- Signature requests (`personal_sign`, `eth_signTypedData*`, including Permit / Permit2 and Safe owner signatures) never go through the RPC, so TxGate cannot see them. For Safe transactions, check them on the Safe multisig page before signing.
- Safe checks read contract state with `eth_call`. Flashbots Protect (the default mainnet upstream) does not serve `eth_call`, so give mainnet a separate read upstream to get them.
- TxGate can only approve or reject a transaction. A signed transaction cannot be changed.
- Listening on `0.0.0.0` currently has no access token. Anyone on your LAN can use the port and the provider keys behind it.
- Requests carrying the `Origin` of an ordinary website are refused, so a page you visit cannot use the endpoint behind your back. Wallet extensions, native wallets and pages on `localhost` are allowed (with CORS).

## Use it

1. Start TxGate. It listens on `http://127.0.0.1:8545`.
2. In **Networks**, add each chain you use. Mainnet comes preconfigured with Flashbots Protect (`https://rpc.flashbots.net`). You can give a network separate *read* and *broadcast* upstreams, and save Alchemy / Infura / Ankr / dRPC keys under **Settings** so you don't have to type URLs. Tenderly credentials, the listen address and logging are there too.
3. In your wallet, set the network's RPC URL to `http://127.0.0.1:8545/<chainId>`, for example `/1` for mainnet or `/8453` for Base (hex such as `/0x2105` also works).

When the wallet sends a transaction, TxGate notifies you and opens the approval dialog. If you reject, the wallet gets JSON-RPC error `4001`. If the wallet gives up waiting, the signed transaction stays in the queue, and you can still broadcast or discard it.

Settings, including API keys in plain text, are stored in `config.toml` in the app config directory. The request log is `txgate.sqlite` in the app data directory and is kept forever.

### macOS: "TxGate is damaged and can't be opened"

The macOS app is not signed or notarized by Apple yet, so macOS quarantines the download and reports it as damaged. Move TxGate to Applications, then clear the quarantine flag in Terminal:

```sh
xattr -cr /Applications/TxGate.app
```

This skips Apple's check for this app, so only run it on a copy downloaded from the [official releases page](https://github.com/BlockEase/TxGate/releases).

## Develop

Requirements: Rust (stable), Node.js 20+, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm run setup        # install the Tauri CLI and UI dependencies
npm run dev          # run the app with hot reload
npm run build        # build installers into target/release/bundle
cargo test --workspace
```

Send wallet-like traffic to a running instance:

```sh
npm run wallet:sim -- reads      # also: estimate | send | batch | all
```

Running `npm --prefix ui run dev` opens the UI in a browser with sample data, which is useful for layout work.

### Layout

| Path | What |
|---|---|
| `crates/txgate-core` | The proxy engine: config, routing, approval queue, decoding, risk checks, Tenderly, SQLite log, HTTP server. No Tauri dependency; fully tested. |
| `src-tauri` | Desktop shell: commands, event bridge, tray icon, notifications. |
| `ui` | React + TypeScript interface on the Classical design system from `UX/`. |
| `ui/src/i18n/locales` | Translations, one JSON file per language. The engine renders its English messages from `en.json` too; a test checks that every language has every text with the same `{placeholders}`. |
| `docs/SAFE.md` | How Safe multisig works and what TxGate checks. |

## License

GPL-3.0
