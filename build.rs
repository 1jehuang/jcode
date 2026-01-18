use std::process::Command;

fn main() {
    // Get Cargo.toml version
    let cargo_version = env!("CARGO_PKG_VERSION");

    // Get git commit hash
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok();

    let git_hash = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Get git commit date
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cs"])
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

    // Get recent commit messages (last 5 commits, one-line format)
    let output = Command::new("git")
        .args(["log", "--oneline", "-5", "--format=%s"])
        .output()
        .ok();

    let changelog = output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Build version string: v0.1.0 (abc1234) or v0.1.0-dev (abc1234-dirty)
    let version = if dirty {
        format!("v{}-dev ({})", cargo_version, git_hash)
    } else {
        format!("v{} ({})", cargo_version, git_hash)
    };

    // Set environment variables for compilation
    println!("cargo:rustc-env=JCODE_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=JCODE_GIT_DATE={}", git_date);
    println!("cargo:rustc-env=JCODE_VERSION={}", version);
    println!("cargo:rustc-env=JCODE_CHANGELOG={}", changelog);

    // Re-run if git HEAD changes or Cargo.toml changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
