# TxGate

[English](../README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | **Italiano** | [Français](README.fr.md) | [Português](README.pt.md)

Il guardiano dell'RPC del tuo wallet: controlla ogni richiesta e approva ogni transazione prima che venga trasmessa.

TxGate è un'app desktop (macOS, Windows, Linux) che esegue un endpoint JSON-RPC Ethereum locale. Collega il wallet a TxGate invece che a un RPC pubblico:

- Le **letture** (`eth_call`, `eth_getBalance`, …) vengono inoltrate all'upstream che scegli e mostrate in tempo reale.
- **`eth_estimateGas`** viene evidenziato: di solito significa che il wallet sta per chiederti di firmare. TxGate decodifica la chiamata e segnala gli schemi rischiosi.
- I **metodi di invio** (`eth_sendRawTransaction`, `eth_sendRawTransactionSync`, `eth_sendRawTransactionConditional`, `eth_sendTransaction`, `eth_sendBundle`, `eth_sendPrivateTransaction`) vengono **trattenuti**. Nulla raggiunge la rete finché non approvi la transazione in TxGate.

Cosa mostra la finestra di approvazione:

- La transazione decodificata: firmatario, destinazione, funzione, argomenti e valore.
- Segnalazioni di rischio, ad esempio:
  - autorizzazioni illimitate
  - `setApprovalForAll`
  - chiamate Permit/Permit2
  - autorizzazioni ad account senza codice di contratto
  - deleghe EIP-7702
  - chain ID non corrispondenti
  - salti di nonce
- Un confronto con la stima del gas fatta prima dal wallet. Se le due differiscono, il front-end della dApp potrebbe aver sostituito la transazione.
- Le variazioni simulate di saldi e autorizzazioni, se è configurata una access key di Tenderly.

L'interfaccia, i messaggi di rischio, gli avvisi Safe e le notifiche desktop sono disponibili in English, 简体中文, 繁體中文, Italiano, Français e Português. TxGate segue la lingua del sistema finché non ne scegli una in **Impostazioni → Lingua**.

### Safe multisig

Una Safe sposta fondi con un'unica `execTransaction` on-chain, dopo che i proprietari hanno firmato off-chain. TxGate copre entrambe le parti (dettagli in [SAFE.md](SAFE.md), in inglese):

- **`execTransaction` trattenuta**: TxGate scompone la transazione Safe, inclusi i batch MultiSend, ed esegue i soliti controlli su ogni chiamata interna. Una `DELEGATECALL` verso qualunque contratto diverso da MultiSend / SignMessageLib ufficiali di Safe viene segnalata come pericolosa: è così che è stata presa la Safe di Bybit. TxGate legge anche la Safe dal tuo upstream, calcola l'hash al nonce attuale e verifica ogni firma rispetto ai proprietari e alla soglia. Segnala un'implementazione non ufficiale, i moduli attivi e le auto-chiamate che cambiano proprietari, soglia, guard, moduli o fallback handler.
- **`approveHash` trattenuta**: TxGate cerca l'hash nel Safe Transaction Service, lo ricalcola dai campi restituiti e mostra cosa stai approvando.
- **Prima di firmare** (pagina Safe multisig): incolla i typed data EIP-712 oppure scegli una transazione in coda. TxGate la decodifica e mostra l'hash del domain, l'hash del messaggio e l'hash della transazione Safe da confrontare con lo schermo del tuo hardware wallet.
- **Safe monitorate**: TxGate interroga periodicamente ogni Safe e ti avvisa, con una notifica desktop, quando cambiano proprietari, soglia, implementazione, guard, fallback handler o moduli, e quando qualcuno propone una nuova transazione. Le proposte del Transaction Service vengono ricalcolate in locale, quindi un servizio manomesso viene rilevato invece che considerato affidabile.

![Pagina Safe multisig](screenshots/safe.png)

## Screenshot

La finestra di approvazione. Questa transazione concede un'autorizzazione USDC illimitata a un account senza codice di contratto, e la transazione firmata punta a un contratto diverso da quello che il wallet aveva simulato con `eth_estimateGas`:

![Finestra di approvazione con una transazione trattenuta](screenshots/approval.png)

Il monitor delle richieste in tempo reale, con una stima del gas (Anteprima) aperta nel pannello dei dettagli:

![Monitor delle richieste con pannello dei dettagli](screenshots/requests.png)

Reti: ogni chain ha il proprio URL RPC locale e upstream separati per lettura e trasmissione:

![Pagina Reti](screenshots/networks.png)

Impostazioni: indirizzo di ascolto, chiavi dei provider, simulazione Tenderly e registro delle richieste:

![Pagina Impostazioni](screenshots/settings.png)

## Limiti

- Le richieste di firma (`personal_sign`, `eth_signTypedData*`, inclusi Permit / Permit2 e le firme dei proprietari Safe) non passano mai dall'RPC, quindi TxGate non le vede. Per le transazioni Safe, verificale nella pagina Safe multisig prima di firmare.
- I controlli Safe leggono lo stato dei contratti con `eth_call`. Flashbots Protect (l'upstream predefinito per mainnet) non supporta `eth_call`, quindi assegna a mainnet un upstream di lettura separato per ottenerli.
- TxGate può solo approvare o rifiutare una transazione. Una transazione firmata non può essere modificata.
- L'ascolto su `0.0.0.0` al momento non ha un token di accesso. Chiunque sulla tua LAN può usare la porta e le chiavi dei provider dietro di essa.
- Le richieste con l'`Origin` di un normale sito web vengono rifiutate, così una pagina che visiti non può usare l'endpoint alle tue spalle. Le estensioni wallet, i wallet nativi e le pagine su `localhost` sono consentiti (con CORS).

## Utilizzo

1. Avvia TxGate. Resta in ascolto su `http://127.0.0.1:8545`.
2. In **Reti**, aggiungi ogni chain che usi. Mainnet è preconfigurata con Flashbots Protect (`https://rpc.flashbots.net`). Puoi assegnare a una rete upstream separati per *lettura* e *trasmissione*, e salvare chiavi Alchemy / Infura / Ankr / dRPC in **Impostazioni** per non dover digitare URL. Lì trovi anche le credenziali Tenderly, l'indirizzo di ascolto e il registro.
3. Nel wallet imposta l'URL RPC della rete su `http://127.0.0.1:8545/<chainId>`, ad esempio `/1` per mainnet o `/8453` per Base (funziona anche l'esadecimale, come `/0x2105`).

Quando il wallet invia una transazione, TxGate ti avvisa e apre la finestra di approvazione. Se rifiuti, il wallet riceve l'errore JSON-RPC `4001`. Se il wallet smette di attendere, la transazione firmata resta in coda e puoi comunque trasmetterla o scartarla.

Le impostazioni, incluse le chiavi API in chiaro, sono salvate in `config.toml` nella cartella di configurazione dell'app. Il registro delle richieste è `txgate.sqlite` nella cartella dei dati dell'app e viene conservato per sempre.

## Sviluppo

Requisiti: Rust (stable), Node.js 20+ e i [prerequisiti di Tauri](https://tauri.app/start/prerequisites/) per il tuo sistema operativo.

```sh
npm run setup        # installa la CLI di Tauri e le dipendenze della UI
npm run dev          # esegue l'app con hot reload
npm run build        # crea gli installer in target/release/bundle
cargo test --workspace
```

Invia traffico simile a quello di un wallet a un'istanza in esecuzione:

```sh
npm run wallet:sim -- reads      # anche: estimate | send | batch | all
```

`npm --prefix ui run dev` apre la UI nel browser con dati di esempio, utile per lavorare sul layout.

### Struttura

| Percorso | Contenuto |
|---|---|
| `crates/txgate-core` | Il motore del proxy: configurazione, instradamento, coda di approvazione, decodifica, controlli di rischio, Tenderly, registro SQLite, server HTTP. Nessuna dipendenza da Tauri; completamente testato. |
| `src-tauri` | Shell desktop: comandi, ponte degli eventi, icona nell'area di notifica, notifiche. |
| `ui` | Interfaccia React + TypeScript basata sul design system Classical di `UX/`. |
| `ui/src/i18n/locales` | Traduzioni, un file JSON per lingua. Anche i messaggi inglesi del motore vengono generati da `en.json`; un test verifica che ogni lingua contenga ogni testo con gli stessi `{segnaposto}`. |
| `docs/SAFE.md` | Come funziona Safe multisig e cosa verifica TxGate (in inglese). |

## Licenza

GPL-3.0
