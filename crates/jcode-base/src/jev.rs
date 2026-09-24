//! Shared Jev typed Decisions transport, separate from chat completions.
//!
//! BYOK credentials are bound to fixed provider endpoints. The Jcode route uses
//! the configured trusted account gateway, and checks its live purpose-specific
//! capability before each evaluation. Credential presence is not entitlement.

use anyhow::{Result, anyhow, bail, ensure};
use reqwest::{Client, Response, Url};
use serde_json::{Map, Value, json};
use std::time::Duration;

const PROVIDER_ENV: &str = "JCODE_MEMORY_JEV_PROVIDER";
const BROWSER_PROVIDER_ENV: &str = "JCODE_BROWSER_JEV_PROVIDER";
const VOICE_PROVIDER_ENV: &str = "JCODE_VOICE_JEV_PROVIDER";
const MAX_REQUEST_BYTES: usize = 80 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ME_BYTES: usize = 16 * 1024;
pub(crate) const MAX_QUESTIONS: usize = 24;
/// Typesafe direct accepts far more (256 verified live on 2026-09-23). A voice
/// request has at most 26 questions, so it always fits in one round trip.
const TYPESAFE_MAX_QUESTIONS: usize = 64;

/// Typesafe latency is bimodal and sticky per connection: a connection is
/// served in ~150ms or 2-12s (measured live 2026-09-23). Voice waits on this
/// before inserting or sending, so duplicates on fresh connections race the
/// primary. Decisions are side-effect-free and cost fractions of a cent.
#[cfg(not(test))]
const VOICE_HEDGE_DELAYS: [Duration; 3] = [
    Duration::from_millis(300),
    Duration::from_millis(700),
    Duration::from_millis(1500),
];
#[cfg(test)]
const VOICE_HEDGE_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(150),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JevPurpose {
    Memory,
    Browser,
    Voice,
}

