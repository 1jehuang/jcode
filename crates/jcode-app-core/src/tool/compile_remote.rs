//! Subscription-backed remote compilation. The tool description is static so it
//! never invalidates the prompt cache. Account state is reported in results:
//! every build verifies access before reading or uploading source, and the
//! server remains authoritative for entitlement and cloud-compute credit admission.
mod source;

use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DESCRIPTION: &str = "Compile in an isolated Linux sandbox using Jcode subscription cloud-compute credits (shared with cloud agents). Uploads eligible source files and returns compiler output and metered usage. Requires a Jcode subscription; use action=status to check access and credits. Use only when the user has requested remote builds or authorized source sharing. Failed builds also consume credits. No automatic top-ups.";
const SUBSCRIBE: &str = "Remote compilation requires a Jcode subscription. Tell the user to subscribe at https://jcode.sh/pricing, then sign in with `jcode account login`. Do not open checkout or purchase automatically.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    SignedOut,
    SubscriptionRequired,
    Ready,
    NotEnabled,
    Unknown,
}

impl Access {
    fn guidance(self) -> &'static str {
        match self {
            Self::SignedOut => {
                "Not signed in. Subscribe at https://jcode.sh/pricing, then `jcode account login`."
            }
            Self::SubscriptionRequired => SUBSCRIBE,
            Self::Ready => {
                "Remote compilation is available. Builds spend the signed-in Jcode subscription's cloud-compute credits, shared with cloud agents. Failed builds also consume credits. No automatic top-ups."
            }
            Self::NotEnabled => {
                "Remote compilation is not enabled for this account or the build service is not configured. Do not promise subscribing will fix service availability. Use action=status to recheck or compile locally."
            }
            Self::Unknown => {
                "Account access could not be verified (the account service may be unreachable). Builds fail closed before source upload. Retry action=status later. Do not claim the user is unsubscribed."
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::SignedOut => "signed_out",
            Self::SubscriptionRequired => "subscription_required",
            Self::Ready => "available",
            Self::NotEnabled => "unavailable",
            Self::Unknown => "unknown",
        }
    }
}

fn credentials() -> Option<(String, String)> {
    crate::subscription_catalog::configured_api_key()
        .filter(|key| !key.trim().is_empty())
        .map(|key| (crate::subscription_api::configured_api_base(), key))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()?)
}

// Never send a bearer credential to plaintext public endpoints. Loopback HTTP
// supports local development and transport tests without weakening production.
fn endpoint(base: &str, suffix: &str) -> Result<String> {
    let url = reqwest::Url::parse(base).map_err(|_| anyhow::anyhow!("Invalid Jcode API base"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!(
            "Remote compilation requires an HTTPS Jcode API base (HTTP is allowed only on loopback)"
        );
    }
    Ok(format!("{}/{suffix}", base.trim_end_matches('/')))
}

async fn bounded_response(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        bail!("Remote compilation response exceeds the size limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Unable to read the remote compilation response"))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            bail!("Remote compilation response exceeds the size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn check_access(client: &reqwest::Client, base: &str, key: &str) -> Access {
    let Ok(url) = endpoint(base, "me") else {
        return Access::Unknown;
    };
    let Ok(response) = client
        .get(url)
        .bearer_auth(key)
        .timeout(crate::subscription_api::ME_FETCH_TIMEOUT)
        .send()
        .await
    else {
        return Access::Unknown;
    };
    match response.status().as_u16() {
        401 => return Access::SignedOut,
        402 | 403 => return Access::SubscriptionRequired,
        200 => {}
        _ => return Access::Unknown,
    }
    let Ok(body) = bounded_response(response, 64 * 1024).await else {
        return Access::Unknown;
    };
    #[derive(Deserialize, Default)]
    struct Entitlements {
        cloud_compute: Option<bool>,
    }
    #[derive(Deserialize)]
    struct Account {
        #[serde(flatten)]
        me: crate::subscription_api::SubscriptionMe,
        #[serde(default)]
        entitlements: Entitlements,
    }
    let Ok(account) = serde_json::from_slice::<Account>(&body) else {
        return Access::Unknown;
    };
    let me = account.me;
    if account.entitlements.cloud_compute == Some(false) {
        return Access::SubscriptionRequired;
    }
    if me.status.eq_ignore_ascii_case("active") && me.capabilities.remote_compile {
        Access::Ready
    } else if account.entitlements.cloud_compute == Some(true) || me.has_active_paid_plan() {
        Access::NotEnabled
    } else {
        Access::SubscriptionRequired
    }
}

async fn access(base: &str, key: &str) -> Access {
    match client() {
        Ok(client) => check_access(&client, base, key).await,
        Err(_) => Access::Unknown,
    }
}

pub struct CompileRemoteTool;
impl CompileRemoteTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Serialize)]
struct BuildRequest {
    request_id: String,
    command: String,
    timeout_seconds: u64,
    files: Vec<source::SourceFile>,
}

