# Safe multisig support: research and plan

TxGate holds and decodes transactions sent through the wallet's RPC. A Safe
(formerly Gnosis Safe) changes the picture: owners approve a *SafeTx* by
signing EIP-712 typed data, which never goes through the RPC, and the only
on-chain transaction is the final `execTransaction`, sent by whichever owner
executes. This document records how Safe works where it matters for TxGate and
what TxGate does about each part.

## How a Safe transaction works

1. Someone proposes a SafeTx: `to, value, data, operation, safeTxGas, baseGas,
   gasPrice, gasToken, refundReceiver, nonce`. Safe{Wallet} stores proposals in
   the Safe Transaction Service (`api.safe.global/tx-service/<chain>/…`).
2. Each owner signs the SafeTx hash, either off-chain (`eth_signTypedData_v4`,
   or `eth_sign` on the hash) or on-chain with `approveHash(bytes32)`.
3. When `threshold` signatures exist, anyone calls
   `execTransaction(to, value, data, operation, safeTxGas, baseGas, gasPrice,
   gasToken, refundReceiver, signatures)`. The Safe recomputes the hash with its
   **current** nonce, checks the signatures against its owners, increments the
   nonce and performs the call.

### The hash

```
domainSeparator = keccak256(abi.encode(DOMAIN_TYPEHASH, chainId, safe))      // v1.3.0+
                = keccak256(abi.encode(DOMAIN_TYPEHASH_LEGACY, safe))         // v1.0.0–v1.2.0
messageHash     = keccak256(abi.encode(SAFE_TX_TYPEHASH, to, value, keccak256(data),
                                       operation, safeTxGas, baseGas, gasPrice,
                                       gasToken, refundReceiver, nonce))
safeTxHash      = keccak256(0x19 ‖ 0x01 ‖ domainSeparator ‖ messageHash)
```

`DOMAIN_TYPEHASH = keccak256("EIP712Domain(uint256 chainId,address verifyingContract)")`,
`DOMAIN_TYPEHASH_LEGACY = keccak256("EIP712Domain(address verifyingContract)")`,
`SAFE_TX_TYPEHASH = keccak256("SafeTx(address to,uint256 value,bytes data,uint8 operation,uint256 safeTxGas,uint256 baseGas,uint256 gasPrice,address gasToken,address refundReceiver,uint256 nonce)")`.

A hardware wallet that blind-signs EIP-712 shows the **domain hash** and the
**message hash**; some show the final safeTxHash. TxGate shows all three.

### Signatures

`signatures` is a concatenation of 65-byte `{r}{s}{v}` entries sorted by owner,
followed by the dynamic data of contract signatures:

| v | Meaning | Signer |
|---|---|---|
| 0 | EIP-1271 contract signature; `s` points at its data | `r` (a contract) |
| 1 | Approved hash: `approveHash` was called, or the executor is the owner | `r` |
| > 30 | `eth_sign` of the hash | `ecrecover(prefixed(hash), v − 4, r, s)` |
| 27, 28 | EIP-712 signature | `ecrecover(hash, v, r, s)` |

Recovering the signers against the hash TxGate computes itself proves which
owners signed *this exact* SafeTx at *this* nonce. If a signer does not
recover to an owner, the owners signed something else.

### What can go wrong

- **DELEGATECALL** (`operation = 1`) runs the target's code as the Safe itself:
  it can overwrite storage slot 0 (the implementation), owners, modules or
  guard. In the Bybit theft (Feb 2025) the signers' UI showed a normal
  transfer while the signed SafeTx was a delegatecall to a contract that
  replaced the Safe's implementation. The only legitimate delegatecall targets
  in daily use are Safe's own `MultiSend`, `MultiSendCallOnly` and
  `SignMessageLib` deployments.
- **Configuration changes** made by the Safe calling itself: `addOwnerWithThreshold`,
  `removeOwner`, `swapOwner`, `changeThreshold`, `enableModule` (a module can
  execute without any signatures), `setGuard`, `setModuleGuard`,
  `setFallbackHandler` (the handler answers `isValidSignature` for the Safe)
  and, before v1.3.0, `changeMasterCopy`.
- **Gas refunds**: a non-zero `gasPrice` pays `refundReceiver` (or the
  executor) out of the Safe, in `gasToken`.
- **A non-official implementation** (singleton) means the Safe is already
  compromised or is not a Safe at all.
- **A compromised front-end or Transaction Service** can show one transaction
  and ask for a signature over another. Hashes must be computed locally from
  the raw fields, and decoding must not rely on the service's `dataDecoded`.

## What TxGate does

### 1. Decode `execTransaction` (held transactions and estimates)

Pure decoding, no network (`safe.rs`):

- Recognise `execTransaction`, decode every argument including `data`.
- Unpack `multiSend(bytes)` batches into their individual calls.
- Run TxGate's ordinary call analysis on each inner call, with the Safe as the
  sender (unlimited approvals, `setApprovalForAll`, …).
- Flag delegatecalls to anything but official MultiSend / MultiSendCallOnly /
  SignMessageLib deployments as **danger**, Safe self-configuration calls,
  gas refunds and nested Safe transactions.

Chain-state checks (`safe_state.rs`, run with the other enrichment):

- Read the Safe's version, nonce, threshold, owners, implementation, guard,
  fallback handler and modules from the user's own read upstream.
- Compute domain hash, message hash and safeTxHash locally.
- Recover every signature, check it against the owners and the threshold
  (`approvedHashes` for v = 1 entries not made by the sender).
- Flag a non-official implementation, signers that are not owners, too few
  valid signatures, and enabled modules.

`approveHash(bytes32)` sent through TxGate is the on-chain form of a signature:
TxGate looks the hash up in the Safe Transaction Service, recomputes the hash
from the returned fields and decodes them itself. If the lookup fails or the
hash does not match, the approval is flagged.

### 2. Verify a SafeTx before signing it (Safe page)

Owners sign with `eth_signTypedData_v4`, which TxGate cannot see. The Safe
page provides an independent check instead:

- Paste the EIP-712 typed data (or a Transaction Service JSON), fill in the
  fields by hand, or pick a queued transaction of a watched Safe.
- TxGate reads the Safe's state from the user's own upstream, computes the
  three hashes, decodes the inner calls and lists the risks.
- The user compares the hashes with what the hardware wallet displays before
  confirming on the device.
- The typed data's `domain.chainId` and `verifyingContract` must match the
  selected network and Safe; a mismatch is flagged.

### 3. Watch Safes

The user adds Safes (chain + address) to a watch list. Every poll
(default 60 s) TxGate:

- reads the on-chain configuration and compares it with the last snapshot:
  owners, threshold, implementation, guard, fallback handler, modules and
  version. Any change raises an alert; nonce changes are recorded too.
- if the Transaction Service is enabled, fetches queued transactions
  (`nonce ≥ current nonce`), recomputes each hash locally, decodes it, checks
  every confirmation against the owners and raises an alert for each new
  proposal, showing who proposed it and its risks. A hash that does not match
  its fields is flagged as a service integrity problem.

Alerts are stored in SQLite and shown on the Safe page; warning and danger
alerts also raise a desktop notification.

## Limits

- TxGate still cannot see or block the signature itself. The Safe page only
  helps when the user checks the hashes before confirming on the device.
- Monitoring of queued transactions depends on the Safe Transaction Service.
  Without an API key it is rate limited; a key or a self-hosted service URL
  can be configured.
- EIP-1271 contract signatures (a Safe owned by another Safe) are shown but
  not verified.
- zkSync-era Safes use different deployment addresses; they are recognised,
  but the hash format is the same as other v1.3.0+ Safes.
