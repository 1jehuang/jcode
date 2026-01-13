use std::process::Command;

fn main() {
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

    let dirty = output
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let dirty_suffix = if dirty { "-dirty" } else { "" };

    // Set environment variables for compilation
    println!("cargo:rustc-env=JCODE_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=JCODE_GIT_DATE={}", git_date);
    println!("cargo:rustc-env=JCODE_VERSION={} ({}{})", git_date, git_hash, dirty_suffix);

    // Re-run if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