#[derive(Deserialize, Serialize)]
struct BuildResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_confirmed: Option<bool>,
}

#[derive(Deserialize, Serialize)]
struct ComputeCredits {
    unit: String,
    granted_microcredits: u64,
    charged_microcredits: u64,
    reserved_microcredits: u64,
    available_microcredits: u64,
    recent_usage: Vec<ComputeJob>,
}

#[derive(Deserialize, Serialize)]
struct ComputeJob {
    job_key: String,
    workload: String,
    state: String,
    reserved_microcredits: u64,
    charged_microcredits: u64,
    actual_seconds: Option<u64>,
    created_at: u64,
    settled_at: Option<u64>,
}

async fn compute_credits(
    client: &reqwest::Client,
    base: &str,
    key: &str,
) -> Result<ComputeCredits> {
    let response = client
        .get(endpoint(base, "compute/usage")?)
        .bearer_auth(key)
        .timeout(crate::subscription_api::ME_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Cloud-compute credit status is temporarily unavailable"))?;
    if response.status().as_u16() != 200 {
        bail!(
            "Cloud-compute credit status is unavailable (HTTP {})",
            response.status().as_u16()
        );
    }
    #[derive(Deserialize)]
    struct Envelope {
        compute: ComputeCredits,
    }
    let result: Envelope =
        serde_json::from_slice(&bounded_response(response, MAX_RESPONSE_BYTES).await?)
            .map_err(|_| anyhow::anyhow!("Invalid cloud-compute credit status"))?;
    if result.compute.unit != "microcredits" {
        bail!("Unknown cloud-compute credit unit");
    }
    Ok(result.compute)
}

fn validate_input(input: &Input) -> Result<()> {
    if !matches!(
        input.action.as_deref().unwrap_or("compile"),
        "compile" | "status"
    ) {
        bail!("action must be compile or status");
    }
    if input.action.as_deref() != Some("status")
        && input
            .command
            .as_ref()
            .is_none_or(|c| c.trim().is_empty() || c.len() > 8192 || c.contains('\0'))
    {
        bail!("command must be a nonempty compilation command, at most 8192 bytes");
    }
    if input
        .timeout_seconds
        .is_some_and(|n| !(1..=600).contains(&n))
    {
        bail!("timeout_seconds must be between 1 and 600");
    }
    Ok(())
}

async fn submit(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    request: BuildRequest,
) -> Result<BuildResult> {
    let timeout = Duration::from_secs(request.timeout_seconds + 120);
    let payload = serde_json::to_vec(&request)?;
    if payload.len() > 30 * 1024 * 1024 {
        bail!("Encoded source snapshot exceeds the 30 MiB upload limit");
    }
    let response = client.post(endpoint(base, "compile")?).bearer_auth(key)
        .timeout(timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload).send().await.map_err(|_| anyhow::anyhow!(
            "Remote compilation request failed or timed out. The job may still be running and consuming reserved credits. Check cloud usage before retrying."
        ))?;
    match response.status().as_u16() {
        200 => {}
        401 => bail!("Jcode sign-in expired. Run `jcode account login` before retrying."),
        402 => bail!(
            "Insufficient cloud-compute credits. Remote builds and cloud agents share the same credit balance. Manage credits at https://jcode.sh/account. No automatic top-up was performed."
        ),
        403 => bail!("{SUBSCRIBE}"),
        409 => bail!(
            "This remote build request is already admitted or completed. Check cloud usage before submitting another build."
        ),
        413 => bail!(
            "Remote compilation source upload or compiler output exceeds the service limit. Compute already consumed may be charged. Check cloud usage before retrying."
        ),
        429 => {
            bail!("Remote compilation capacity or account concurrency limit reached. Retry later.")
        }
        404 | 503 => bail!(
            "Remote compilation service is unavailable or has not been configured. Compile locally for now."
        ),
        504 => bail!(
            "Remote compilation timed out. Consumed compute is charged against cloud credits."
        ),
        status => bail!(
            "Remote compilation service returned HTTP {status}. Check cloud usage before retrying."
        ),
    }
    let body = bounded_response(response, MAX_RESPONSE_BYTES).await?;
    serde_json::from_slice(&body).map_err(|_| {
        anyhow::anyhow!("Invalid remote compilation result. Check cloud usage before retrying.")
    })
}

#[async_trait]
impl Tool for CompileRemoteTool {
    fn name(&self) -> &str {
        "compile_remote"
    }
    // Static by design: tool definitions sit in the provider prompt-cache
    // prefix, so account state is reported in results (action=status or a
    // denied compile), never in the description.
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {"type":"string","enum":["compile","status"],"description":"compile (default) uploads source and spends credits. status checks access without uploading."},
                "command": {"type":"string","maxLength":8192,"description":"Remote Linux build command, e.g. cargo check. Required for compile. Never include secrets."},
                "path": {"type":"string","description":"Git repo root relative to workspace (default cwd). Uploads tracked/untracked non-ignored files."},
                "timeout_seconds": {"type":"integer","minimum":1,"maximum":600,"description":"Build deadline in seconds, default 300. Credits reserved up front, settled by measured usage."}
            }
        })
    }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: Input = serde_json::from_value(input)?;
        validate_input(&input)?;
        let credentials = credentials();
        let access = match &credentials {
            Some((base, key)) => access(base, key).await,
            None => Access::SignedOut,
        };
        if input.action.as_deref() == Some("status") {
            let mut status = json!({
                "access": access.label(), "guidance": access.guidance(),
                "credits": "Builds and cloud agents spend the same cloud-compute balance. The server reserves credits before starting a sandbox."
            });
            if !matches!(access, Access::SignedOut | Access::Unknown)
                && let Some((base, key)) = &credentials
            {
                match compute_credits(&client()?, base, key).await {
                    Ok(credits) => status["compute"] = serde_json::to_value(credits)?,
                    Err(error) => status["compute_error"] = json!(error.to_string()),
                }
            }
            return Ok(ToolOutput::new(serde_json::to_string_pretty(&status)?));
        }
        if access != Access::Ready {
            bail!("{}", access.guidance());
        }
        let (base, key) = credentials.expect("verified access requires credentials");
        let root = ctx.resolve_path(Path::new(input.path.as_deref().unwrap_or(".")));
        let snapshot = source::snapshot(&root).await?;
        let request_id = format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", ctx.session_id, ctx.tool_call_id).as_bytes())
        );
        let result = submit(
            &client()?,
            &base,
            &key,
            BuildRequest {
                request_id,
                command: input.command.expect("validated command"),
                timeout_seconds: input.timeout_seconds.unwrap_or(300),
                files: snapshot.files,
            },
        )
        .await?;
        Ok(ToolOutput::new(serde_json::to_string_pretty(&result)?))
    }
}

#[cfg(test)]
mod tests;
