# TxGate

[English](../README.md) | [简体中文](README.zh-CN.md) | **繁體中文** | [Italiano](README.it.md) | [Français](README.fr.md) | [Português](README.pt.md)

錢包的 RPC 守門員：檢視每一個請求，每筆交易都要經過你批准才會廣播。

TxGate 是一個桌面應用程式（macOS、Windows、Linux），在本機執行一個 Ethereum JSON-RPC 端點。把錢包的 RPC 指向它，而不是公共 RPC：

- **讀取請求**（`eth_call`、`eth_getBalance` 等）會轉發到你選擇的上游節點，並即時顯示。
- **`eth_estimateGas`** 會被標示出來：這通常代表錢包即將請你簽名。TxGate 會解碼該呼叫並標記有風險的模式。
- **發送類方法**（`eth_sendRawTransaction`、`eth_sendRawTransactionSync`、`eth_sendRawTransactionConditional`、`eth_sendTransaction`、`eth_sendBundle`、`eth_sendPrivateTransaction`）會被**攔下**。在你於 TxGate 中批准之前，任何東西都不會送到網路上。

審批視窗會顯示：

- 解碼後的交易：簽名者、目標、函式、參數與金額。
- 風險標記，例如：
  - 無限額度授權
  - `setApprovalForAll`
  - Permit / Permit2 呼叫
  - 授權給沒有合約程式碼的帳戶
  - EIP-7702 委託
  - 鏈 ID 不符
  - nonce 斷層
- 與錢包先前 gas 預估的比對。兩者不同時，dApp 前端可能已經偷換了交易。
- 設定 Tenderly access key 後，會顯示模擬出的餘額與授權變化。

介面、風險訊息、Safe 警報與系統通知都支援 English、简体中文、繁體中文、Italiano、Français 和 Português。TxGate 預設跟隨系統語言，你也可以在 **設定 → 語言** 中自行選擇。

### Safe 多簽

Safe 在擁有者鏈下簽名之後，用一筆鏈上的 `execTransaction` 轉移資金。TxGate 兩個環節都會檢查（詳見 [SAFE.md](SAFE.md)，英文）：

- **攔下的 `execTransaction`**：TxGate 展開 Safe 交易（包括 MultiSend 批次），並對每個內部呼叫執行一般檢查。除了 Safe 官方的 MultiSend / SignMessageLib 合約之外，任何 `DELEGATECALL` 都會被標為危險；Bybit 的 Safe 就是這樣被奪走的。TxGate 也會用你自己的上游讀取 Safe，在其目前 nonce 下計算哈希，並根據擁有者與門檻驗證每一個簽名。非官方實作合約、已啟用的模組，以及變更擁有者、門檻、守衛、模組或 fallback handler 的自我呼叫都會被標記。
- **攔下的 `approveHash`**：TxGate 到 Safe Transaction Service 查詢該哈希，用回傳的欄位重新計算，並顯示你實際批准的內容。
- **簽名之前**（Safe 多簽頁面）：貼上 EIP-712 typed data，或選擇一筆待簽交易。TxGate 會解碼並顯示 domain 哈希、message 哈希與 Safe 交易哈希，讓你與硬體錢包螢幕比對。
- **監控的 Safe**：TxGate 定期讀取每個 Safe，當擁有者、門檻、實作合約、守衛、fallback handler 或模組變化，或有人提議新交易時，以系統通知提醒你。來自 Transaction Service 的提議會在本機重新計算哈希，因此服務被竄改時會被偵測出來，而不是被信任。

![Safe 多簽頁面](screenshots/safe.png)

## 截圖

審批視窗。這筆交易把無限額度的 USDC 授權給一個沒有合約程式碼的帳戶，而且簽名的交易目標與錢包先前用 `eth_estimateGas` 預覽的合約不同：

![攔下交易的審批視窗](screenshots/approval.png)

即時請求監控，詳情面板中開啟了一個 gas 預估（預覽）：

