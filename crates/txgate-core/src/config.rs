//! User configuration: listen address, networks, provider keys, Tenderly and logging.
//!
//! Persisted as plain TOML (API keys included, by user decision).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::providers::Provider;

pub const DEFAULT_PORT: u16 = 8545;
pub const FLASHBOTS_PROTECT_RPC: &str = "https://rpc.flashbots.net";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub listen: ListenConfig,
    pub networks: Vec<NetworkConfig>,
    pub providers: ProviderKeys,
    pub tenderly: TenderlyConfig,
    pub logging: LoggingConfig,
    pub safe: SafeConfig,
    /// How far back an `eth_estimateGas` preview is considered when matching a send.
    pub preview_window_secs: u64,
    /// Interface language (`en`, `zh-CN`, `es`, `ru`, `pt`); `None` follows the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ListenConfig::default(),
            networks: vec![NetworkConfig {
                chain_id: 1,
                name: "Ethereum Mainnet".into(),
                read: RpcSource::Custom { url: FLASHBOTS_PROTECT_RPC.into() },
                broadcast: Some(RpcSource::Custom { url: FLASHBOTS_PROTECT_RPC.into() }),
            }],
            providers: ProviderKeys::default(),
            tenderly: TenderlyConfig::default(),
            logging: LoggingConfig::default(),
            safe: SafeConfig::default(),
            preview_window_secs: 600,
            language: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenConfig {
    /// `false` binds 127.0.0.1, `true` binds 0.0.0.0 (whole LAN).
    pub lan: bool,
    pub port: u16,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self { lan: false, port: DEFAULT_PORT }
    }
}

