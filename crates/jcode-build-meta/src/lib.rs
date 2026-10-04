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

fn parse_release_semver(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('v');
    let mut parts = value.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{major}.{minor}.{patch}"))
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

/// A build value that is actually present: trimmed, and blank is None.
///
/// `None` and `Some("")` are different answers and callers get this wrong. A
/// `Some("")` short-circuits the fallbacks behind it, so a variable that was
/// merely set-but-empty wins over a real value that was available.
pub fn present(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolve one build-metadata value: env var, then a metadata file, then a git
/// command.
///
/// A set-but-empty env var counts as *absent*. `set JCODE_BUILD_GIT_HASH=` in a
/// build script, and a stale variable left in a CI environment, both set the
/// variable to the empty string, and `std::env::var` reports that as present.
/// Treating it as present short-circuits the metadata file and the git fallback;
/// the original code filtered the emptiness only on the chain's *result*, by
/// which point the chain had already committed to the empty value, so the binary
/// was stamped `(unknown)`. That is worse than a stale hash: the publish gate in
/// `jcode_build_support::validate_binary_version_matches_source_report` ends up
/// with no hash at all to compare against.
///
/// Probed on this tree before the fix: unset -> `c023d8053`, set-to-empty ->
/// `unknown`, set-correct -> `c023d8053`.
///
/// `build.rs` carries the same rule inline for its own use and cannot call this:
/// cargo compiles a build script against `[build-dependencies]` only, so its own
/// crate's lib is not in scope there, and cargo does not run `#[cfg(test)]` tests
/// inside a build script at all. The two copies are why this one has tests.
pub fn resolve_build_value(
    env_name: &str,
    metadata_lookup: impl FnOnce() -> Option<String>,
    git_lookup: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match present(std::env::var(env_name).ok()) {
        Some(value) => Some(value),
        None => present(metadata_lookup()).or_else(|| present(git_lookup())),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_release_semver, present, resolve_build_value};
    use std::sync::Mutex;

    // `set_var` is process-global and unsafe under edition 2024, so every case
    // that touches the environment takes this lock and the calls are wrapped.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `present` is where "blank means absent" now lives, and it is reached
    /// from two crates. Test it directly rather than through a caller.
    ///
    /// The selfdev caller cannot cover it: outside a repository `git rev-parse`
    /// exits 128, so `short_git_hash` returns before trimming and the blank arm
    /// is unreachable there. Probed, and recorded as a mutation that went
    /// undetected because no test could have failed.
    #[test]
    fn present_treats_blank_as_absent_and_trims() {
        assert_eq!(present(None), None);
        assert_eq!(present(Some(String::new())), None);
        assert_eq!(present(Some("   ".to_string())), None);
        assert_eq!(present(Some("\t\n".to_string())), None);
        assert_eq!(
            present(Some("  f41aa86ab  ".to_string())),
            Some("f41aa86ab".to_string())
        );
    }

    fn with_env(name: &str, value: Option<&str>, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let previous = std::env::var(name).ok();
        // SAFETY: the lock above is the only thing in this process that mutates
        // the environment while these tests run.
        unsafe {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        f();
        // SAFETY: as above, and single-threaded under the lock.
        unsafe {
            match previous {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }

    const RESOLVE_ENV: &str = "JCODE_TEST_BUILD_RESOLUTION";

    #[test]
    fn env_var_wins_when_set() {
        with_env(RESOLVE_ENV, Some("from-env"), || {
            assert_eq!(
                resolve_build_value(
                    RESOLVE_ENV,
                    || Some("from-meta".into()),
                    || Some("from-git".into()),
                ),
                Some("from-env".to_string())
            );
        });
    }

    #[test]
    fn falls_back_to_metadata_when_env_unset() {
        with_env(RESOLVE_ENV, None, || {
            assert_eq!(
                resolve_build_value(
                    RESOLVE_ENV,
                    || Some("from-meta".into()),
                    || Some("from-git".into()),
                ),
                Some("from-meta".to_string())
            );
        });
    }

    #[test]
    fn falls_back_to_git_when_env_and_metadata_unset() {
        with_env(RESOLVE_ENV, None, || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || None, || Some("from-git".into())),
                Some("from-git".to_string())
            );
        });
    }

    /// The regression. An empty env var is set-but-absent, so it must not
    /// shadow the git fallback. Before the fix this returned `Some("")` and the
    /// built binary reported `(unknown)`.
    #[test]
    fn empty_env_var_does_not_shadow_the_git_fallback() {
        with_env(RESOLVE_ENV, Some(""), || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || None, || Some("from-git".into())),
                Some("from-git".to_string()),
                "a set-but-empty {} must fall through to git, not win the chain",
                RESOLVE_ENV
            );
        });
    }

    #[test]
    fn whitespace_only_env_var_does_not_shadow_the_fallback() {
        with_env(RESOLVE_ENV, Some("   \t"), || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || None, || Some("from-git".into())),
                Some("from-git".to_string())
            );
        });
    }

    #[test]
    fn env_var_is_trimmed() {
        with_env(RESOLVE_ENV, Some("  from-env  "), || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || None, || None),
                Some("from-env".to_string())
            );
        });
    }

    #[test]
    fn empty_metadata_does_not_shadow_git_either() {
        with_env(RESOLVE_ENV, None, || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || Some("".into()), || Some("from-git".into())),
                Some("from-git".to_string())
            );
        });
    }

    #[test]
    fn all_sources_empty_yields_none() {
        with_env(RESOLVE_ENV, Some(""), || {
            assert_eq!(
                resolve_build_value(RESOLVE_ENV, || Some("".into()), || Some("".into())),
                None
            );
        });
    }

    #[test]
    fn runtime_release_semver_accepts_only_three_numeric_components() {
        assert_eq!(parse_release_semver("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(parse_release_semver(" 1.2.3 ").as_deref(), Some("1.2.3"));
        assert_eq!(parse_release_semver("1.2"), None);
        assert_eq!(parse_release_semver("1.2.3.4"), None);
        assert_eq!(parse_release_semver("1.2.beta"), None);
    }
}