impl JevPurpose {
    fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Browser => "browser",
            Self::Voice => "voice",
        }
    }

    fn capability(self) -> &'static str {
        match self {
            Self::Memory => "memory_jev",
            Self::Browser => "browser_jev",
            // Voice uses the gateway's existing typed noul contract.
            Self::Voice => "memory_jev",
        }
    }

    fn selector_with(
        self,
        env: impl FnOnce(&str) -> Result<String, std::env::VarError>,
        memory_default: impl FnOnce() -> String,
    ) -> Result<String> {
        let key = match self {
            Self::Memory => PROVIDER_ENV,
            Self::Browser => BROWSER_PROVIDER_ENV,
            Self::Voice => VOICE_PROVIDER_ENV,
        };
        match env(key) {
            Ok(value) => Ok(value),
            Err(std::env::VarError::NotPresent) => Ok(match self {
                Self::Memory => memory_default(),
                Self::Browser => "auto".into(),
                Self::Voice => "auto".into(),
            }),
            Err(_) => bail!("{key} must contain a valid provider name"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JevProvider {
    OpenRouter,
    TypeSafe,
    Aimlapi,
    Jcode,
}

impl JevProvider {
    fn name(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::TypeSafe => "typesafe",
            Self::Aimlapi => "aimlapi",
            Self::Jcode => "jcode",
        }
    }

    fn credentials(self) -> (&'static str, &'static str) {
        match self {
            Self::OpenRouter => ("OPENROUTER_API_KEY", "openrouter.env"),
            Self::TypeSafe => ("TYPESAFE_API_KEY", "typesafe.env"),
            Self::Aimlapi => ("AIMLAPI_API_KEY", "aimlapi.env"),
            Self::Jcode => (
                crate::subscription_catalog::JCODE_API_KEY_ENV,
                crate::subscription_catalog::JCODE_ENV_FILE,
            ),
        }
    }

    fn max_questions(self) -> usize {
        match self {
            Self::TypeSafe => TYPESAFE_MAX_QUESTIONS,
            _ => MAX_QUESTIONS,
        }
    }

    fn model(self) -> &'static str {
        match self {
            Self::OpenRouter | Self::Jcode => "typesafe/jev-1.13",
            Self::TypeSafe => "jev-latest",
            Self::Aimlapi => "typesafe/jev",
        }
    }

    fn endpoint(self, gateway_base: &str) -> Result<String> {
        Ok(match self {
            Self::OpenRouter => "https://openrouter.ai/api/alpha/decisions".into(),
            Self::TypeSafe => "https://api.typesafe.ai/v1/systemone".into(),
            Self::Aimlapi => "https://api.aimlapi.com/v1/decisions".into(),
            Self::Jcode => format!("{}/decisions", trusted_gateway_base(gateway_base)?),
        })
    }
}

/// Do not derive Debug: this contains a provider secret.
#[derive(Clone)]
pub struct JevClient {
    client: Client,
    /// Voice only. Typesafe speed is sticky per connection (a slow connection
    /// stays slow), so each hedge needs its own client and connection.
    hedge_clients: Vec<Client>,
    purpose: JevPurpose,
    provider: JevProvider,
    api_key: String,
    endpoint: String,
    me_endpoint: Option<String>,
}

impl JevClient {
    /// A configured credential route exists. This is not a health or entitlement
    /// probe. In particular, Jcode entitlement is checked live by `evaluate`.
    pub fn available() -> bool {
        Self::resolve(JevPurpose::Memory).is_ok()
    }

    pub fn new() -> Result<Self> {
        Self::for_purpose(JevPurpose::Memory)
    }

    /// Browser routing is independent of memory configuration and defaults to
    /// subscription-first auto selection. Evaluation never changes accounts.
    pub fn for_browser() -> Result<Self> {
        Self::for_purpose(JevPurpose::Browser)
    }

    /// Voice uses included Jcode access (whose gateway uses Typesafe directly),
    /// then Typesafe BYOK when no Jcode credential exists. Other provider keys and
    /// memory/browser configuration are ignored. JCODE_VOICE_JEV_PROVIDER may
    /// explicitly select typesafe or jcode. Evaluation never changes accounts.
    pub fn for_voice() -> Result<Self> {
        Self::for_purpose(JevPurpose::Voice)
    }

    fn for_purpose(purpose: JevPurpose) -> Result<Self> {
        let (provider, api_key, endpoint, me_endpoint) = Self::resolve(purpose)?;
        let build = || {
            client_builder()
                .build()
                .map_err(|_| anyhow!("Could not initialize the Jev decision client"))
        };
        let client = build()?;
        let hedge_clients = if purpose == JevPurpose::Voice {
            VOICE_HEDGE_DELAYS
                .iter()
                .map(|_| build())
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        Ok(Self {
            client,
            hedge_clients,
            purpose,
            provider,
            api_key,
            endpoint,
            me_endpoint,
        })
    }

    fn resolve(purpose: JevPurpose) -> Result<(JevProvider, String, String, Option<String>)> {
        let selector = purpose.selector_with(
            |key| std::env::var(key),
            || crate::config::config().agents.memory_jev_provider.clone(),
        )?;
        // Unlike the API-key helper, this does not consult registered
        // cross-provider fallback resolvers or the shared compatible slot.
        let load = |env: &str, file: &str| {
            crate::provider_catalog::load_env_value_from_env_or_config(env, file)
        };
        let (provider, api_key) = if purpose == JevPurpose::Voice {
            resolve_voice_with(&selector, load)?
        } else {
            resolve_with(&selector, load)?
        };
        let base = if provider == JevProvider::Jcode {
            crate::subscription_api::configured_api_base()
        } else {
            String::new()
        };
        let endpoint = provider.endpoint(&base)?;
        let me_endpoint = if provider == JevProvider::Jcode {
            Some(format!("{}/me", trusted_gateway_base(&base)?))
        } else {
            None
        };
        Ok((provider, api_key, endpoint, me_endpoint))
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn model_id(&self) -> &str {
        self.provider.model()
    }

    /// Largest question batch this route accepts in one request.
    pub(crate) fn max_questions(&self) -> usize {
        self.provider.max_questions()
    }

    /// Return the full typed Decisions response, including provider usage.
    /// Never retries using another provider or account after an auth/billing
    /// failure. Callers own the relevance threshold and uncertainty policy.
    pub async fn evaluate(&self, state: Value, questions: Map<String, Value>) -> Result<Value> {
        let body = request_body_for(self.purpose, self.provider, state, &questions)?;
        if let Some(endpoint) = &self.me_endpoint {
            let response = self
                .client
                .get(endpoint)
                .bearer_auth(&self.api_key)
                .timeout(crate::subscription_api::ME_FETCH_TIMEOUT)
                .send()
                .await
                .map_err(|_| {
                    anyhow!(
                        "Could not verify Jcode {} entitlement; try again later",
                        self.purpose.name()
                    )
                })?;
            let me = read_response(response, MAX_ME_BYTES).await?;
            ensure!(
                me["capabilities"]
                    .get(self.purpose.capability())
                    .and_then(Value::as_bool)
                    == Some(true),
                "Jcode Jev {} is unavailable for this account or gateway. An active entitled subscription and a gateway with {} support are required. Configure a Jev BYOK provider to use your own account.",
                self.purpose.name(),
                self.purpose.capability()
            );
        }
        if self.purpose == JevPurpose::Voice {
            return self.send_hedged(body, &questions).await;
        }
        let value = self.send(&self.endpoint, body).await?;
        validate_answers(&value, &questions)?;
        Ok(value)
    }

    /// Send once, then again on a fresh connection after each
    /// [`VOICE_HEDGE_DELAYS`] step while nothing has answered. The first valid
    /// answer wins and the rest are dropped. A primary failure returns at once,
    /// so auth/billing errors are never duplicated. Hedge failures are ignored
    /// while any other attempt is still pending.
    async fn send_hedged(&self, body: Vec<u8>, questions: &Map<String, Value>) -> Result<Value> {
        use futures::StreamExt;
        let attempt = |client: &Client, delay: Duration, primary: bool| {
            let body = body.clone();
            let client = client.clone();
            async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let result = async {
                    let value = self.send_with(&client, &self.endpoint, body).await?;
                    validate_answers(&value, questions)?;
                    Ok::<_, anyhow::Error>(value)
                }
                .await;
                (primary, result)
            }
        };
        let mut attempts = futures::stream::FuturesUnordered::new();
        attempts.push(attempt(&self.client, Duration::ZERO, true));
        for (client, delay) in self.hedge_clients.iter().zip(VOICE_HEDGE_DELAYS) {
            attempts.push(attempt(client, delay, false));
        }
        let mut hedge_error = None;
        while let Some((primary, result)) = attempts.next().await {
            match result {
                Ok(value) => return Ok(value),
                Err(error) if primary => return Err(error),
                Err(error) => hedge_error = Some(error),
            }
        }
        Err(hedge_error.unwrap_or_else(|| anyhow!("Jev decision request failed")))
    }

    async fn send(&self, endpoint: &str, body: Vec<u8>) -> Result<Value> {
        self.send_with(&self.client, endpoint, body).await
    }

    /// Decisions requests are side-effect-free classifications, so transient
    /// overload responses (429/502/503/504/529) are retried with a short bounded
    /// backoff on the same provider and account. Auth, billing, redirect, and
    /// other failures are never retried and never fall back to another account.
    async fn send_with(&self, client: &Client, endpoint: &str, body: Vec<u8>) -> Result<Value> {
        let mut attempt = 0;
        loop {
            let response = self.send_once(client, endpoint, body.clone()).await?;
            let status = response.status().as_u16();
            if attempt < TRANSIENT_RETRY_DELAYS.len() && is_transient_status(status) {
                let delay = retry_after(&response).unwrap_or(TRANSIENT_RETRY_DELAYS[attempt]);
                attempt += 1;
                crate::logging::info(&format!(
                    "Jev {} returned HTTP {status}; retry {attempt}/{} in {}ms",
                    self.provider.name(),
                    TRANSIENT_RETRY_DELAYS.len(),
                    delay.as_millis()
                ));
                tokio::time::sleep(delay).await;
                continue;
            }
            return read_response(response, MAX_RESPONSE_BYTES).await;
        }
    }

    async fn send_once(&self, client: &Client, endpoint: &str, body: Vec<u8>) -> Result<Response> {
        let mut request = client
            .post(endpoint)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if self.provider == JevProvider::OpenRouter {
            request = request.header("HTTP-Referer", "https://jcode.sh").header(
                "X-Title",
                match self.purpose {
                    JevPurpose::Memory => "Jcode Memory",
                    JevPurpose::Browser => "Jcode Browser",
                    JevPurpose::Voice => "Jcode Voice",
                },
            );
        }
        request.send().await.map_err(|_| {
            anyhow!("Jev decision request failed or timed out; check the selected provider")
        })
    }
}

#[cfg(not(test))]
const TRANSIENT_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(600), Duration::from_millis(1800)];
#[cfg(test)]
const TRANSIENT_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(1), Duration::from_millis(1)];
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

fn is_transient_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504 | 529)
}

/// Honor a short numeric Retry-After. Long waits fail fast instead of
/// stalling the caller beyond the bounded retry budget.
fn retry_after(response: &Response) -> Option<Duration> {
    let secs: u64 = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let delay = Duration::from_secs(secs);
    if cfg!(test) {
        return Some(Duration::from_millis(1));
    }
    (delay <= MAX_RETRY_AFTER).then_some(delay)
}

fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
}

fn resolve_voice_with(
    selector: &str,
    load: impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    match selector.trim().to_ascii_lowercase().as_str() {
        "auto" => resolve_providers(&[JevProvider::Jcode, JevProvider::TypeSafe], load),
        "typesafe" | "jcode" | "subscription" | "jcode-subscription" => {
            resolve_with(selector, load)
        }
        _ => bail!(
            "Invalid voice Jev provider. Choose auto, typesafe, or jcode; voice never uses OpenRouter or AIMLAPI"
        ),
    }
}

fn resolve_with(
    selector: &str,
    load: impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    let providers: &[JevProvider] = match selector.trim().to_ascii_lowercase().as_str() {
        "auto" => &[
            // Included subscriber access wins over personal paid provider keys.
            // Entitlement is checked live before evaluation. Failure must not
            // silently spend a BYOK balance; users can select BYOK explicitly.
            // Typesafe serves Jev directly, so it beats resellers of the same model.
            JevProvider::Jcode,
            JevProvider::TypeSafe,
            JevProvider::OpenRouter,
            JevProvider::Aimlapi,
        ],
        "openrouter" => &[JevProvider::OpenRouter],
        "typesafe" => &[JevProvider::TypeSafe],
        "aimlapi" => &[JevProvider::Aimlapi],
        "jcode" | "subscription" | "jcode-subscription" => &[JevProvider::Jcode],
        _ => bail!("Invalid Jev provider. Choose auto, openrouter, typesafe, aimlapi, or jcode"),
    };
    resolve_providers(providers, load)
}

