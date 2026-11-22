//! Opt-in preview discovery and explicit release-channel selection.

use super::*;

/// GitHub's latest endpoint deliberately excludes prereleases. Keep preview
/// discovery separate so stable users never see opt-in builds.
#[derive(serde::Deserialize)]
struct PreviewRelease {
    #[serde(flatten)]
    release: GitHubRelease,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub(super) fn selfdev_prerelease_sha(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix("selfdev-")?;
    let (platform, sha) = rest.split_once('-')?;
    if platform.is_empty()
        || !platform
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    if sha.len() != 40 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    Some(sha)
}

fn parse_non_negative_no_leading_zeros(part: &str) -> Option<u64> {
    if part.is_empty()
        || !part.bytes().all(|b| b.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return None;
    }
    part.parse::<u64>().ok()
}

pub(super) fn semver_preview_version(tag: &str) -> Option<(u64, u64, u64, u64)> {
    let rest = tag.strip_prefix('v')?;
    let (core, number) = rest.split_once("-preview.")?;
    let mut parts = core.split('.');
    let major = parse_non_negative_no_leading_zeros(parts.next()?)?;
    let minor = parse_non_negative_no_leading_zeros(parts.next()?)?;
    let patch = parse_non_negative_no_leading_zeros(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    let preview = parse_non_negative_no_leading_zeros(number)?;
    if preview == 0 {
        return None;
    }
    Some((major, minor, patch, preview))
}

pub(super) fn is_prerelease_tag(tag: &str) -> bool {
    selfdev_prerelease_sha(tag).is_some() || semver_preview_version(tag).is_some()
}

fn parse_release_timestamp(value: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
}

fn parse_git_commit_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = value.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("unknown") {
        return None;
    }
    chrono::DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S %z")
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
}

fn commit_matches_current(release_sha: &str, current_hash: &str) -> bool {
    let current = current_hash.trim();
    current.len() >= 7
        && current.bytes().all(|b| b.is_ascii_hexdigit())
        && release_sha.len() >= current.len()
        && release_sha[..current.len()].eq_ignore_ascii_case(current)
}

fn preview_is_installable(candidate: &PreviewRelease) -> bool {
    let release = &candidate.release;
    !candidate.draft
        && candidate.prerelease
        && release
            ._published_at
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        && is_prerelease_tag(&release.tag_name)
        && release.assets.iter().any(|asset| {
            (asset.name == get_asset_name() || asset.name == format!("{}.tar.gz", get_asset_name()))
                && asset._size > 0
                && !asset.browser_download_url.is_empty()
        })
        && checksum_asset(release)
            .is_some_and(|asset| asset._size > 0 && !asset.browser_download_url.is_empty())
}

type PreviewSortKey = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<(u64, u64, u64, u64)>,
);

fn preview_sort_key(release: &GitHubRelease) -> PreviewSortKey {
    (
        parse_release_timestamp(release._published_at.as_deref()),
        semver_preview_version(&release.tag_name),
    )
}

fn select_latest_preview(
    latest: &mut Option<GitHubRelease>,
    releases: impl IntoIterator<Item = PreviewRelease>,
) {
    for candidate in releases.into_iter().filter(preview_is_installable) {
        if latest.as_ref().is_none_or(|previous| {
            preview_sort_key(&candidate.release) > preview_sort_key(previous)
        }) {
            *latest = Some(candidate.release);
        }
    }
}

pub fn fetch_latest_preview_blocking() -> Result<Option<GitHubRelease>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;
    let mut latest: Option<GitHubRelease> = None;
    // Paginate because stable releases may otherwise bury preview releases.
    // Bound work to avoid an unbounded automatic network check.
    for page in 1..=20 {
        let url =
            format!("https://api.github.com/repos/{GITHUB_REPO}/releases?per_page=100&page={page}");
        let response = github_api_request(&client, &url)
            .send()
            .context("Failed to fetch preview releases")?;
        if let Some(error) = rate_limit_error(&response) {
            return Err(error);
        }
        if !response.status().is_success() {
            anyhow::bail!("GitHub API error: {}", response.status());
        }
        let releases: Vec<PreviewRelease> = response
            .json()
            .context("Failed to parse preview releases")?;
        let last_page = releases.len() < 100;
        select_latest_preview(&mut latest, releases);
        if last_page {
            break;
        }
    }
    clear_rate_limit_backoff();
    Ok(latest)
}

