//! RPC provider URL templates.
//!
//! A user stores one API key per provider; TxGate then derives the endpoint
//! URL for each supported chain so the user never has to hand-write it.

use serde::{Deserialize, Serialize};

/// Well-known networks offered as presets when adding a network.
pub const KNOWN_CHAINS: &[(u64, &str)] = &[
    (1, "Ethereum Mainnet"),
    (11155111, "Sepolia"),
    (8453, "Base"),
    (84532, "Base Sepolia"),
    (42161, "Arbitrum One"),
    (10, "OP Mainnet"),
    (137, "Polygon PoS"),
    (56, "BNB Smart Chain"),
    (43114, "Avalanche C-Chain"),
    (59144, "Linea"),
    (534352, "Scroll"),
    (324, "zkSync Era"),
];

/// Block explorers, by chain id. An address page is `{base}/address/{address}`.
pub const EXPLORERS: &[(u64, &str)] = &[
    (1, "https://etherscan.io"),
    (11155111, "https://sepolia.etherscan.io"),
    (560048, "https://hoodi.etherscan.io"),
    (8453, "https://basescan.org"),
    (84532, "https://sepolia.basescan.org"),
    (42161, "https://arbiscan.io"),
    (421614, "https://sepolia.arbiscan.io"),
    (10, "https://optimistic.etherscan.io"),
    (11155420, "https://sepolia-optimism.etherscan.io"),
    (137, "https://polygonscan.com"),
    (80002, "https://amoy.polygonscan.com"),
    (56, "https://bscscan.com"),
    (97, "https://testnet.bscscan.com"),
    (43114, "https://snowtrace.io"),
    (43113, "https://testnet.snowtrace.io"),
    (59144, "https://lineascan.build"),
    (534352, "https://scrollscan.com"),
    (324, "https://explorer.zksync.io"),
    (100, "https://gnosisscan.io"),
    (5000, "https://mantlescan.xyz"),
    (42220, "https://celoscan.io"),
    (146, "https://sonicscan.org"),
    (25, "https://cronoscan.com"),
    (80094, "https://berascan.com"),
    (130, "https://uniscan.xyz"),
    (81457, "https://blastscan.io"),
];

/// Native currencies that are not ETH. Every other chain (Ethereum, its
/// testnets and the ETH-based L2s) pays in ETH.
pub const NATIVE_CURRENCIES: &[(u64, &str)] = &[
    (56, "BNB"),
    (97, "tBNB"),
    (137, "POL"),
    (80002, "POL"),
    (43114, "AVAX"),
    (43113, "AVAX"),
    (100, "xDAI"),
    (5000, "MNT"),
    (42220, "CELO"),
    (146, "S"),
    (250, "FTM"),
    (25, "CRO"),
    (1284, "GLMR"),
    (2222, "KAVA"),
    (80094, "BERA"),
];

/// Symbol of the chain's native currency.
pub fn native_symbol(chain_id: u64) -> &'static str {
    NATIVE_CURRENCIES.iter().find(|(id, _)| *id == chain_id).map_or("ETH", |(_, s)| s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Alchemy,
    Infura,
    Ankr,
    Drpc,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Alchemy, Provider::Infura, Provider::Ankr, Provider::Drpc];

    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Alchemy => "Alchemy",
            Provider::Infura => "Infura",
            Provider::Ankr => "Ankr",
            Provider::Drpc => "dRPC",
        }
    }

    /// Provider-specific network slug for a chain, if the provider serves it.
    fn slug(self, chain_id: u64) -> Option<&'static str> {
        let table: &[(u64, &str)] = match self {
            Provider::Alchemy => &[
                (1, "eth-mainnet"),
                (11155111, "eth-sepolia"),
                (8453, "base-mainnet"),
                (84532, "base-sepolia"),
                (42161, "arb-mainnet"),
                (10, "opt-mainnet"),
                (137, "polygon-mainnet"),
                (56, "bnb-mainnet"),
                (43114, "avax-mainnet"),
                (59144, "linea-mainnet"),
                (534352, "scroll-mainnet"),
                (324, "zksync-mainnet"),
            ],
            Provider::Infura => &[
                (1, "mainnet"),
                (11155111, "sepolia"),
                (8453, "base-mainnet"),
                (84532, "base-sepolia"),
                (42161, "arbitrum-mainnet"),
                (10, "optimism-mainnet"),
                (137, "polygon-mainnet"),
                (56, "bsc-mainnet"),
                (43114, "avalanche-mainnet"),
                (59144, "linea-mainnet"),
                (534352, "scroll-mainnet"),
                (324, "zksync-mainnet"),
            ],
            Provider::Ankr => &[
                (1, "eth"),
                (11155111, "eth_sepolia"),
                (8453, "base"),
                (84532, "base_sepolia"),
                (42161, "arbitrum"),
                (10, "optimism"),
                (137, "polygon"),
                (56, "bsc"),
                (43114, "avalanche"),
                (59144, "linea"),
                (534352, "scroll"),
                (324, "zksync_era"),
            ],
            Provider::Drpc => &[
                (1, "ethereum"),
                (11155111, "sepolia"),
                (8453, "base"),
                (84532, "base-sepolia"),
                (42161, "arbitrum"),
                (10, "optimism"),
                (137, "polygon"),
                (56, "bsc"),
                (43114, "avalanche"),
                (59144, "linea"),
                (534352, "scroll"),
                (324, "zksync"),
            ],
        };
        table.iter().find(|(id, _)| *id == chain_id).map(|(_, s)| *s)
    }

    /// Chain ids this provider can serve, used to suggest provider sources in the UI.
    pub fn supported_chains(self) -> Vec<u64> {
        KNOWN_CHAINS.iter().map(|(id, _)| *id).filter(|id| self.supports(*id)).collect()
    }

    pub fn supports(self, chain_id: u64) -> bool {
        self.slug(chain_id).is_some()
    }

    /// Builds the endpoint URL for `chain_id` using `key`.
    pub fn endpoint(self, chain_id: u64, key: &str) -> Option<String> {
        let slug = self.slug(chain_id)?;
        Some(match self {
            Provider::Alchemy => format!("https://{slug}.g.alchemy.com/v2/{key}"),
            Provider::Infura => format!("https://{slug}.infura.io/v3/{key}"),
            Provider::Ankr => format!("https://rpc.ankr.com/{slug}/{key}"),
            Provider::Drpc => format!("https://lb.drpc.org/ogrpc?network={slug}&dkey={key}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mainnet_endpoints() {
        assert_eq!(
            Provider::Alchemy.endpoint(1, "k").unwrap(),
            "https://eth-mainnet.g.alchemy.com/v2/k"
        );
        assert_eq!(Provider::Infura.endpoint(1, "k").unwrap(), "https://mainnet.infura.io/v3/k");
        assert_eq!(Provider::Ankr.endpoint(8453, "k").unwrap(), "https://rpc.ankr.com/base/k");
        assert_eq!(
            Provider::Drpc.endpoint(42161, "k").unwrap(),
            "https://lb.drpc.org/ogrpc?network=arbitrum&dkey=k"
        );
    }

    #[test]
    fn unknown_chain_has_no_endpoint() {
        assert!(Provider::Alchemy.endpoint(999_999, "k").is_none());
        assert!(!Provider::Infura.supports(999_999));
    }
}
