use super::{JevProvider, LAYA_BASE_URL_ENV, LAYA_DEFAULT_BASE};
use anyhow::{Result, anyhow, ensure};
use reqwest::Url;

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
