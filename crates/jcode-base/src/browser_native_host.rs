use super::*;

pub(super) fn native_host_manifest_json(kind: BrowserKind, host_path: &str) -> serde_json::Value {
    let mut manifest = serde_json::json!({
        "name": NATIVE_HOST_NAME,
        "description": "Native host for Browser Agent Bridge (managed by jcode)",
        "path": host_path,
        "type": "stdio",
    });
    if kind.family() == BrowserFamily::Gecko {
        manifest["allowed_extensions"] =
            serde_json::json!([EXTENSION_ID_LOCAL, EXTENSION_ID_LISTED]);
    } else {
        manifest["allowed_origins"] =
            serde_json::json!([format!("chrome-extension://{}/", CHROMIUM_EXTENSION_ID)]);
    }
    manifest
}

/// Validate the managed host identity, executable and extension allowlist.
pub(super) fn native_host_manifest_is_valid_for_path(
    kind: BrowserKind,
    existing: &serde_json::Value,
    expected_path: &std::path::Path,
) -> bool {
    let host_ok = existing["path"].as_str().is_some_and(|p| {
        let path = std::path::Path::new(p);
        path.is_absolute() && path == expected_path && host_is_executable(path)
    });
    let allowed_ok = if kind.family() == BrowserFamily::Gecko {
        existing["allowed_extensions"]
            .as_array()
            .is_some_and(|ids| {
                ids.len() == 2
                    && ids
                        .iter()
                        .any(|id| id.as_str() == Some(EXTENSION_ID_LISTED))
                    && ids.iter().any(|id| id.as_str() == Some(EXTENSION_ID_LOCAL))
            })
    } else {
        let origin = format!("chrome-extension://{}/", CHROMIUM_EXTENSION_ID);
        existing["allowed_origins"]
            .as_array()
            .is_some_and(|origins| {
                origins.len() == 1 && origins[0].as_str() == Some(origin.as_str())
            })
    };
    existing["name"].as_str() == Some(NATIVE_HOST_NAME)
        && existing["type"].as_str() == Some("stdio")
        && host_ok
        && allowed_ok
}

fn host_is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(super) fn install_native_host_manifest_for(kind: BrowserKind) -> Result<bool> {
    let dirs = native_messaging_hosts_dirs_for(kind)?;
    let host_path = std::path::absolute(host_binary_path())?;
    write_native_host_manifests(kind, dirs, &host_path)
}

pub(super) fn write_native_host_manifests(
    kind: BrowserKind,
    dirs: Vec<PathBuf>,
    host_path: &std::path::Path,
) -> Result<bool> {
    if !host_path.is_absolute() || !host_is_executable(host_path) {
        return Err(anyhow::anyhow!(
            "Host binary not found at {}. The native messaging host is required for the {} extension to communicate with the bridge.",
            host_path.display(),
            kind.display_name()
        ));
    }
    let manifest = native_host_manifest_json(kind, &host_path.to_string_lossy());
    let mut wrote_any = false;
    for manifest_dir in dirs {
        let manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_NAME));
        let valid = match std::fs::read_to_string(&manifest_path) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(existing) => native_host_manifest_is_valid_for_path(kind, &existing, host_path),
                Err(error) => {
                    crate::logging::warn(&format!(
                        "Replacing invalid native host manifest {}: {error}",
                        manifest_path.display()
                    ));
                    false
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Cannot read native host manifest {}: {error}",
                    manifest_path.display()
                ));
                false
            }
        };
        if !valid {
            std::fs::create_dir_all(&manifest_dir)?;
            use std::io::Write;
            let mut file = tempfile::NamedTempFile::new_in(&manifest_dir)?;
            file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
            file.persist(&manifest_path)?;
            wrote_any = true;
        }
        #[cfg(target_os = "windows")]
        register_windows_native_host_manifest(kind, &manifest_path)?;
    }
    Ok(wrote_any)
}

#[cfg(target_os = "windows")]
fn register_windows_native_host_manifest(
    kind: BrowserKind,
    manifest_path: &std::path::Path,
) -> Result<()> {
    for root in kind.windows_native_host_registry_roots() {
        let key = format!(r"{}\{}", root, NATIVE_HOST_NAME);
        let output = std::process::Command::new("reg")
            .args([
                "add",
                &key,
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                &manifest_path.to_string_lossy(),
                "/f",
            ])
            .output()
            .context("Failed to register the native messaging host in the Windows registry")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let details = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            anyhow::bail!(
                "Failed to register the {} native messaging host in the Windows registry: {}",
                kind.display_name(),
                details
            );
        }
    }
    Ok(())
}

pub(super) fn native_messaging_hosts_dirs_for(kind: BrowserKind) -> Result<Vec<PathBuf>> {
    let mut dirs = kind.native_messaging_dirs();
    if kind.family() == BrowserFamily::Chromium {
        dirs.extend(extra_native_messaging_dirs(
            std::env::var_os("JCODE_BROWSER_NATIVE_HOST_DIRS").as_deref(),
        ));
    }
    if dirs.is_empty() {
        anyhow::bail!(
            "{} does not use native messaging on this platform",
            kind.display_name()
        );
    }
    Ok(dirs)
}

/// Extra Chromium fork NativeMessagingHosts directories from an OS path list.
pub(super) fn extra_native_messaging_dirs(value: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let Some(value) = value else {
        return Vec::new();
    };
    std::env::split_paths(value)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}
