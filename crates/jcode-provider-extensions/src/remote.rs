use super::{PluginBundle, ProviderManifest};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REF_LEN: usize = 256;
const MAX_GIT_OUTPUT: usize = 64 * 1024;
const INSTALL_METADATA_FILE: &str = "install.json";
const CURRENT_FILE: &str = "current";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSource {
    pub repository: String,
    pub reference: Option<String>,
}

impl PluginSource {
    pub fn parse(input: &str) -> Result<Self, RemotePluginError> {
        let input = input.trim();
        if input.is_empty() || input.chars().any(|ch| ch == '\0' || ch.is_whitespace()) {
            return Err(RemotePluginError::InvalidSource(
                "source must be a non-empty GitHub repository without whitespace".to_string(),
            ));
        }

        let (repository, reference) = split_reference(input);
        let repository = if let Some(path) = repository.strip_prefix("https://github.com/") {
            format!("https://github.com/{}", normalize_repository(path)?)
        } else if repository.starts_with("http://")
            || repository.starts_with("ssh://")
            || repository.starts_with("git@")
            || repository.starts_with("file:")
        {
            return Err(RemotePluginError::InvalidSource(
                "only HTTPS GitHub repositories are allowed".to_string(),
            ));
        } else {
            format!(
                "https://github.com/{}.git",
                normalize_repository(repository)?
            )
        };

        if let Some(reference) = reference.as_deref() {
            validate_reference(reference)?;
        }
        Ok(Self {
            repository,
            reference,
        })
    }

    pub fn display(&self) -> String {
        match &self.reference {
            Some(reference) => format!("{}@{}", self.repository, reference),
            None => self.repository.clone(),
        }
    }
}

fn split_reference(input: &str) -> (&str, Option<String>) {
    let Some(index) = input.rfind('@') else {
        return (input, None);
    };
    let (repository, reference) = input.split_at(index);
    if repository.contains('/') && !reference[1..].is_empty() {
        (repository, Some(reference[1..].to_string()))
    } else {
        (input, None)
    }
}

