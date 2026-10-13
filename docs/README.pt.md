# TxGate

[English](../README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [Italiano](README.it.md) | [Français](README.fr.md) | **Português**

O guardião do RPC da sua carteira: inspecione cada pedido e aprove cada transação antes de ser transmitida.

O TxGate é uma aplicação de desktop (macOS, Windows, Linux) que executa um endpoint JSON-RPC Ethereum local. Aponte a carteira para o TxGate em vez de um RPC público:

- As **leituras** (`eth_call`, `eth_getBalance`, …) são encaminhadas para o upstream que escolher e mostradas em tempo real.
- **`eth_estimateGas`** é destacado: normalmente significa que a carteira vai pedir-lhe para assinar. O TxGate descodifica a chamada e assinala padrões arriscados.
- Os **métodos de envio** (`eth_sendRawTransaction`, `eth_sendRawTransactionSync`, `eth_sendRawTransactionConditional`, `eth_sendTransaction`, `eth_sendBundle`, `eth_sendPrivateTransaction`, `eth_sendPrivateRawTransaction`, `mev_sendBundle`, `personal_sendTransaction`, `eth_sendUserOperation`) ficam **retidos**. Nada chega à rede até aprovar a transação no TxGate.

O que a janela de aprovação mostra:

- A transação descodificada: signatário, destino, função, argumentos e valor.
- Alertas de risco, por exemplo:
  - autorizações ilimitadas
  - `setApprovalForAll`
  - chamadas Permit/Permit2
  - autorizações para contas sem código de contrato
  - delegações EIP-7702
  - IDs de chain que não coincidem
  - saltos de nonce
- Uma comparação com a estimativa de gas feita antes pela carteira. Se forem diferentes, o front-end da dApp pode ter trocado a transação.
- As alterações simuladas de saldos e autorizações, quando existe uma access key do Tenderly configurada.

A interface, as mensagens de risco, os alertas Safe e as notificações de desktop estão disponíveis em English, 简体中文, 繁體中文, Italiano, Français e Português. O TxGate segue o idioma do sistema até escolher um em **Definições → Idioma**.

### Safe multisig

Um Safe move fundos com uma única `execTransaction` on-chain, depois de os proprietários assinarem fora da chain. O TxGate cobre as duas partes (detalhes em [SAFE.md](SAFE.md), em inglês):

- **`execTransaction` retida**: o TxGate desdobra a transação Safe, incluindo lotes MultiSend, e aplica as verificações habituais a cada chamada interna. Um `DELEGATECALL` para qualquer contrato que não seja o MultiSend / SignMessageLib oficial do Safe é assinalado como perigoso: foi assim que o Safe da Bybit foi tomado. O TxGate também lê o Safe a partir do seu próprio upstream, calcula o hash no nonce atual e verifica cada assinatura face aos proprietários e ao limite. Assinala uma implementação não oficial, módulos ativos e auto-chamadas que alteram proprietários, limite, guard, módulos ou fallback handler.
- **`approveHash` retido**: o TxGate procura o hash no Safe Transaction Service, recalcula-o a partir dos campos devolvidos e mostra o que está a aprovar.
- **Antes de assinar** (página Safe multisig): cole os typed data EIP-712 ou escolha uma transação em fila. O TxGate descodifica-a e mostra o hash do domain, o hash da mensagem e o hash da transação Safe para comparar com o ecrã da sua carteira de hardware.
- **Safes monitorizados**: o TxGate consulta cada Safe periodicamente e alerta-o, com uma notificação de desktop, quando os proprietários, o limite, a implementação, o guard, o fallback handler ou os módulos mudam, e quando alguém propõe uma nova transação. As propostas do Transaction Service são recalculadas localmente, por isso um serviço adulterado é detetado em vez de ser considerado fiável.

![Página Safe multisig](screenshots/safe.png)

## Capturas de ecrã

A janela de aprovação. Esta transação concede uma autorização ilimitada de USDC a uma conta sem código de contrato, e a transação assinada tem como destino um contrato diferente do que a carteira pré-visualizou com `eth_estimateGas`:

![Janela de aprovação com uma transação retida](screenshots/approval.png)

O monitor de pedidos em tempo real, com uma estimativa de gas (Pré-visualização) aberta no painel de detalhes:

![Monitor de pedidos com painel de detalhes](screenshots/requests.png)

Redes: cada chain tem o seu próprio URL RPC local e upstreams separados para leitura e transmissão:

![Página Redes](screenshots/networks.png)

Definições: endereço de escuta, chaves dos fornecedores, simulação Tenderly e registo de pedidos:

![Página Definições](screenshots/settings.png)

## Limites

- Os pedidos de assinatura (`personal_sign`, `eth_signTypedData*`, incluindo Permit / Permit2 e as assinaturas dos proprietários Safe) nunca passam pelo RPC, por isso o TxGate não os vê. Nas transações Safe, verifique-as na página Safe multisig antes de assinar.
- As verificações Safe leem o estado dos contratos com `eth_call`. O Flashbots Protect (o upstream predefinido da mainnet) não suporta `eth_call`, por isso atribua à mainnet um upstream de leitura separado para as obter.
- O TxGate só pode aprovar ou rejeitar uma transação. Uma transação assinada não pode ser alterada.
- A escuta em `0.0.0.0` ainda não tem token de acesso. Qualquer pessoa na sua rede local pode usar a porta e as chaves de fornecedor por trás dela.
- Os pedidos com o `Origin` de um site normal são recusados, para que uma página que visite não possa usar o endpoint sem o seu conhecimento. As extensões de carteira, as carteiras nativas e as páginas em `localhost` são permitidas (com CORS).

## Utilização

1. Inicie o TxGate. Fica à escuta em `http://127.0.0.1:8545`.
2. Em **Redes**, adicione cada chain que usa. A mainnet vem pré-configurada com o Flashbots Protect (`https://rpc.flashbots.net`). Pode dar a uma rede upstreams separados de *leitura* e *transmissão*, e guardar chaves Alchemy / Infura / Ankr / dRPC em **Definições** para não ter de escrever URLs. As credenciais do Tenderly, o endereço de escuta e o registo também estão aí.
3. Na carteira, defina o URL RPC da rede como `http://127.0.0.1:8545/<chainId>`, por exemplo `/1` para a mainnet ou `/8453` para a Base (o hexadecimal, como `/0x2105`, também funciona).

Quando a carteira envia uma transação, o TxGate avisa-o e abre a janela de aprovação. Se rejeitar, a carteira recebe o erro JSON-RPC `4001`. Se a carteira deixar de esperar, a transação assinada continua na fila e ainda pode transmiti-la ou descartá-la.

As definições, incluindo as chaves de API em texto simples, ficam guardadas em `config.toml` na pasta de configuração da aplicação. O registo de pedidos é `txgate.sqlite` na pasta de dados da aplicação e é guardado para sempre.

## Desenvolvimento

Requisitos: Rust (stable), Node.js 20+ e os [pré-requisitos do Tauri](https://tauri.app/start/prerequisites/) para o seu sistema operativo.

```sh
npm run setup        # instala a CLI do Tauri e as dependências da UI
npm run dev          # executa a aplicação com recarregamento automático
npm run build        # cria os instaladores em target/release/bundle
cargo test --workspace
```

Enviar tráfego semelhante ao de uma carteira para uma instância em execução:

```sh
npm run wallet:sim -- reads      # também: estimate | send | batch | all
```

`npm --prefix ui run dev` abre a UI no navegador com dados de exemplo, útil para trabalhar no layout.

### Estrutura

| Caminho | Conteúdo |
|---|---|
| `crates/txgate-core` | O motor do proxy: configuração, encaminhamento, fila de aprovação, descodificação, verificações de risco, Tenderly, registo SQLite, servidor HTTP. Sem dependência do Tauri; totalmente testado. |
| `src-tauri` | Shell de desktop: comandos, ponte de eventos, ícone da bandeja, notificações. |
| `ui` | Interface React + TypeScript baseada no design system Classical de `UX/`. |
| `ui/src/i18n/locales` | Traduções, um ficheiro JSON por idioma. As mensagens em inglês do motor também são geradas a partir de `en.json`; um teste verifica que cada idioma tem todos os textos com os mesmos `{marcadores}`. |
| `docs/SAFE.md` | Como funciona o Safe multisig e o que o TxGate verifica (em inglês). |

## Licença

GPL-3.0
