//! Build and runtime version metadata for jcode.
//!
//! The build script (`build.rs`) computes git- and version-derived values and
//! emits them via `cargo:rustc-env`. Most binaries use those compile-time values.
//! A fast local release may wrap an already-built selfdev binary and set
//! `JCODE_RUNTIME_RELEASE_SEMVER`; the accessor functions below then present the
//! tagged release identity without recompiling the dependency graph solely to
//! change version strings.

use std::sync::OnceLock;

/// Compile-time human-readable version string, e.g. `v0.14.6-dev (abc1234)`.
pub const VERSION: &str = env!("JCODE_VERSION");
/// Short git hash of the build commit, e.g. `abc1234` (or `unknown`).
pub const GIT_HASH: &str = env!("JCODE_GIT_HASH");
/// Commit date/time of the build commit (or `unknown`).
pub const GIT_DATE: &str = env!("JCODE_GIT_DATE");
/// `git describe --tags --always` output (may be empty).
pub const GIT_TAG: &str = env!("JCODE_GIT_TAG");
/// Compile-time auto-incrementing build semver (dev) or explicit release semver.
pub const SEMVER: &str = env!("JCODE_SEMVER");
/// Compile-time base semver taken from the root `Cargo.toml` package version.
pub const BASE_SEMVER: &str = env!("JCODE_BASE_SEMVER");
/// Compile-time semver used for update comparisons.
pub const UPDATE_SEMVER: &str = env!("JCODE_UPDATE_SEMVER");
/// Encoded changelog (record/unit separated). See build.rs for the format.
pub const CHANGELOG: &str = env!("JCODE_CHANGELOG");
/// Compile-time root crate package version.
pub const PKG_VERSION: &str = env!("JCODE_PKG_VERSION");

static RUNTIME_RELEASE_SEMVER: OnceLock<Option<String>> = OnceLock::new();
static RUNTIME_VERSION: OnceLock<Option<String>> = OnceLock::new();
static RUNTIME_GIT_HASH: OnceLock<Option<String>> = OnceLock::new();
static RUNTIME_GIT_DATE: OnceLock<Option<String>> = OnceLock::new();
static RUNTIME_GIT_TAG: OnceLock<Option<String>> = OnceLock::new();

/// Parse a semver value supplied by the fast-release wrapper at process start.
///
/// Accepts the same `v1.2.3` and ` 1.2.3 ` shapes the previous version
/// did, but additionally preserves an optional channel suffix (everything
/// after the first `-`) so the fast-release wrapper can also override the
/// reported identity for a channel-bearing build. The fork's channel
/// scheme is `MAJOR.MINOR.PATCH-kFORK_SEMVER-CHANNEL` (e.g.
/// `0.91.0-k0.1.2-channel`); SemVer-style prerelease tags such as
/// `1.0.0-alpha.1` are also accepted and the `-alpha.1` portion is
/// carried through unchanged.
fn parse_release_semver(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('v');
    // Split on the first `-` only. Anything before is the version part;
    // anything after is the channel (which may itself contain `-`).
    let (version_part, channel_part) = match value.split_once('-') {
        Some((v, c)) => (v, Some(c)),
        None => (value, None),
    };
    let mut parts = version_part.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let channel_suffix = match channel_part.map(str::trim) {
        Some(c) if !c.is_empty() => format!("-{c}"),
        _ => String::new(),
    };
    Some(format!("{major}.{minor}.{patch}{channel_suffix}"))
}

/// Optional release semver supplied by the fast-release wrapper at process start.
pub fn runtime_release_semver() -> Option<&'static str> {
    RUNTIME_RELEASE_SEMVER
        .get_or_init(|| {
            std::env::var("JCODE_RUNTIME_RELEASE_SEMVER")
                .ok()
                .and_then(|value| parse_release_semver(&value))
        })
        .as_deref()
}

/// Human-readable runtime version, honoring a fast-release wrapper override.
pub fn version() -> &'static str {
    RUNTIME_VERSION
        .get_or_init(|| {
            runtime_release_semver().map(|semver| format!("v{semver} ({})", git_hash()))
        })
        .as_deref()
        .unwrap_or(VERSION)
}

