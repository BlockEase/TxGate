# TxGate

[English](../README.md) | [简体中文](README.zh-CN.md) | **Español** | [Русский](README.ru.md) | [Português](README.pt.md)

El guardián del RPC de tu billetera: inspecciona cada solicitud y aprueba cada transacción antes de que se transmita.

TxGate es una aplicación de escritorio (macOS, Windows, Linux) que ejecuta un endpoint JSON-RPC de Ethereum local. Apunta tu billetera a TxGate en lugar de a un RPC público:

- Las **lecturas** (`eth_call`, `eth_getBalance`, …) se reenvían al upstream que elijas y se muestran en vivo.
- **`eth_estimateGas`** se destaca: suele significar que la billetera está a punto de pedirte una firma. TxGate decodifica la llamada y señala los patrones de riesgo.
- Los **métodos de envío** (`eth_sendRawTransaction`, `eth_sendRawTransactionSync`, `eth_sendRawTransactionConditional`, `eth_sendTransaction`, `eth_sendBundle`, `eth_sendPrivateTransaction`, `eth_sendPrivateRawTransaction`, `mev_sendBundle`, `personal_sendTransaction`, `eth_sendUserOperation`) quedan **retenidos**. Nada llega a la red hasta que apruebes la transacción en TxGate.

Lo que muestra la ventana de aprobación:

- La transacción decodificada: firmante, destino, función, argumentos y valor.
- Alertas de riesgo, por ejemplo:
  - autorizaciones ilimitadas
  - `setApprovalForAll`
  - llamadas Permit/Permit2
  - autorizaciones a cuentas sin código de contrato
  - delegaciones EIP-7702
  - ID de cadena que no coinciden
  - saltos de nonce
- Una comparación con la estimación de gas que hizo antes la billetera. Si difieren, es posible que el front-end de la dApp haya cambiado la transacción.
- Los cambios simulados de saldos y autorizaciones, si hay una access key de Tenderly configurada.

La interfaz, los mensajes de riesgo, las alertas de Safe y las notificaciones de escritorio están disponibles en English, 简体中文, Español, Русский y Português. TxGate sigue el idioma del sistema hasta que elijas uno en **Ajustes → Idioma**.

### Safe multisig

Una Safe mueve fondos con una única `execTransaction` on-chain, después de que los propietarios firmen fuera de la cadena. TxGate cubre ambas partes (detalles en [SAFE.md](SAFE.md), en inglés):

- **`execTransaction` retenida**: TxGate desglosa la transacción de Safe, incluidos los lotes MultiSend, y aplica las comprobaciones habituales a cada llamada interna. Un `DELEGATECALL` a cualquier contrato que no sean los MultiSend / SignMessageLib oficiales de Safe se marca como peligroso: así se tomó el control de la Safe de Bybit. TxGate también lee la Safe desde tu propio upstream, calcula el hash con su nonce actual y verifica cada firma frente a los propietarios y el umbral. Señala una implementación no oficial, módulos activados y llamadas a la propia Safe que cambian los propietarios, el umbral, el guard, los módulos o el fallback handler.
- **`approveHash` retenido**: TxGate busca el hash en el Safe Transaction Service, lo recalcula a partir de los campos devueltos y muestra lo que estás aprobando.
- **Antes de firmar** (página Safe multisig): pega los datos tipados EIP-712 o elige una transacción en cola. TxGate la decodifica y muestra el hash del dominio, el hash del mensaje y el hash de la transacción de Safe para compararlos con la pantalla de tu billetera de hardware.
- **Safes vigiladas**: TxGate consulta cada Safe y te avisa con una notificación de escritorio cuando cambian los propietarios, el umbral, la implementación, el guard, el fallback handler o los módulos, y cuando alguien propone una nueva transacción. Las propuestas del Transaction Service se recalculan localmente, así que un servicio manipulado se detecta en lugar de aceptarse.

![Página Safe multisig](screenshots/safe.png)

## Capturas de pantalla

La ventana de aprobación. Esta transacción concede una autorización ilimitada de USDC a una cuenta sin código de contrato, y la transacción firmada apunta a un contrato distinto del que la billetera previsualizó con `eth_estimateGas`:

![Ventana de aprobación con una transacción retenida](screenshots/approval.png)

El monitor de solicitudes en vivo, con una estimación de gas (Vista previa) abierta en el panel de detalles:

