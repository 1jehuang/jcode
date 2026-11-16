//! Pluggable embedding backends for memory retrieval.
//!
//! Memory dense-retrieval embeds two kinds of text: stored memories (passages)
//! and the current query. Historically jcode had exactly one embedder, the
//! bundled local all-MiniLM-L6-v2 ONNX model, reached directly via
//! [`crate::embedding`]. This module introduces a small abstraction so the
//! embedder can be swapped (e.g. a stronger local model, or a remote provider
//! like OpenAI when the user has an embeddings-capable key) without the rest of
//! the memory system caring which one is active.
//!
//! Design invariants:
//! - **One vector space per index.** Embeddings from different models are not
//!   comparable. Every backend reports a stable [`EmbeddingBackend::model_id`],
//!   which is stored on each `MemoryEntry` (`embedding_model`). Dense similarity
//!   only compares vectors sharing the active model id; mismatched memories stay
//!   reachable via lexical (BM25) search + RRF fusion, so switching backends
//!   never silently corrupts results.
//! - **Asymmetric query/passage formatting is per-model.** Some models (e5/bge)
//!   require instruction prefixes; others (MiniLM, OpenAI) do not. Each backend
//!   owns its own input formatting via [`EmbeddingBackend::format_query`] /
//!   [`EmbeddingBackend::format_passage`], so callers never hardcode prefixes.
//! - **Local is the always-available default.** Remote backends are opt-in and
//!   only selected when an embeddings-capable credential is present.

use anyhow::Result;

use crate::memory_types::LEGACY_EMBEDDING_MODEL;

/// A source of embedding vectors for memory retrieval.
///
/// Implementations must keep `model_id()` stable for a given vector space: it is
/// persisted alongside each embedding and used to gate cross-model comparisons.
pub trait EmbeddingBackend: Send + Sync {
    /// Stable identifier for the model/vector-space this backend produces, e.g.
    /// `"minilm-l6-v2"` or `"openai:text-embedding-3-small"`. Persisted on
    /// `MemoryEntry::embedding_model`.
    fn model_id(&self) -> &str;

    /// Embedding dimensionality (used for sanity checks and index metadata).
    fn dim(&self) -> usize;

    /// Embed a single text already formatted for this backend's role. Prefer
    /// [`Self::embed_query`] / [`Self::embed_passage`] which apply formatting.
    fn embed_raw(&self, text: &str) -> Result<Vec<f32>>;

    /// Apply this model's query-side formatting (e.g. an `"query: "` prefix).
    /// Default: identity (no prefix), correct for MiniLM and OpenAI.
    fn format_query(&self, text: &str) -> String {
        text.to_string()
    }

    /// Apply this model's passage-side formatting (e.g. a `"passage: "` prefix).
    /// Default: identity.
    fn format_passage(&self, text: &str) -> String {
        text.to_string()
    }

    /// Embed a retrieval query (applies query formatting).
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_raw(&self.format_query(text))
    }

    /// Embed a stored passage/memory (applies passage formatting).
    fn embed_passage(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_raw(&self.format_passage(text))
    }

    /// Batch-embed many passages. Backends with a remote API override this to
    /// amortize one HTTP round-trip over many inputs; the default loops over
    /// [`Self::embed_passage`] so local backends need no special-casing.
    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_passage(t)).collect()
    }
}

/// The bundled local ONNX embedder (currently all-MiniLM-L6-v2).
///
/// Wraps the process-wide embedder facade in [`crate::embedding`]. Requires no
/// network, no API key, and is always available, so it is the default backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalOnnxBackend;

impl EmbeddingBackend for LocalOnnxBackend {
    fn model_id(&self) -> &str {
        // Matches MemoryEntry::effective_embedding_model() for untagged legacy
        // memories, so existing embeddings remain comparable with new ones.
        LEGACY_EMBEDDING_MODEL
    }

    fn dim(&self) -> usize {
        crate::embedding::embedding_dim()
    }

    fn embed_raw(&self, text: &str) -> Result<Vec<f32>> {
        crate::embedding::embed(text)
    }

    // MiniLM is symmetric and prefix-free: default identity formatting is correct.
}

