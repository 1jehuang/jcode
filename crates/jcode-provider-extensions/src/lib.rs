//! Upgrade-safe external provider manifests and local registry.
//!
//! This crate deliberately owns metadata and registration only. Providers are
//! executed by `jcode-provider-subprocess`, which keeps executable lifecycle,
//! deadlines, cancellation, and frame limits outside the persistence layer.

use jcode_provider_protocol::{Frame, PROTOCOL_VERSION};
use jcode_provider_subprocess::{AdapterError, Handshake, SubprocessProvider};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MANIFEST_VERSION: u32 = 1;
pub const REGISTRY_VERSION: u32 = 1;
pub const DEFAULT_REGISTRY_FILE: &str = "providers.json";

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("invalid provider manifest: {0}")]
    InvalidManifest(String),
    #[error("provider '{0}' is already registered")]
    DuplicateProvider(String),
    #[error("provider '{0}' is not registered")]
    MissingProvider(String),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse manifest {path}: {source}")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to encode provider registry: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("failed to resolve provider registry path: {0}")]
    Path(String),
    #[error("provider '{0}' is disabled")]
    DisabledProvider(String),
    #[error("provider '{0}' is not trusted")]
    UntrustedProvider(String),
    #[error("provider process failed: {0}")]
    Process(#[from] AdapterError),
}

/// A registered provider process with its completed protocol handshake.
///
/// The manifest and registry remain independent from process state. Dropping
/// this value drops the bounded subprocess adapter, which terminates the child
/// process instead of leaving an orphan behind.
pub struct ExternalProviderProcess {
    manifest: ProviderManifest,
    transport: SubprocessProvider,
    handshake: Handshake,
}

impl ExternalProviderProcess {
    pub async fn start(record: &ProviderRecord, client: &str) -> Result<Self, ExtensionError> {
        if !record.enabled {
            return Err(ExtensionError::DisabledProvider(record.manifest.id.clone()));
        }
        if !record.trusted {
            return Err(ExtensionError::UntrustedProvider(
                record.manifest.id.clone(),
            ));
        }
        record.manifest.validate()?;
        let transport = SubprocessProvider::spawn(
            &record.manifest.executable,
            &record.manifest.args,
            client,
            record.manifest.capabilities.clone(),
        )
        .await?;
        let handshake = transport.handshake().await?;
        Ok(Self {
            manifest: record.manifest.clone(),
            transport,
            handshake,
        })
    }

