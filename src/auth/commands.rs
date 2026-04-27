use std::sync::OnceLock;

use super::COMMAND_EXISTS_CACHE;

pub(crate) fn command_available_from_env(env_var: &str, fallback: &str) -> bool {
    if let Ok(cmd) = std::env::var(env_var) {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() && command_exists(trimmed) {
            return true;
        }
    }

    command_exists(fallback)
}

pub(crate) fn command_exists(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }

    // Absolute/relative path: direct stat, no caching needed
    let path = std::path::Path::new(command);
    if path.is_absolute() || contains_path_separator(command) {
        return explicit_command_exists(path);
    }

    // Check per-process cache first (O(1) on repeated calls)
    if let Ok(cache) = COMMAND_EXISTS_CACHE.lock()
        && let Some(&cached) = cache.get(command)
    {
        return cached;
    }

    let path_var = match std::env::var_os("PATH") {
        Some(p) if !p.is_empty() => p,
        _ => {
            cache_command_result(command, false);
            return false;
        }
    };

    let wsl2 = is_wsl2();
    let found = std::env::split_paths(&path_var)
        // On WSL2 skip Windows DrvFs mounts (/mnt/c, /mnt/d, …) — they are
        // accessed via the slow 9P filesystem and CLI tools are never there.
        .filter(|dir| !(wsl2 && is_wsl2_windows_path(dir)))
        .flat_map(|dir| {
            command_candidates(command)
                .into_iter()
                .map(move |c| dir.join(c))
        })
        .any(|p| p.exists());

    cache_command_result(command, found);
    found
}

fn cache_command_result(command: &str, exists: bool) {
    if let Ok(mut cache) = COMMAND_EXISTS_CACHE.lock() {
        cache.insert(command.to_string(), exists);
    }
}

/// Detect WSL2: reads `/proc/version` once and caches the result for the
/// process lifetime.  Returns false on any platform without that file.
fn is_wsl2() -> bool {
    static IS_WSL2: OnceLock<bool> = OnceLock::new();
    *IS_WSL2.get_or_init(|| {
        std::fs::read_to_string("/proc/version")
            .map(|s| s.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
    })
}

/// Returns true for paths like `/mnt/c`, `/mnt/d`, … that are Windows drive
/// mounts under WSL2 (DrvFs via 9P).
pub(crate) fn is_wsl2_windows_path(dir: &std::path::Path) -> bool {
    use std::path::Component;
    let mut it = dir.components();
    if !matches!(it.next(), Some(Component::RootDir)) {
        return false;
    }
    if !matches!(it.next(), Some(Component::Normal(s)) if s == "mnt") {
        return false;
    }
    if let Some(Component::Normal(drive)) = it.next() {
        let s = drive.to_string_lossy();
        return s.len() == 1 && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    }
    false
}

fn explicit_command_exists(path: &std::path::Path) -> bool {
    if path.exists() {
        return true;
    }

    if has_extension(path) {
        return false;
    }

    #[cfg(windows)]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        for ext in pathext
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
        {
            let candidate = path.with_extension(ext.trim_start_matches('.'));
            if candidate.exists() {
                return true;
            }
        }
    }

    false
}

pub(crate) fn command_candidates(command: &str) -> Vec<std::ffi::OsString> {
    let path = std::path::Path::new(command);
    let file_name = match path.file_name() {
        Some(name) => name.to_os_string(),
        None => return Vec::new(),
    };

    if has_extension(path) {
        return vec![file_name];
    }

    #[cfg(windows)]
    let mut candidates = vec![file_name.clone()];
    #[cfg(not(windows))]
    let candidates = vec![file_name.clone()];

    #[cfg(windows)]
    {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let exts: Vec<&str> = pathext
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
            .collect();

        for ext in exts {
            let ext_no_dot = ext.trim_start_matches('.');
            if ext_no_dot.is_empty() {
                continue;
            }
            let mut candidate = path.to_path_buf();
            candidate.set_extension(ext_no_dot);
            if let Some(cand_name) = candidate.file_name() {
                candidates.push(cand_name.to_os_string());
            }
        }
    }

    dedup_preserve_order(candidates)
}

pub(crate) fn contains_path_separator(command: &str) -> bool {
    command.contains('/')
        || command.contains('\\')
        || std::path::Path::new(command).components().count() > 1
}

pub(crate) fn has_extension(path: &std::path::Path) -> bool {
    path.extension().is_some()
}

pub(crate) fn dedup_preserve_order(mut values: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut out = Vec::new();
    for value in values.drain(..) {
        if !out.iter().any(|v| v == &value) {
            out.push(value);
        }
    }

    out
}