/// A remote OpenAI-compatible embeddings backend (`POST /v1/embeddings`).
///
/// Works against OpenAI proper and any OpenAI-compatible gateway that exposes
/// the same schema (the `base_url` + `Authorization: Bearer` contract). The
/// model id is stored as `"openai:<model>"` so its vectors never get compared
/// against local MiniLM vectors (different vector space): see the module
/// invariants. OpenAI embeddings are L2-normalized by the API, so plain cosine
/// over the returned vectors is correct.
#[derive(Debug, Clone)]
pub struct OpenAiEmbeddingBackend {
    /// Stable `"openai:<model>"` identifier persisted on each MemoryEntry.
    model_id: String,
    /// Bare model name sent in the request body (e.g. `text-embedding-3-small`).
    model: String,
    /// API base, no trailing slash, e.g. `https://api.openai.com/v1`.
    base_url: String,
    /// Bearer credential.
    api_key: String,
    /// Reported embedding dimensionality.
    dim: usize,
}

/// Default OpenAI embedding model. 3-small is the cost/quality sweet spot
/// (1536-d, ~5x cheaper than ada-002 with higher MTEB scores).
pub const DEFAULT_OPENAI_EMBEDDING_MODEL: &str = "text-embedding-3-small";
const OPENAI_EMBEDDINGS_BASE: &str = "https://api.openai.com/v1";

/// Native dimensionality for the common OpenAI embedding models.
fn default_openai_dim(model: &str) -> usize {
    match model {
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        "text-embedding-ada-002" => 1536,
        _ => 1536,
    }
}

impl OpenAiEmbeddingBackend {
    /// Construct a backend for `model`, resolving `base_url`/`dim` to sensible
    /// defaults when not supplied.
    pub fn new(
        model: impl Into<String>,
        api_key: impl Into<String>,
        base_url: Option<String>,
        dim: Option<usize>,
    ) -> Self {
        let model = model.into();
        let dim = dim.unwrap_or_else(|| default_openai_dim(&model));
        let base_url = base_url
            .map(|b| b.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| OPENAI_EMBEDDINGS_BASE.to_string());
        Self {
            model_id: format!("openai:{model}"),
            model,
            base_url,
            api_key: api_key.into(),
            dim,
        }
    }

    /// POST a batch of inputs to `/embeddings` and return their vectors in order.
    ///
    /// Runs the blocking HTTP call on a dedicated thread so it is safe to call
    /// from a synchronous context (the bench) AND from inside a tokio runtime
    /// worker (the live memory path) without the nested-runtime panic that
    /// `reqwest::blocking` would otherwise trigger.
    fn embed_inputs(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);
        let api_key = &self.api_key;
        let model = self.model.as_str();
        let want = inputs.len();
        let expected_dim = self.dim;

        let vectors = std::thread::scope(|scope| {
            scope
                .spawn(move || -> Result<Vec<Vec<f32>>> {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(60))
                        .build()?;
                    let request = {
                        let body = EmbeddingsRequest {
                            input: &inputs,
                            model,
                        };
                        client
                            .post(&url)
                            .header("Authorization", format!("Bearer {api_key}"))
                            .header("Content-Type", "application/json")
                            .json(&body)
                    };
                    // RequestBuilder owns the serialized bytes. Release the source
                    // strings before waiting on the network and response body.
                    drop(inputs);
                    let resp = request.send()?;
                    let status = resp.status();
                    let text = resp.text()?;
                    if !status.is_success() {
                        anyhow::bail!(
                            "OpenAI embeddings request failed ({status}): {}",
                            text.chars().take(400).collect::<String>()
                        );
                    }
                    let parsed: EmbeddingsResponse = serde_json::from_str(&text)
                        .map_err(|e| anyhow::anyhow!("parse embeddings response: {e}"))?;
                    let mut data = parsed.data;
                    // The API returns items with an `index` field; sort to be safe.
                    data.sort_by_key(|d| d.index);
                    Ok(data.into_iter().map(|d| d.embedding).collect())
                })
                .join()
                .map_err(|_| anyhow::anyhow!("OpenAI embeddings worker thread panicked"))?
        })?;

        if vectors.len() != want {
            anyhow::bail!(
                "OpenAI embeddings returned {} vectors for {} inputs",
                vectors.len(),
                want
            );
        }
        if let Some(first) = vectors.first()
            && first.len() != expected_dim
        {
            // Not fatal (cosine still works), but log so an index/dim mismatch is
            // visible rather than silently corrupting cross-model gating.
            crate::logging::warn(&format!(
                "OpenAI embedding dim {} != configured {} for model {}",
                first.len(),
                expected_dim,
                self.model
            ));
        }
        Ok(vectors)
    }
}

