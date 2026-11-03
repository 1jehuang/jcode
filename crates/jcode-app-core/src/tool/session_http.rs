//! Session tool HTTP only. Never use these clients for provider or daemon work.
use anyhow::{Result, anyhow};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{LazyLock, Mutex},
    time::Duration,
};

const CAPACITY: usize = 32;
const KEYS: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

#[derive(Clone, PartialEq, Eq)]
struct ProxyConfig {
    http: Option<String>,
    https: Option<String>,
    no: Option<String>,
}
impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxyConfig([REDACTED])")
    }
}
impl ProxyConfig {
    fn resolve(
        env: &HashMap<String, String>,
        daemon: impl Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        if !KEYS.iter().any(|key| env.contains_key(*key)) {
            return None;
        }
        // Layer each exact key first, then apply reqwest's uppercase-first aliases.
        let get = |upper: &str, lower: &str| {
            env.get(upper)
                .cloned()
                .or_else(|| daemon(upper))
                .or_else(|| env.get(lower).cloned().or_else(|| daemon(lower)))
        };
        if env.contains_key("REQUEST_METHOD") || daemon("REQUEST_METHOD").is_some() {
            return Some(Self {
                http: None,
                https: None,
                no: None,
            });
        }
        let all = get("ALL_PROXY", "all_proxy").filter(|v| !v.is_empty());
        Some(Self {
            http: get("HTTP_PROXY", "http_proxy")
                .filter(|v| !v.is_empty())
                .or_else(|| all.clone()),
            https: get("HTTPS_PROXY", "https_proxy")
                .filter(|v| !v.is_empty())
                .or(all),
            no: get("NO_PROXY", "no_proxy"),
        })
    }
    fn apply(&self, mut builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
        builder = builder.no_proxy();
        let no = self.no.as_deref().and_then(reqwest::NoProxy::from_string);
        for (https, value) in [(false, &self.http), (true, &self.https)] {
            if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
                let value = match reqwest::Url::parse(value) {
                    Ok(url) if url.has_host() => Ok(url),
                    Ok(_) | Err(url::ParseError::RelativeUrlWithoutBase) => {
                        reqwest::Url::parse(&format!("http://{value}"))
                    }
                    Err(error) => Err(error),
                }
                .map_err(|_| anyhow!("Invalid session HTTP proxy configuration"))?;
                // Reqwest accepts arbitrary URL schemes here, but its matcher
                // silently ignores unsupported schemes. Never fail open.
                if !matches!(
                    value.scheme(),
                    "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
                ) {
                    return Err(anyhow!("Unsupported session HTTP proxy protocol"));
                }
                let proxy = if https {
                    reqwest::Proxy::https(value)
                } else {
                    reqwest::Proxy::http(value)
                }
                .map_err(|_| anyhow!("Invalid session HTTP proxy configuration"))?;
                builder = builder.proxy(proxy.no_proxy(no.clone()));
            }
        }
        Ok(builder)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    Shared,
    Custom,
    Kagi,
}
struct Entry {
    config: ProxyConfig,
    profile: Profile,
    client: reqwest::Client,
}
static CACHE: LazyLock<Mutex<VecDeque<Entry>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn builder(profile: Profile) -> reqwest::ClientBuilder {
    match profile {
        Profile::Shared => reqwest::Client::builder()
            .user_agent(crate::provider::JCODE_USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Some(Duration::from_secs(30)))
            .http2_keep_alive_timeout(Duration::from_secs(15))
            .http2_keep_alive_while_idle(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8),
        Profile::Custom => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30)),
        Profile::Kagi => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(30)),
    }
}

pub(crate) fn client(session: &str, profile: Profile) -> Result<reqwest::Client> {
    clients(session, &[profile]).map(|mut clients| clients.remove(0))
}

pub(crate) fn clients(session: &str, profiles: &[Profile]) -> Result<Vec<reqwest::Client>> {
    crate::session_env::with_session_env(session, |env| {
        profiles
            .iter()
            .map(|&profile| select(env, profile))
            .collect()
    })
}

fn daemon_env(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            crate::logging::warn(&format!(
                "Ignoring non-Unicode daemon environment variable {key}"
            ));
            None
        }
    }
}

fn select(env: &HashMap<String, String>, profile: Profile) -> Result<reqwest::Client> {
    let config = ProxyConfig::resolve(env, daemon_env);
    let Some(config) = config else {
        return if profile == Profile::Shared {
            Ok(crate::provider::shared_http_client())
        } else {
            builder(profile)
                .build()
                .map_err(|_| anyhow!("Tool HTTP client initialization failed"))
        };
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = cache
        .iter()
        .position(|e| e.config == config && e.profile == profile)
    {
        let entry = cache
            .remove(index)
            .ok_or_else(|| anyhow!("Session HTTP cache entry disappeared"))?;
        let client = entry.client.clone();
        cache.push_back(entry);
        return Ok(client);
    }
    let client = config
        .apply(builder(profile))?
        .build()
        .map_err(|_| anyhow!("Session HTTP client initialization failed"))?;
    if cache.len() == CAPACITY {
        cache.pop_front();
    }
    cache.push_back(Entry {
        config,
        profile,
        client: client.clone(),
    });
    Ok(client)
}

// Called while the registry write lock is held. Selection holds its read lock,
// so replacement cannot race a stale cache insertion. In-flight clones survive.
pub(crate) fn prune(envs: &HashMap<String, HashMap<String, String>>) {
    let configs: Vec<_> = envs
        .values()
        .filter_map(|env| ProxyConfig::resolve(env, daemon_env))
        .collect();
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|entry| configs.contains(&entry.config));
}

pub(crate) fn transport_error(_: reqwest::Error) -> anyhow::Error {
    // Do not retain the source chain: proxy transport errors can embed credentials.
    anyhow!("Tool HTTP request or response failed")
}

#[cfg(test)]
#[path = "session_http_tests.rs"]
mod tests;