fn resolve_providers(
    providers: &[JevProvider],
    mut load: impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    for &provider in providers {
        let (env, file) = provider.credentials();
        if let Some(key) = load(env, file) {
            let key = jcode_provider_env::sanitize_secret_value(&key);
            if !key.is_empty() {
                // Reject malformed headers now rather than leaking a provider's
                // request-builder error through a later error chain.
                ensure!(
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).is_ok(),
                    "The selected Jev provider credential is not a valid HTTP bearer value"
                );
                return Ok((provider, key.to_owned()));
            }
        }
    }
    if let [provider] = providers {
        let (env, file) = provider.credentials();
        bail!(
            "No credential for the selected Jev provider {}. Configure {env} ({file}). Explicit provider selection never falls back to another account.",
            provider.name()
        );
    }
    let credentials = providers
        .iter()
        .map(|provider| {
            let (env, file) = provider.credentials();
            format!("{env} ({file})")
        })
        .collect::<Vec<_>>()
        .join(" or ");
    bail!(
        "No credential for the selected Jev route. Configure {credentials}. Provider failures never fall back to another account."
    )
}

fn trusted_gateway_base(base: &str) -> Result<String> {
    let url = Url::parse(base.trim()).map_err(|_| anyhow!("Invalid Jcode gateway base URL"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.host_str().is_some()
            && (url.scheme() == "https" || (url.scheme() == "http" && loopback))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Jcode Jev gateway requires HTTPS (HTTP only for loopback), without URL credentials, query, or fragment"
    );
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
fn request_body(
    provider: JevProvider,
    state: Value,
    questions: &Map<String, Value>,
) -> Result<Vec<u8>> {
    request_body_for(JevPurpose::Memory, provider, state, questions)
}

fn request_body_for(
    purpose: JevPurpose,
    provider: JevProvider,
    state: Value,
    questions: &Map<String, Value>,
) -> Result<Vec<u8>> {
    if purpose == JevPurpose::Browser {
        ensure!(
            questions.len() == 1
                && questions.get("action").is_some_and(|question| {
                    question["type"] == "choice"
                        && question["instructions"]
                            .as_str()
                            .is_some_and(|s| !s.trim().is_empty())
                        && question["criteria"].as_object().is_some_and(|criteria| {
                            (2..=255).contains(&criteria.len())
                                && criteria.values().all(Value::is_string)
                        })
                }),
            "Browser Decisions requires exactly one action choice question with text instructions and 2 to 255 text criteria"
        );
    }
    ensure!(
        state.is_string() || state.is_object() || state.is_array(),
        "Jev state must be text, an object, or an array"
    );
    ensure!(
        (1..=provider.max_questions()).contains(&questions.len()),
        "Jev requests require between 1 and {} questions",
        provider.max_questions()
    );
    for (id, question) in questions {
        ensure!(!id.is_empty() && id.len() <= 64, "Invalid Jev question ID");
        let instructions = &question["instructions"];
        ensure!(
            instructions.is_string() || instructions.is_object() || instructions.is_array(),
            "Jev questions require instructions"
        );
        match question["type"].as_str() {
            Some("noul") => {}
            Some("choice") => {
                let criteria = question["criteria"].as_object();
                ensure!(
                    criteria.is_some_and(|criteria| {
                        (2..=255).contains(&criteria.len())
                            && criteria.values().all(|v| v.is_string() || v.is_null())
                    }),
                    "Jev choice questions require 2 to 255 described options"
                );
            }
            Some("score") => ensure!(
                question["criteria"].as_array().is_some_and(|criteria| {
                    (2..=255).contains(&criteria.len()) && criteria.iter().all(Value::is_string)
                }),
                "Jev score questions require 2 to 255 level descriptions"
            ),
            _ => bail!("Unsupported Jev question type; expected noul, choice, or score"),
        }
        if provider == JevProvider::Jcode && purpose != JevPurpose::Browser {
            ensure!(
                question["type"] == "noul"
                    && instructions.as_str().is_some_and(|s| !s.trim().is_empty())
                    && question["criteria"]["true"].is_string()
                    && question["criteria"]["false"].is_string(),
                "Jcode memory Decisions supports noul questions with text instructions and true/false criteria"
            );
        }
    }
    // String state is accepted by every provider and preserves the existing
    // OpenRouter Decisions wire contract. Direct providers retain structured data.
    let state =
        if matches!(provider, JevProvider::OpenRouter | JevProvider::Jcode) && !state.is_string() {
            Value::String(serde_json::to_string(&state).map_err(|_| anyhow!("Invalid Jev state"))?)
        } else {
            state
        };
    let body = serde_json::to_vec(&json!({
        "model": provider.model(), "state": state, "questions": questions
    }))
    .map_err(|_| anyhow!("Could not encode Jev request"))?;
    ensure!(
        body.len() <= MAX_REQUEST_BYTES,
        "Jev request exceeds the bounded context size"
    );
    Ok(body)
}

async fn read_response(mut response: Response, limit: usize) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 => "selected provider credential is invalid or revoked",
            403 => "selected provider denied access or the account is not entitled",
            402 => "selected provider credits or account spending limit are exhausted",
            404 => "selected gateway does not support this Jev endpoint",
            429 | 529 => "selected provider is rate limited or overloaded; try again later",
            300..=399 => "redirect refused to protect provider credentials",
            _ => "selected provider is unavailable or rejected the request",
        };
        bail!("Jev returned HTTP {}: {hint}", status.as_u16());
    }
    ensure!(
        !response
            .content_length()
            .is_some_and(|length| length > limit as u64),
        "Jev response exceeds the bounded response size"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Could not read Jev response"))?
    {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "Jev response exceeds the bounded response size"
        );
        bytes.extend_from_slice(&chunk);
    }
    // Never retain serde's diagnostic, which can quote untrusted response data.
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Jev returned invalid response JSON"))
}

