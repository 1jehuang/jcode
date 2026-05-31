//! Compile-time build/version metadata for jcode.
//!
//! The build script (`build.rs`) computes git- and version-derived values and
//! emits them via `cargo:rustc-env`. This module re-exposes them as `pub const`
//! so any workspace crate can read identical values through e.g.
//! `jcode_build_meta::VERSION` instead of `env!("JCODE_VERSION")`.

/// Human-readable version string, e.g. `v0.14.6-dev (abc1234)`.
pub const VERSION: &str = env!("JCODE_VERSION");
/// Short git hash of the build commit, e.g. `abc1234` (or `unknown`).
pub const GIT_HASH: &str = env!("JCODE_GIT_HASH");
/// Commit date/time of the build commit (or `unknown`).
pub const GIT_DATE: &str = env!("JCODE_GIT_DATE");
/// `git describe --tags --always` output (may be empty).
pub const GIT_TAG: &str = env!("JCODE_GIT_TAG");
/// Auto-incrementing build semver (dev) or explicit release semver.
pub const SEMVER: &str = env!("JCODE_SEMVER");
/// Base semver taken from the root `Cargo.toml` package version.
pub const BASE_SEMVER: &str = env!("JCODE_BASE_SEMVER");
/// Semver used for update comparisons.
pub const UPDATE_SEMVER: &str = env!("JCODE_UPDATE_SEMVER");
/// Encoded changelog (record/unit separated). See build.rs for the format.
pub const CHANGELOG: &str = env!("JCODE_CHANGELOG");
/// Root crate package version (mirrors the historical `CARGO_PKG_VERSION`).
pub const PKG_VERSION: &str = env!("JCODE_PKG_VERSION");

/// Whether this binary was built as a release build (`JCODE_RELEASE_BUILD=1`).
pub const fn is_release_build() -> bool {
    option_env!("JCODE_RELEASE_BUILD").is_some()
}
