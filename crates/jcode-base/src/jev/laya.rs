use super::{
    JevClient, JevProvider, JevPurpose, LAYA_BASE_URL_ENV, LAYA_DEFAULT_BASE, client_builder_for,
    resolve_jev_model, resolve_providers, resolve_with,
};
use anyhow::{Result, anyhow, ensure};
use reqwest::Url;
use serde_json::Value;

impl JevClient {
    /// Suggestions routing prefers explicit OpenJev SystemOne (`OPENJEV_API_KEY`),
    /// then Jcode / TypeSafe / OpenRouter / AIMLAPI when `auto` is selected.
    /// Evaluation never changes accounts after an auth/billing failure.
    pub fn for_suggestions() -> Result<Self> {
        Self::for_purpose(JevPurpose::Suggestions)
    }

    pub fn for_suggestions_with_selector(config_selector: &str) -> Result<Self> {
        let (provider, api_key, endpoint, me_endpoint) =
            Self::resolve_with_suggestions_default(config_selector)?;
        Self::build_suggestions_client(provider, api_key, endpoint, me_endpoint)
    }

    pub fn for_suggestions_fixed_selector(selector: &str) -> Result<Self> {
        let (provider, api_key, endpoint, me_endpoint) =
            Self::resolve_for_selector(JevPurpose::Suggestions, selector)?;
        Self::build_suggestions_client(provider, api_key, endpoint, me_endpoint)
    }

    fn build_suggestions_client(
        provider: JevProvider,
        api_key: String,
        endpoint: String,
        me_endpoint: Option<String>,
    ) -> Result<Self> {
        let bypass_proxy = should_bypass_proxy(provider, &endpoint);
        let client = client_builder_for(bypass_proxy)
            .build()
            .map_err(|_| anyhow!("Could not initialize the Jev decision client"))?;
        Ok(Self {
            client,
            hedge_clients: Vec::new(),
            purpose: JevPurpose::Suggestions,
            provider,
            model_id: resolve_jev_model(JevPurpose::Suggestions, provider),
            api_key,
            endpoint,
            me_endpoint,
        })
    }

    /// True when the active route supports direct SystemOne `choice` questions
    /// (`OpenJev`, `TypeSafe`, `OpenRouter`, `Aimlapi`). `Jcode` subscription
    /// decisions only accept `noul` outside of Browser mode.
    pub fn supports_choice_questions(&self) -> bool {
        self.provider != JevProvider::Jcode || self.purpose == JevPurpose::Browser
    }

    /// True when `JCODE_LAYA_BASE_URL`, `LAYA_BASE_URL`, or `LAYA_API_KEY` is configured in env or `laya.env`.
    pub fn has_configured_laya_route() -> bool {
        has_configured_laya(&mut |env, file| {
            crate::provider_catalog::load_env_value_from_env_or_config(env, file)
        })
    }
}

pub(super) fn validate_choice_answer(question: &Value, answer: &Value) -> Result<()> {
    let criteria = question["criteria"]
        .as_object()
        .ok_or_else(|| anyhow!("Jev choice question is missing criteria"))?;
    let choice = answer["choice"]
        .as_str()
        .ok_or_else(|| anyhow!("Jev choice answer omitted selected choice"))?;
    ensure!(
        criteria.contains_key(choice),
        "Jev choice answer selected an unknown option"
    );
    if let Some(conf) = answer.get("confidence").and_then(Value::as_f64) {
        ensure!(
            conf.is_finite() && (0.0..=1.0).contains(&conf),
            "Jev returned an invalid choice confidence"
        );
    }
    if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
        for (opt, prob) in probs {
            ensure!(
                criteria.contains_key(opt),
                "Jev returned probability for an unknown choice option"
            );
            ensure!(
                prob.as_f64()
                    .is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
                "Jev returned an invalid choice probability"
            );
        }
    }
    Ok(())
}

