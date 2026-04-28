use std::process::Command;

fn main() {
    let pkg_version = env!("CARGO_PKG_VERSION");
    let git_hash = git_output(["rev-parse", "--short", "HEAD"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = git_output([
        "status",
        "--porcelain",
        "--",
        "crates/jcode-desktop",
        "Cargo.toml",
        "Cargo.lock",
    ])
    .map(|output| !output.trim().is_empty())
    .unwrap_or(false);
    let version = if dirty {
        format!("v{pkg_version}-dev ({git_hash}, dirty)")
    } else {
        format!("v{pkg_version}-dev ({git_hash})")
    };

    println!("cargo:rustc-env=JCODE_DESKTOP_VERSION={version}");
    println!("cargo:rustc-env=JCODE_DESKTOP_GIT_HASH={git_hash}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn git_output<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
}
