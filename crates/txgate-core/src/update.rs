//! "Check for updates": the latest release of TxGate on GitHub. Only runs when
//! the user asks; nothing is checked in the background.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::upstream::{Upstream, UpstreamError};

/// GitHub's latest published (non-draft, non-prerelease) release.
pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/BlockEase/TxGate/releases/latest";
/// Where users download TxGate.
pub const RELEASES_PAGE: &str = "https://github.com/BlockEase/TxGate/releases";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UpdateCheck {
    /// The latest release differs from the running version and can be downloaded from `url`.
    Available { current: String, latest: String, url: String },
    /// The running version is the latest release.
    UpToDate { current: String, latest: String },
    /// No release has been published yet.
    NoRelease { current: String },
}

/// Asks GitHub for the latest release and compares it with `current`.
pub async fn check_for_update(up: &dyn Upstream, current: &str) -> Result<UpdateCheck, String> {
    let release = match up.get(LATEST_RELEASE_URL, None).await {
        Ok(r) => r,
        Err(UpstreamError::Http { status: 404, .. }) => return Ok(UpdateCheck::NoRelease { current: current.into() }),
        Err(e) => return Err(e.to_string()),
    };
    let tag = release.get("tag_name").and_then(Value::as_str).ok_or("the release has no tag")?;
    // Only ever send the user to TxGate's own repository.
    let url = release
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://github.com/BlockEase/TxGate/"))
        .unwrap_or(RELEASES_PAGE);
    let latest = tag.trim_start_matches('v').to_string();
    // Any version other than the running one is offered; numbers are not ranked.
    Ok(if latest != current.trim() {
        UpdateCheck::Available { current: current.into(), latest, url: url.into() }
    } else {
        UpdateCheck::UpToDate { current: current.into(), latest }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::fake::FakeUpstream;
    use serde_json::json;

    #[tokio::test]
    async fn a_newer_release_is_offered_for_download() {
        let up = FakeUpstream::new();
        up.on_get(LATEST_RELEASE_URL, json!({"tag_name": "v0.2.0", "html_url": "https://github.com/BlockEase/TxGate/releases/tag/v0.2.0"}));
        let check = check_for_update(&up, "0.1.1").await.unwrap();
        assert_eq!(
            check,
            UpdateCheck::Available { current: "0.1.1".into(), latest: "0.2.0".into(), url: "https://github.com/BlockEase/TxGate/releases/tag/v0.2.0".into() }
        );
    }

    #[tokio::test]
    async fn only_the_same_version_is_up_to_date() {
        let up = FakeUpstream::new();
        up.on_get(LATEST_RELEASE_URL, json!({"tag_name": "v0.9.0", "html_url": "https://github.com/BlockEase/TxGate/releases/tag/v0.9.0"}));
        assert_eq!(check_for_update(&up, "0.9.0").await.unwrap(), UpdateCheck::UpToDate { current: "0.9.0".into(), latest: "0.9.0".into() });
        // Any other version is offered, whichever number is higher.
        assert!(matches!(check_for_update(&up, "0.10.0").await.unwrap(), UpdateCheck::Available { .. }));
    }

    #[tokio::test]
    async fn no_published_release_is_not_an_error() {
        // GitHub answers 404 until the first release is published.
        let up = FakeUpstream::new();
        assert_eq!(check_for_update(&up, "0.1.1").await.unwrap(), UpdateCheck::NoRelease { current: "0.1.1".into() });
    }

    #[tokio::test]
    async fn the_download_link_always_points_to_the_txgate_repository() {
        let up = FakeUpstream::new();
        up.on_get(LATEST_RELEASE_URL, json!({"tag_name": "v9.0.0", "html_url": "https://evil.example/TxGate.dmg"}));
        let UpdateCheck::Available { url, .. } = check_for_update(&up, "0.1.1").await.unwrap() else { panic!() };
        assert_eq!(url, RELEASES_PAGE);
    }
}