// Match the legacy serde_json::Value map's sorted key order on the wire.
#[derive(serde::Serialize)]
struct EmbeddingsRequest<'a> {
    input: &'a [String],
    model: &'a str,
}

#[derive(serde::Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingsDatum>,
}

#[derive(serde::Deserialize)]
struct EmbeddingsDatum {
    index: usize,
    embedding: Vec<f32>,
}

impl EmbeddingBackend for OpenAiEmbeddingBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_raw(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed_inputs(vec![text.to_string()])?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("OpenAI embeddings returned no vector"))
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts.iter().map(|t| self.format_passage(t)).collect();
        self.embed_inputs(inputs)
    }

    // OpenAI embeddings are symmetric and prefix-free: identity formatting.
}

/// Resolve the active embedding backend.
///
/// Selects the OpenAI/openai-compatible remote backend when the user has
/// opted in (`agents.memory_embedding_backend = "openai"`) AND an embeddings
/// credential is resolvable; otherwise falls back to the always-available local
/// ONNX backend. Selection is intentionally conservative: a misconfigured or
/// keyless remote setting silently degrades to local rather than failing
/// retrieval.
pub fn active_backend() -> Box<dyn EmbeddingBackend> {
    if let Some(remote) = openai_backend_from_config() {
        return Box::new(remote);
    }
    Box::new(LocalOnnxBackend)
}

/// Build an [`OpenAiEmbeddingBackend`] from config + resolved credentials, or
/// `None` when remote embeddings are not selected/available.
pub fn openai_backend_from_config() -> Option<OpenAiEmbeddingBackend> {
    let agents = &crate::config::config().agents;
    if !agents
        .memory_embedding_backend
        .eq_ignore_ascii_case("openai")
    {
        return None;
    }
    let api_key =
        crate::provider_catalog::load_api_key_from_env_or_config("OPENAI_API_KEY", "openai.env")?;
    let model = agents
        .memory_embedding_model
        .clone()
        .unwrap_or_else(|| DEFAULT_OPENAI_EMBEDDING_MODEL.to_string());
    let base_url = agents.memory_embedding_base_url.clone();
    let dim = agents.memory_embedding_dim;
    Some(OpenAiEmbeddingBackend::new(model, api_key, base_url, dim))
}

/// The model id (vector-space tag) of the currently active backend.
///
/// Persisted on freshly embedded memories and used to gate cross-model dense
/// comparisons. When remote embeddings are not active this is the local
/// MiniLM tag, matching legacy untagged memories.
pub fn active_model_id() -> String {
    active_backend().model_id().to_string()
}

/// Whether `entry_model` (an entry's `effective_embedding_model()`) shares a
/// vector space with the active backend, so their dense cosine is meaningful.
pub fn model_matches_active(entry_model: &str) -> bool {
    entry_model == active_model_id()
}

/// Embed a retrieval QUERY with the active backend, returning the vector and the
/// backend's model id. The local backend round-trips through the cached
/// `crate::embedding` facade; remote backends call their API directly.
pub fn embed_query_active(text: &str) -> anyhow::Result<(Vec<f32>, String)> {
    let backend = active_backend();
    let vec = backend.embed_query(text)?;
    Ok((vec, backend.model_id().to_string()))
}

