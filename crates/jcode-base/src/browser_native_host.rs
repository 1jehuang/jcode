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

/// Whether an existing manifest already points at a live host and allows the
/// extension this browser uses.
pub(super) fn native_host_manifest_is_valid(
    kind: BrowserKind,
    existing: &serde_json::Value,
) -> bool {
    let host_ok = existing["path"]
        .as_str()
        .is_some_and(|p| std::path::Path::new(p).exists());
    let allowed_ok = if kind.family() == BrowserFamily::Gecko {
        existing["allowed_extensions"]
            .as_array()
            .is_some_and(|ids| {
                ids.iter()
                    .any(|id| id.as_str() == Some(EXTENSION_ID_LISTED))
            })
    } else {
        let origin = format!("chrome-extension://{}/", CHROMIUM_EXTENSION_ID);
        existing["allowed_origins"]
            .as_array()
            .is_some_and(|o| o.iter().any(|v| v.as_str() == Some(origin.as_str())))
    };
    host_ok && allowed_ok
}

pub(super) fn install_native_host_manifest_for(kind: BrowserKind) -> Result<bool> {
    let dirs = native_messaging_hosts_dirs_for(kind)?;
    let host_path = host_binary_path();
    if !host_path.exists() {
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
                Ok(existing) => native_host_manifest_is_valid(kind, &existing),
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
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
            wrote_any = true;
        }
        #[cfg(target_os = "windows")]
        register_windows_native_host_manifest(kind, &manifest_path)?;
    }
    Ok(wrote_any)
}

#[cfg(target_os = "windows")]
pub(super) fn register_windows_native_host_manifest(
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

/// Extra `NativeMessagingHosts` directories for Chromium forks jcode does not
/// know (Helium, Vivaldi, Thorium, ...), from `JCODE_BROWSER_NATIVE_HOST_DIRS`
/// (an OS path list). Setup installs the Chromium host manifest there too.
pub(super) fn extra_native_messaging_dirs(value: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let Some(value) = value else {
        return Vec::new();
    };
    std::env::split_paths(value)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}
