use std::time::Duration;

use semver::Version;
use serde::Deserialize;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/Magnus-Gille/sagascript/releases/latest";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    Available { version: Version },
    UpToDate,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
}

/// Checks GitHub for a newer stable Sagascript release.
///
/// This is deliberately only called from an explicit tray-menu action. It
/// neither downloads nor installs anything, and it stores no release data.
pub async fn check_for_update(current_version: &str) -> Result<UpdateCheck, String> {
    let client = reqwest::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent(format!("sagascript/{current_version}"))
        .build()
        .map_err(|error| format!("failed to build update client: {error}"))?;
    let release = client
        .get(LATEST_RELEASE_URL)
        .send()
        .await
        .map_err(|error| format!("update request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("update request failed: {error}"))?
        .json::<LatestRelease>()
        .await
        .map_err(|error| format!("invalid update response: {error}"))?;

    if release.draft || release.prerelease {
        return Err("latest release was not a stable release".to_string());
    }
    compare_release_version(current_version, &release.tag_name)
}

fn compare_release_version(current_version: &str, release_tag: &str) -> Result<UpdateCheck, String> {
    let current = Version::parse(current_version)
        .map_err(|error| format!("invalid current version '{current_version}': {error}"))?;
    let release = parse_stable_release_tag(release_tag)?;
    Ok(if release > current {
        UpdateCheck::Available { version: release }
    } else {
        UpdateCheck::UpToDate
    })
}

fn parse_stable_release_tag(tag: &str) -> Result<Version, String> {
    let version = Version::parse(tag.trim().trim_start_matches('v'))
        .map_err(|error| format!("invalid release tag '{tag}': {error}"))?;
    if version.pre.is_empty() {
        Ok(version)
    } else {
        Err(format!("release tag '{tag}' is not stable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_stable_release_is_available() {
        assert_eq!(
            compare_release_version("1.0.1", "v1.0.2").unwrap(),
            UpdateCheck::Available {
                version: Version::new(1, 0, 2)
            }
        );
    }

    #[test]
    fn equal_or_older_release_is_up_to_date() {
        assert_eq!(
            compare_release_version("1.0.1", "v1.0.1").unwrap(),
            UpdateCheck::UpToDate
        );
        assert_eq!(
            compare_release_version("1.0.1", "v0.9.9").unwrap(),
            UpdateCheck::UpToDate
        );
    }

    #[test]
    fn prerelease_or_malformed_tag_is_rejected() {
        assert!(parse_stable_release_tag("v1.0.2-beta.1").is_err());
        assert!(parse_stable_release_tag("latest").is_err());
    }
}
