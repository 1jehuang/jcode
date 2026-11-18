//! Stable release selection that tolerates asynchronous asset publication.
//!
//! Release assets are uploaded per platform after the release is published, so
//! `releases/latest` can briefly lack this platform's binary or its SHA256SUMS
//! entry. Instead of reporting "no update", fall back to the newest published
//! stable release that is fully installable here. Never downgrade.

use super::*;

#[derive(serde::Deserialize)]
pub(super) struct ListedRelease {
    #[serde(flatten)]
    pub(super) release: GitHubRelease,
    #[serde(default)]
    pub(super) draft: bool,
    #[serde(default)]
    pub(super) prerelease: bool,
}

fn is_stable_tag(tag: &str) -> bool {
    let core = tag.strip_prefix('v').unwrap_or(tag);
    !core.contains(['-', '+']) && version_is_newer(tag, "0.0.0-0")
}

/// Pick the newest stable release that is an update, has this platform's
/// asset, and whose SHA256SUMS lists that asset. `checksum_lists` is consulted
/// lazily, newest first, so at most a few SHA256SUMS files are downloaded.
pub(super) fn select_installable_stable(
    releases: Vec<ListedRelease>,
    mut is_update: impl FnMut(&GitHubRelease) -> Result<bool>,
    mut checksum_lists: impl FnMut(&GitHubRelease, &GitHubAsset) -> bool,
) -> Result<Option<GitHubRelease>> {
    let mut candidates: Vec<GitHubRelease> = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter(|r| {
            r.release
                ._published_at
                .as_ref()
                .is_some_and(|s| !s.is_empty())
        })
        .filter(|r| is_stable_tag(&r.release.tag_name))
        .map(|r| r.release)
        .collect();
    candidates.sort_by(|a, b| {
        if version_is_newer(&a.tag_name, &b.tag_name) {
            std::cmp::Ordering::Less
        } else if version_is_newer(&b.tag_name, &a.tag_name) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    for release in candidates {
        if !is_update(&release)? {
            // Sorted newest first: nothing older can be an update either.
            return Ok(None);
        }
        let Ok(asset) = platform_asset(&release) else {
            continue;
        };
        if checksum_asset(&release).is_none() {
            continue;
        }
        if checksum_lists(&release, asset) {
            return Ok(Some(release));
        }
    }
    Ok(None)
}

fn sums_list_asset(
    client: &reqwest::blocking::Client,
    release: &GitHubRelease,
    asset: &GitHubAsset,
) -> bool {
    let Some(sums) = checksum_asset(release) else {
        return false;
    };
    let Ok(response) = client.get(&sums.browser_download_url).send() else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(text) = response.text() else {
        return false;
    };
    jcode_update_core::parse_sha256sums(&text)
        .map(|map| map.contains_key(&asset.name))
        .unwrap_or(false)
}

fn fetch_recent_releases_blocking(
    client: &reqwest::blocking::Client,
) -> Result<Vec<ListedRelease>> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases?per_page=30");
    let response = github_api_request(client, &url)
        .send()
        .context("Failed to fetch releases")?;
    if let Some(error) = rate_limit_error(&response) {
        return Err(error);
    }
    if !response.status().is_success() {
        anyhow::bail!("GitHub API error: {}", response.status());
    }
    response.json().context("Failed to parse releases")
}

/// Newest installable stable update, or `None` when already current.
pub(super) fn find_stable_update_blocking() -> Result<Option<GitHubRelease>> {
    find_stable_update_with(release_is_update)
}

/// Same fallback, gated by a caller-supplied update predicate.
pub(super) fn find_stable_update_with(
    mut is_update: impl FnMut(&GitHubRelease) -> Result<bool>,
) -> Result<Option<GitHubRelease>> {
    let latest = fetch_latest_release_blocking()?;
    if !is_update(&latest)? {
        return Ok(None);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;
    if release_url_override().is_some() {
        // Rehearsal endpoints serve a single release; keep the legacy check.
        return Ok(platform_asset(&latest).is_ok().then_some(latest));
    }
    if let Ok(asset) = platform_asset(&latest)
        && sums_list_asset(&client, &latest, asset)
    {
        return Ok(Some(latest));
    }
    crate::logging::info(&format!(
        "Latest release {} is not yet installable on this platform; checking older releases",
        latest.tag_name
    ));
    let releases = fetch_recent_releases_blocking(&client)?;
    clear_rate_limit_backoff();
    select_installable_stable(releases, is_update, |release, asset| {
        sums_list_asset(&client, release, asset)
    })
}

#[cfg(test)]
#[path = "update_stable_tests.rs"]
mod tests;