impl ListenConfig {
    pub fn host(&self) -> &'static str {
        if self.lan { "0.0.0.0" } else { "127.0.0.1" }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub chain_id: u64,
    pub name: String,
    /// Upstream for every non-sending method.
    pub read: RpcSource,
    /// Upstream for sending methods; falls back to `read` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<RpcSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RpcSource {
    Custom { url: String },
    Provider { provider: Provider },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderKeys {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alchemy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infura: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ankr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drpc: Option<String>,
}

impl ProviderKeys {
    pub fn get(&self, provider: Provider) -> Option<&str> {
        let key = match provider {
            Provider::Alchemy => &self.alchemy,
            Provider::Infura => &self.infura,
            Provider::Ankr => &self.ankr,
            Provider::Drpc => &self.drpc,
        };
        key.as_deref().map(str::trim).filter(|k| !k.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TenderlyConfig {
    pub account: String,
    pub project: String,
    pub access_key: String,
}

impl TenderlyConfig {
    pub fn is_configured(&self) -> bool {
        !self.account.trim().is_empty()
            && !self.project.trim().is_empty()
            && !self.access_key.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogMode {
    /// Persist only sending methods and `eth_estimateGas`.
    #[default]
    TxOnly,
    /// Persist every request.
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub mode: LogMode,
}

/// A Safe multisig the user wants watched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedSafe {
    pub chain_id: u64,
    pub address: alloy_primitives::Address,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafeConfig {
    pub watched: Vec<WatchedSafe>,
    /// Use the Safe Transaction Service to look up queued transactions and
    /// what an `approveHash` approves. Everything it returns is re-hashed and
    /// decoded locally.
    pub service_enabled: bool,
    /// Optional Safe API key; without one the public service is rate limited.
    pub service_api_key: String,
    /// Base URL of a self-hosted Transaction Service; empty uses
    /// `https://api.safe.global/tx-service/<chain>`.
    pub service_url: String,
    /// Seconds between checks of the watched Safes.
    pub poll_secs: u64,
}

impl Default for SafeConfig {
    fn default() -> Self {
        Self { watched: Vec::new(), service_enabled: true, service_api_key: String::new(), service_url: String::new(), poll_secs: 60 }
    }
}

/// Concrete upstream URLs for one network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedNetwork {
    pub chain_id: u64,
    pub name: String,
    pub read_url: String,
    pub broadcast_url: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("network {0} is not configured")]
    UnknownChain(u64),
    #[error("no API key saved for {0}")]
    MissingKey(&'static str),
    #[error("{provider} does not serve chain {chain_id}")]
    UnsupportedChain { provider: &'static str, chain_id: u64 },
    #[error("upstream URL is empty")]
    EmptyUrl,
    #[error("the listen port must be between 1 and 65535")]
    InvalidPort,
    #[error("chain {0} is configured more than once")]
    DuplicateChain(u64),
    #[error("failed to read config: {0}")]
    Io(String),
    #[error("invalid config file: {0}")]
    Parse(String),
}

impl Config {
    pub fn network(&self, chain_id: u64) -> Option<&NetworkConfig> {
        self.networks.iter().find(|n| n.chain_id == chain_id)
    }

    pub fn resolve_source(&self, chain_id: u64, source: &RpcSource) -> Result<String, ConfigError> {
        match source {
            RpcSource::Custom { url } => {
                let url = url.trim();
                if url.is_empty() {
                    return Err(ConfigError::EmptyUrl);
                }
                Ok(url.to_string())
            }
            RpcSource::Provider { provider } => {
                let key = self
                    .providers
                    .get(*provider)
                    .ok_or(ConfigError::MissingKey(provider.display_name()))?;
                provider.endpoint(chain_id, key).ok_or(ConfigError::UnsupportedChain {
                    provider: provider.display_name(),
                    chain_id,
                })
            }
        }
    }

    /// Resolves the read and broadcast URLs for `chain_id`.
    pub fn resolve(&self, chain_id: u64) -> Result<ResolvedNetwork, ConfigError> {
        let net = self.network(chain_id).ok_or(ConfigError::UnknownChain(chain_id))?;
        let read_url = self.resolve_source(chain_id, &net.read)?;
        let broadcast_url = match &net.broadcast {
            Some(src) => self.resolve_source(chain_id, src)?,
            None => read_url.clone(),
        };
        Ok(ResolvedNetwork { chain_id, name: net.name.clone(), read_url, broadcast_url })
    }

    /// Structural checks that do not need the network.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.listen.port == 0 {
            return Err(ConfigError::InvalidPort);
        }
        let mut seen = std::collections::HashSet::new();
        for n in &self.networks {
            if !seen.insert(n.chain_id) {
                return Err(ConfigError::DuplicateChain(n.chain_id));
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(e.to_string())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ConfigError::Io(e.to_string()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| ConfigError::Parse(e.to_string()))?;
        // Write to a temp file first so a crash never leaves a truncated config.
        let tmp = path.with_extension("toml.tmp");
        let io = |e: std::io::Error| ConfigError::Io(e.to_string());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // The file holds API keys in plain text: readable by the user only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&tmp).map_err(io)?, text.as_bytes()).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(|e| ConfigError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_trimmed() {
        let mut c = Config::default();
        c.providers.alchemy = Some("  abc\n".into());
        c.networks[0].read = RpcSource::Provider { provider: Provider::Alchemy };
        assert_eq!(c.resolve(1).unwrap().read_url, "https://eth-mainnet.g.alchemy.com/v2/abc");
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn port_zero_is_rejected() {
        let mut c = Config::default();
        c.listen.port = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn default_mainnet_uses_flashbots_protect_for_read_and_broadcast() {
        let r = Config::default().resolve(1).unwrap();
        assert_eq!(r.read_url, FLASHBOTS_PROTECT_RPC);
        assert_eq!(r.broadcast_url, FLASHBOTS_PROTECT_RPC);
    }

    #[test]
    fn broadcast_falls_back_to_read() {
        let mut c = Config::default();
        c.networks.push(NetworkConfig {
            chain_id: 8453,
            name: "Base".into(),
            read: RpcSource::Custom { url: "https://base.example".into() },
            broadcast: None,
        });
        assert_eq!(c.resolve(8453).unwrap().broadcast_url, "https://base.example");
    }

    #[test]
    fn provider_source_needs_a_saved_key() {
        let mut c = Config::default();
        c.networks[0].read = RpcSource::Provider { provider: Provider::Alchemy };
        assert_eq!(c.resolve(1), Err(ConfigError::MissingKey("Alchemy")));
        c.providers.alchemy = Some("abc".into());
        assert_eq!(c.resolve(1).unwrap().read_url, "https://eth-mainnet.g.alchemy.com/v2/abc");
    }

    #[test]
    fn unknown_chain_is_an_error() {
        assert_eq!(Config::default().resolve(137), Err(ConfigError::UnknownChain(137)));
    }

    #[test]
    fn duplicate_chain_fails_validation() {
        let mut c = Config::default();
        c.networks.push(c.networks[0].clone());
        assert_eq!(c.validate(), Err(ConfigError::DuplicateChain(1)));
    }

    #[test]
    fn round_trips_through_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        c.providers.infura = Some("key".into());
        c.listen.lan = true;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(&dir.path().join("nope.toml")).unwrap(), Config::default());
    }
}
