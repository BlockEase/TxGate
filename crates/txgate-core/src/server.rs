//! HTTP front-end: wallets talk JSON-RPC to `http://<host>:<port>/<chainId>`.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::config::ListenConfig;
use crate::engine::{Engine, ServerStatus};
use crate::jsonrpc::error_response;

/// Large enough for blob transactions submitted with their sidecars.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub struct ServerHandle {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl ServerHandle {
    /// Stops accepting connections and waits until the port is released, so it
    /// can be bound again right away. Connections already open (wallets waiting
    /// on a held transaction) are left alone and still get their answer.
    pub async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Binds with `SO_REUSEADDR` on Unix so a restart is not refused while
/// connections accepted by the previous listener are still open.
fn bind(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let socket = if addr.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}

fn router(engine: Engine) -> Router {
    Router::new()
        .route("/", get(index).post(missing_chain))
        .route("/{chain}", post(rpc))
        .route("/{chain}/", post(rpc))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(origin_policy))
        .with_state(engine)
}

async fn index() -> &'static str {
    "TxGate is running. Point your wallet at http://<this host>:<port>/<chainId>, e.g. /1 for Ethereum mainnet.\n"
}

pub(crate) const USAGE: &str = "TxGate: use http://<host>:<port>/<chainId>, e.g. /1 for Ethereum mainnet";

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(error_response(Value::Null, -32000, USAGE))).into_response()
}

/// Refuses ordinary websites and answers CORS for every other browser origin
/// (wallet extensions, pages on this machine). Requests without an Origin,
/// i.e. native wallets, pass untouched.
async fn origin_policy(req: Request, next: Next) -> Response {
    let Some(origin) = req.headers().get(header::ORIGIN).cloned() else {
        return next.run(req).await;
    };
    if is_remote_web_page(origin.to_str().unwrap_or("")) {
        let msg = "TxGate: requests from web pages are refused; only wallets may use this endpoint";
        return (StatusCode::FORBIDDEN, Json(error_response(Value::Null, -32000, msg))).into_response();
    }
    let mut resp = if req.method() == Method::OPTIONS {
        let mut pre = StatusCode::NO_CONTENT.into_response();
        let h = pre.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("POST, GET, OPTIONS"));
        let requested = req.headers().get(header::ACCESS_CONTROL_REQUEST_HEADERS).cloned();
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, requested.unwrap_or(HeaderValue::from_static("content-type")));
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        // Chrome's Private Network Access asks before letting a page reach localhost.
        if req.headers().contains_key("access-control-request-private-network") {
            h.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
        }
        pre
    } else {
        next.run(req).await
    };
    resp.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    resp.headers_mut().insert(header::VARY, HeaderValue::from_static("Origin"));
    resp
}

async fn missing_chain(State(engine): State<Engine>, ConnectInfo(addr): ConnectInfo<SocketAddr>, body: Bytes) -> Response {
    rpc(State(engine), ConnectInfo(addr), Path(String::new()), body).await
}

async fn rpc(State(engine): State<Engine>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Path(chain): Path<String>, body: Bytes) -> Response {
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Json(error_response(Value::Null, -32700, "Parse error")).into_response(),
    };
    Json(engine.handle(&chain, addr.ip().to_string(), value).await).into_response()
}

/// True for an `Origin` of an ordinary website. Browsers attach one to every
/// cross-site POST, so without this check any page the user visits could
/// spend their provider quota or flood the approval queue. Wallet extensions
/// (`chrome-extension://`, `moz-extension://`, ...) and native apps (no
/// Origin) pass, as do pages served from this machine. `null` is refused:
/// any website can send it from a sandboxed iframe or a `data:` URL.
fn is_remote_web_page(origin: &str) -> bool {
    if origin.trim() == "null" {
        return true;
    }
    let Some(rest) = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://")) else {
        return false;
    };
    let host = match rest.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => rest.split(':').next().unwrap_or(""),
    };
    !(host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "::1")
}

/// Binds the listener and serves until [`ServerHandle::stop`]. Reports the
/// outcome through [`Engine::set_server_status`].
pub async fn start(engine: Engine, listen: &ListenConfig) -> std::io::Result<ServerHandle> {
    let ip: std::net::IpAddr = listen.host().parse().expect("listen host is a literal IP");
    let addr = SocketAddr::new(ip, listen.port);
    let listener = match bind(addr) {
        Ok(l) => l,
        Err(e) => {
            engine.set_server_status(ServerStatus { listening: false, address: Some(addr.to_string()), error: Some(e.to_string()) });
            return Err(e);
        }
    };
    let addr = listener.local_addr()?;
    engine.set_server_status(ServerStatus { listening: true, address: Some(addr.to_string()), error: None });
    let app = router(engine.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            engine.set_server_status(ServerStatus { listening: false, address: Some(addr.to_string()), error: Some(e.to_string()) });
        }
    });
    Ok(ServerHandle { addr, task })
}

#[cfg(test)]
mod tests {
    use super::is_remote_web_page;

    #[test]
    fn only_remote_web_origins_are_refused() {
        assert!(is_remote_web_page("https://evil.example"));
        assert!(is_remote_web_page("http://192.168.1.4:3000"));
        assert!(!is_remote_web_page("chrome-extension://nkbihfbeogaeaoehlefnkodbefgpgknn"));
        assert!(!is_remote_web_page("moz-extension://1234"));
        assert!(is_remote_web_page("null"), "sandboxed iframes on any site send Origin: null");
        assert!(!is_remote_web_page("http://localhost:5173"));
        assert!(!is_remote_web_page("http://127.0.0.1:8080"));
        assert!(!is_remote_web_page("http://[::1]:3000"));
    }
}
