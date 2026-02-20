use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let pkg_version = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = pkg_version.split('.').collect();
    let major = parts.get(0).unwrap_or(&"0");
    let minor = parts.get(1).unwrap_or(&"0");

    let build_number = increment_build_number(major, minor);

    // Get git commit hash
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok();

    let git_hash = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Get git commit date (full datetime with timezone for accurate age calculation)
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ci"])
        .output()
        .ok();

    let git_date = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Check if working directory is dirty
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok();

    let dirty = output.map(|o| !o.stdout.is_empty()).unwrap_or(false);

    // Get git tag (e.g., "v0.1.2" if HEAD is tagged, or "v0.1.2-3-gabc1234" if ahead)
    let output = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .ok();

    let git_tag = output
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Get recent commit messages (last 20 commits, with short hash for tracking "last seen")
    // Format: "hash:subject" per line so runtime can filter to only new-since-last-seen
    let output = Command::new("git")
        .args(["log", "--oneline", "-20", "--format=%h:%s"])
        .output()
        .ok();

    let changelog = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Build version string:
    //   Release: v0.2.0 (abc1234)
    //   Dev:     v0.2.5 (abc1234)
    //   Dirty:   v0.2.5-dirty (abc1234)
    let is_release = std::env::var("JCODE_RELEASE_BUILD").is_ok();
    let patch = parts.get(2).unwrap_or(&"0");
    let version = if is_release {
        format!("v{}.{}.{} ({})", major, minor, patch, git_hash)
    } else if dirty {
        format!("v{}.{}.{}-dirty ({})", major, minor, build_number, git_hash)
    } else {
        format!("v{}.{}.{} ({})", major, minor, build_number, git_hash)
    };

    // Get actual build timestamp
    let build_time = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S %z")
        .to_string();

    // Set environment variables for compilation
    println!("cargo:rustc-env=JCODE_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=JCODE_GIT_DATE={}", git_date);
    println!("cargo:rustc-env=JCODE_BUILD_TIME={}", build_time);
    println!("cargo:rustc-env=JCODE_VERSION={}", version);
    println!("cargo:rustc-env=JCODE_BUILD_NUMBER={}", build_number);
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
}

/// Get and increment the build number, scoped to the current major.minor version.
/// Resets to 1 when the version in Cargo.toml is bumped.
fn increment_build_number(major: &str, minor: &str) -> u32 {
    let jcode_dir = dirs::home_dir()
        .map(|h| h.join(".jcode"))
        .unwrap_or_else(|| PathBuf::from(".jcode"));

    let _ = fs::create_dir_all(&jcode_dir);

    let build_file = jcode_dir.join("build_number");
    let version_file = jcode_dir.join("build_version");

    let current_version = format!("{}.{}", major, minor);

    // Check if the version changed (Cargo.toml was bumped)
    let stored_version = fs::read_to_string(&version_file)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if stored_version != current_version {
        // Version bumped — reset build number
        let _ = fs::write(&version_file, &current_version);
        let _ = fs::write(&build_file, "1");
        return 1;
    }

    // Same version — increment
    let current = fs::read_to_string(&build_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let next = current + 1;
    let _ = fs::write(&build_file, next.to_string());

    next
}
