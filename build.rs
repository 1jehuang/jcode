use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime};

fn main() {
    let pkg_version = env!("CARGO_PKG_VERSION");
    let base_version = parse_semver(pkg_version).unwrap_or((0, 0, 0));
    let build_semver = resolve_build_semver(base_version).unwrap_or_else(|err| {
        eprintln!("cargo:warning=failed to resolve auto build semver: {err}");
        pkg_version.to_string()
    });
    let (major, minor, patch) = parse_semver(&build_semver).unwrap_or(base_version);
    let base_semver = format!("{}.{}.{}", base_version.0, base_version.1, base_version.2);
    let update_semver = if explicit_build_semver_override().is_some() {
        build_semver.clone()
    } else {
        base_semver.clone()
    };

    let git_hash = env_or_metadata_or_git(
        "JCODE_BUILD_GIT_HASH",
        "git_hash",
        ["rev-parse", "--short", "HEAD"],
    )
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

    // Get git commit date (full datetime with timezone for accurate age calculation)
    let git_date = env_or_metadata_or_git(
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
            .or_else(|| git_output(["status", "--porcelain"]).map(|output| !output.is_empty()))
            .unwrap_or(false),
    };

    // Get git tag (e.g., "v0.1.2" if HEAD is tagged, or "v0.1.2-3-gabc1234" if ahead)
    let git_tag = env_or_metadata_or_git(
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
        .or_else(|| git_output(["log", "-700", "--format=%h|%ct|%D|%s"]))
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

    // Forward JCODE_RELEASE_BUILD env var if set (CI sets this for release binaries)
    if std::env::var("JCODE_RELEASE_BUILD").is_ok() {
        println!("cargo:rustc-env=JCODE_RELEASE_BUILD=1");
    }

    // Re-run if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=JCODE_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=JCODE_BUILD_SEMVER");
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

    let next_patch = next_build_patch(base_version)?;
    Ok(format!(
        "{}.{}.{}",
        base_version.0, base_version.1, next_patch
    ))
}

fn next_build_patch(base_version: (u32, u32, u32)) -> Result<u32, String> {
    let counter_file = build_counter_file();
    if let Some(parent) = counter_file.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create counter dir {}: {err}", parent.display()))?;
    }

    let lock_path = counter_file.with_extension("lock");
    let _lock = BuildCounterLock::acquire(&lock_path)?;
    let mut counters = load_patch_counters(&counter_file)
        .map_err(|err| format!("read counter file {}: {err}", counter_file.display()))?;

    let key = format!("{}.{}", base_version.0, base_version.1);
    let previous = counters.get(&key).copied().unwrap_or(base_version.2);
    let next = previous.max(base_version.2).saturating_add(1);
    counters.insert(key, next);
    save_patch_counters(&counter_file, &counters)
        .map_err(|err| format!("write counter file {}: {err}", counter_file.display()))?;
    Ok(next)
}

fn build_counter_file() -> PathBuf {
    if let Some(target_root) = target_root_from_out_dir() {
        return target_root.join("jcode-build").join("patch-counters.txt");
    }

    std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("target")
        .join("jcode-build")
        .join("patch-counters.txt")
}

fn target_root_from_out_dir() -> Option<PathBuf> {
    let out_dir = std::env::var("OUT_DIR").ok()?;
    let out_dir = PathBuf::from(out_dir);
    for ancestor in out_dir.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some("target") {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn load_patch_counters(path: &Path) -> std::io::Result<std::collections::BTreeMap<String, u32>> {
    let mut counters = std::collections::BTreeMap::new();
    let data = match fs::read_to_string(path) {
        Ok(data) => data,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(counters),
        Err(err) => return Err(err),
    };

    for line in data.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some((key, value)) = line.split_once('=')
            && let Ok(value) = value.trim().parse::<u32>()
        {
            counters.insert(key.trim().to_string(), value);
        }
    }

    Ok(counters)
}

fn save_patch_counters(
    path: &Path,
    counters: &std::collections::BTreeMap<String, u32>,
) -> std::io::Result<()> {
    let contents = counters
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{contents}\n"))
}

struct BuildCounterLock {
    path: PathBuf,
}

impl BuildCounterLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        const MAX_ATTEMPTS: usize = 200;
        const SLEEP_MS: u64 = 50;
        const STALE_SECS: u64 = 300;

        for _ in 0..MAX_ATTEMPTS {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if lock_is_stale(path, STALE_SECS) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(SLEEP_MS));
                }
                Err(err) => {
                    return Err(format!("create lock {}: {err}", path.display()));
                }
            }
        }

        Err(format!(
            "timed out waiting for build counter lock {}",
            path.display()
        ))
    }
}

impl Drop for BuildCounterLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path, stale_after_secs: u64) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(elapsed) = SystemTime::now().duration_since(modified) else {
        return false;
    };
    elapsed.as_secs() >= stale_after_secs
}

fn env_or_metadata_or_git<const N: usize>(
    env_name: &str,
    metadata_key: &str,
    git_args: [&str; N],
) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .or_else(|| metadata_value(metadata_key))
        .or_else(|| git_output(git_args))
        .map(|value| value.trim().to_string())
}

fn git_output<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
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
