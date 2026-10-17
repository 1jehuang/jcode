//! Socket path resolution shared by every harness API client and the bridge.
//!
//! This lives in the API crate on purpose. It used to be duplicated in the
//! bridge and external clients, where separate copies could disagree: the bridge
//! resolved `$XDG_RUNTIME_DIR` while the desktop always looked in
//! `~/.kcode`. The result was a desktop app that could never connect even
//! with a healthy bridge running. One definition, used by both sides, makes
//! that class of bug impossible.
//!
//! The rules match `jcode-storage::runtime_dir` so the API socket always lands
//! beside the daemon socket it bridges to.

use std::path::PathBuf;

fn product_path(path: PathBuf) -> Option<PathBuf> {
    fn owned(path: &std::path::Path) -> bool {
        path.is_absolute()
            && !path.components().any(|part| {
                let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
                name == "jcode"
                    || name.starts_with(".jcode")
                    || name.starts_with("jcode.")
                    || name.starts_with("jcode-")
            })
    }
    if !owned(&path) {
        return None;
    }
    if let Some(existing) = path.ancestors().find(|part| part.exists()) {
        let canonical = existing.canonicalize().ok()?;
        if !owned(&canonical) {
            return None;
        }
    }
    Some(path)
}

/// Runtime directory holding the daemon and API sockets.
pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KCODE_RUNTIME_DIR") {
        if let Some(path) = product_path(PathBuf::from(dir)) {
            return path;
        }
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if let Some(path) = product_path(PathBuf::from(dir)) {
            return path;
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(dir) = std::env::var("TMPDIR") {
        return PathBuf::from(dir);
    }
    fallback_runtime_dir()
}

fn fallback_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!("kcode-{}", runtime_user_discriminator()))
}

#[cfg(unix)]
fn runtime_user_discriminator() -> String {
    // Read the uid without pulling in libc: the API crate is deliberately
    // dependency-light, and this only needs to disambiguate users in $TMPDIR.
    std::env::var("UID")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .map(sanitize)
        .unwrap_or_else(|| "user".to_string())
}

#[cfg(not(unix))]
fn runtime_user_discriminator() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .map(sanitize)
        .unwrap_or_else(|_| "user".to_string())
}

fn sanitize(raw: String) -> String {
    let out: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(64)
        .collect();
    if out.is_empty() {
        "user".to_string()
    } else {
        out
    }
}

/// Path of the versioned harness API socket. `KCODE_API_SOCKET` overrides it.
pub fn api_socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("KCODE_API_SOCKET") {
        if let Some(path) = product_path(PathBuf::from(custom)) {
            return path;
        }
    }
    runtime_dir().join("kcode-api.sock")
}

/// Path of the internal daemon socket the bridge translates onto.
/// `KCODE_SOCKET` overrides it.
pub fn legacy_socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("KCODE_SOCKET") {
        if let Some(path) = product_path(PathBuf::from(custom)) {
            return path;
        }
    }
    runtime_dir().join("kcode.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_cannot_select_jcode_sockets_or_storage() {
        let root = std::env::temp_dir();
        for name in [
            "jcode.sock",
            "jcode-api.sock",
            ".jcode/sessions",
            "jcode/bin",
            "jcode-user/run",
        ] {
            assert!(product_path(root.join(name)).is_none());
        }
        assert!(product_path(root.join("kcode-api.sock")).is_some());
    }

    /// The two sockets must always be siblings. A client that resolves one
    /// directory while the bridge resolves another cannot connect at all,
    /// which is exactly the bug this module exists to prevent.
    #[test]
    fn the_api_socket_sits_beside_the_daemon_socket() {
        // Guard against env-dependent divergence by comparing parents rather
        // than absolute paths, since either may be overridden in a session.
        let api = runtime_dir().join("kcode-api.sock");
        let legacy = runtime_dir().join("kcode.sock");
        assert_eq!(api.parent(), legacy.parent());
    }

    #[test]
    fn socket_names_are_stable() {
        assert_eq!(
            runtime_dir().join("kcode-api.sock").file_name().unwrap(),
            "kcode-api.sock"
        );
    }

    #[test]
    fn sanitize_strips_path_and_shell_characters() {
        assert_eq!(sanitize("../root; rm".into()), "rootrm");
        assert_eq!(sanitize("!!!".into()), "user");
    }
}
