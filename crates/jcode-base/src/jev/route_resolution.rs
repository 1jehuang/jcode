use super::*;

pub(super) fn resolve_voice_with(
    selector: &str,
    load: impl FnMut(&str, &str) -> Option<String>,
) -> Result<(JevProvider, String)> {
    match selector.trim().to_ascii_lowercase().as_str() {
        "auto" => resolve_providers(&[JevProvider::Jcode, JevProvider::TypeSafe], load),
        "typesafe" | "laya" | "jcode" | "subscription" | "jcode-subscription" => {
            resolve_with(selector, load)
        }
        _ => bail!(
            "Invalid voice Jev provider. Choose auto, typesafe, laya, or jcode; voice never uses OpenRouter or AIMLAPI"
        ),
    }
}

pub(super) fn resolve_with(
    selector: &str,
    mut load: impl FnMut(&str, &str) -> Option<String>,
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
        "laya" => return resolve_laya_with(&mut load),
        "aimlapi" => &[JevProvider::Aimlapi],
        "jcode" | "subscription" | "jcode-subscription" => &[JevProvider::Jcode],
        _ => bail!(
            "Invalid Jev provider. Choose auto, laya, openrouter, typesafe, aimlapi, or jcode"
        ),
    };
    resolve_providers(providers, load)
}

pub(super) fn resolve_providers(
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

pub(super) fn trusted_gateway_base(base: &str) -> Result<String> {
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
