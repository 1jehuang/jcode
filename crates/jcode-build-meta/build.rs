use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Build metadata generator for the jcode workspace.
//
// This is the single source of truth for the JCODE_* compile-time values that
// previously lived in the root `jcode` crate's build.rs. It is hosted in the
// leaf `jcode-build-meta` crate so every workspace crate can read identical
// values (via `jcode_build_meta::*`) without duplicating this script.
//
// NOTE: because this crate's own package version is unrelated to jcode's, we
// parse the root `Cargo.toml` `[package].version` for the base semver instead
// of `CARGO_PKG_VERSION`.

fn main() {
    let repo_root = repo_root();

    let pkg_version = root_package_version(&repo_root).unwrap_or_else(|| "0.0.0".to_string());
    let base_version = parse_semver(&pkg_version).unwrap_or((0, 0, 0));
    let build_semver = resolve_build_semver(base_version).unwrap_or_else(|err| {
        eprintln!("cargo:warning=failed to resolve auto build semver: {err}");
        pkg_version.clone()
    });
    let (major, minor, patch) = parse_semver(&build_semver).unwrap_or(base_version);
    let base_semver = format!("{}.{}.{}", base_version.0, base_version.1, base_version.2);
    let update_semver = if explicit_build_semver_override().is_some() {
        build_semver.clone()
    } else {
        base_semver.clone()
    };

    let git_hash = env_or_metadata_or_git(
        &repo_root,
        "JCODE_BUILD_GIT_HASH",
        "git_hash",
        ["rev-parse", "--short", "HEAD"],
    )
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

    // Get git commit date (full datetime with timezone for accurate age calculation)
    let git_date = env_or_metadata_or_git(
        &repo_root,
        "JCODE_BUILD_GIT_DATE",
        "git_date",
        ["log", "-1", "--format=%ci"],
    )
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

    let dirty = match std::env::var("JCODE_BUILD_GIT_DIRTY") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "dirty"
        ),
        Err(_) => metadata_value("git_dirty")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "dirty"
                )
            })
            .or_else(|| {
                git_output(&repo_root, ["status", "--porcelain"]).map(|output| !output.is_empty())
            })
            .unwrap_or(false),
    };

    // Get git tag (e.g., "v0.1.2" if HEAD is tagged, or "v0.1.2-3-gabc1234" if ahead)
    let git_tag = env_or_metadata_or_git(
        &repo_root,
        "JCODE_BUILD_GIT_TAG",
        "git_tag",
        ["describe", "--tags", "--always"],
    )
    .unwrap_or_default();

    // Get recent commit messages with commit timestamps and version tag decorations.
    // Format: "hash|timestamp|decorations|subject" per line.
    // We embed a deeper window so /changelog can cover many more releases.
    let raw_log = std::env::var("JCODE_BUILD_CHANGELOG_RAW")
        .ok()
        .or_else(|| metadata_value("changelog_raw"))
        .or_else(|| git_output(&repo_root, ["log", "-700", "--format=%h|%ct|%D|%s"]))
        .unwrap_or_default();

    // Normalize to "hash<RS>tag<RS>timestamp<RS>subject" — extract version tag or
    // leave empty. We use ASCII record/unit separators so fields can safely
    // contain punctuation.
    let changelog = raw_log
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let hash = parts.next()?;
            let timestamp = parts.next().unwrap_or("");
            let decorations = parts.next().unwrap_or("");
            let subject = parts.next()?;
            let tag = decorations
                .split(',')
                .map(|d| d.trim())
                .find(|d| d.starts_with("tag: v"))
                .and_then(|d| d.strip_prefix("tag: "))
                .unwrap_or("");
            Some(format!(
                "{}\x1e{}\x1e{}\x1e{}",
                hash, tag, timestamp, subject
            ))
        })
        .collect::<Vec<_>>()
        .join("\x1f");

    // Build version string:
    //   Release: v0.2.17 (abc1234)
    //   Dev:     v0.2.17-dev (abc1234)
    //   Dirty:   v0.2.17-dev (abc1234, dirty)
    let is_release = std::env::var("JCODE_RELEASE_BUILD").is_ok();
    let version = if is_release {
        format!("v{}.{}.{} ({})", major, minor, patch, git_hash)
    } else if dirty {
        format!("v{}.{}.{}-dev ({}, dirty)", major, minor, patch, git_hash)
    } else {
        format!("v{}.{}.{}-dev ({})", major, minor, patch, git_hash)
    };

    // Set environment variables for compilation
    println!("cargo:rustc-env=JCODE_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=JCODE_GIT_DATE={}", git_date);
    println!("cargo:rustc-env=JCODE_VERSION={}", version);
    println!("cargo:rustc-env=JCODE_SEMVER={}", build_semver);
    println!("cargo:rustc-env=JCODE_BASE_SEMVER={}", base_semver);
    println!("cargo:rustc-env=JCODE_UPDATE_SEMVER={}", update_semver);
    println!("cargo:rustc-env=JCODE_GIT_TAG={}", git_tag);
    println!("cargo:rustc-env=JCODE_CHANGELOG={}", changelog);
    println!("cargo:rustc-env=JCODE_PKG_VERSION={}", pkg_version);

    // Forward JCODE_RELEASE_BUILD env var if set (CI sets this for release binaries)
    if std::env::var("JCODE_RELEASE_BUILD").is_ok() {
        println!("cargo:rustc-env=JCODE_RELEASE_BUILD=1");
    }

    // Re-run only on inputs that should genuinely change the embedded metadata.
    //
    // IMPORTANT: we deliberately do NOT declare `.git/HEAD` or `.git/index` as
    // `rerun-if-changed` inputs. Those files' mtimes change on every `git add`,
    // `git status`, commit, and concurrent-agent git op. Cargo treats a build
    // script as dirty whenever any declared input is newer than the script's
    // output file, reruns it, and then force-recompiles every dependent crate
    // via StaleDepFingerprint -- even when the emitted output is byte-identical.
    // Since `jcode-build-meta` sits at the bottom of the crate graph
    // (base -> app-core -> tui -> cli all depend on it), watching the git files
    // turned routine git activity into a full-tree recompile (~18s) on every
    // incremental build. See the deterministic-semver note in
    // `resolve_build_semver` for the companion fix.
    //
    // Correctness is preserved where it matters:
    //   * Release/dist builds set JCODE_RELEASE_BUILD=1 and JCODE_BUILD_SEMVER,
    //     both of which DO force a rerun (declared below), so released binaries
    //     always embed the exact version/hash.
    //   * A `[package].version` bump touches Cargo.toml (declared below), which
    //     refreshes the embedded metadata for the next build.
    //   * `cargo clean` / editing this build script naturally re-runs it.
    // For ordinary dev builds the embedded git hash/dirty flag may lag the very
    // latest commit within a session; that is a cosmetic `--version` detail and
    // an acceptable trade for keeping incremental builds incremental.
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("Cargo.toml").display()
    );
    println!("cargo:rerun-if-env-changed=JCODE_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_SEMVER");
    // Allow callers to force a metadata refresh (e.g. install scripts) without a
    // full clean, by bumping this env var.
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_GIT_HASH");
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_GIT_DATE");
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_GIT_DIRTY");
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_GIT_TAG");
}