    pub fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    pub fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    pub async fn request_and_collect(
        &self,
        id: impl Into<String>,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Result<Vec<Frame>, ExtensionError> {
        Ok(self
            .transport
            .request_and_collect(id, method, params)
            .await?)
    }

    pub async fn cancel(&self, request_id: impl Into<String>) -> Result<(), ExtensionError> {
        Ok(self.transport.cancel(request_id).await?)
    }

    pub async fn kill(&self) -> Result<(), ExtensionError> {
        Ok(self.transport.kill().await?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderManifest {
    #[serde(default = "default_manifest_version")]
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

fn default_manifest_version() -> u32 {
    MANIFEST_VERSION
}

fn default_protocol_version() -> String {
    PROTOCOL_VERSION.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Network,
    Filesystem,
    Environment,
    Subprocess,
    NativeTools,
}

impl ProviderManifest {
    pub fn from_toml(contents: &str, path: impl Into<PathBuf>) -> Result<Self, ExtensionError> {
        let path = path.into();
        let manifest: Self =
            toml::from_str(contents).map_err(|source| ExtensionError::ParseToml {
                path: path.clone(),
                source,
            })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ExtensionError> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(ExtensionError::InvalidManifest(format!(
                "unsupported manifest_version {}, expected {}",
                self.manifest_version, MANIFEST_VERSION
            )));
        }
        validate_identifier("id", &self.id)?;
        validate_non_empty("name", &self.name)?;
        validate_non_empty("version", &self.version)?;
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ExtensionError::InvalidManifest(format!(
                "unsupported protocol_version '{}', expected '{}'",
                self.protocol_version, PROTOCOL_VERSION
            )));
        }
        validate_executable(&self.executable)?;
        for (kind, values) in [("capability", &self.capabilities), ("model", &self.models)] {
            for value in values {
                validate_non_empty(kind, value)?;
            }
        }
        Ok(())
    }
}

fn validate_identifier(field: &str, value: &str) -> Result<(), ExtensionError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && matches!(ch, '-' | '_'))
        });
    if valid {
        Ok(())
    } else {
        Err(ExtensionError::InvalidManifest(format!(
            "{field} must match [a-z0-9][a-z0-9_-]{{0,63}}"
        )))
    }
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), ExtensionError> {
    if value.trim().is_empty() || value.contains(['\0', '\n', '\r']) {
        return Err(ExtensionError::InvalidManifest(format!(
            "{field} must be non-empty and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_executable(path: &Path) -> Result<(), ExtensionError> {
    if path.as_os_str().is_empty() {
        return Err(ExtensionError::InvalidManifest(
            "executable must not be empty".to_string(),
        ));
    }
    if path.to_string_lossy().contains('\0') {
        return Err(ExtensionError::InvalidManifest(
            "executable must not contain NUL".to_string(),
        ));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(ExtensionError::InvalidManifest(
            "executable must not contain '..' path components".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderRecord {
    pub manifest: ProviderManifest,
    #[serde(default)]
    pub source: Option<PathBuf>,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "unix_timestamp")]
    pub registered_at: u64,
}

fn default_enabled() -> bool {
    true
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RegistryFile {
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, ProviderRecord>,
}

#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    path: PathBuf,
    providers: BTreeMap<String, ProviderRecord>,
}

impl ProviderRegistry {
    pub fn default_path() -> Result<PathBuf, ExtensionError> {
        if let Some(home) = std::env::var_os("JCODE_HOME") {
            return Ok(PathBuf::from(home)
                .join("config")
                .join("jcode")
                .join(DEFAULT_REGISTRY_FILE));
        }
        let config = dirs::config_dir()
            .ok_or_else(|| ExtensionError::Path("no platform config directory".to_string()))?;
        Ok(config.join("jcode").join(DEFAULT_REGISTRY_FILE))
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ExtensionError> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self {
                path,
                providers: BTreeMap::new(),
            });
        }
        let bytes = fs::read(&path).map_err(|source| ExtensionError::Read {
            path: path.clone(),
            source,
        })?;
        let file: RegistryFile =
            serde_json::from_slice(&bytes).map_err(|source| ExtensionError::Decode {
                path: path.clone(),
                source,
            })?;
        if file.version != REGISTRY_VERSION {
            return Err(ExtensionError::InvalidManifest(format!(
                "unsupported registry version {}, expected {}",
                file.version, REGISTRY_VERSION
            )));
        }
        for record in file.providers.values() {
            record.manifest.validate()?;
        }
        Ok(Self {
            path,
            providers: file.providers,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> impl Iterator<Item = &ProviderRecord> {
        self.providers.values()
    }

    pub fn get(&self, id: &str) -> Option<&ProviderRecord> {
        self.providers.get(id)
    }

    pub fn register(
        &mut self,
        manifest: ProviderManifest,
        source: Option<PathBuf>,
        trusted: bool,
    ) -> Result<(), ExtensionError> {
        manifest.validate()?;
        if self.providers.contains_key(&manifest.id) {
            return Err(ExtensionError::DuplicateProvider(manifest.id));
        }
        let id = manifest.id.clone();
        self.providers.insert(
            id,
            ProviderRecord {
                manifest,
                source,
                trusted,
                enabled: true,
                registered_at: unix_timestamp(),
            },
        );
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<ProviderRecord, ExtensionError> {
        self.providers
            .remove(id)
            .ok_or_else(|| ExtensionError::MissingProvider(id.to_string()))
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), ExtensionError> {
        let record = self
            .providers
            .get_mut(id)
            .ok_or_else(|| ExtensionError::MissingProvider(id.to_string()))?;
        record.enabled = enabled;
        Ok(())
    }

    pub fn save(&self) -> Result<(), ExtensionError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| ExtensionError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec_pretty(&RegistryFile {
            version: REGISTRY_VERSION,
            providers: self.providers.clone(),
        })?;
        let mut encoded = bytes;
        encoded.push(b'\n');

        let temp = self
            .path
            .with_extension(format!("json.tmp.{}", std::process::id()));
        fs::write(&temp, encoded).map_err(|source| ExtensionError::Write {
            path: temp.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&temp, &self.path) {
            let _ = fs::remove_file(&temp);
            return Err(ExtensionError::Write {
                path: self.path.clone(),
                source,
            });
        }
        Ok(())
    }
}

pub fn load_manifest_file(path: impl AsRef<Path>) -> Result<ProviderManifest, ExtensionError> {
    let path = path.as_ref().to_path_buf();
    let contents = fs::read_to_string(&path).map_err(|source| ExtensionError::Read {
        path: path.clone(),
        source,
    })?;
    ProviderManifest::from_toml(&contents, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manifest(id: &str) -> ProviderManifest {
        ProviderManifest {
            manifest_version: MANIFEST_VERSION,
            id: id.to_string(),
            name: "Fixture Provider".to_string(),
            version: "1.2.3".to_string(),
            executable: PathBuf::from("fixture-provider"),
            args: vec!["--stdio".to_string()],
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: vec!["streaming".to_string()],
            models: vec!["fixture-model".to_string()],
            permissions: vec![Permission::Network],
        }
    }

    #[test]
    fn manifest_toml_roundtrip_and_validation() {
        let input = r#"
manifest_version = 1
id = "fixture-provider"
name = "Fixture Provider"
version = "1.2.3"
executable = "fixture-provider"
args = ["--stdio"]
protocol_version = "0.1"
capabilities = ["streaming"]
models = ["fixture-model"]
permissions = ["network"]
"#;
        let parsed = ProviderManifest::from_toml(input, "provider.toml").unwrap();
        assert_eq!(parsed, manifest("fixture-provider"));
    }

    #[test]
    fn unsafe_manifest_values_are_rejected() {
        let mut invalid = manifest("Bad-ID");
        assert!(invalid.validate().is_err());
        invalid = manifest("valid");
        invalid.executable = PathBuf::from("../provider");
        assert!(invalid.validate().is_err());
        invalid = manifest("valid");
        invalid.protocol_version = "9.0".to_string();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn registry_roundtrip_duplicate_and_lifecycle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let mut registry = ProviderRegistry::open(&path).unwrap();
        registry
            .register(manifest("fixture-provider"), None, true)
            .unwrap();
        assert!(matches!(
            registry.register(manifest("fixture-provider"), None, true),
            Err(ExtensionError::DuplicateProvider(_))
        ));
        registry.set_enabled("fixture-provider", false).unwrap();
        registry.save().unwrap();

        let mut loaded = ProviderRegistry::open(&path).unwrap();
        assert!(!loaded.get("fixture-provider").unwrap().enabled);
        loaded.remove("fixture-provider").unwrap();
        assert_eq!(loaded.list().count(), 0);
    }

    #[test]
    fn missing_registry_starts_empty() {
        let dir = tempdir().unwrap();
        let registry = ProviderRegistry::open(dir.path().join("missing.json")).unwrap();
        assert_eq!(registry.list().count(), 0);
    }

    #[tokio::test]
    async fn trusted_enabled_record_starts_and_collects_a_request() {
        let script = concat!(
            "import sys,json\n",
            "for line in sys.stdin:\n",
            " f=json.loads(line)\n",
            " if f['kind']=='hello':\n",
            "  print(json.dumps({'kind':'hello_ok','protocol_version':'0.1','provider':{'id':'fixture','name':'Fixture','version':'1'},'capabilities':['streaming']}),flush=True)\n",
            " elif f['kind']=='request':\n",
            "  print(json.dumps({'kind':'response','protocol_version':'0.1','id':f['id'],'ok':True,'result':{'text':'ok'}}),flush=True)\n",
        );
        let mut provider = manifest("fixture");
        provider.executable = PathBuf::from("python3");
        provider.args = vec!["-c".to_string(), script.to_string()];
        let record = ProviderRecord {
            manifest: provider,
            source: None,
            trusted: true,
            enabled: true,
            registered_at: 0,
        };
        let process = ExternalProviderProcess::start(&record, "jcode-test")
            .await
            .unwrap();
        assert_eq!(process.handshake().provider.id, "fixture");
        let frames = process
            .request_and_collect("request-1", "complete", serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(
            frames.last(),
            Some(Frame::Response { id, ok: true, .. }) if id == "request-1"
        ));
        process.kill().await.unwrap();
    }
}