/// Local/private HTTP endpoints (loopback, LAN, Tailscale) must connect directly
/// rather than routing plaintext request bodies and bearer tokens through an
/// ambient `HTTP_PROXY` / `ALL_PROXY`.
pub(super) fn should_bypass_proxy(provider: JevProvider, endpoint: &str) -> bool {
    endpoint.starts_with("http://")
        || (provider == JevProvider::Laya
            && match Url::parse(endpoint) {
                Ok(url) => url.host_str().is_none_or(is_trusted_local_or_private_host),
                Err(_) => true,
            })
}

pub(super) fn resolve_suggestions_with(
    selector: &str,
    mut load: impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    let providers: &[JevProvider] = match selector.trim().to_ascii_lowercase().as_str() {
        "auto" => &[
            JevProvider::OpenJev,
            JevProvider::Jcode,
            JevProvider::TypeSafe,
            JevProvider::OpenRouter,
            JevProvider::Aimlapi,
        ],
        "openjev" => &[JevProvider::OpenJev],
        "laya" => return resolve_laya_with(&mut load),
        _ => return resolve_with(selector, load),
    };
    match resolve_providers(providers, &mut load) {
        Ok(found) => Ok(found),
        Err(err) if selector.trim().eq_ignore_ascii_case("auto") => {
            if has_configured_laya(&mut load) {
                resolve_laya_with(&mut load)
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

pub(super) fn has_configured_laya(load: &mut impl FnMut(&str, &str) -> Option<String>) -> bool {
    for env_key in [LAYA_BASE_URL_ENV, "LAYA_BASE_URL", "LAYA_API_KEY"] {
        if let Some(val) = load(env_key, "laya.env")
            && !jcode_provider_env::sanitize_secret_value(&val).is_empty()
        {
            return true;
        }
    }
    false
}

pub(super) fn resolve_laya_with(
    load: &mut impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    let (env, file) = JevProvider::Laya.credentials();
    if let Some(key) = load(env, file) {
        let key = jcode_provider_env::sanitize_secret_value(&key);
        if !key.is_empty() {
            ensure!(
                reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).is_ok(),
                "The selected Jev provider credential is not a valid HTTP bearer value"
            );
            return Ok((JevProvider::Laya, key.to_owned()));
        }
    }
    Ok((JevProvider::Laya, String::new()))
}

pub(super) fn configured_laya_base() -> String {
    if let Some(base) =
        crate::provider_catalog::load_env_value_from_env_or_config(LAYA_BASE_URL_ENV, "laya.env")
            .or_else(|| {
                crate::provider_catalog::load_env_value_from_env_or_config(
                    "LAYA_BASE_URL",
                    "laya.env",
                )
            })
    {
        let trimmed = jcode_provider_env::sanitize_secret_value(&base);
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    LAYA_DEFAULT_BASE.to_owned()
}

pub(super) fn is_trusted_local_or_private_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".ts.net")
        || lower.ends_with(".local")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| match ip {
                std::net::IpAddr::V4(v4) => {
                    v4.is_loopback()
                        || v4.is_private()
                        || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
                }
                std::net::IpAddr::V6(v6) => {
                    v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00
                }
            })
}

pub(super) fn trusted_laya_base(base: &str) -> Result<String> {
    let url = Url::parse(base.trim()).map_err(|_| anyhow!("Invalid Laya base URL"))?;
    let trusted_http_host = url.host_str().is_some_and(is_trusted_local_or_private_host);
    ensure!(
        url.host_str().is_some()
            && (url.scheme() == "https" || (url.scheme() == "http" && trusted_http_host))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Laya endpoint requires HTTPS (HTTP only for loopback, private LAN, or Tailscale), without URL credentials, query, or fragment"
    );
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) fn laya_endpoint(base: &str) -> Result<String> {
    let effective = if base.trim().is_empty() {
        LAYA_DEFAULT_BASE
    } else {
        base.trim()
    };
    let root = trusted_laya_base(effective)?;
    if let Some(stripped) = root
        .strip_suffix("/v1/systemone")
        .or_else(|| root.strip_suffix("/v1/decisions"))
        .or_else(|| root.strip_suffix("/v1"))
    {
        Ok(format!("{stripped}/v1/systemone"))
    } else {
        Ok(format!("{root}/v1/systemone"))
    }
}
