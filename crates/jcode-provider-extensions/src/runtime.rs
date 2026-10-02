use super::{ExtensionError, ExternalProviderProcess};
use async_trait::async_trait;
use jcode_provider_protocol::{Frame, WireError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Request shape shared by embedded and external extensions.
///
/// Embedded extensions receive the already-decoded value directly. External
/// extensions receive the same value through the versioned provider protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionRequest {
    pub id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionEvent {
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionInvocation {
    pub id: String,
    #[serde(default)]
    pub events: Vec<ExtensionEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl ExtensionInvocation {
    pub fn response(id: impl Into<String>, result: Value) -> Self {
        Self {
            id: id.into(),
            events: Vec::new(),
            result: Some(result),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddedExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl EmbeddedExtensionManifest {
    pub fn validate(&self) -> Result<(), ExtensionError> {
        validate_extension_id(&self.id)?;
        for (field, value) in [
            ("name", self.name.as_str()),
            ("version", self.version.as_str()),
        ] {
            if value.trim().is_empty() || value.contains(['\0', '\n', '\r']) {
                return Err(ExtensionError::InvalidManifest(format!(
                    "embedded extension {field} must be non-empty and contain no control characters"
                )));
            }
        }
        Ok(())
    }
}

fn validate_extension_id(id: &str) -> Result<(), ExtensionError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && matches!(ch, '-' | '_'))
        });
    if valid {
        Ok(())
    } else {
        Err(ExtensionError::InvalidManifest(
            "extension id must match [a-z0-9][a-z0-9_-]{0,63}".to_string(),
        ))
    }
}

/// Stable source-level API for statically linked Rust extensions.
///
/// This trait is intentionally not an ABI. Extensions are compiled into the
/// same Jcode binary and registered directly, which gives the embedded tier its
/// low-overhead path without promising binary compatibility between builds.
#[async_trait]
pub trait EmbeddedExtension: Send + Sync {
    fn manifest(&self) -> &EmbeddedExtensionManifest;

    async fn invoke(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionInvocation, ExtensionError>;

    async fn cancel(&self, request_id: &str) -> Result<(), ExtensionError> {
        Err(ExtensionError::CancellationUnsupported(format!(
            "{} ({request_id})",
            self.manifest().id
        )))
    }
}

pub enum ExtensionBackend {
    Embedded(Arc<dyn EmbeddedExtension>),
    External(ExternalProviderProcess),
}

impl ExtensionBackend {
    async fn invoke(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionInvocation, ExtensionError> {
        match self {
            Self::Embedded(extension) => extension.invoke(request).await,
            Self::External(process) => process.invoke(request).await,
        }
    }

    async fn cancel(&self, request_id: &str) -> Result<(), ExtensionError> {
        match self {
            Self::Embedded(extension) => extension.cancel(request_id).await,
            Self::External(process) => process.cancel(request_id).await,
        }
    }
}

/// Runtime registry for both tiers. Embedded extensions are usually populated
/// by a compile-time Cargo feature, while external extensions are inserted after
/// an explicit trust and permission check.
pub struct ExtensionRuntimeRegistry {
    backends: BTreeMap<String, ExtensionBackend>,
}

impl Default for ExtensionRuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRuntimeRegistry {
    pub fn new() -> Self {
        Self {
            backends: BTreeMap::new(),
        }
    }

    pub fn register_embedded<E>(&mut self, extension: E) -> Result<(), ExtensionError>
    where
        E: EmbeddedExtension + 'static,
    {
        self.register_embedded_arc(Arc::new(extension))
    }

    pub fn register_embedded_arc(
        &mut self,
        extension: Arc<dyn EmbeddedExtension>,
    ) -> Result<(), ExtensionError> {
        let manifest = extension.manifest();
        manifest.validate()?;
        if self.backends.contains_key(&manifest.id) {
            return Err(ExtensionError::DuplicateEmbeddedExtension(
                manifest.id.clone(),
            ));
        }
        self.backends
            .insert(manifest.id.clone(), ExtensionBackend::Embedded(extension));
        Ok(())
    }

    pub fn register_external(
        &mut self,
        id: impl Into<String>,
        process: ExternalProviderProcess,
    ) -> Result<(), ExtensionError> {
        let id = id.into();
        validate_extension_id(&id)?;
        if self.backends.contains_key(&id) {
            return Err(ExtensionError::DuplicateEmbeddedExtension(id));
        }
        self.backends
            .insert(id, ExtensionBackend::External(process));
        Ok(())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.backends.contains_key(id)
    }

    pub fn list(&self) -> impl Iterator<Item = &str> {
        self.backends.keys().map(String::as_str)
    }

    pub async fn invoke(
        &self,
        id: &str,
        request: ExtensionRequest,
    ) -> Result<ExtensionInvocation, ExtensionError> {
        self.backends
            .get(id)
            .ok_or_else(|| ExtensionError::MissingExtension(id.to_string()))?
            .invoke(request)
            .await
    }

    pub async fn cancel(&self, id: &str, request_id: &str) -> Result<(), ExtensionError> {
        self.backends
            .get(id)
            .ok_or_else(|| ExtensionError::MissingExtension(id.to_string()))?
            .cancel(request_id)
            .await
    }
}

impl ExternalProviderProcess {
    pub async fn invoke(
        &self,
        request: ExtensionRequest,
    ) -> Result<ExtensionInvocation, ExtensionError> {
        let frames = self
            .request_and_collect(request.id.clone(), request.method, request.params)
            .await?;
        let mut events = Vec::new();
        for frame in frames {
            match frame {
                Frame::Event { event, payload, .. } => {
                    events.push(ExtensionEvent { event, payload })
                }
                Frame::Response { ok, result, .. } if ok => {
                    return Ok(ExtensionInvocation {
                        id: request.id,
                        events,
                        result,
                    });
                }
                Frame::Response {
                    error: Some(error), ..
                } => return Err(request_error(error)),
                other => {
                    return Err(ExtensionError::RequestFailed(format!(
                        "unexpected frame after request: {other:?}"
                    )));
                }
            }
        }
        Err(ExtensionError::RequestFailed(
            "provider ended without a response".to_string(),
        ))
    }
}

fn request_error(error: WireError) -> ExtensionError {
    ExtensionError::RequestFailed(error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct EchoExtension {
        manifest: EmbeddedExtensionManifest,
    }

    #[async_trait]
    impl EmbeddedExtension for EchoExtension {
        fn manifest(&self) -> &EmbeddedExtensionManifest {
            &self.manifest
        }

        async fn invoke(
            &self,
            request: ExtensionRequest,
        ) -> Result<ExtensionInvocation, ExtensionError> {
            Ok(ExtensionInvocation {
                id: request.id,
                events: vec![ExtensionEvent {
                    event: "complete".to_string(),
                    payload: serde_json::json!({"embedded": true}),
                }],
                result: Some(request.params),
            })
        }
    }

    #[tokio::test]
    async fn embedded_registry_invokes_without_wire_serialization() {
        let mut registry = ExtensionRuntimeRegistry::new();
        registry
            .register_embedded(EchoExtension {
                manifest: EmbeddedExtensionManifest {
                    id: "echo".to_string(),
                    name: "Embedded Echo".to_string(),
                    version: "1.0.0".to_string(),
                    capabilities: vec!["streaming".to_string()],
                },
            })
            .unwrap();
        let result = registry
            .invoke(
                "echo",
                ExtensionRequest {
                    id: "request-1".to_string(),
                    method: "complete".to_string(),
                    params: serde_json::json!({"value": 7}),
                },
            )
            .await
            .unwrap();
        assert_eq!(result.id, "request-1");
        assert_eq!(result.events[0].event, "complete");
        assert_eq!(result.result, Some(serde_json::json!({"value": 7})));
    }

    #[test]
    fn registry_rejects_duplicate_and_invalid_embedded_ids() {
        let mut registry = ExtensionRuntimeRegistry::new();
        let extension = EchoExtension {
            manifest: EmbeddedExtensionManifest {
                id: "echo".to_string(),
                name: "Embedded Echo".to_string(),
                version: "1.0.0".to_string(),
                capabilities: Vec::new(),
            },
        };
        registry.register_embedded(extension).unwrap();
        let duplicate = EchoExtension {
            manifest: EmbeddedExtensionManifest {
                id: "echo".to_string(),
                name: "Other".to_string(),
                version: "1.0.0".to_string(),
                capabilities: Vec::new(),
            },
        };
        assert!(matches!(
            registry.register_embedded(duplicate),
            Err(ExtensionError::DuplicateEmbeddedExtension(_))
        ));
        assert!(
            EmbeddedExtensionManifest {
                id: "Bad-ID".to_string(),
                name: "Bad".to_string(),
                version: "1.0.0".to_string(),
                capabilities: Vec::new(),
            }
            .validate()
            .is_err()
        );
    }

    #[tokio::test]
    async fn external_backend_maps_wire_frames_to_common_invocation() {
        let script = concat!(
            "import sys,json\n",
            "for line in sys.stdin:\n",
            " f=json.loads(line)\n",
            " if f['kind']=='hello':\n",
            "  print(json.dumps({'kind':'hello_ok','protocol_version':'0.1','provider':{'id':'fixture','name':'Fixture','version':'1'},'capabilities':['streaming']}),flush=True)\n",
            " elif f['kind']=='request':\n",
            "  print(json.dumps({'kind':'event','protocol_version':'0.1','request_id':f['id'],'event':'progress','payload':{'step':1}}),flush=True)\n",
            "  print(json.dumps({'kind':'response','protocol_version':'0.1','id':f['id'],'ok':True,'result':{'text':'ok'}}),flush=True)\n",
        );
        let provider = super::super::ProviderManifest {
            manifest_version: super::super::MANIFEST_VERSION,
            id: "fixture".to_string(),
            name: "Fixture".to_string(),
            version: "1.0.0".to_string(),
            executable: PathBuf::from("python3"),
            args: vec!["-c".to_string(), script.to_string()],
            protocol_version: super::super::PROTOCOL_VERSION.to_string(),
            capabilities: vec!["streaming".to_string()],
            models: Vec::new(),
            permissions: Vec::new(),
        };
        provider.validate().unwrap();
        let process = super::super::ExternalProviderProcess::start(
            &super::super::ProviderRecord {
                manifest: provider,
                source: None,
                trusted: true,
                enabled: true,
                registered_at: 0,
            },
            "jcode-test",
        )
        .await
        .unwrap();
        let mut registry = ExtensionRuntimeRegistry::new();
        registry.register_external("fixture", process).unwrap();
        let result = registry
            .invoke(
                "fixture",
                ExtensionRequest {
                    id: "request-1".to_string(),
                    method: "complete".to_string(),
                    params: serde_json::json!({"prompt":"hello"}),
                },
            )
            .await
            .unwrap();
        assert_eq!(result.id, "request-1");
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].event, "progress");
        assert_eq!(result.result, Some(serde_json::json!({"text":"ok"})));
    }
}