![帶詳情面板的請求監控](screenshots/requests.png)

網路：每條鏈都有自己的本機 RPC URL，以及分開的讀取與廣播上游：

![網路頁面](screenshots/networks.png)

設定：監聽位址、供應商金鑰、Tenderly 模擬與請求紀錄：

![設定頁面](screenshots/settings.png)

## 限制

- 簽名請求（`personal_sign`、`eth_signTypedData*`，包括 Permit / Permit2 與 Safe 擁有者簽名）從不經過 RPC，所以 TxGate 看不到。Safe 交易請在簽名前到 Safe 多簽頁面檢查。
- Safe 檢查需要用 `eth_call` 讀取合約狀態。Flashbots Protect（預設的主網上游）不支援 `eth_call`，因此請為主網另外設定讀取上游。
- TxGate 只能批准或拒絕交易。已簽名的交易無法修改。
- 在 `0.0.0.0` 上監聽目前沒有存取權杖。區域網路中的任何人都能使用這個連接埠以及其背後的供應商金鑰。
- 帶有一般網站 `Origin` 的請求會被拒絕，所以你瀏覽的網頁無法在背後偷用這個端點。錢包擴充功能、原生錢包與 `localhost` 上的頁面則允許（含 CORS）。

## 使用方式

1. 啟動 TxGate。它會監聽 `http://127.0.0.1:8545`。
2. 在 **網路** 頁面加入你使用的每條鏈。主網預設使用 Flashbots Protect（`https://rpc.flashbots.net`）。你可以為一個網路設定分開的*讀取*與*廣播*上游，也可以在 **設定** 中儲存 Alchemy / Infura / Ankr / dRPC 金鑰，不必手動輸入 URL。Tenderly 憑證、監聽位址與紀錄設定也在那裡。
3. 在錢包中把該網路的 RPC URL 設為 `http://127.0.0.1:8545/<chainId>`，例如主網用 `/1`，Base 用 `/8453`（也可用十六進位，如 `/0x2105`）。

錢包送出交易時，TxGate 會通知你並開啟審批視窗。拒絕時，錢包會收到 JSON-RPC 錯誤 `4001`。如果錢包不再等待，已簽名的交易仍會留在佇列中，你依然可以廣播或丟棄它。

設定（包括明文的 API 金鑰）存放在應用程式設定目錄的 `config.toml` 中。請求紀錄是應用程式資料目錄中的 `txgate.sqlite`，會永久保存。

## 開發

需求：Rust（stable）、Node.js 20+，以及你作業系統的 [Tauri 前置需求](https://tauri.app/start/prerequisites/)。

```sh
npm run setup        # 安裝 Tauri CLI 與 UI 依賴
npm run dev          # 以熱重載執行應用程式
npm run build        # 在 target/release/bundle 建置安裝檔
cargo test --workspace
```

向執行中的實例傳送模擬錢包的流量：

```sh
npm run wallet:sim -- reads      # 也可用：estimate | send | batch | all
```

執行 `npm --prefix ui run dev` 會在瀏覽器中以範例資料開啟 UI，方便調整版面。

### 目錄結構

| 路徑 | 內容 |
|---|---|
| `crates/txgate-core` | 代理引擎：設定、路由、審批佇列、解碼、風險檢查、Tenderly、SQLite 紀錄、HTTP 伺服器。不依賴 Tauri，完整測試。 |
| `src-tauri` | 桌面外殼：指令、事件橋接、系統匣圖示、通知。 |
| `ui` | React + TypeScript 介面，使用 `UX/` 中的 Classical 設計系統。 |
| `ui/src/i18n/locales` | 翻譯，每種語言一個 JSON 檔。引擎的英文訊息也由 `en.json` 產生；有測試確保每種語言都包含所有文字，且 `{佔位符}` 一致。 |
| `docs/SAFE.md` | Safe 多簽的運作方式，以及 TxGate 檢查哪些項目（英文）。 |

## 授權條款

GPL-3.0