fn validate_answers(value: &Value, questions: &Map<String, Value>) -> Result<()> {
    let answers = value["answers"]
        .as_object()
        .ok_or_else(|| anyhow!("Jev returned no typed answers"))?;
    ensure!(
        answers.len() == questions.len(),
        "Jev returned an incomplete or unexpected answer set"
    );
    for (id, question) in questions {
        let answer = answers
            .get(id)
            .ok_or_else(|| anyhow!("Jev omitted a requested answer"))?;
        ensure!(
            answer["type"] == question["type"],
            "Jev answer type does not match its question"
        );
        if question["type"] == "noul" {
            ensure!(
                answer["noul"]
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
                "Jev returned an invalid noul probability"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn questions() -> Map<String, Value> {
        json!({"m0": {"type": "noul", "instructions": "Is this memory relevant?", "criteria": {"true": "Useful", "false": "Irrelevant"}}})
            .as_object().unwrap().clone()
    }

    fn response() -> Value {
        json!({"answers": {"m0": {"type": "noul", "noul": 0.91}}, "usage": {"input_tokens": 123, "output_tokens": 4}})
    }

    fn browser_questions() -> Map<String, Value> {
        json!({"action": {"type": "choice", "instructions": "Choose the next browser action", "criteria": {"click": "Click the button", "stop": "Return control"}}})
            .as_object().unwrap().clone()
    }

    #[test]
    fn voice_routes_independently_to_subscription_or_typesafe_only() {
        let selector = JevPurpose::Voice
            .selector_with(
                |key| {
                    assert_eq!(key, VOICE_PROVIDER_ENV);
                    Err(std::env::VarError::NotPresent)
                },
                || panic!("voice must ignore memory configuration"),
            )
            .unwrap();
        assert_eq!(selector, "auto");
        assert_eq!(
            JevPurpose::Voice
                .selector_with(
                    |key| {
                        assert_eq!(key, VOICE_PROVIDER_ENV);
                        Ok("typesafe".into())
                    },
                    || panic!()
                )
                .unwrap(),
            "typesafe"
        );
        for (available, expected) in [
            ("JCODE_API_KEY", JevProvider::Jcode),
            ("TYPESAFE_API_KEY", JevProvider::TypeSafe),
        ] {
            let (provider, _) = resolve_voice_with(&selector, |env, _| {
                assert!(matches!(env, "JCODE_API_KEY" | "TYPESAFE_API_KEY"));
                (env == available).then(|| "test-key".into())
            })
            .unwrap();
            assert_eq!(provider, expected);
        }
        assert_eq!(
            resolve_voice_with("auto", |_, _| Some("present".into()))
                .unwrap()
                .0,
            JevProvider::Jcode
        );
        let (provider, _) = resolve_voice_with("typesafe", |env, file| {
            assert_eq!((env, file), ("TYPESAFE_API_KEY", "typesafe.env"));
            Some("typesafe-test-key".into())
        })
        .unwrap();
        assert_eq!(provider, JevProvider::TypeSafe);
        assert_eq!(provider.model(), "jev-latest");
        assert_eq!(
            provider.endpoint("").unwrap(),
            "https://api.typesafe.ai/v1/systemone"
        );
        for selector in ["auto", "typesafe", "jcode"] {
            let error = resolve_voice_with(selector, |env, _| {
                assert!(matches!(env, "JCODE_API_KEY" | "TYPESAFE_API_KEY"));
                // Other credentials cannot make this route available.
                (env == "OPENROUTER_API_KEY" || env == "AIMLAPI_API_KEY")
                    .then(|| "other-account-key".into())
            })
            .unwrap_err();
            assert!(!error.to_string().contains("OPENROUTER_API_KEY"));
            assert!(!error.to_string().contains("AIMLAPI_API_KEY"));
        }
        for selector in ["openrouter", "aimlapi", ""] {
            assert!(
                resolve_voice_with(selector, |_, _| panic!(
                    "invalid voice route must not load keys"
                ))
                .is_err()
            );
        }
        assert_eq!(JevPurpose::Voice.capability(), "memory_jev");
    }

    #[test]
    fn purpose_selectors_are_independent_and_browser_defaults_to_subscription_first() {
        let env = |key: &str| match key {
            PROVIDER_ENV => Ok("typesafe".into()),
            BROWSER_PROVIDER_ENV => Ok("openrouter".into()),
            _ => panic!("unexpected configuration lookup"),
        };
        assert_eq!(
            JevPurpose::Memory.selector_with(env, || panic!()).unwrap(),
            "typesafe"
        );
        assert_eq!(
            JevPurpose::Browser.selector_with(env, || panic!()).unwrap(),
            "openrouter"
        );
        let selector = JevPurpose::Browser
            .selector_with(
                |key| {
                    assert_eq!(key, BROWSER_PROVIDER_ENV);
                    Err(std::env::VarError::NotPresent)
                },
                || panic!("browser must not consult memory config"),
            )
            .unwrap();
        assert_eq!(selector, "auto");
        assert_eq!(
            resolve_with(&selector, |_, _| Some("present".into()))
                .unwrap()
                .0,
            JevProvider::Jcode
        );
        assert_eq!(
            JevPurpose::Memory
                .selector_with(
                    |key| {
                        assert_eq!(key, PROVIDER_ENV);
                        Err(std::env::VarError::NotPresent)
                    },
                    || "aimlapi".into(),
                )
                .unwrap(),
            "aimlapi"
        );
        for purpose in [JevPurpose::Memory, JevPurpose::Browser] {
            assert!(
                purpose
                    .selector_with(
                        |_| Err(std::env::VarError::NotUnicode("invalid".into())),
                        || panic!("invalid override must not use defaults"),
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn browser_choice_contract_is_distinct_from_memory_noul() {
        let valid = browser_questions();
        for provider in [
            JevProvider::Jcode,
            JevProvider::OpenRouter,
            JevProvider::TypeSafe,
            JevProvider::Aimlapi,
        ] {
            assert!(request_body_for(JevPurpose::Browser, provider, json!("page"), &valid).is_ok());
            assert!(
                request_body_for(JevPurpose::Browser, provider, json!("page"), &questions())
                    .is_err()
            );
            for invalid in [
                json!({}),
                json!({"pick": valid["action"]}),
                json!({"action": valid["action"], "extra": valid["action"]}),
                json!({"action": {"type": "noul", "instructions": "Pick", "criteria": {"a": "A", "b": "B"}}}),
                json!({"action": {"type": "choice", "instructions": {}, "criteria": {"a": "A", "b": "B"}}}),
                json!({"action": {"type": "choice", "instructions": "  ", "criteria": {"a": "A", "b": "B"}}}),
                json!({"action": {"type": "choice", "instructions": "Pick", "criteria": {"a": "A"}}}),
                json!({"action": {"type": "choice", "instructions": "Pick", "criteria": {"a": "A", "b": null}}}),
            ] {
                assert!(
                    request_body_for(
                        JevPurpose::Browser,
                        provider,
                        json!("page"),
                        invalid.as_object().unwrap()
                    )
                    .is_err(),
                    "{invalid}"
                );
            }
            for count in [255, 256] {
                let mut q = valid.clone();
                q.get_mut("action").unwrap()["criteria"] = Value::Object(
                    (0..count)
                        .map(|i| (i.to_string(), json!("option")))
                        .collect(),
                );
                assert_eq!(
                    request_body_for(JevPurpose::Browser, provider, json!("page"), &q).is_ok(),
                    count == 255
                );
            }
        }
        assert!(request_body(JevProvider::Jcode, json!("state"), &valid).is_err());
        assert!(request_body(JevProvider::Jcode, json!("state"), &questions()).is_ok());
    }

    #[test]
    fn resolver_keeps_provider_credentials_and_endpoints_isolated() {
        for (selector, expected, key, file, endpoint, model) in [
            (
                "openrouter",
                JevProvider::OpenRouter,
                "OPENROUTER_API_KEY",
                "openrouter.env",
                "https://openrouter.ai/api/alpha/decisions",
                "typesafe/jev-1.13",
            ),
            (
                "typesafe",
                JevProvider::TypeSafe,
                "TYPESAFE_API_KEY",
                "typesafe.env",
                "https://api.typesafe.ai/v1/systemone",
                "jev-latest",
            ),
            (
                "aimlapi",
                JevProvider::Aimlapi,
                "AIMLAPI_API_KEY",
                "aimlapi.env",
                "https://api.aimlapi.com/v1/decisions",
                "typesafe/jev",
            ),
            (
                "jcode",
                JevProvider::Jcode,
                "JCODE_API_KEY",
                "jcode-subscription.env",
                "https://api.jcode.sh/v1/decisions",
                "typesafe/jev-1.13",
            ),
        ] {
            let (provider, secret) = resolve_with(selector, |env, env_file| {
                assert_eq!((env, env_file), (key, file));
                Some(format!("test-{selector}"))
            })
            .unwrap();
            assert_eq!(provider, expected);
            assert_eq!(secret, format!("test-{selector}"));
            assert_eq!(
                provider.endpoint("https://api.jcode.sh/v1/").unwrap(),
                endpoint
            );
            assert_eq!(provider.model(), model);
        }
    }

    #[test]
    fn explicit_missing_provider_never_uses_another_key() {
        let mut lookups = Vec::new();
        let error = resolve_with("typesafe", |env, _| {
            lookups.push(env.to_string());
            (env != "TYPESAFE_API_KEY").then(|| "other-provider-secret".into())
        })
        .err()
        .unwrap();
        assert_eq!(lookups, ["TYPESAFE_API_KEY"]);
        assert!(!error.to_string().contains("other-provider-secret"));
        assert!(
            error
                .to_string()
                .contains("TYPESAFE_API_KEY (typesafe.env)")
        );
        assert!(!error.to_string().contains("OPENROUTER_API_KEY"));
    }

    #[test]
    fn auto_prefers_included_subscription_without_shared_slot() {
        for available in [
            "OPENROUTER_API_KEY",
            "TYPESAFE_API_KEY",
            "AIMLAPI_API_KEY",
            "JCODE_API_KEY",
        ] {
            let (provider, _) = resolve_with("auto", |env, _| {
                assert!(!env.starts_with("JCODE_OPENROUTER"));
                (env == available).then(|| "test-secret".into())
            })
            .unwrap();
            assert_eq!(provider.credentials().0, available);
        }
        assert_eq!(
            resolve_with("auto", |_, _| Some("all-present".into()))
                .unwrap()
                .0,
            JevProvider::Jcode
        );
        // Typesafe direct wins over resellers of the same model.
        assert_eq!(
            resolve_with("auto", |env, _| (env != "JCODE_API_KEY").then(|| "k".into()))
                .unwrap()
                .0,
            JevProvider::TypeSafe
        );
        // Deliberate BYOK remains available even when a Jcode login is present.
        assert_eq!(
            resolve_with("openrouter", |_, _| Some("all-present".into()))
                .unwrap()
                .0,
            JevProvider::OpenRouter
        );
        assert!(resolve_with("auto", |_, _| None).is_err());
        assert!(
            resolve_with("untrusted-selector", |_, _| panic!(
                "must not load any credential"
            ))
            .is_err()
        );
        assert!(
            resolve_with("", |_, _| panic!(
                "empty override must not choose another account"
            ))
            .is_err()
        );
    }

    #[test]
    fn empty_and_malformed_credentials_are_rejected_without_echoing() {
        assert!(resolve_with("openrouter", |_, _| Some(" \"\" ".into())).is_err());
        let secret = "sensitive\r\nheader";
        let error = resolve_with("openrouter", |_, _| Some(secret.into()))
            .err()
            .unwrap();
        assert!(!format!("{error:#}").contains("sensitive"));
    }

    #[test]
    fn gateway_requires_secure_trusted_base() {
        for base in [
            "https://api.jcode.sh/v1",
            "https://custom.example/v1/",
            "http://127.0.0.1:4444/v1",
            "http://[::1]:4444/v1",
            "http://localhost/v1",
        ] {
            assert!(trusted_gateway_base(base).is_ok(), "{base}");
        }
        for base in [
            "http://remote.example/v1",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#secret",
            "file:///secret",
            "not-a-url",
        ] {
            assert!(trusted_gateway_base(base).is_err());
        }
    }

    #[test]
    fn request_uses_decisions_not_chat_and_preserves_provider_models() {
        for provider in [
            JevProvider::OpenRouter,
            JevProvider::TypeSafe,
            JevProvider::Aimlapi,
            JevProvider::Jcode,
        ] {
            let body: Value = serde_json::from_slice(
                &request_body(provider, json!({"memory": "example"}), &questions()).unwrap(),
            )
            .unwrap();
            assert_eq!(body["model"], provider.model());
            assert_eq!(body["questions"], Value::Object(questions()));
            assert!(body.get("messages").is_none());
        }
    }

    #[test]
    fn request_bounds_and_gateway_contract_are_checked_before_network() {
        assert!(
            request_body(
                JevProvider::OpenRouter,
                json!("x".repeat(MAX_REQUEST_BYTES)),
                &questions()
            )
            .is_err()
        );
        assert!(request_body(JevProvider::OpenRouter, Value::Null, &questions()).is_err());
        assert!(request_body(JevProvider::OpenRouter, json!("state"), &Map::new()).is_err());
        let many = (0..25)
            .map(|i| (format!("m{i}"), questions()["m0"].clone()))
            .collect();
        assert!(request_body(JevProvider::Jcode, json!("state"), &many).is_err());
        let choice = json!({"pick": {"type": "choice", "instructions": "Pick", "criteria": {"a": "A", "b": "B"}}}).as_object().unwrap().clone();
        assert!(request_body(JevProvider::OpenRouter, json!("state"), &choice).is_ok());
        assert!(request_body(JevProvider::Jcode, json!("state"), &choice).is_err());
        let mut missing_criteria = questions();
        missing_criteria
            .get_mut("m0")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("criteria");
        assert!(request_body(JevProvider::Jcode, json!("state"), &missing_criteria).is_err());
    }

    #[test]
    fn answers_must_match_questions_and_have_valid_noul_probabilities() {
        assert!(validate_answers(&response(), &questions()).is_ok());
        for value in [
            json!({}),
            json!({"answers": {}}),
            json!({"answers": {"other": {"type": "noul", "noul": 0.9}}}),
            json!({"answers": {"m0": {"type": "choice", "noul": 0.9}}}),
            json!({"answers": {"m0": {"type": "noul", "noul": 1.1}}}),
        ] {
            assert!(validate_answers(&value, &questions()).is_err());
        }
    }

    // One listener can serve preflight + decision, and captures the exact wire
    // requests without touching credentials, process environment, or real APIs.
    type MockReply = (u16, String, Vec<(String, String)>);

    fn mock_server(replies: Vec<MockReply>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let count = replies.len();
        let mut replies = replies.into_iter();
        mock_server_with(count, move |_| replies.next().unwrap())
    }

    /// Serve `count` requests, choosing each reply from the request itself, so
    /// concurrent requests get correct replies whatever order they connect in.
    fn mock_server_with(
        count: usize,
        mut respond: impl FnMut(&str) -> MockReply + Send + 'static,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..count {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "expected mock request"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                // On BSD/macOS an accepted socket inherits the listener's
                // non-blocking flag, so without this the timed read below
                // returns WouldBlock instead of waiting for the request.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let (status, body, headers) = respond(&request);
                requests.push(request);
                let chunked = headers.iter().any(|(key, value)| {
                    key.eq_ignore_ascii_case("transfer-encoding") && value == "chunked"
                });
                let extra: String = headers
                    .into_iter()
                    .map(|(k, v)| format!("{k}: {v}\r\n"))
                    .collect();
                let (length_header, wire_body) = if chunked {
                    (
                        String::new(),
                        format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len()),
                    )
                } else {
                    (format!("Content-Length: {}\r\n", body.len()), body)
                };
                let reply = format!(
                    "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\n{length_header}Connection: close\r\n{extra}\r\n{wire_body}"
                );
                // A bounded client may close early on an oversized response.
                let _ = stream.write_all(reply.as_bytes());
            }
            requests
        });
        (base, worker)
    }

    fn mock_client(base: &str, provider: JevProvider) -> JevClient {
        JevClient {
            client: client_builder().no_proxy().build().unwrap(),
            hedge_clients: VOICE_HEDGE_DELAYS
                .iter()
                .map(|_| client_builder().no_proxy().build().unwrap())
                .collect(),
            purpose: JevPurpose::Memory,
            provider,
            api_key: "test-route-secret".into(),
            endpoint: format!("{base}/v1/decisions"),
            me_endpoint: (provider == JevProvider::Jcode).then(|| format!("{base}/v1/me")),
        }
    }

    #[tokio::test]
    async fn voice_subscription_uses_existing_noul_entitlement_without_bypassing_it() {
        for entitled in [false, true] {
            let mut replies = vec![(
                200,
                json!({"capabilities": {"memory_jev": entitled, "browser_jev": false}}).to_string(),
                vec![],
            )];
            if entitled {
                replies.push((200, response().to_string(), vec![]));
            }
            let (base, worker) = mock_server(replies);
            let mut client = mock_client(&base, JevProvider::Jcode);
            client.purpose = JevPurpose::Voice;
            let result = client
                .evaluate(json!({"transcript": "synthetic"}), questions())
                .await;
            let requests = worker.join().unwrap();
            assert!(requests[0].starts_with("GET /v1/me "));
            if entitled {
                result.unwrap();
                assert_eq!(requests.len(), 2);
                let body: Value =
                    serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(body["model"], "typesafe/jev-1.13");
                assert!(body["state"].is_string());
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("voice"));
                assert!(error.contains("memory_jev"));
                assert_eq!(
                    requests.len(),
                    1,
                    "denied voice entitlement must not send a decision"
                );
            }
        }
    }

    /// Serve every connection on its own thread. Connection `n` waits
    /// `delays[n]` before replying, so a slow primary can overlap a hedge.
    fn concurrent_mock(
        delays: Vec<(Duration, u16)>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                let Some(&(delay, status)) = delays.get(index) else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    std::thread::sleep(delay);
                    let body = response().to_string();
                    let _ = stream.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                });
            }
        });
        (base, seen)
    }

    #[tokio::test]
    async fn voice_hedges_a_slow_request_and_takes_the_first_valid_answer() {
        let (base, seen) =
            concurrent_mock(vec![(Duration::from_secs(3), 200), (Duration::ZERO, 200)]);
        let mut client = mock_client(&base, JevProvider::TypeSafe);
        client.purpose = JevPurpose::Voice;
        let started = std::time::Instant::now();
        let value = client
            .evaluate(json!({"transcript": "synthetic"}), questions())
            .await
            .unwrap();
        assert!(validate_answers(&value, &questions()).is_ok());
        assert!(started.elapsed() < Duration::from_secs(2), "hedge won");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn voice_hedge_failure_waits_for_primary_and_memory_never_hedges() {
        let (base, seen) = concurrent_mock(vec![
            (Duration::from_millis(300), 200),
            (Duration::ZERO, 500),
        ]);
        let mut client = mock_client(&base, JevProvider::TypeSafe);
        client.purpose = JevPurpose::Voice;
        client
            .evaluate(json!({"transcript": "synthetic"}), questions())
            .await
            .unwrap();
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);

        let (base, seen) = concurrent_mock(vec![(Duration::from_millis(300), 200)]);
        mock_client(&base, JevProvider::TypeSafe)
            .evaluate(json!({"transcript": "synthetic"}), questions())
            .await
            .unwrap();
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn voice_typesafe_errors_never_retry_or_return_a_partial_report() {
        for status in [401, 402, 403, 500, 302] {
            let (base, worker) = mock_server(vec![(
                status,
                "private-error test-route-secret".into(),
                vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())],
            )]);
            let mut client = mock_client(&base, JevProvider::TypeSafe);
            client.purpose = JevPurpose::Voice;
            let error =
                crate::voice_intent::classify_with_client("Start a new session", &[], &client)
                    .await
                    .unwrap_err()
                    .to_string();
            assert!(error.contains(&status.to_string()));
            assert!(!error.contains("test-route-secret"));
            assert!(!error.contains("private-error"));
            assert_eq!(worker.join().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn voice_direct_transport_preserves_typed_state_and_never_sends_openrouter_headers() {
        use crate::voice_intent::{
            QuickAction, VoiceIntent, classify_with_client, describe_questions,
        };
        let transcript = "Start a new Jcode conversation";
        let answers: Map<String, Value> = describe_questions(transcript, &[])
            .unwrap()
            .into_iter()
            .map(|question| {
                let probability = match question.id.as_str() {
                    "quick_action" | "new_session" => 0.99,
                    _ => 0.01,
                };
                (question.id, json!({"type": "noul", "noul": probability}))
            })
            .collect();
        let (base, worker) =
            mock_server(vec![(200, json!({"answers": answers}).to_string(), vec![])]);
        let mut client = mock_client(&base, JevProvider::TypeSafe);
        client.purpose = JevPurpose::Voice;
        client.endpoint = format!("{base}/v1/systemone");
        let report = classify_with_client(transcript, &[], &client)
            .await
            .unwrap();
        assert_eq!(
            report.intent,
            VoiceIntent::QuickAction(QuickAction::NewSession)
        );
        let requests = worker.join().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "direct API has no gateway entitlement preflight"
        );
        let request = &requests[0];
        assert!(request.starts_with("POST /v1/systemone "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-route-secret\r\n")
        );
        assert!(!request.to_ascii_lowercase().contains("http-referer:"));
        assert!(!request.to_ascii_lowercase().contains("x-title:"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["transcript"], transcript);
        assert_eq!(body["questions"].as_object().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn voice_twenty_candidates_cross_real_transport_in_bounded_same_state_batches() {
        use crate::voice_intent::{
            SessionCandidate, VoiceIntent, classify_with_client, describe_questions,
        };
        let offered: Vec<_> = (0..20)
            .map(|index| SessionCandidate {
                id: format!("private-session-{index}"),
                title: format!("Investigate voice routing request budgets and preserve classification safety for recent conversation {index}: compare candidate metadata, JSON escaping, provider transport limits, and regression coverage"),
                working_dir: Some(r#"C:\Users\example\projects\jcode\"voice routing""#.into()),
            })
            .collect();
        let transcript = "Open conversation 19";
        let questions = describe_questions(transcript, &offered).unwrap();
        assert_eq!(questions.len(), 26);
        // Exercise both object-state and string-state providers, including the
        // gateway's real entitlement preflight on every bounded evaluation.
        for provider in [
            JevProvider::TypeSafe,
            JevProvider::OpenRouter,
            JevProvider::Aimlapi,
            JevProvider::Jcode,
        ] {
            // All batches participate in ranking, with no competing-score veto.
            for competing in [0.01, 0.9, 0.99, 1.0] {
                let batches = questions.chunks(provider.max_questions()).count();
                let per_batch = if provider == JevProvider::Jcode { 2 } else { 1 };
                // Batches run concurrently, so answer whatever each request asks.
                let (base, worker) = mock_server_with(batches * per_batch, move |request| {
                    if request.starts_with("GET ") {
                        let me = json!({"capabilities": {"memory_jev": true}});
                        return (200, me.to_string(), vec![]);
                    }
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    let answers: Map<String, Value> = body["questions"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(|id| {
                            let probability = match id.as_str() {
                                "navigation" | "candidate_19" => 0.99,
                                "coding_agent" => competing,
                                _ => 0.01,
                            };
                            (id.clone(), json!({"type": "noul", "noul": probability}))
                        })
                        .collect();
                    (200, json!({"answers": answers}).to_string(), vec![])
                });
                let result =
                    classify_with_client(transcript, &offered, &mock_client(&base, provider))
                        .await
                        .unwrap();
                assert_eq!(
                    result.intent,
                    if competing < 0.99 {
                        VoiceIntent::OpenSession(offered[19].id.clone())
                    } else {
                        VoiceIntent::CodingAgent
                    }
                );
                assert_eq!(result.answers.len(), 26);
                assert_eq!(
                    result.answers.iter().map(|a| &a.id).collect::<Vec<_>>(),
                    questions.iter().map(|q| &q.id).collect::<Vec<_>>()
                );
                assert_eq!(
                    result
                        .answers
                        .iter()
                        .find(|a| a.id == "candidate_19")
                        .unwrap()
                        .probability,
                    0.99
                );
                let requests = worker.join().unwrap();
                let bodies: Vec<Value> = requests
                    .iter()
                    .filter(|r| r.starts_with("POST "))
                    .map(|r| {
                        let wire = r.split_once("\r\n\r\n").unwrap().1;
                        assert!(wire.len() <= MAX_REQUEST_BYTES);
                        assert!(wire.len() < 32 * 1024, "voice batch bytes: {}", wire.len());
                        serde_json::from_str(wire).unwrap()
                    })
                    .collect();
                assert_eq!(bodies.len(), batches);
                assert!(
                    bodies
                        .iter()
                        .all(|body| body["state"] == bodies[0]["state"])
                );
                let state = if let Some(text) = bodies[0]["state"].as_str() {
                    serde_json::from_str::<Value>(text).unwrap()
                } else {
                    bodies[0]["state"].clone()
                };
                assert_eq!(state["candidates"].as_object().unwrap().len(), 20);
                assert_eq!(state["transcript"], transcript);
                assert!(
                    state["policy"]
                        .as_str()
                        .unwrap()
                        .contains("untrusted evidence")
                );
                for (index, candidate) in offered.iter().enumerate() {
                    assert_eq!(
                        state["candidates"][format!("candidate_{index}")]["title"],
                        candidate.title
                    );
                    assert_eq!(
                        state["candidates"][format!("candidate_{index}")]["working_dir"],
                        candidate.working_dir.as_deref().unwrap()
                    );
                }
                assert!(!state.to_string().contains("private-session"));
                let mut sent = Map::new();
                for body in &bodies {
                    let batch = body["questions"].as_object().unwrap();
                    assert!(batch.len() <= provider.max_questions());
                    sent.extend(batch.clone());
                }
                assert_eq!(sent.len(), questions.len());
                for question in &questions {
                    assert_eq!(sent[&question.id]["instructions"], question.instructions);
                    assert_eq!(sent[&question.id]["criteria"]["true"], question.yes);
                    assert_eq!(sent[&question.id]["criteria"]["false"], question.no);
                }
            }
        }
    }

    #[tokio::test]
    async fn voice_later_batch_errors_never_return_partial_classification() {
        use crate::voice_intent::{SessionCandidate, classify_with_client, describe_questions};
        let offered: Vec<_> = (0..20)
            .map(|index| SessionCandidate {
                id: format!("session-{index}"),
                title: format!("Conversation {index}"),
                working_dir: None,
            })
            .collect();
        let questions = describe_questions("next conversation", &offered).unwrap();
        let first: Map<String, Value> = questions[..MAX_QUESTIONS].iter().map(|q| (q.id.clone(), json!({"type": "noul", "noul": if q.id == "coding_agent" { 0.99 } else { 0.01 }}))).collect();
        let last: Map<String, Value> = questions[MAX_QUESTIONS..]
            .iter()
            .map(|q| (q.id.clone(), json!({"type": "noul", "noul": 0.01})))
            .collect();
        let mut failures = vec![
            (500, "{}".into(), vec![]),
            (200, json!({"answers": {}}).to_string(), vec![]),
        ];
        for invalid in [
            json!({"type": "noul", "noul": 1.1}),
            json!({"type": "noul", "noul": "0.99"}),
            json!({"type": "choice", "choice": "yes"}),
        ] {
            let mut bad = last.clone();
            bad.insert(questions[MAX_QUESTIONS].id.clone(), invalid);
            failures.push((200, json!({"answers": bad}).to_string(), vec![]));
        }
        let mut wrong_ids = last.clone();
        wrong_ids.remove(&questions[MAX_QUESTIONS].id);
        wrong_ids.insert("invented".into(), json!({"type": "noul", "noul": 0.99}));
        failures.push((200, json!({"answers": wrong_ids}).to_string(), vec![]));
        for failure in failures {
            // Batches run concurrently, so reply by which batch was requested.
            let later = questions[MAX_QUESTIONS].id.clone();
            let good = json!({"answers": first}).to_string();
            let mut failure = Some(failure);
            let (base, worker) = mock_server_with(2, move |request| {
                let body: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                if body["questions"].get(&later).is_some() {
                    failure.take().unwrap()
                } else {
                    (200, good.clone(), vec![])
                }
            });
            assert!(
                classify_with_client(
                    "next conversation",
                    &offered,
                    &mock_client(&base, JevProvider::OpenRouter)
                )
                .await
                .is_err()
            );
            assert_eq!(worker.join().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn browser_subscription_checks_browser_capability_and_posts_choice() {
        let answer = json!({"answers": {"action": {"type": "choice", "choice": "click", "confidence": 0.9}}});
        let (base, worker) = mock_server(vec![
            (
                200,
                json!({"capabilities": {"browser_jev": true, "memory_jev": false}}).to_string(),
                vec![],
            ),
            (200, answer.to_string(), vec![]),
        ]);
        let mut client = mock_client(&base, JevProvider::Jcode);
        client.purpose = JevPurpose::Browser;
        assert_eq!(client.provider_name(), "jcode");
        assert_eq!(client.model_id(), "typesafe/jev-1.13");
        assert_eq!(
            client
                .evaluate(json!({"page": "private-page"}), browser_questions())
                .await
                .unwrap(),
            answer
        );
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /v1/me "));
        assert!(!requests[0].contains("private-page"));
        assert!(requests[1].starts_with("POST /v1/decisions "));
        let body: Value =
            serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["questions"], Value::Object(browser_questions()));
        assert!(body["state"].is_string());
    }

    #[tokio::test]
    async fn browser_capability_denial_prevents_page_upload() {
        for me in [
            json!({"capabilities": {"memory_jev": true}}),
            json!({"capabilities": {"browser_jev": false}}),
            json!({"capabilities": {"browser_jev": "true"}}),
        ] {
            let (base, worker) = mock_server(vec![(200, me.to_string(), vec![])]);
            let mut client = mock_client(&base, JevProvider::Jcode);
            client.purpose = JevPurpose::Browser;
            let error = client
                .evaluate(json!("private-page"), browser_questions())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("browser_jev"));
            let requests = worker.join().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with("GET /v1/me "));
            assert!(!requests[0].contains("private-page"));
        }
    }

    #[tokio::test]
    async fn browser_auth_billing_and_redirect_failures_never_retry_or_fallback() {
        for provider in [JevProvider::Jcode, JevProvider::OpenRouter] {
            for preflight in [false, true] {
                if preflight && provider != JevProvider::Jcode {
                    continue;
                }
                for status in [401, 402, 403, 302, 307] {
                    let mut replies = Vec::new();
                    if provider == JevProvider::Jcode && !preflight {
                        replies.push((
                            200,
                            json!({"capabilities": {"browser_jev": true}}).to_string(),
                            vec![],
                        ));
                    }
                    replies.push((
                        status,
                        "private-provider-error test-route-secret".into(),
                        vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())],
                    ));
                    let expected_requests = replies.len();
                    let (base, worker) = mock_server(replies);
                    let mut client = mock_client(&base, provider);
                    client.purpose = JevPurpose::Browser;
                    let error = client
                        .evaluate(json!("private-page"), browser_questions())
                        .await
                        .unwrap_err();
                    let detail = format!("{error:#}");
                    assert!(detail.contains(&status.to_string()));
                    for secret in [
                        "private-provider-error",
                        "test-route-secret",
                        "private-page",
                    ] {
                        assert!(!detail.contains(secret));
                    }
                    let requests = worker.join().unwrap();
                    assert_eq!(requests.len(), expected_requests);
                    if preflight {
                        assert!(!requests[0].contains("private-page"));
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn subscription_requires_live_capability_then_sends_bound_bearer() {
        let (base, worker) = mock_server(vec![
            (
                200,
                json!({"capabilities": {"memory_jev": true}}).to_string(),
                vec![],
            ),
            (200, response().to_string(), vec![]),
        ]);
        let client = mock_client(&base, JevProvider::Jcode);
        let value = client.evaluate(json!("state"), questions()).await.unwrap();
        assert_eq!(value, response());
        let requests = worker.join().unwrap();
        assert!(requests[0].starts_with("GET /v1/me "));
        assert!(requests[1].starts_with("POST /v1/decisions "));
        for request in requests {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-route-secret\r\n")
            );
            assert!(!request.lines().next().unwrap().contains("secret"));
        }
    }

    #[tokio::test]
    async fn missing_or_false_capability_never_posts_decisions() {
        for me in [
            json!({"tier": "flagship", "status": "active"}),
            json!({"capabilities": {"memory_jev": false}}),
            json!({"capabilities": {"memory_jev": "true"}}),
        ] {
            let (base, worker) = mock_server(vec![(200, me.to_string(), vec![])]);
            let client = mock_client(&base, JevProvider::Jcode);
            let error = client
                .evaluate(json!("state"), questions())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("memory_jev"));
            assert_eq!(worker.join().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn transient_overload_is_retried_on_same_route_then_succeeds() {
        for status in [429, 502, 503, 504, 529] {
            let (base, worker) = mock_server(vec![
                (
                    status,
                    "{}".into(),
                    vec![("Retry-After".into(), "1".into())],
                ),
                (status, "{}".into(), vec![]),
                (200, response().to_string(), vec![]),
            ]);
            let client = mock_client(&base, JevProvider::OpenRouter);
            let value = client.evaluate(json!("state"), questions()).await.unwrap();
            assert_eq!(value, response());
            let requests = worker.join().unwrap();
            assert_eq!(requests.len(), 3);
            assert!(
                requests
                    .iter()
                    .all(|r| r.starts_with("POST /v1/decisions "))
            );
        }
    }

    #[tokio::test]
    async fn persistent_overload_fails_after_bounded_retries_without_echo() {
        for status in [429, 529] {
            let replies = (0..=TRANSIENT_RETRY_DELAYS.len())
                .map(|_| {
                    (
                        status,
                        "private-provider-error test-route-secret".into(),
                        vec![],
                    )
                })
                .collect();
            let (base, worker) = mock_server(replies);
            let client = mock_client(&base, JevProvider::OpenRouter);
            let detail = format!(
                "{:#}",
                client
                    .evaluate(json!("private-state"), questions())
                    .await
                    .unwrap_err()
            );
            assert!(detail.contains(&status.to_string()));
            assert!(detail.contains("overloaded"));
            assert!(!detail.contains("test-route-secret"));
            assert!(!detail.contains("private-state"));
            assert_eq!(
                worker.join().unwrap().len(),
                TRANSIENT_RETRY_DELAYS.len() + 1
            );
        }
    }

    #[tokio::test]
    async fn auth_billing_and_redirect_errors_are_redacted_and_never_retried() {
        for status in [401, 402, 403, 404, 500, 302, 307] {
            let headers = vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())];
            let (base, worker) = mock_server(vec![(
                status,
                "private-provider-error test-route-secret".into(),
                headers,
            )]);
            let client = mock_client(&base, JevProvider::OpenRouter);
            let error = client
                .evaluate(json!("private-state"), questions())
                .await
                .unwrap_err();
            let detail = format!("{error:#}");
            assert!(detail.contains(&status.to_string()));
            assert!(!detail.contains("test-route-secret"));
            assert!(!detail.contains("private-provider-error"));
            assert!(!detail.contains("private-state"));
            assert_eq!(worker.join().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn oversized_and_invalid_json_responses_are_rejected_without_echo() {
        for body in [
            "x".repeat(MAX_RESPONSE_BYTES + 1),
            "private-response-invalid-json".into(),
        ] {
            let (base, worker) = mock_server(vec![(200, body, vec![])]);
            let client = mock_client(&base, JevProvider::TypeSafe);
            let error = client
                .evaluate(json!("state"), questions())
                .await
                .unwrap_err();
            assert!(!format!("{error:#}").contains("private-response"));
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn streamed_body_limit_is_enforced_without_content_length() {
        let (base, worker) = mock_server(vec![(
            200,
            "x".repeat(MAX_RESPONSE_BYTES + 1),
            vec![("Transfer-Encoding".into(), "chunked".into())],
        )]);
        let client = mock_client(&base, JevProvider::Aimlapi);
        let error = client
            .evaluate(json!("state"), questions())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("bounded response size"));
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn failed_preflight_does_not_send_decision_request() {
        for (status, body, headers) in [
            (401, "private-account-response".into(), vec![]),
            (200, "x".repeat(MAX_ME_BYTES + 1), vec![]),
            (
                307,
                "private-account-response".into(),
                vec![("Location".into(), "https://untrusted.example/me".into())],
            ),
        ] {
            let (base, worker) = mock_server(vec![(status, body, headers)]);
            let client = mock_client(&base, JevProvider::Jcode);
            let error = client
                .evaluate(json!("state"), questions())
                .await
                .unwrap_err();
            assert!(!format!("{error:#}").contains("private-account-response"));
            let requests = worker.join().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with("GET /v1/me "));
        }
    }
}
