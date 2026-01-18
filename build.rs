use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Get and increment build number (stored in ~/.jcode/build_number)
    let build_number = increment_build_number();

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

    // Build version string with auto-incremented build number
    // Format: v0.1.47 (abc1234) or v0.1.47-dev (abc1234)
    let version = if dirty {
        format!("v0.1.{}-dev ({})", build_number, git_hash)
    } else {
        format!("v0.1.{} ({})", build_number, git_hash)
    };

    // Set environment variables for compilation
    println!("cargo:rustc-env=JCODE_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=JCODE_GIT_DATE={}", git_date);
    println!("cargo:rustc-env=JCODE_VERSION={}", version);
    println!("cargo:rustc-env=JCODE_BUILD_NUMBER={}", build_number);
    println!("cargo:rustc-env=JCODE_CHANGELOG={}", changelog);

    // Re-run if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}

/// Get and increment the build number stored in ~/.jcode/build_number
fn increment_build_number() -> u32 {
    let jcode_dir = dirs::home_dir()
        .map(|h| h.join(".jcode"))
        .unwrap_or_else(|| PathBuf::from(".jcode"));

    // Ensure directory exists
    let _ = fs::create_dir_all(&jcode_dir);

    let build_file = jcode_dir.join("build_number");

    // Read current build number
    let current = fs::read_to_string(&build_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    // Increment and save
    let next = current + 1;
    let _ = fs::write(&build_file, next.to_string());

    next
}