/// Embed a stored PASSAGE/memory with the active backend, returning the vector
/// and the backend's model id (to persist on the entry for space-gating).
pub fn embed_passage_active(text: &str) -> anyhow::Result<(Vec<f32>, String)> {
    let backend = active_backend();
    let vec = backend.embed_passage(text)?;
    Ok((vec, backend.model_id().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One local HTTP exchange, with no credentials, model loading, or API access.
    fn loopback_backend(
        status: &str,
        response: &str,
    ) -> (OpenAiEmbeddingBackend, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline, "no loopback request");
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("loopback accept: {e}"),
                }
            };
            // BSD/macOS can inherit the listener's nonblocking flag.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0, "incomplete request");
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..end]).unwrap();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(reply.as_bytes()).unwrap();
            request
        });
        (
            OpenAiEmbeddingBackend::new(
                "test-model",
                "dummy-loopback-key",
                Some(format!("http://{address}/v1")),
                Some(2),
            ),
            server,
        )
    }

    #[test]
    fn openai_loopback_preserves_legacy_bytes_headers_and_index_order() {
        let (backend, server) = loopback_backend(
            "200 OK",
            r#"{"data":[{"index":1,"embedding":[3.0,4.0]},{"index":0,"embedding":[1.0,2.0]}]}"#,
        );
        let inputs = vec!["quotes \" newline\n雪".to_string(), "".to_string()];
        let legacy = serde_json::to_vec(&serde_json::json!({
            "model": "test-model",
            "input": inputs,
        }))
        .unwrap();
        let vectors = backend.embed_inputs(inputs).unwrap();
        assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        let request = server.join().unwrap();
        let end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&request[..end]).unwrap();
        assert!(headers.starts_with("POST /v1/embeddings HTTP/1.1\r\n"));
        let headers = headers.to_ascii_lowercase();
        assert!(
            headers
                .lines()
                .any(|h| h == "authorization: bearer dummy-loopback-key")
        );
        assert!(
            headers
                .lines()
                .any(|h| h == "content-type: application/json")
        );
        assert_eq!(&request[end + 4..], legacy);
        assert!(legacy.starts_with(b"{\"input\":"));
    }

    #[test]
    fn openai_loopback_errors_and_empty_response() {
        for (status, response, expected) in [
            (
                "429 Too Many Requests",
                "slow down",
                "OpenAI embeddings request failed (429 Too Many Requests): slow down",
            ),
            ("200 OK", "not json", "parse embeddings response:"),
            (
                "200 OK",
                r#"{"data":[]}"#,
                "OpenAI embeddings returned 0 vectors for 1 inputs",
            ),
        ] {
            let (backend, server) = loopback_backend(status, response);
            let error = backend.embed_raw("hello").unwrap_err().to_string();
            server.join().unwrap();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn openai_empty_inputs_do_not_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let backend = OpenAiEmbeddingBackend::new(
            "test-model",
            "dummy-loopback-key",
            Some(format!("http://{}/v1", listener.local_addr().unwrap())),
            Some(2),
        );
        assert!(backend.embed_passages(&[]).unwrap().is_empty());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn local_backend_model_id_matches_legacy_tag() {
        // Critical for backward compatibility: the local backend's model id must
        // equal the legacy tag so pre-tagging memories stay in the same space.
        assert_eq!(LocalOnnxBackend.model_id(), LEGACY_EMBEDDING_MODEL);
    }

    #[test]
    fn default_formatting_is_identity() {
        let b = LocalOnnxBackend;
        assert_eq!(b.format_query("hello"), "hello");
        assert_eq!(b.format_passage("world"), "world");
    }

    #[test]
    fn openai_backend_namespaces_model_id_and_infers_dim() {
        let b = OpenAiEmbeddingBackend::new("text-embedding-3-small", "sk-x", None, None);
        // model_id must be namespaced so OpenAI vectors never compare against
        // local MiniLM vectors (different vector spaces).
        assert_eq!(b.model_id(), "openai:text-embedding-3-small");
        assert_ne!(b.model_id(), LocalOnnxBackend.model_id());
        assert_eq!(b.dim(), 1536);

        let large = OpenAiEmbeddingBackend::new("text-embedding-3-large", "sk-x", None, None);
        assert_eq!(large.dim(), 3072);

        // Explicit dim override wins (for truncated 3-small/3-large indexes).
        let truncated =
            OpenAiEmbeddingBackend::new("text-embedding-3-large", "sk-x", None, Some(256));
        assert_eq!(truncated.dim(), 256);
    }
}
