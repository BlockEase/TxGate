# TxGate

[English](../README.md) | **简体中文** | [繁體中文](README.zh-TW.md) | [Italiano](README.it.md) | [Français](README.fr.md) | [Português](README.pt.md)

钱包的 RPC 守门员：查看每一个请求，每笔交易都要经过你批准才会广播。

TxGate 是一个桌面应用（macOS、Windows、Linux），在本机运行一个 Ethereum JSON-RPC 端点。把钱包的 RPC 指向它，而不是公共 RPC：

- **读取请求**（`eth_call`、`eth_getBalance` 等）会转发到你选择的上游节点，并实时显示。
- **`eth_estimateGas`** 会被突出显示：这通常意味着钱包马上要请你签名。TxGate 会解码该调用并标记有风险的模式。
- **发送类方法**（`eth_sendRawTransaction`、`eth_sendRawTransactionSync`、`eth_sendRawTransactionConditional`、`eth_sendTransaction`、`eth_sendBundle`、`eth_sendPrivateTransaction`）会被**拦截**。在你于 TxGate 中批准之前，任何内容都不会发送到网络上。

审批窗口会显示：

- 解码后的交易：签名者、目标、函数、参数与金额。
- 风险标记，例如：
  - 无限额度授权
  - `setApprovalForAll`
  - Permit / Permit2 调用
  - 授权给没有合约代码的账户
  - EIP-7702 委托
  - 链 ID 不匹配
  - nonce 断档
- 与钱包之前 gas 预估的比对。两者不同时，dApp 前端可能已经偷换了交易。
- 配置 Tenderly access key 后，会显示模拟出的余额与授权变化。

界面、风险信息、Safe 警报与系统通知都支持 English、简体中文、繁體中文、Italiano、Français 和 Português。TxGate 默认跟随系统语言，你也可以在 **设置 → 语言** 中自行选择。

### Safe 多签

Safe 在所有者链下签名之后，用一笔链上的 `execTransaction` 转移资金。TxGate 两个环节都会检查（详见 [SAFE.md](SAFE.md)，英文）：

- **拦截的 `execTransaction`**：TxGate 展开 Safe 交易（包括 MultiSend 批次），并对每个内部调用执行常规检查。除了 Safe 官方的 MultiSend / SignMessageLib 合约之外，任何 `DELEGATECALL` 都会被标为危险；Bybit 的 Safe 就是这样被夺走的。TxGate 还会用你自己的上游读取 Safe，在其当前 nonce 下计算哈希，并根据所有者与阈值验证每一个签名。非官方实现合约、已启用的模块，以及变更所有者、阈值、守卫、模块或 fallback handler 的自调用都会被标记。
- **拦截的 `approveHash`**：TxGate 到 Safe Transaction Service 查询该哈希，用返回的字段重新计算，并显示你实际批准的内容。
- **签名之前**（Safe 多签页面）：粘贴 EIP-712 typed data，或选择一笔待签交易。TxGate 会解码并显示 domain 哈希、message 哈希与 Safe 交易哈希，供你与硬件钱包屏幕比对。
- **监控的 Safe**：TxGate 定期读取每个 Safe，当所有者、阈值、实现合约、守卫、fallback handler 或模块变化，或有人提议新交易时，以系统通知提醒你。来自 Transaction Service 的提议会在本地重新计算哈希，因此服务被篡改时会被检测出来，而不是被信任。

![Safe 多签页面](screenshots/safe.png)

## 截图

审批窗口。这笔交易把无限额度的 USDC 授权给一个没有合约代码的账户，而且签名的交易目标与钱包之前用 `eth_estimateGas` 预览的合约不同：

![拦截交易的审批窗口](screenshots/approval.png)

实时请求监控，详情面板中打开了一个 gas 预估（预览）：

![带详情面板的请求监控](screenshots/requests.png)

网络：每条链都有自己的本机 RPC URL，以及分开的读取与广播上游：

![网络页面](screenshots/networks.png)

设置：监听地址、服务商密钥、Tenderly 模拟与请求日志：

![设置页面](screenshots/settings.png)

## 限制

- 签名请求（`personal_sign`、`eth_signTypedData*`，包括 Permit / Permit2 与 Safe 所有者签名）从不经过 RPC，所以 TxGate 看不到。Safe 交易请在签名前到 Safe 多签页面检查。
- Safe 检查需要用 `eth_call` 读取合约状态。Flashbots Protect（默认的主网上游）不支持 `eth_call`，因此请为主网另外设置读取上游。
- TxGate 只能批准或拒绝交易。已签名的交易无法修改。
- 在 `0.0.0.0` 上监听目前没有访问令牌。局域网中的任何人都能使用这个端口以及其背后的服务商密钥。
- 带有普通网站 `Origin` 的请求会被拒绝，所以你访问的网页无法在背后偷用这个端点。钱包扩展、原生钱包与 `localhost` 上的页面则允许（含 CORS）。

## 使用方法

1. 启动 TxGate。它会监听 `http://127.0.0.1:8545`。
2. 在 **网络** 页面添加你使用的每条链。主网默认使用 Flashbots Protect（`https://rpc.flashbots.net`）。你可以为一个网络设置分开的*读取*与*广播*上游，也可以在 **设置** 中保存 Alchemy / Infura / Ankr / dRPC 密钥，不必手动输入 URL。Tenderly 凭证、监听地址与日志设置也在那里。
3. 在钱包中把该网络的 RPC URL 设为 `http://127.0.0.1:8545/<chainId>`，例如主网用 `/1`，Base 用 `/8453`（也可用十六进制，如 `/0x2105`）。

钱包发送交易时，TxGate 会通知你并打开审批窗口。拒绝时，钱包会收到 JSON-RPC 错误 `4001`。如果钱包不再等待，已签名的交易仍会留在队列中，你依然可以广播或丢弃它。

设置（包括明文的 API 密钥）保存在应用配置目录的 `config.toml` 中。请求日志是应用数据目录中的 `txgate.sqlite`，会永久保存。

## 开发

要求：Rust（stable）、Node.js 20+，以及你操作系统的 [Tauri 前置条件](https://tauri.app/start/prerequisites/)。

```sh
npm run setup        # 安装 Tauri CLI 与 UI 依赖
npm run dev          # 以热重载运行应用
npm run build        # 在 target/release/bundle 构建安装包
cargo test --workspace
```

向运行中的实例发送模拟钱包的流量：

```sh
npm run wallet:sim -- reads      # 也可用：estimate | send | batch | all
```

运行 `npm --prefix ui run dev` 会在浏览器中用示例数据打开 UI，方便调整布局。

### 目录结构

| 路径 | 内容 |
|---|---|
| `crates/txgate-core` | 代理引擎：配置、路由、审批队列、解码、风险检查、Tenderly、SQLite 日志、HTTP 服务器。不依赖 Tauri，完整测试。 |
| `src-tauri` | 桌面外壳：命令、事件桥接、托盘图标、通知。 |
| `ui` | React + TypeScript 界面，使用 `UX/` 中的 Classical 设计系统。 |
| `ui/src/i18n/locales` | 翻译，每种语言一个 JSON 文件。引擎的英文信息也由 `en.json` 生成；有测试确保每种语言都包含所有文本，且 `{占位符}` 一致。 |
| `docs/SAFE.md` | Safe 多签的工作方式，以及 TxGate 检查哪些项目（英文）。 |

## 许可证

GPL-3.0