fn normalize_repository(repository: &str) -> Result<String, RemotePluginError> {
    let repository = repository.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if parts.next().is_some() || !valid_segment(owner) || !valid_segment(name) {
        return Err(RemotePluginError::InvalidSource(
            "expected owner/repository or https://github.com/owner/repository".to_string(),
        ));
    }
    Ok(format!("{owner}/{name}"))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn validate_reference(reference: &str) -> Result<(), RemotePluginError> {
    if reference.is_empty()
        || reference.len() > MAX_REF_LEN
        || reference.starts_with('-')
        || reference.contains(['\0', '\n', '\r', ' ', '\t'])
        || reference.contains("..")
        || reference.contains("@{")
    {
        return Err(RemotePluginError::InvalidSource(
            "invalid Git reference".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInstallMetadata {
    pub name: String,
    pub version: String,
    pub repository: String,
    pub reference: Option<String>,
    pub commit: String,
    pub installed_at: u64,
    pub path: PathBuf,
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub bundle: PluginBundle,
    pub metadata: PluginInstallMetadata,
    previous: PluginStoreSnapshot,
}

#[derive(Debug, Clone)]
pub struct PluginStoreSnapshot {
    name: String,
    metadata: Option<PluginInstallMetadata>,
    current: Option<Vec<u8>>,
}

impl InstalledPlugin {
    pub fn previous_provider_id(&self) -> Option<&str> {
        self.previous
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.provider_id.as_deref())
    }

    pub fn provider_manifest(&self) -> Result<Option<ProviderManifest>, RemotePluginError> {
        let Some(mut manifest) = self.bundle.provider_manifest.clone() else {
            return Ok(None);
        };
        let root = self
            .bundle
            .root
            .canonicalize()
            .map_err(|source| RemotePluginError::Io {
                path: self.bundle.root.clone(),
                source,
            })?;
        if manifest.executable.is_relative() {
            manifest.executable = root.join(manifest.executable);
        }
        let executable =
            manifest
                .executable
                .canonicalize()
                .map_err(|source| RemotePluginError::Io {
                    path: manifest.executable.clone(),
                    source,
                })?;
        if !executable.starts_with(&root) {
            return Err(RemotePluginError::Bundle(format!(
                "provider executable escapes the installed plugin root: {}",
                executable.display()
            )));
        }
        manifest.executable = executable;
        Ok(Some(manifest))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RemotePluginError {
    #[error("invalid plugin source: {0}")]
    InvalidSource(String),
    #[error("plugin source is not available: {0}")]
    Unavailable(String),
    #[error("git operation timed out after {0} seconds")]
    Timeout(u64),
    #[error("git operation failed: {0}")]
    Git(String),
    #[error("plugin filesystem operation failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("plugin bundle validation failed: {0}")]
    Bundle(String),
    #[error("plugin metadata encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin '{0}' is already installed at this version")]
    AlreadyInstalled(String),
    #[error("installed plugin name mismatch: expected '{expected}', got '{actual}'")]
    NameMismatch { expected: String, actual: String },
    #[error("plugin installation rollback failed: {0}")]
    Rollback(String),
}

#[derive(Debug, Clone)]
pub struct PluginStore {
    root: PathBuf,
}

impl PluginStore {
    pub fn default_root() -> Result<PathBuf, RemotePluginError> {
        if let Some(home) = std::env::var_os("JCODE_HOME") {
            return Ok(PathBuf::from(home).join("plugins"));
        }
        dirs::data_dir()
            .map(|path| path.join("jcode").join("plugins"))
            .ok_or_else(|| RemotePluginError::Unavailable("no platform data directory".to_string()))
    }

    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn inspect(&self, source: &PluginSource) -> Result<PluginBundle, RemotePluginError> {
        let staging = self.prepare_staging("inspect")?;
        let result = self.clone_and_load(source, &staging);
        let _ = fs::remove_dir_all(&staging);
        result
    }

    pub fn install(&self, source: &PluginSource) -> Result<InstalledPlugin, RemotePluginError> {
        self.install_inner(source, None)
    }

    fn install_inner(
        &self,
        source: &PluginSource,
        expected_name: Option<&str>,
    ) -> Result<InstalledPlugin, RemotePluginError> {
        let staging = self.prepare_staging("install")?;
        let bundle = match self.clone_and_load(source, &staging) {
            Ok(bundle) => bundle,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        if let Some(expected_name) = expected_name {
            if bundle.manifest.name != expected_name {
                let _ = fs::remove_dir_all(&staging);
                return Err(RemotePluginError::NameMismatch {
                    expected: expected_name.to_string(),
                    actual: bundle.manifest.name,
                });
            }
        }
        let previous = match self.snapshot(&bundle.manifest.name) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let commit = match git_output(&staging, &["rev-parse", "HEAD"]) {
            Ok(commit) => commit,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let commit = commit.trim().to_string();
        if commit.len() != 40 || !commit.chars().all(|ch| ch.is_ascii_hexdigit()) {
            let _ = fs::remove_dir_all(&staging);
            return Err(RemotePluginError::Git(
                "git returned an invalid commit".to_string(),
            ));
        }

        let plugin_root = self.root.join(&bundle.manifest.name);
        let version_name = format!(
            "{}+{}",
            safe_segment(&bundle.manifest.version),
            &commit[..12]
        );
        let versions = plugin_root.join("versions");
        let final_path = versions.join(version_name);
        if final_path.exists() {
            let _ = fs::remove_dir_all(&staging);
            return Err(RemotePluginError::AlreadyInstalled(
                bundle.manifest.name.clone(),
            ));
        }
        fs::create_dir_all(&versions).map_err(|source| RemotePluginError::Io {
            path: versions.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&staging, &final_path) {
            let _ = fs::remove_dir_all(&staging);
            return Err(RemotePluginError::Io {
                path: final_path.clone(),
                source,
            });
        }

        let installed_bundle = match PluginBundle::load(&final_path) {
            Ok(bundle) => bundle,
            Err(error) => {
                let _ = fs::remove_dir_all(&final_path);
                return Err(RemotePluginError::Bundle(error.to_string()));
            }
        };
        let metadata = PluginInstallMetadata {
            name: installed_bundle.manifest.name.clone(),
            version: installed_bundle.manifest.version.clone(),
            repository: source.repository.clone(),
            reference: source.reference.clone(),
            commit,
            installed_at: unix_timestamp(),
            path: final_path.clone(),
            provider_id: installed_bundle
                .provider_manifest
                .as_ref()
                .map(|manifest| manifest.id.clone()),
        };
        if let Err(error) = self.write_metadata(&plugin_root, &metadata) {
            let _ = fs::remove_dir_all(&final_path);
            return Err(error);
        }
        if let Err(error) = self.atomic_write(
            &plugin_root.join(CURRENT_FILE),
            final_path.to_string_lossy().as_bytes(),
        ) {
            let _ = fs::remove_dir_all(&final_path);
            if let Some(previous_metadata) = &previous.metadata {
                let _ = self.write_metadata(&plugin_root, previous_metadata);
            } else {
                let _ = fs::remove_file(plugin_root.join(INSTALL_METADATA_FILE));
            }
            if let Some(previous_current) = &previous.current {
                let _ = self.atomic_write(&plugin_root.join(CURRENT_FILE), previous_current);
            } else {
                let _ = fs::remove_file(plugin_root.join(CURRENT_FILE));
            }
            return Err(error);
        }
        Ok(InstalledPlugin {
            bundle: installed_bundle,
            metadata,
            previous,
        })
    }

    /// Fetch and install the current revision for an installed plugin's source.
    /// The previous version remains available until the new version is fully
    /// validated and selected as current.
    pub fn update(&self, name: &str) -> Result<InstalledPlugin, RemotePluginError> {
        if !valid_segment(name) {
            return Err(RemotePluginError::InvalidSource(
                "invalid plugin name".to_string(),
            ));
        }
        let previous = self
            .list()?
            .into_iter()
            .find(|plugin| plugin.name == name)
            .ok_or_else(|| {
                RemotePluginError::Unavailable(format!("plugin '{name}' is not installed"))
            })?;
        let source = PluginSource::parse(
            &PluginSource {
                repository: previous.repository,
                reference: previous.reference,
            }
            .display(),
        )?;
        self.install_inner(&source, Some(name))
    }

    pub fn snapshot(&self, name: &str) -> Result<PluginStoreSnapshot, RemotePluginError> {
        if !valid_segment(name) {
            return Err(RemotePluginError::InvalidSource(
                "invalid plugin name".to_string(),
            ));
        }
        let metadata = self.list()?.into_iter().find(|plugin| plugin.name == name);
        let current_path = self.root.join(name).join(CURRENT_FILE);
        let current = if current_path.is_file() {
            Some(
                fs::read(&current_path).map_err(|source| RemotePluginError::Io {
                    path: current_path.clone(),
                    source,
                })?,
            )
        } else {
            None
        };
        Ok(PluginStoreSnapshot {
            name: name.to_string(),
            metadata,
            current,
        })
    }

    pub fn rollback(&self, installed: &InstalledPlugin) -> Result<(), RemotePluginError> {
        let snapshot = &installed.previous;
        let plugin_root = self.root.join(&snapshot.name);
        let mut failures = Vec::new();
        if installed.metadata.path.exists() {
            if let Err(error) = fs::remove_dir_all(&installed.metadata.path) {
                failures.push(error.to_string());
            }
        }
        let metadata_path = plugin_root.join(INSTALL_METADATA_FILE);
        match &snapshot.metadata {
            Some(metadata) => {
                if let Err(error) = self.write_metadata(&plugin_root, metadata) {
                    failures.push(error.to_string());
                }
            }
            None => {
                if let Err(error) = fs::remove_file(&metadata_path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        failures.push(error.to_string());
                    }
                }
            }
        }
        let current_path = plugin_root.join(CURRENT_FILE);
        match &snapshot.current {
            Some(current) => {
                if let Err(error) = self.atomic_write(&current_path, current) {
                    failures.push(error.to_string());
                }
            }
            None => {
                if let Err(error) = fs::remove_file(&current_path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        failures.push(error.to_string());
                    }
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(RemotePluginError::Rollback(failures.join("; ")))
        }
    }

    pub fn list(&self) -> Result<Vec<PluginInstallMetadata>, RemotePluginError> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut plugins = Vec::new();
        let entries = fs::read_dir(&self.root).map_err(|source| RemotePluginError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| RemotePluginError::Io {
                path: self.root.clone(),
                source,
            })?;
            let file_type = entry.file_type().map_err(|source| RemotePluginError::Io {
                path: entry.path(),
                source,
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let plugin_dir = entry.path();
            let path = plugin_dir.join(INSTALL_METADATA_FILE);
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| RemotePluginError::Io {
                path: path.clone(),
                source,
            })?;
            let metadata: PluginInstallMetadata = serde_json::from_slice(&bytes)?;
            if metadata.name != entry.file_name().to_string_lossy()
                || !metadata.path.starts_with(&plugin_dir)
            {
                continue;
            }
            plugins.push(metadata);
        }
        plugins.sort_by(|left: &PluginInstallMetadata, right| left.name.cmp(&right.name));
        Ok(plugins)
    }

    pub fn remove(&self, name: &str) -> Result<(), RemotePluginError> {
        if !valid_segment(name) {
            return Err(RemotePluginError::InvalidSource(
                "invalid plugin name".to_string(),
            ));
        }
        let path = self.root.join(name);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|source| RemotePluginError::Io { path, source })?;
        }
        Ok(())
    }

    fn prepare_staging(&self, label: &str) -> Result<PathBuf, RemotePluginError> {
        let staging_root = self.root.join(".staging");
        fs::create_dir_all(&staging_root).map_err(|source| RemotePluginError::Io {
            path: staging_root.clone(),
            source,
        })?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = staging_root.join(format!("{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).map_err(|source| RemotePluginError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    fn clone_and_load(
        &self,
        source: &PluginSource,
        destination: &Path,
    ) -> Result<PluginBundle, RemotePluginError> {
        let mut args = vec![
            "clone".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "--no-tags".to_string(),
            "--single-branch".to_string(),
        ];
        if let Some(reference) = &source.reference {
            args.push("--branch".to_string());
            args.push(reference.clone());
        }
        args.push(source.repository.clone());
        args.push(destination.to_string_lossy().into_owned());
        run_git(&args, None)?;
        PluginBundle::load(destination)
            .map_err(|error| RemotePluginError::Bundle(error.to_string()))
    }

    fn write_metadata(
        &self,
        plugin_root: &Path,
        metadata: &PluginInstallMetadata,
    ) -> Result<(), RemotePluginError> {
        let bytes = serde_json::to_vec_pretty(metadata)?;
        self.atomic_write(&plugin_root.join(INSTALL_METADATA_FILE), &bytes)
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), RemotePluginError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RemotePluginError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let temp = path.with_extension(format!("tmp.{}", std::process::id()));
        fs::write(&temp, bytes).map_err(|source| RemotePluginError::Io {
            path: temp.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(RemotePluginError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(())
    }
}

fn run_git(args: &[String], current_dir: Option<&Path>) -> Result<(), RemotePluginError> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| RemotePluginError::Git(error.to_string()))?;
    wait_for_child(&mut child, args).map(|_| ())
}

fn git_output(current_dir: &Path, args: &[&str]) -> Result<String, RemotePluginError> {
    let mut command = Command::new("git");
    command
        .args(["-C"])
        .arg(current_dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| RemotePluginError::Git(error.to_string()))?;
    let status = wait_for_child(&mut child, args)?;
    if !status.success() {
        return Err(RemotePluginError::Git(format!(
            "git {} exited with {status}",
            args.join(" ")
        )));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| RemotePluginError::Git(error.to_string()))?;
    if output.stdout.len() > MAX_GIT_OUTPUT {
        return Err(RemotePluginError::Git(
            "git output exceeded the limit".to_string(),
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| RemotePluginError::Git("git output was not UTF-8".to_string()))
}

fn wait_for_child(
    child: &mut std::process::Child,
    args: &[impl AsRef<str>],
) -> Result<std::process::ExitStatus, RemotePluginError> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(RemotePluginError::Git(format!(
                        "git {} exited with {status}",
                        args.iter()
                            .map(|arg| arg.as_ref())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )));
                }
                return Ok(status);
            }
            Ok(None) if started.elapsed() >= GIT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RemotePluginError::Timeout(GIT_TIMEOUT.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(RemotePluginError::Git(error.to_string())),
        }
    }
}

fn safe_segment(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    result.truncate(80);
    if result.is_empty() {
        "version".to_string()
    } else {
        result
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_github_shorthand_and_pinned_reference() {
        let source = PluginSource::parse("owner/repo@v1.2.3").unwrap();
        assert_eq!(source.repository, "https://github.com/owner/repo.git");
        assert_eq!(source.reference.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn rejects_unsafe_sources_and_references() {
        assert!(PluginSource::parse("file:///tmp/plugin").is_err());
        assert!(PluginSource::parse("owner/repo@../main").is_err());
        assert!(PluginSource::parse("owner/repo --upload-pack=evil").is_err());
    }

    #[test]
    fn rejects_provider_executable_outside_installed_plugin_root() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let executable = outside.path().join("provider");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::write(
            root.path().join("plugin.json"),
            r#"{"name":"path-plugin","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("provider.toml"),
            format!(
                "manifest_version = 1\nid = 'path-provider'\nname = 'Path Provider'\nversion = '1.0.0'\nexecutable = '{}'\nprotocol_version = '0.1'\n",
                executable.display()
            ),
        )
        .unwrap();
        let bundle = PluginBundle::load(root.path()).unwrap();
        let installed = InstalledPlugin {
            bundle,
            metadata: PluginInstallMetadata {
                name: "path-plugin".to_string(),
                version: "1.0.0".to_string(),
                repository: "https://github.com/owner/repo.git".to_string(),
                reference: None,
                commit: "a".repeat(40),
                installed_at: 0,
                path: root.path().to_path_buf(),
                provider_id: Some("path-provider".to_string()),
            },
            previous: PluginStoreSnapshot {
                name: "path-plugin".to_string(),
                metadata: None,
                current: None,
            },
        };
        assert!(matches!(
            installed.provider_manifest(),
            Err(RemotePluginError::Bundle(message)) if message.contains("escapes")
        ));
    }
}
