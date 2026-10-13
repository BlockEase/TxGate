//! TxGate core: a JSON-RPC proxy that forwards wallet traffic to user-configured
//! upstreams and holds every transaction-sending call until the user approves it.

pub mod account;
pub mod config;
pub mod decode;
pub mod jsonrpc;
pub mod preview;
pub mod providers;
pub mod server;
pub mod record;
pub mod store;
pub mod engine;
pub mod enrich;
pub mod safe;
pub mod safe_service;
pub mod safe_state;
pub mod safe_watch;
pub mod simulate;
pub mod text;
pub mod update;
pub mod upstream;
