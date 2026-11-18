use super::*;

/// Read an HTTP error body without hiding failures behind an empty string.
///
/// This is useful after a non-success status when the response is about to be
/// converted into an error. If reading the body itself fails, the returned text
/// preserves that failure so callers can include it in their error message.
pub async fn http_error_body(response: reqwest::Response, context: &str) -> String {
    match response.text().await {
        Ok(body) => body,
        Err(err) => format!("<failed to read {context} response body: {err}>"),
    }
}

/// Shared HTTP client for all generic provider requests. Creating a `reqwest::Client` is expensive
/// (~10ms due to TLS init, connection pool setup), so we reuse a single instance. Provider-specific
/// transports may override the User-Agent on individual requests when they intentionally need to
/// match an official client.
pub fn shared_http_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(JCODE_USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Some(Duration::from_secs(30)))
                // Proactively detect half-dead pooled HTTP/2 connections before we
                // reuse them. Without keepalive pings, a stale multiplexed connection
                // (common behind NAT/VPN/proxy or flaky Wi-Fi) surfaces as
                // "http2 error: stream error received: unspecific protocol error".
                // Pinging while idle lets reqwest drop the connection instead.
                .http2_keep_alive_interval(Some(Duration::from_secs(30)))
                .http2_keep_alive_timeout(Duration::from_secs(15))
                .http2_keep_alive_while_idle(true)
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(8)
                .build()
                .unwrap_or_else(|err| {
                    eprintln!("jcode: failed to build shared provider HTTP client: {err}");
                    match reqwest::Client::builder()
                        .user_agent(JCODE_USER_AGENT)
                        .build()
                    {
                        Ok(client) => client,
                        Err(fallback_err) => {
                            eprintln!(
                                "jcode: failed to build fallback provider HTTP client: {fallback_err}"
                            );
                            reqwest::Client::new()
                        }
                    }
                })
        })
        .clone()
}

/// Fresh HTTP client for transport-fault retries.
///
/// Retrying on the shared pooled client can reuse *other* idle connections
/// established through the same broken network path (corrupting middlebox,
/// flaky NAT/VPN) that produced a TLS fault like `BadRecordMac` - so the
/// retry fails the same way. This client disables connection pooling, which
/// guarantees the retry opens a brand-new TCP+TLS connection (the property
/// that makes transport-fault retries actually succeed). Building a client
/// costs ~10ms, which is fine on a retry path that already backs off >=1s.
pub fn fresh_transport_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(JCODE_USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Some(Duration::from_secs(30)))
        .http2_keep_alive_timeout(Duration::from_secs(15))
        .http2_keep_alive_while_idle(true)
        // No pooled reuse: every request gets a fresh connection.
        .pool_max_idle_per_host(0)
        .build()
        .unwrap_or_else(|_| shared_http_client())
}