fn preview_release_is_update_with<F>(
    release: &GitHubRelease,
    explicit_preview: bool,
    current_semver: &str,
    current_hash: &str,
    current_git_date: &str,
    release_build: bool,
    dev_guard: F,
) -> Result<bool>
where
    F: FnOnce(&str) -> Result<bool>,
{
    if let Some(release_sha) = selfdev_prerelease_sha(&release.tag_name) {
        if commit_matches_current(release_sha, current_hash) {
            return Ok(false);
        }
        if explicit_preview {
            return Ok(true);
        }
        if !release_build {
            return dev_guard(&release.tag_name);
        }
        if let (Some(published_at), Some(current_date)) = (
            parse_release_timestamp(release._published_at.as_deref()),
            parse_git_commit_timestamp(current_git_date),
        ) {
            return Ok(published_at > current_date);
        }
        return Ok(true);
    }
    release_is_update_with(&release.tag_name, current_semver, release_build, || {
        dev_guard(&release.tag_name)
    })
}

fn preview_release_is_update(release: &GitHubRelease, explicit_preview: bool) -> Result<bool> {
    preview_release_is_update_with(
        release,
        explicit_preview,
        current_update_semver(),
        jcode_build_meta::git_hash(),
        jcode_build_meta::git_date(),
        is_release_build(),
        update_dev_guard::should_install_release,
    )
}

pub(super) fn prepare_preview_update_blocking(explicit_preview: bool) -> Result<PreparedUpdate> {
    match fetch_latest_preview_blocking()? {
        Some(release) if preview_release_is_update(&release, explicit_preview)? => {
            prepare_release_update(release)
        }
        _ => Ok(PreparedUpdate::None {
            current: jcode_build_meta::version().to_string(),
        }),
    }
}

fn release_identity(version: &str) -> &str {
    version
        .trim_start_matches('v')
        .split('+')
        .next()
        .unwrap_or(version)
}

fn explicit_stable_is_update(release: &str, current: &str, release_build: bool) -> bool {
    !release_build || release_identity(release) != release_identity(current)
}

/// An explicit channel selection can intentionally switch onto prereleases or
/// return from a prerelease/development build to stable. Automatic checks never downgrade.
pub fn check_for_update_with_channel_blocking(
    explicit_channel: Option<crate::config::UpdateChannel>,
) -> Result<Option<GitHubRelease>> {
    let channel =
        explicit_channel.unwrap_or_else(|| crate::config::config().features.update_channel);
    match channel {
        crate::config::UpdateChannel::Main => check_for_main_update_blocking(),
        crate::config::UpdateChannel::Stable if explicit_channel.is_some() => {
            super::update_stable::find_stable_update_with(|release| {
                Ok(explicit_stable_is_update(
                    &release.tag_name,
                    current_update_semver(),
                    is_release_build(),
                ))
            })
        }
        crate::config::UpdateChannel::Stable => check_for_stable_update_blocking(),
        crate::config::UpdateChannel::Preview => match fetch_latest_preview_blocking()? {
            Some(release) if preview_release_is_update(&release, explicit_channel.is_some())? => {
                Ok(Some(release))
            }
            _ => Ok(None),
        },
    }
}

pub(super) fn prepare_stable_update_blocking(explicit_stable: bool) -> Result<PreparedUpdate> {
    let release = if explicit_stable {
        super::update_stable::find_stable_update_with(|release| {
            Ok(explicit_stable_is_update(
                &release.tag_name,
                current_update_semver(),
                is_release_build(),
            ))
        })?
    } else {
        super::update_stable::find_stable_update_blocking()?
    };
    match release {
        Some(release) => prepare_release_update(release),
        None => Ok(PreparedUpdate::None {
            current: jcode_build_meta::version().to_string(),
        }),
    }
}

fn check_for_stable_update_blocking() -> Result<Option<GitHubRelease>> {
    super::update_stable::find_stable_update_blocking()
}

#[cfg(test)]
#[path = "update_preview_tests.rs"]
mod tests;