/// Workspace root, derived from this crate's manifest dir (`crates/jcode-build-meta`).
fn repo_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    // crates/jcode-build-meta -> crates -> <repo root>
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

/// Parse `[package].version` from the root `Cargo.toml` without a toml dep.
fn root_package_version(repo_root: &Path) -> Option<String> {
    let data = fs::read_to_string(repo_root.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let value = rest.trim().trim_matches('"').to_string();
                    if !value.is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn parse_semver(value: &str) -> Option<(u32, u32, u32)> {
    let trimmed = value.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn explicit_build_semver_override() -> Option<String> {
    std::env::var("JCODE_BUILD_SEMVER")
        .ok()
        .map(|value| value.trim().trim_start_matches('v').to_string())
        .filter(|value| parse_semver(value).is_some())
}

fn resolve_build_semver(base_version: (u32, u32, u32)) -> Result<String, String> {
    if let Some(explicit) = explicit_build_semver_override() {
        return Ok(explicit);
    }

    // Dev builds derive the patch number deterministically from committed git
    // state: `base.patch + <commits since the base-version tag>`. This is a pure
    // function of HEAD, so the emitted JCODE_SEMVER/JCODE_VERSION only change when
    // an actual commit lands, NOT on every build-script rerun.
    //
    // The previous implementation incremented a persistent counter on every
    // rerun. Because the build script reruns whenever `.git/index`/`.git/HEAD`
    // change (any `git add`, commit, or concurrent agent git op), that side
    // effect churned the version string on essentially every build, which in
    // turn invalidated `jcode-build-meta` and force-recompiled the entire crate
    // graph (base -> app-core -> tui -> cli). Deriving the value deterministically
    // keeps incremental rebuilds incremental.
    let offset = commits_since_base_tag(base_version).unwrap_or(0);
    let patch = base_version.2.saturating_add(offset);
    Ok(format!("{}.{}.{}", base_version.0, base_version.1, patch))
}

/// Count commits between the base-version tag (`vMAJOR.MINOR.PATCH`) and HEAD.
/// Returns `None` when git is unavailable or the tag does not exist yet, in which
/// case the caller falls back to the base patch (still deterministic).
fn commits_since_base_tag(base_version: (u32, u32, u32)) -> Option<u32> {
    let repo_root = repo_root();
    let tag = format!("v{}.{}.{}", base_version.0, base_version.1, base_version.2);
    let range = format!("{tag}..HEAD");
    let out = git_output(&repo_root, ["rev-list", "--count", range.as_str()])?;
    out.trim().parse::<u32>().ok()
}

fn env_or_metadata_or_git<const N: usize>(
    repo_root: &Path,
    env_name: &str,
    metadata_key: &str,
    git_args: [&str; N],
) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .or_else(|| metadata_value(metadata_key))
        .or_else(|| git_output(repo_root, git_args))
        .map(|value| value.trim().to_string())
}

fn git_output<const N: usize>(repo_root: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn metadata_value(key: &str) -> Option<String> {
    let path = std::env::var("JCODE_BUILD_METADATA_FILE").ok()?;
    let data = fs::read_to_string(path).ok()?;
    let mut lines = data.lines();
    while let Some(line) = lines.next() {
        if let Some((entry_key, marker)) = line.split_once("<<") {
            if entry_key == key {
                let mut value = String::new();
                for value_line in lines.by_ref() {
                    if value_line == marker {
                        return Some(value);
                    }
                    if !value.is_empty() {
                        value.push('\n');
                    }
                    value.push_str(value_line);
                }
                return Some(value);
            }
            continue;
        }

        if let Some((entry_key, entry_value)) = line.split_once('=')
            && entry_key == key
        {
            return Some(entry_value.to_string());
        }
    }
    None
}
