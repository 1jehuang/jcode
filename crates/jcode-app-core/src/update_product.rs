//! Product identity for the auto-updater.
//!
//! jcode upstream and this fork are **separate products**: separate release
//! channels, separate binaries, forward-only data sync and no shared binaries.
//! That makes "which GitHub repo may this build update itself from" a product
//! identity question, not a configuration one — and it must never be answered
//! by falling back to upstream. Installing an upstream release over a fork build
//! silently replaces the fork with upstream code, which is precisely the
//! product-decollision break this module exists to prevent.
//!
//! Identity is read from the build's semver **channel suffix**, reusing the
//! fork-versioning scheme this repo already documents and implements in
//! `jcode-build-meta`: `BASEVERSION-<first-letter>semver[-channel]`. Upstream
//! builds carry a bare `MAJOR.MINOR.PATCH`; fork builds carry a letter-prefixed
//! fork semver, e.g. this product's `0.85.1-k1.1.1` (`k` = the KooshaPari/KCode
//! fork line, `p` = the Phenotype org fork). Deriving identity from the same
//! signal the rest of the build metadata uses means it needs no new build-time
//! knob and cannot drift from the version already shown by `--version`.

/// GitHub repo slug for **upstream** builds.
///
/// A fork build never updates from this slug; see [`update_repo`].
pub const GITHUB_REPO: &str = "1jehuang/jcode";

/// Resolve the GitHub repo slug this build is allowed to update itself from.
///
/// Returns `None` for a fork build. `None` is the fail-closed answer and every
/// caller must treat it as "auto-update is unavailable", never as "fall back to
/// upstream".
pub fn update_repo() -> Option<&'static str> {
    update_repo_for(current_semver())
}

/// Pure form of [`update_repo`], so the product-identity decision is directly
/// testable without depending on process-global build metadata.
pub fn update_repo_for(semver: &str) -> Option<&'static str> {
    if is_fork_build_for(semver) {
        log_disabled(semver);
        None
    } else {
        Some(GITHUB_REPO)
    }
}

/// Whether this build is a fork of upstream jcode rather than upstream itself.
#[allow(dead_code)] // Public API surface for fork decollision; not all builds call it.
pub fn is_fork_build() -> bool {
    is_fork_build_for(current_semver())
}

/// Whether the given build semver identifies a fork build.
pub fn is_fork_build_for(semver: &str) -> bool {
    semver_has_channel(semver)
}

/// Whether a build semver carries a known fork prefix (`k`, `p`, `fork-`).
///
/// Fork-versioning scheme per `~/AGENTS.md`:
///   `BASEVERSION-<first-letter>semver`
/// where `<first-letter>` is one of:
///   - `k`  (KooshaPari fork)
///   - `p`  (Phenotype org fork)
///   - `fork-` (generic fork marker)
///
/// Upstream prereleases such as `1.0.0-alpha.1` or `1.0.0-rc.1` carry
/// standard SemVer suffixes that are **not** fork prefixes and must NOT
/// be classified as fork builds.
pub fn semver_has_channel(semver: &str) -> bool {
    let trimmed = semver.trim().trim_start_matches('v');
    match trimmed.split_once('-') {
        Some((_, channel)) => is_known_fork_prefix(channel.trim()),
        None => false,
    }
}

/// Whether the channel portion of a semver starts with a known fork prefix.
///
/// Accepts: `k`, `p`, `fork-` (the official fork markers documented in
/// `~/AGENTS.md`). Rejects SemVer prerelease suffixes (`alpha.1`, `rc.1`,
/// `beta`, `nightly`, etc.) and bare dashes.
fn is_known_fork_prefix(channel: &str) -> bool {
    // Must start with a known fork prefix letter followed by a digit or dot
    // (e.g. `k1.1.1`, `p0.4.0`), OR be the `fork-` prefix (followed by
    // anything non-empty).
    if channel.is_empty() {
        return false;
    }
    // Check `fork-` prefix first (longest match).
    if let Some(rest) = channel.strip_prefix("fork-") {
        return !rest.is_empty();
    }
    // Single-letter fork prefixes: the letter must be followed by a digit
    // (start of a semver), not by a letter (which would be a prerelease
    // label like `alpha`, `beta`, `rc`).
    if channel.starts_with('k') || channel.starts_with('p') {
        let after_prefix = &channel[1..];
        // Must be followed by a digit (start of fork semver like `k1.1.1`)
        // or be the channel-only case (e.g. just `k` with nothing after).
        // Reject `alpha`, `beta.1`, etc.
        return after_prefix.is_empty()
            || after_prefix
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit());
    }
    false
}