![Monitor de solicitudes con panel de detalles](screenshots/requests.png)

Redes: cada cadena tiene su propia URL RPC local y upstreams separados para lectura y transmisión:

![Página Redes](screenshots/networks.png)

Ajustes: dirección de escucha, claves de proveedores, simulación con Tenderly y registro de solicitudes:

![Página Ajustes](screenshots/settings.png)

## Límites

- Las solicitudes de firma (`personal_sign`, `eth_signTypedData*`, incluidos Permit / Permit2 y las firmas de propietarios de Safe) nunca pasan por el RPC, así que TxGate no las ve. Para las transacciones de Safe, revísalas en la página Safe multisig antes de firmar.
- Las comprobaciones de Safe leen el estado de los contratos con `eth_call`. Flashbots Protect (el upstream predeterminado de mainnet) no admite `eth_call`, así que asigna a mainnet un upstream de lectura distinto para obtenerlas.
- TxGate solo puede aprobar o rechazar una transacción. Una transacción firmada no se puede modificar.
- Escuchar en `0.0.0.0` todavía no tiene token de acceso. Cualquiera en tu red local puede usar el puerto y las claves de proveedor que hay detrás.
- Las solicitudes con el `Origin` de un sitio web normal se rechazan, para que una página que visites no pueda usar el endpoint a tus espaldas. Se permiten las extensiones de billetera, las billeteras nativas y las páginas en `localhost` (con CORS).

## Uso

1. Inicia TxGate. Escucha en `http://127.0.0.1:8545`.
2. En **Redes**, añade cada cadena que uses. Mainnet viene configurada con Flashbots Protect (`https://rpc.flashbots.net`). Puedes dar a una red upstreams separados de *lectura* y *transmisión*, y guardar claves de Alchemy / Infura / Ankr / dRPC en **Ajustes** para no tener que escribir URLs. Allí también están las credenciales de Tenderly, la dirección de escucha y el registro.
3. En tu billetera, configura la URL RPC de la red como `http://127.0.0.1:8545/<chainId>`, por ejemplo `/1` para mainnet o `/8453` para Base (también funciona en hexadecimal, como `/0x2105`).

Cuando la billetera envía una transacción, TxGate te avisa y abre la ventana de aprobación. Si la rechazas, la billetera recibe el error JSON-RPC `4001`. Si la billetera deja de esperar, la transacción firmada sigue en la cola y aún puedes transmitirla o descartarla.

Los ajustes, incluidas las claves de API en texto plano, se guardan en `config.toml` en la carpeta de configuración de la aplicación. El registro de solicitudes es `txgate.sqlite` en la carpeta de datos de la aplicación y se conserva para siempre.

## Desarrollo

Requisitos: Rust (stable), Node.js 20+ y los [requisitos previos de Tauri](https://tauri.app/start/prerequisites/) para tu sistema operativo.

```sh
npm run setup        # instala la CLI de Tauri y las dependencias de la UI
npm run dev          # ejecuta la aplicación con recarga en caliente
npm run build        # genera los instaladores en target/release/bundle
cargo test --workspace
```

Enviar tráfico similar al de una billetera a una instancia en ejecución:

```sh
npm run wallet:sim -- reads      # también: estimate | send | batch | all
```

`npm --prefix ui run dev` abre la UI en el navegador con datos de ejemplo, útil para trabajar en el diseño.

### Estructura

| Ruta | Contenido |
|---|---|
| `crates/txgate-core` | El motor del proxy: configuración, enrutamiento, cola de aprobación, decodificación, comprobaciones de riesgo, Tenderly, registro SQLite, servidor HTTP. Sin dependencia de Tauri; totalmente probado. |
| `src-tauri` | Shell de escritorio: comandos, puente de eventos, icono de la bandeja, notificaciones. |
| `ui` | Interfaz React + TypeScript sobre el sistema de diseño Classical de `UX/`. |
| `ui/src/i18n/locales` | Traducciones, un archivo JSON por idioma. Los mensajes en inglés del motor también se generan a partir de `en.json`; una prueba comprueba que cada idioma tenga todos los textos con los mismos `{marcadores}`. |
| `docs/SAFE.md` | Cómo funciona Safe multisig y qué comprueba TxGate (en inglés). |

## Licencia

GPL-3.0