fn runtime_identity_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Runtime git hash, honoring the fast-release wrapper identity.
pub fn git_hash() -> &'static str {
    RUNTIME_GIT_HASH
        .get_or_init(|| runtime_identity_value("JCODE_RUNTIME_RELEASE_GIT_HASH"))
        .as_deref()
        .unwrap_or(GIT_HASH)
}

/// Runtime git date, honoring the fast-release wrapper identity.
pub fn git_date() -> &'static str {
    RUNTIME_GIT_DATE
        .get_or_init(|| runtime_identity_value("JCODE_RUNTIME_RELEASE_GIT_DATE"))
        .as_deref()
        .unwrap_or(GIT_DATE)
}

/// Runtime git tag, honoring the fast-release wrapper identity.
pub fn git_tag() -> &'static str {
    RUNTIME_GIT_TAG
        .get_or_init(|| runtime_identity_value("JCODE_RUNTIME_RELEASE_GIT_TAG"))
        .as_deref()
        .unwrap_or(GIT_TAG)
}

/// Runtime build semver, honoring a fast-release wrapper override.
pub fn semver() -> &'static str {
    runtime_release_semver().unwrap_or(SEMVER)
}

/// Runtime base semver, honoring a fast-release wrapper override.
pub fn base_semver() -> &'static str {
    runtime_release_semver().unwrap_or(BASE_SEMVER)
}

/// Runtime update-comparison semver, honoring a fast-release wrapper override.
pub fn update_semver() -> &'static str {
    runtime_release_semver().unwrap_or(UPDATE_SEMVER)
}

/// Runtime package version, honoring a fast-release wrapper override.
pub fn pkg_version() -> &'static str {
    runtime_release_semver().unwrap_or(PKG_VERSION)
}

/// Whether this process should behave as a release build.
pub fn is_release_build() -> bool {
    option_env!("JCODE_RELEASE_BUILD").is_some() || runtime_release_semver().is_some()
}

#[cfg(test)]
mod tests {
    use super::parse_release_semver;

    #[test]
    fn runtime_release_semver_strict_three_numeric_components() {
        // Existing behavior — three numeric components, no channel.
        assert_eq!(parse_release_semver("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(parse_release_semver(" 1.2.3 ").as_deref(), Some("1.2.3"));
        assert_eq!(parse_release_semver("0.0.0").as_deref(), Some("0.0.0"));
    }

    #[test]
    fn runtime_release_semver_rejects_malformed() {
        // Too few / too many numeric parts.
        assert_eq!(parse_release_semver("1.2"), None);
        assert_eq!(parse_release_semver("1.2.3.4"), None);
        // Non-numeric patch with no `-` to trigger the channel path.
        assert_eq!(parse_release_semver("1.2.beta"), None);
        // Total junk.
        assert_eq!(parse_release_semver(""), None);
        assert_eq!(parse_release_semver("not.a.semver"), None);
    }

    #[test]
    fn runtime_release_semver_preserves_simple_channel() {
        // The fast-release wrapper can override the reported identity for
        // a channel-bearing build, e.g. a fork on the `dev` channel.
        assert_eq!(
            parse_release_semver("1.2.3-channel").as_deref(),
            Some("1.2.3-channel")
        );
        assert_eq!(
            parse_release_semver("v1.2.3-channel").as_deref(),
            Some("1.2.3-channel")
        );
    }

    #[test]
    fn runtime_release_semver_preserves_dashed_channel() {
        // The fork's full channel scheme is `MAJOR.MINOR.PATCH-kFORK-CHANNEL`;
        // both halves of the channel must survive the parse.
        assert_eq!(
            parse_release_semver("0.91.0-k0.1.2-channel").as_deref(),
            Some("0.91.0-k0.1.2-channel")
        );
        assert_eq!(
            parse_release_semver("0.91.0-k0.1.2").as_deref(),
            Some("0.91.0-k0.1.2")
        );
    }

    #[test]
    fn runtime_release_semver_treats_empty_channel_as_no_channel() {
        // `1.2.3-` has nothing after the dash, so it's not a real channel.
        assert_eq!(parse_release_semver("1.2.3-").as_deref(), Some("1.2.3"));
    }
}