/// Error surfaced when an update is explicitly requested on a build that must
/// not self-update, so `/update` reports a clear reason instead of silently
/// claiming success or an opaque network failure.
pub fn fork_update_disallowed_error() -> anyhow::Error {
    anyhow::anyhow!(
        "Auto-update is disabled for this build. It is a fork build ({}) and never updates \
         from upstream {}. Install updates for it from its own release channel.",
        current_semver(),
        GITHUB_REPO
    )
}

/// Resolve the repo slug for a network update call, or fail with
/// [`fork_update_disallowed_error`].
///
/// Keeping this here means the call sites in [`super::update`] do not each
/// repeat the resolve-or-bail boilerplate, which keeps that already oversized
/// module from growing.
pub fn resolve_update_repo() -> anyhow::Result<&'static str> {
    update_repo().ok_or_else(fork_update_disallowed_error)
}

/// True when `path` sits inside a git work tree.
///
/// The updater refuses to run from a developer checkout; that check happens
/// on the hot path in [`super::update`], so the walk lives here to keep the
/// oversized updater module from carrying it.
pub fn is_inside_git_repo(path: &std::path::Path) -> bool {
    let mut dir = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };

    while let Some(d) = dir {
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// The compile-time semver this binary was built with (immutable).
///
/// This is the canonical product identity. A runtime override must not
/// change a fork build into an upstream build or vice-versa.
fn compiled_semver() -> &'static str {
    jcode_build_meta::UPDATE_SEMVER
}

/// The semver this build uses for update comparisons, honoring the fast-release
/// wrapper's runtime override only when it preserves product identity.
///
/// A runtime override that would change a fork build into an upstream build
/// (or vice-versa) is rejected with a security warning. This closes the
/// attack path where `JCODE_RUNTIME_RELEASE_SEMVER` erases fork identity
/// and causes a fork build to pull upstream binaries.
///
/// See Issue 2 from `KooshaPari/KCode#104`.
fn current_semver() -> &'static str {
    let compiled = compiled_semver();
    let runtime = jcode_build_meta::runtime_release_semver();

    match runtime {
        Some(rt) => {
            // Guard: the runtime override must not change product identity.
            // If the compiled build is a fork, the override must also carry a
            // fork prefix. If the compiled build is upstream, the override must
            // not add a fork prefix.
            let compiled_is_fork = is_fork_build_for(compiled);
            let runtime_is_fork = is_fork_build_for(rt);

            if compiled_is_fork != runtime_is_fork {
                // SECURITY: runtime override would change product identity.
                // Reject it and use the compiled semver instead.
                crate::logging::warn(&format!(
                    "SECURITY: JCODE_RUNTIME_RELEASE_SEMVER={rt} would change \
                     product identity from {} to {}; override rejected. \
                     Compiled semver {compiled} is authoritative.",
                    if compiled_is_fork { "fork" } else { "upstream" },
                    if runtime_is_fork { "fork" } else { "upstream" },
                ));
                return compiled;
            }

            // Identity preserved; honor the runtime override.
            // Cast to 'static: runtime_release_semver() returns &'static str.
            // The match arm above already returns compiled (also 'static),
            // so the function's return type is satisfied.
            // SAFETY: runtime_release_semver() holds the value in a OnceLock,
            // so the str reference is valid for 'static.
            rt
        }
        None => compiled,
    }
}

