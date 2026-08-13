//! Jcode-managed Grok Build backend discovery and provisioning.
//!
//! Grok Build currently exposes its subscription runtime through ACP. Jcode
//! keeps that implementation as a private provider backend, downloading the
//! official xAI binary into Jcode's data directory when first needed. Users do
//! not need to install the `grok` CLI or put it on `PATH`.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

pub const CLI_PATH_ENV: &str = "JCODE_GROK_CLI_PATH";
const PRIMARY_BASE_URL: &str = "https://x.ai/cli";
const FALLBACK_BASE_URL: &str = "https://storage.googleapis.com/grok-build-public-artifacts/cli";

fn managed_cli_path() -> Result<PathBuf> {
    let name = if cfg!(windows) { "grok.exe" } else { "grok" };
    Ok(crate::storage::jcode_dir()?
        .join("provider-backends")
        .join("grok-build")
        .join(name))
}

pub fn cli_path() -> PathBuf {
    if let Some(path) = std::env::var_os(CLI_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return path;
    }
    if let Ok(path) = managed_cli_path()
        && path.is_file()
    {
        return path;
    }
    PathBuf::from("grok")
}

pub fn cli_available() -> bool {
    super::command_exists(cli_path().to_string_lossy().as_ref())
}

/// Whether the managed backend has a credential that it can attempt to use.
/// Backend presence alone is not authentication and must not make `/login` or
/// `jcode auth status` claim that Grok Build is ready.
pub fn has_cached_login() -> bool {
    if std::env::var("GROK_DEPLOYMENT_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(PathBuf::from(home).join(".grok").join("auth.json")) else {
        return false;
    };
    credentials_json_has_login(&bytes)
}

fn credentials_json_has_login(bytes: &[u8]) -> bool {
    let Ok(serde_json::Value::Object(scopes)) = serde_json::from_slice(bytes) else {
        return false;
    };
    scopes.values().any(|credential| {
        credential
            .get("key")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|key| !key.trim().is_empty())
    })
}

fn platform_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("windows", "aarch64") => Ok("windows-aarch64"),
        (os, arch) => bail!("Grok Build is not available for {os}-{arch}"),
    }
}

fn valid_version(version: &str) -> bool {
    let mut core_and_suffix = version.splitn(2, '-');
    let core = core_and_suffix.next().unwrap_or_default();
    let suffix_ok = core_and_suffix.next().is_none_or(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
    });
    suffix_ok
        && core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

async fn download_from_base(client: &reqwest::Client, base: &str) -> Result<Vec<u8>> {
    let version = client
        .get(format!("{base}/stable"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let version = version.trim();
    if !valid_version(version) {
        bail!("xAI returned an invalid Grok Build version: {version:?}");
    }
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let url = format!("{base}/grok-{version}-{}{extension}", platform_name()?);
    Ok(client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {url}"))?
        .bytes()
        .await?
        .to_vec())
}

/// Return a usable Grok Build ACP backend, downloading the official binary
/// into Jcode's private data directory when no explicit/system binary exists.
pub async fn ensure_cli() -> Result<PathBuf> {
    let existing = cli_path();
    if super::command_exists(existing.to_string_lossy().as_ref()) {
        return Ok(existing);
    }

    let destination = managed_cli_path()?;
    let parent = destination
        .parent()
        .context("managed Grok Build path has no parent")?;
    std::fs::create_dir_all(parent)?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("jcode/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let bytes = match download_from_base(&client, PRIMARY_BASE_URL).await {
        Ok(bytes) => bytes,
        Err(primary) => download_from_base(&client, FALLBACK_BASE_URL)
            .await
            .with_context(|| format!("x.ai download failed first: {primary:#}"))?,
    };
    if bytes.is_empty() {
        bail!("downloaded Grok Build backend was empty");
    }

    let temporary = destination.with_extension(format!("download-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::rename(&temporary, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::{credentials_json_has_login, ensure_cli, valid_version};

    #[test]
    fn accepts_only_safe_release_versions() {
        assert!(valid_version("1.2.3"));
        assert!(valid_version("1.2.3-alpha.1"));
        assert!(!valid_version("latest"));
        assert!(!valid_version("1.2.3/../../bad"));
        assert!(!valid_version("1.2"));
    }

    #[test]
    fn backend_presence_is_not_mistaken_for_login() {
        assert!(!credentials_json_has_login(br#"{}"#));
        assert!(!credentials_json_has_login(
            br#"{"https://auth.x.ai::client":{"key":""}}"#
        ));
        assert!(credentials_json_has_login(
            br#"{"https://auth.x.ai::client":{"key":"token"}}"#
        ));
    }

    #[tokio::test]
    #[ignore = "downloads the official ~160 MB Grok Build provider backend"]
    async fn provisions_working_backend_without_system_cli() {
        let path = ensure_cli().await.expect("managed backend should download");
        assert!(path.is_file());
        let status = std::process::Command::new(path)
            .arg("--version")
            .status()
            .expect("managed backend should launch");
        assert!(status.success());
    }
}
