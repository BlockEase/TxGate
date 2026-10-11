//! Upstream JSON-RPC transport.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum UpstreamError {
    #[error("upstream request failed: {0}")]
    Transport(String),
    #[error("upstream returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("upstream returned invalid JSON: {0}")]
    InvalidJson(String),
}

/// Sends one JSON-RPC request body to an upstream URL and returns its JSON response.
#[async_trait]
pub trait Upstream: Send + Sync {
    async fn call(&self, url: &str, body: &Value) -> Result<Value, UpstreamError>;
}

/// Production transport over HTTPS.
pub struct HttpUpstream {
    client: reqwest::Client,
}

impl HttpUpstream {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("TxGate/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("static reqwest configuration is valid");
        Self { client }
    }
}

impl Default for HttpUpstream {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Upstream for HttpUpstream {
    async fn call(&self, url: &str, body: &Value) -> Result<Value, UpstreamError> {
        let resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| UpstreamError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| UpstreamError::Transport(e.without_url().to_string()))?;
        match serde_json::from_str::<Value>(&text) {
            // Many providers return JSON-RPC errors with non-200 codes; pass those through.
            Ok(v) if v.is_object() || v.is_array() => Ok(v),
            _ if !status.is_success() => Err(UpstreamError::Http { status: status.as_u16(), body: truncate(&text, 300) }),
            _ => Err(UpstreamError::InvalidJson(truncate(&text, 300))),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Convenience: performs a single JSON-RPC method call and returns `result`.
pub async fn rpc_result(upstream: &dyn Upstream, url: &str, method: &str, params: Value) -> Result<Value, UpstreamError> {
    let resp = upstream.call(url, &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).await?;
    if let Some(err) = resp.get("error") {
        return Err(UpstreamError::Transport(format!("{method}: {err}")));
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

/// Queries `eth_chainId` and parses it.
pub async fn chain_id(upstream: &dyn Upstream, url: &str) -> Result<u64, UpstreamError> {
    let v = rpc_result(upstream, url, "eth_chainId", json!([])).await?;
    // The spec says hex quantity; some endpoints answer with a plain number.
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()))
        .ok_or_else(|| UpstreamError::InvalidJson(format!("eth_chainId returned {v}")))
}

#[cfg(any(test, feature = "test-util"))]
pub mod fake {
    //! In-memory upstream for tests: answers by method name and records calls.

    use std::collections::HashMap;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    type Handler = Arc<dyn Fn(&Value) -> Result<Value, UpstreamError> + Send + Sync>;

    #[derive(Clone, Default)]
    pub struct FakeUpstream {
        handlers: Arc<Mutex<HashMap<String, Handler>>>,
        calls: Arc<Mutex<Vec<(String, Value)>>>,
    }

    impl FakeUpstream {
        pub fn new() -> Self {
            Self::default()
        }

        /// Answers `method` with `result` for every call.
        pub fn on(&self, method: &str, result: Value) -> &Self {
            self.on_fn(method, move |req| Ok(json!({"jsonrpc": "2.0", "id": req["id"], "result": result})))
        }

        pub fn on_fn(&self, method: &str, f: impl Fn(&Value) -> Result<Value, UpstreamError> + Send + Sync + 'static) -> &Self {
            self.handlers.lock().insert(method.to_string(), Arc::new(f));
            self
        }

        /// All calls so far as (url, body).
        pub fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().clone()
        }

        pub fn calls_to(&self, method: &str) -> Vec<(String, Value)> {
            self.calls().into_iter().filter(|(_, b)| b["method"] == method).collect()
        }
    }

    #[async_trait]
    impl Upstream for FakeUpstream {
        async fn call(&self, url: &str, body: &Value) -> Result<Value, UpstreamError> {
            self.calls.lock().push((url.to_string(), body.clone()));
            let method = body["method"].as_str().unwrap_or_default().to_string();
            let handler = self.handlers.lock().get(&method).cloned();
            match handler {
                Some(h) => h(body),
                None => Ok(json!({"jsonrpc": "2.0", "id": body["id"], "error": {"code": -32601, "message": "method not found"}})),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeUpstream;
    use super::*;

    #[tokio::test]
    async fn chain_id_accepts_hex_and_plain_numbers() {
        let up = FakeUpstream::new();
        up.on("eth_chainId", json!("0x2105"));
        assert_eq!(chain_id(&up, "u").await.unwrap(), 8453);
        up.on("eth_chainId", json!(8453));
        assert_eq!(chain_id(&up, "u").await.unwrap(), 8453);
    }
}
