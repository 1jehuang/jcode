//! Command Code model discovery: live /provider/v1/models replaces the
//! catalog only on success; otherwise the conservative curated fallback
//! remains served (D-15, CMDC-03). Alias inputs canonicalize offline.

use anyhow::{Context, Result};
use jcode_provider_command_code::{
    CURATED_MODELS, DEFAULT_MODEL, MODEL_ALIASES, MODELS_URL, PROVIDER_KEY,
};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

/// Upstream discovery caps mirror the opencodex contract: responses larger
/// than 256 KiB or catalogs over 256 ids are truncated, never trusted whole.
const MAX_RESPONSE_BYTES: usize = 262_144;
const MAX_MODELS: usize = 256;

/// One catalog snapshot: canonical model ids plus fetch bookkeeping.
#[derive(Debug, Clone)]
struct CatalogSnapshot {
    models: Vec<String>,
    fetched_at: Instant,
    observed_at: SystemTime,
    live: bool,
}

impl CatalogSnapshot {
    fn curated() -> Self {
        Self {
            models: CURATED_MODELS.iter().map(|model| (*model).to_string()).collect(),
            fetched_at: Instant::now(),
            observed_at: SystemTime::now(),
            live: false,
        }
    }
}

/// Scoped, process-lifetime catalog for the Command Code family. Seeded with
/// the curated fallback; live discovery replaces it wholesale only on
/// success, and a failed refresh never removes fallback entries (D-15).
#[derive(Debug)]
pub struct CommandCodeCatalog {
    snapshot: RwLock<CatalogSnapshot>,
}

impl CommandCodeCatalog {
    pub fn new() -> Self {
        Self {
            snapshot: RwLock::new(CatalogSnapshot::curated()),
        }
    }

    /// Canonical model ids currently served, curated fallback guaranteed.
    pub fn model_ids(&self) -> Vec<String> {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.models.clone())
            .unwrap_or_else(|_| {
                CURATED_MODELS
                    .iter()
                    .map(|model| (*model).to_string())
                    .collect()
            })
    }

    pub fn is_recent(&self, ttl: Duration) -> bool {
        self.snapshot
            .read()
            .ok()
            .map(|snapshot| snapshot.live && snapshot.fetched_at.elapsed() <= ttl)
            .unwrap_or(false)
    }

    pub fn observed_at(&self) -> Option<SystemTime> {
        self.snapshot.read().ok().and_then(|snapshot| {
            snapshot.live.then_some(snapshot.observed_at)
        })
    }

    /// Replace-on-success refresh: the fetch closure encapsulates the live
    /// GET so both the network path and offline tests exercise the same gate.
    /// A non-empty replacement is stored wholesale; failures keep the
    /// previous snapshot (curated fallback never disappears).
    pub fn refresh_with(
        &self,
        fetch: impl FnOnce() -> Result<Vec<String>>,
    ) -> Result<bool> {
        let live_models = fetch()?;
        if live_models.is_empty() {
            anyhow::bail!("Command Code /provider/v1/models returned zero models");
        }
        let models = live_models
            .into_iter()
            .take(MAX_MODELS)
            .collect::<Vec<_>>();
        let mut snapshot = self.snapshot.write().map_err(|_| anyhow::anyhow!("catalog lock poisoned"))?;
        *snapshot = CatalogSnapshot {
            models,
            fetched_at: Instant::now(),
            observed_at: SystemTime::now(),
            live: true,
        };
        Ok(true)
    }

    /// Live discovery against the authenticated /provider/v1/models
    /// endpoint, bounded to MAX_RESPONSE_BYTES with json feature parsing.
    pub async fn refresh_live(&self, client: &reqwest::Client, api_key: &str) -> Result<()> {
        let response = client
            .get(MODELS_URL)
            .bearer_auth(api_key)
            .header(
                reqwest::header::USER_AGENT,
                jcode_provider_command_code::USER_AGENT,
            )
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("Command Code /provider/v1/models request failed (network)")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Command Code /provider/v1/models failed: {}", status);
        }
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("reading {} body", MODELS_URL))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!(
                "Command Code /provider/v1/models body exceeded {} bytes",
                MAX_RESPONSE_BYTES
            );
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .context("Command Code model discovery returned invalid JSON")?;
        let models = parse_command_code_models(&value);
        self.refresh_with(|| Ok(models))?;
        Ok(())
    }
}

/// Parse both wrapped shapes the endpoint has been observed in:
/// {"data":[{"id":...},...]} and a bare [{"id":...},...]. Case-sensitive
/// canonical ids are preserved verbatim (D-16).
pub fn parse_command_code_models(value: &serde_json::Value) -> Vec<String> {
    let rows = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| value.as_array().cloned().unwrap_or_default());
    rows_to_ids(&rows)
}

fn rows_to_ids(rows: &[serde_json::Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for row in rows.iter().take(MAX_MODELS) {
        let id = row
            .get("id")
            .or_else(|| row.get("model"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Canonicalize user input to the exact upstream model id. Lookup is
/// case-insensitive over the display aliases; canonical ids themselves stay
/// case-sensitive and pass through only when they appear in the live catalog
/// or curated fallback (D-16).
pub fn canonicalize_command_code_model(
    input: &str,
    catalog: &CommandCodeCatalog,
) -> Option<String> {
    let candidate = input.trim();
    if candidate.is_empty() {
        return None;
    }
    let models = catalog.model_ids();
    if models.iter().any(|model| model == candidate) {
        return Some(candidate.to_string());
    }
    if let Some((_, canonical)) = MODEL_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(candidate))
    {
        if models.iter().any(|model| model == canonical) {
            return Some((*canonical).to_string());
        }
    }
    None
}

/// The user-safe default model id.
pub fn default_command_code_model() -> &'static str {
    DEFAULT_MODEL
}

/// Detector key used by caller-side logging/status hooks.
pub fn provider_key() -> &'static str {
    PROVIDER_KEY
}
