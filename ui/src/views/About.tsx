// What TxGate protects against, what it cannot see, and how to connect a wallet.

import type { Snapshot } from "../api";
import { walletUrl } from "../format";

export function About({ snapshot }: { snapshot: Snapshot }) {
  const port = snapshot.config.listen.port;
  return (
    <div className="page">
      <div className="prose">
        <h3 style={{ fontSize: 22 }}>How TxGate protects you</h3>
        <p>
          TxGate sits between your wallet and the RPC providers you choose. Reads are forwarded as they are. When the wallet estimates gas, which usually means it is
          about to ask you to sign, TxGate decodes the transaction and highlights it. When the wallet sends a signed transaction, TxGate holds it. Nothing reaches the
          network until you have read it here and approved it.
        </p>

        <h4>Held for your approval</h4>
        <p>
          These methods are held: {snapshot.intercepted_methods.map((m, i) => (
            <span key={m}>
              {i > 0 && ", "}
              <code className="mono">{m}</code>
            </span>
          ))}
          . Every other call is forwarded and shown under Requests.
        </p>

        <h4>What TxGate cannot see</h4>
        <div className="attention">
          <span className="note-accent">
            Signature requests never pass through the RPC. Messages signed with personal_sign or eth_signTypedData, including Permit, Permit2 and marketplace
            listings, are signed inside your wallet and handed straight to the website. A malicious site can use such a signature to move your tokens without any
            transaction from you. TxGate cannot warn you about these. Read every signature request in your wallet with the same care.
          </span>
        </div>
        <ul>
          <li>Approving or rejecting is all TxGate can do. A signed transaction cannot be changed.</li>
          <li>If the wallet sends directly to its own servers instead of the configured RPC, TxGate never sees the transaction.</li>
          <li>Simulation shows what would happen at the current block. The outcome can differ when the transaction is mined.</li>
        </ul>

        <h4>Connect a wallet</h4>
        <ol>
          <li>Add each network under Networks.</li>
          <li>
            In your wallet, add or edit the network and set its RPC URL to the address shown there, for example <code className="mono">{walletUrl("127.0.0.1", port, 1)}</code>.
          </li>
          <li>Use dApps as usual. Keep TxGate running; closing the window leaves it in the menu bar or system tray.</li>
        </ol>

        <h4>Where your data lives</h4>
        <ul>
          <li>
            Settings and API keys: <code className="mono">{snapshot.config_path}</code>
          </li>
          <li>
            Request log: <code className="mono">{snapshot.db_path}</code>
          </li>
        </ul>
      </div>
    </div>
  );
}