fn log_disabled(semver: &str) {
    crate::logging::info(&format!(
        "Auto-update disabled: fork build ({}) does not update from upstream {}",
        semver, GITHUB_REPO
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fork build must not resolve to the upstream slug under any fork identity
    /// this repo's versioning scheme can produce. Without this the updater would
    /// fetch an upstream release and replace the fork binary with upstream code.
    #[test]
    fn fork_builds_never_resolve_to_upstream_repo() {
        for semver in [
            // This product's own build semver.
            "0.85.1-k1.1.1",
            // Other fork lines in the documented scheme.
            "1.5.3-p0.4.0",
            // Fork semver carrying a channel suffix as well.
            "0.91.0-k0.1.2-night",
            // `v` prefix, as `jcode-build-meta` preserves it.
            "v1.5.3-k0.1.2",
        ] {
            assert_eq!(
                update_repo_for(semver),
                None,
                "fork build {semver} must not resolve to an update source"
            );
            assert!(
                is_fork_build_for(semver),
                "{semver} must be recognized as a fork build"
            );
        }
    }

    /// The inverse guard: an upstream build must keep updating from upstream,
    /// unchanged. A regression here would silently disable updates for everyone
    /// on the upstream product.
    #[test]
    fn upstream_builds_still_resolve_to_upstream_repo() {
        for semver in ["0.85.1", "1.5.3", "0.0.0", "v0.91.42"] {
            assert_eq!(
                update_repo_for(semver),
                Some(GITHUB_REPO),
                "upstream build {semver} must keep updating from {GITHUB_REPO}"
            );
            assert!(
                !is_fork_build_for(semver),
                "{semver} must not be treated as a fork build"
            );
        }
    }

    /// An empty channel is not a channel, matching `jcode-build-meta`. This keeps
    /// a malformed semver from being read as a fork marker, and from disabling
    /// auto-update for an upstream build over a stray `-`.
    #[test]
    fn bare_trailing_dash_is_not_a_fork_channel() {
        assert!(!semver_has_channel("1.2.3-"));
        assert_eq!(update_repo_for("1.2.3-"), Some(GITHUB_REPO));
    }

    /// Whitespace must not change the decision: `jcode-build-meta` trims semver
    /// values before parsing them.
    #[test]
    fn channel_detection_trims_and_tolerates_v_prefix() {
        assert!(semver_has_channel("  1.5.3-k0.1.2  "));
        assert!(!semver_has_channel("  1.5.3  "));
        assert!(!semver_has_channel(""));
    }

    /// Upstream prereleases (alpha, rc, beta, nightly, etc.) carry standard
    /// SemVer suffixes that are NOT fork prefixes. These must NOT be classified
    /// as fork builds or their auto-update path is silently broken.
    ///
    /// See Issue 1 from `KooshaPari/KCode#104`.
    #[test]
    fn upstream_prereleases_are_not_forks() {
        for semver in [
            "1.0.0-alpha.1",
            "1.0.0-rc.1",
            "1.0.0-beta",
            "1.0.0-nightly",
            "2.3.4-alpha.1",
            "0.91.0-dev.5",
            "1.5.3-pre.2",
        ] {
            assert_eq!(
                update_repo_for(semver),
                Some(GITHUB_REPO),
                "upstream prerelease {semver} must NOT be classified as a fork build"
            );
            assert!(
                !is_fork_build_for(semver),
                "{semver} must not be treated as a fork build"
            );
        }
    }

    /// `fork-` prefix is a valid fork marker (generic fork convention).
    #[test]
    fn fork_prefix_is_recognized() {
        for semver in ["1.0.0-fork-1", "1.0.0-fork-2.0", "v2.0.0-fork-0.1"] {
            assert!(
                is_fork_build_for(semver),
                "{semver} should be recognized as a fork build via fork- prefix"
            );
            assert_eq!(
                update_repo_for(semver),
                None,
                "{semver} must not resolve to upstream"
            );
        }
    }

    /// Bare `fork-` with nothing after it is not a valid fork marker.
    #[test]
    fn bare_fork_prefix_is_not_valid() {
        assert!(!semver_has_channel("1.0.0-fork-"));
        assert_eq!(update_repo_for("1.0.0-fork-"), Some(GITHUB_REPO));
    }

    /// The build this binary was actually compiled with must be classified the
    /// way its own semver says, so the live resolver agrees with the pure one.
    #[test]
    fn live_resolver_agrees_with_pure_resolver() {
        assert_eq!(update_repo(), update_repo_for(current_semver()));
        assert_eq!(is_fork_build(), is_fork_build_for(current_semver()));
    }
}
