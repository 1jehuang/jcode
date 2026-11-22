use std::future::Future;
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use tokio::process::Command;
use tokio::time::sleep;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use tokio::time::timeout;

/// ConnectionPhase prefix emitted by the server/agent when paused waiting for
/// network connectivity to return. Clients parse this via
/// `parse_waiting_for_network_connection_phase` to render the waiting status and
/// exempt the turn from client-side stall cancellation.
pub const WAITING_FOR_NETWORK_PHASE_PREFIX: &str = "waiting for network";

/// Maximum consecutive network-error retries allowed when our connectivity
/// probes indicate the machine is already online (e.g. brief Wi-Fi roam, stale
/// pooled socket, or a provider-specific outage). Offline waits do not consume
/// this budget and reset it once connectivity returns.
pub const MAX_ONLINE_NETWORK_RETRIES: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkWaitPlan {
    pub reason: String,
    pub listener_summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetworkRecoveryTracker {
    online_retries: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkRecoveryOutcome {
    /// Connectivity was lost, we waited until it came back online, and the
    /// caller should retry the request.
    RestoredAfterOffline,
    /// Connectivity appears online (e.g. brief Wi-Fi roam, stale pooled socket,
    /// or network already recovered while the request was failing). Caller
    /// should retry after the short backoff (`attempt` of `max`).
    RetryingWhileOnline { attempt: u32, max: u32 },
    /// Connectivity stayed online and we exhausted `MAX_ONLINE_NETWORK_RETRIES`
    /// consecutive retries against this endpoint. Give up and surface the error.
    OnlineRetryBudgetExhausted { max: u32 },
    /// Cancelled while waiting for connectivity or backoff.
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkWaitNotice {
    /// Connectivity probe reported offline; waiting until online.
    Offline { listener_summary: String },
    /// Connectivity probe reported online; retrying after a brief backoff.
    OnlineRetry {
        attempt: u32,
        max: u32,
        delay_secs: u64,
    },
}

pub fn waiting_for_network_connection_phase(listener_summary: &str) -> String {
    format!("{WAITING_FOR_NETWORK_PHASE_PREFIX}: {listener_summary}")
}

pub fn parse_waiting_for_network_connection_phase(phase: &str) -> Option<String> {
    if phase == WAITING_FOR_NETWORK_PHASE_PREFIX {
        return Some(wait_plan().listener_summary);
    }
    phase
        .strip_prefix(&format!("{WAITING_FOR_NETWORK_PHASE_PREFIX}: "))
        .map(|listener| listener.to_string())
}

/// True when the provider stream was already established and timed out waiting
/// for the model/upstream to emit the next event, rather than failing due to a
/// local network disconnect.
pub fn is_stream_idle_timeout_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("stream read timeout: no data received")
        || lower.contains("timed out waiting for next websocket activity")
        || lower.contains("did not complete within")
}

#[cfg(test)]
pub mod test_hooks {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub type ProbeFn = Arc<dyn Fn() -> bool + Send + Sync>;
    pub type WaitFn =
        Arc<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

    #[derive(Clone)]
    pub struct TestNetworkHooks {
        pub probe: ProbeFn,
        pub wait: WaitFn,
        pub online_backoff: Duration,
    }

    impl TestNetworkHooks {
        pub fn from_probe_sequence(sequence: Vec<bool>) -> Self {
            let state = Arc::new(Mutex::new((sequence, 0usize)));
            let probe: ProbeFn = Arc::new(move || {
                let mut guard = state.lock().expect("test network probe mutex poisoned");
                let (seq, idx) = &mut *guard;
                let value = seq
                    .get(*idx)
                    .copied()
                    .unwrap_or_else(|| seq.last().copied().unwrap_or(true));
                if *idx + 1 < seq.len() {
                    *idx += 1;
                }
                value
            });
            let wait: WaitFn = Arc::new(|_| Box::pin(async { true }));
            Self {
                probe,
                wait,
                online_backoff: Duration::from_millis(1),
            }
        }
    }

    tokio::task_local! {
        pub(super) static ACTIVE_TEST_HOOKS: TestNetworkHooks;
    }

    pub async fn with_test_network_hooks<F, T>(hooks: TestNetworkHooks, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        ACTIVE_TEST_HOOKS.scope(hooks, fut).await
    }

    pub(super) fn current_hooks() -> Option<TestNetworkHooks> {
        match ACTIVE_TEST_HOOKS.try_with(Clone::clone) {
            Ok(hooks) => Some(hooks),
            Err(_) => None,
        }
    }
}

pub fn classify_network_interruption(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = Some(error);
    while let Some(err) = current {
        let text = err.to_string().to_ascii_lowercase();
        parts.push(text);
        current = err.source();
    }
    classify_text(&parts.join(" | "))
}

pub fn classify_message(message: &str) -> Option<String> {
    classify_text(&message.to_ascii_lowercase())
}

fn classify_text(text: &str) -> Option<String> {
    if is_stream_idle_timeout_message(text) {
        return None;
    }

    let non_network_markers = [
        "400 bad request",
        "401 unauthorized",
        "402 payment required",
        "403 forbidden",
        "404 not found",
        "404 not_found_error",
        "429 too many requests",
        "rate_limit",
        "rate limit exceeded",
        "usage_limit_reached",
        "insufficient_quota",
        "invalid_request_error",
        "context_length_exceeded",
        "context length exceeded",
        "content_policy_violation",
        "permission_denied",
        "invalid x-api-key",
    ];
    if non_network_markers
        .iter()
        .any(|marker| text.contains(marker))
    {
        return None;
    }

    let network_markers = [
        "connection reset",
        "connection aborted",
        "connection refused",
        "broken pipe",
        "network is down",
        "network is unreachable",
        "network unreachable",
        "network dropped connection",
        "software caused connection abort",
        "host is down",
        "no route to host",
        "not connected",
        // TLS-level drops. rustls reports an abrupt close as "peer closed
        // connection without sending TLS close_notify" (its docs URL spells
        // it "unexpected-eof", which the plain "unexpected eof" marker below
        // does not match).
        "close_notify",
        "peer closed connection",
        "tls handshake eof",
        // reqwest/hyper wrap connect-phase failures as "client error
        // (Connect)" and connection-level faults as "connection error: ...".
        "client error (connect)",
        "connection error",
        "dns error",
        "failed to lookup address",
        "temporary failure in name resolution",
        "name or service not known",
        "could not resolve host",
        "couldn't resolve host",
        "host is unreachable",
        "operation timed out",
        "timed out",
        "timeout",
        "error trying to connect",
        "error sending request for url",
        "error sending request",
        "connection closed before message completed",
        "unexpected eof",
        "end of file before message completed",
        // HTTP/2 faults on an established connection. A Wi-Fi roam breaks the
        // multiplexed stream without any timeout or reset wording: "client
        // error (SendRequest): http2 error: stream error received: unspecific
        // protocol error detected".
        "client error (sendrequest)",
        "http2 error",
        "error decoding response body",
        "request or response body error",
        "websocket protocol error",
        "websocket closed",
        "network unstable:",
        // Provider transport-send wrappers where outer Display omits the inner
        // reqwest/hyper cause chain.
        "failed to send antigravity generatecontent request",
        "failed to send openai-compatible chat request",
        "failed to send request to anthropic api",
        "failed to send request to openai api",
        "failed to send request to claude api",
        "failed to send cursor agent request headers",
    ];
    if network_markers.iter().any(|marker| text.contains(marker)) {
        return Some("the network connection appears to have dropped".to_string());
    }
    None
}

pub fn wait_plan() -> NetworkWaitPlan {
    #[cfg(target_os = "linux")]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for Linux netlink changes via `ip monitor`; also verifying with reconnect probes"
                    .to_string(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for macOS route/interface changes via `route -n monitor`; also verifying with reconnect probes"
                    .to_string(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary: "waiting with lightweight reconnect probes".to_string(),
        }
    }
}

impl NetworkRecoveryTracker {
    pub fn new() -> Self {
        Self { online_retries: 0 }
    }

    /// Call after an API request succeeds so future steps in the same turn get
    /// a fresh online retry budget.
    pub fn record_success(&mut self) {
        self.online_retries = 0;
    }

    /// Wait for network recovery after a network-classified error, supporting
    /// cancellation via `cancel_fut` and periodic keepalive notifications via
    /// `on_keepalive` (every `keepalive_interval`).
    ///
    /// - If connectivity probes report offline: emits `NetworkWaitNotice::Offline`
    ///   and waits until connectivity returns (without consuming the online retry
    ///   budget; resets the online retry budget when back online).
    /// - If connectivity probes report online: allows up to
    ///   `MAX_ONLINE_NETWORK_RETRIES` consecutive retries with short backoff
    ///   (`1s`, `2s`, `4s`) to ride out brief Wi-Fi roams or stale pooled sockets
    ///   before returning `OnlineRetryBudgetExhausted`.
    /// - When `offline_only` is true (e.g. an ambiguous empty EOF with no error
    ///   string), returns `OnlineRetryBudgetExhausted` immediately if we appear
    ///   online instead of retrying.
    pub async fn wait_for_recovery<CF, NF, KF>(
        &mut self,
        offline_only: bool,
        mut cancel_fut: CF,
        mut on_notice: NF,
        keepalive_interval: Duration,
        mut on_keepalive: KF,
    ) -> NetworkRecoveryOutcome
    where
        CF: Future<Output = ()> + Unpin,
        NF: FnMut(NetworkWaitNotice),
        KF: FnMut(),
    {
        #[cfg(test)]
        let test_hooks = match test_hooks::current_hooks() {
            Some(hooks) => hooks,
            None => {
                // In unit tests that do not install test network hooks, never
                // make live outbound probes or sleep on mock provider errors.
                return NetworkRecoveryOutcome::OnlineRetryBudgetExhausted {
                    max: MAX_ONLINE_NETWORK_RETRIES,
                };
            }
        };

        let initially_online = {
            #[cfg(test)]
            {
                (test_hooks.probe)()
            }
            #[cfg(not(test))]
            {
                tokio::select! {
                    biased;
                    _ = &mut cancel_fut => return NetworkRecoveryOutcome::Cancelled,
                    online = probe_connectivity() => online,
                }
            }
        };

        if !initially_online {
            let plan = wait_plan();
            on_notice(NetworkWaitNotice::Offline {
                listener_summary: plan.listener_summary,
            });

            let mut delay = Duration::from_secs(1);
            let mut keepalive = tokio::time::interval_at(
                tokio::time::Instant::now() + keepalive_interval,
                keepalive_interval,
            );
            keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                let changed_early = {
                    #[cfg(test)]
                    {
                        let wait_fut = async {
                            tokio::task::yield_now().await;
                            (test_hooks.wait)(delay).await
                        };
                        tokio::pin!(wait_fut);
                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut cancel_fut => return NetworkRecoveryOutcome::Cancelled,
                                _ = keepalive.tick() => {
                                    on_keepalive();
                                }
                                changed = &mut wait_fut => break changed,
                            }
                        }
                    }
                    #[cfg(not(test))]
                    {
                        let wait_fut = wait_for_platform_change_or_delay(delay);
                        tokio::pin!(wait_fut);
                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut cancel_fut => return NetworkRecoveryOutcome::Cancelled,
                                _ = keepalive.tick() => {
                                    on_keepalive();
                                }
                                changed = &mut wait_fut => break changed,
                            }
                        }
                    }
                };

                let online_now = {
                    #[cfg(test)]
                    {
                        (test_hooks.probe)()
                    }
                    #[cfg(not(test))]
                    {
                        tokio::select! {
                            biased;
                            _ = &mut cancel_fut => return NetworkRecoveryOutcome::Cancelled,
                            online = probe_connectivity() => online,
                        }
                    }
                };

                if online_now {
                    self.online_retries = 0;
                    return NetworkRecoveryOutcome::RestoredAfterOffline;
                }

                // If an OS interface/route event woke us early (e.g. Wi-Fi link
                // up before DHCP/DNS finished), reset backoff to 1s so we
                // notice connectivity as soon as DNS settles instead of waiting
                // 30s.
                if changed_early {
                    delay = Duration::from_secs(1);
                } else {
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }

        if offline_only {
            return NetworkRecoveryOutcome::OnlineRetryBudgetExhausted {
                max: MAX_ONLINE_NETWORK_RETRIES,
            };
        }

        if self.online_retries >= MAX_ONLINE_NETWORK_RETRIES {
            return NetworkRecoveryOutcome::OnlineRetryBudgetExhausted {
                max: MAX_ONLINE_NETWORK_RETRIES,
            };
        }

        self.online_retries += 1;
        let attempt = self.online_retries;
        let backoff_secs = 1u64 << (attempt - 1); // 1s, 2s, 4s
        on_notice(NetworkWaitNotice::OnlineRetry {
            attempt,
            max: MAX_ONLINE_NETWORK_RETRIES,
            delay_secs: backoff_secs,
        });

        #[cfg(test)]
        let sleep_dur = test_hooks.online_backoff;
        #[cfg(not(test))]
        let sleep_dur = Duration::from_secs(backoff_secs);

        let sleeper = sleep(sleep_dur);
        tokio::pin!(sleeper);
        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + keepalive_interval,
            keepalive_interval,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                _ = &mut cancel_fut => return NetworkRecoveryOutcome::Cancelled,
                _ = keepalive.tick() => {
                    on_keepalive();
                }
                _ = &mut sleeper => {
                    return NetworkRecoveryOutcome::RetryingWhileOnline {
                        attempt,
                        max: MAX_ONLINE_NETWORK_RETRIES,
                    };
                }
            }
        }
    }
}

pub async fn wait_until_probably_online() {
    wait_until_online_with(
        false,
        probe_connectivity,
        wait_for_platform_change_or_delay_unit,
    )
    .await;
}

/// A reachable internet probe on the first attempt is not a restoration:
/// a provider-only failure must keep its original backoff.
pub async fn wait_for_connectivity_restoration() {
    wait_until_online_with(
        true,
        probe_connectivity,
        wait_for_platform_change_or_delay_unit,
    )
    .await;
}

async fn wait_for_platform_change_or_delay_unit(delay: Duration) {
    let _changed = wait_for_platform_change_or_delay(delay).await;
}

async fn wait_until_online_with<P, W>(
    require_transition: bool,
    mut probe: impl FnMut() -> P,
    mut wait: impl FnMut(Duration) -> W,
) where
    P: std::future::Future<Output = bool>,
    W: std::future::Future<Output = ()>,
{
    let mut delay = Duration::from_secs(1);
    if probe().await {
        if require_transition {
            std::future::pending::<()>().await;
        }
        return;
    }
    loop {
        wait(delay).await;
        if probe().await {
            return;
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

pub async fn is_probably_online() -> bool {
    probe_connectivity().await
}

async fn probe_connectivity() -> bool {
    let client = jcode_provider_core::shared_http_client();
    let endpoints = [
        "https://www.gstatic.com/generate_204",
        "https://cp.cloudflare.com/generate_204",
    ];
    for endpoint in endpoints {
        let request = client.head(endpoint).timeout(Duration::from_secs(3));
        if matches!(
            request.send().await,
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204
        ) {
            return true;
        }
    }
    false
}

async fn wait_for_platform_change_or_delay(delay: Duration) -> bool {
    #[cfg(target_os = "linux")]
    {
        if command_exists("ip").await {
            let fut = wait_for_command_output("ip", &["monitor", "link", "address", "route"]);
            return matches!(timeout(delay, fut).await, Ok(true));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if command_exists("route").await {
            let fut = wait_for_command_output("route", &["-n", "monitor"]);
            return matches!(timeout(delay, fut).await, Ok(true));
        }
    }
    sleep(delay).await;
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "command -v {} >/dev/null 2>&1",
            shell_escape(command)
        ))
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn shell_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn wait_for_command_output(command: &str, args: &[&str]) -> bool {
    let mut command_builder = Command::new(command);
    command_builder
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    let mut saw_event = false;
    if let Some(mut stdout) = child.stdout.take() {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 1];
        if let Ok(bytes) = stdout.read(&mut buf).await {
            saw_event = bytes > 0;
        }
    }
    if let Err(err) = child.kill().await {
        crate::logging::debug(&format!(
            "Failed to kill network monitor child process: {err}"
        ));
    }
    saw_event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restoration_needs_an_offline_probe_then_a_verified_change() {
        use std::future::{poll_fn, ready};
        use std::task::Poll;
        let online = std::cell::Cell::new(false);
        let change = tokio::sync::Notify::new();
        let wait = wait_until_online_with(true, || ready(online.get()), |_| change.notified());
        tokio::pin!(wait);
        poll_fn(|cx| Poll::Ready(assert!(wait.as_mut().poll(cx).is_pending()))).await;
        change.notify_one(); // Unrelated route event, still offline.
        poll_fn(|cx| Poll::Ready(assert!(wait.as_mut().poll(cx).is_pending()))).await;
        online.set(true);
        change.notify_one();
        poll_fn(|cx| Poll::Ready(assert!(wait.as_mut().poll(cx).is_ready()))).await;
        let provider_only = wait_until_online_with(true, || ready(true), |_| ready(()));
        tokio::pin!(provider_only);
        poll_fn(|cx| Poll::Ready(assert!(provider_only.as_mut().poll(cx).is_pending()))).await;
    }

    #[test]
    fn classifies_common_network_errors() {
        assert!(classify_message("connection reset by peer").is_some());
        assert!(classify_message("temporary failure in name resolution").is_some());
        assert!(classify_message("network is unreachable").is_some());
        assert!(classify_message("401 unauthorized").is_none());
    }

    /// Real error strings harvested from ~/.jcode/logs. Every wifi-outage
    /// shape observed in the wild must classify as a network interruption so
    /// the wait-for-network path engages instead of failing the turn.
    #[test]
    fn classifies_real_world_outage_errors() {
        let real_errors = [
            // DNS failure: wifi down when a new request starts, or link up
            // before DHCP delivers DNS servers.
            "Failed to send request to Anthropic API: error sending request for url \
             (https://api.anthropic.com/v1/messages): client error (Connect): dns error: \
             failed to lookup address information: Name or service not known",
            "error sending request for url (https://models.dev/api.json): client error \
             (Connect): dns error: failed to lookup address information: Temporary failure \
             in name resolution",
            // Stale pooled HTTP/2 connection dying after an outage.
            "Failed to send request to Anthropic API: error sending request for url \
             (https://api.anthropic.com/v1/messages): client error (SendRequest): \
             http2 error: keep-alive timed out: operation timed out",
            // Wi-Fi roam between access points mid-request.
            "Failed to send request to Anthropic API: error sending request for url \
             (https://api.anthropic.com/v1/messages?beta=true): client error (SendRequest): \
             http2 error: stream error received: unspecific protocol error detected",
            // rustls abrupt-close shape (docs URL spells "unexpected-eof" so
            // the plain "unexpected eof" marker never matched it).
            "error sending request for url (https://generativelanguage.googleapis.com/v1beta/models): \
             client error (SendRequest): connection error: peer closed connection without \
             sending TLS close_notify: https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof",
            // Connect-phase timeout.
            "error sending request for url (https://html.duckduckgo.com/html/): \
             client error (Connect): operation timed out",
            // Connection-level timeout on an established connection.
            "error sending request for url (https://dblp.org/search/author/api): \
             client error (SendRequest): connection error: timed out",
            // TLS handshake dying mid-outage.
            "error sending request for url (https://generativelanguage.googleapis.com/v1beta/models): \
             client error (Connect): tls handshake eof",
            // Body cut off mid-stream.
            "error decoding response body: request or response body error: operation timed out",
        ];
        for error in real_errors {
            assert!(
                classify_message(error).is_some(),
                "should classify as network interruption: {error}"
            );
        }
    }

    /// Deterministic failures must NOT trigger the wait-for-network path,
    /// otherwise a bad API key or exhausted quota would hang forever waiting
    /// for connectivity that is already fine.
    #[test]
    fn does_not_classify_deterministic_failures() {
        let deterministic = [
            "401 unauthorized",
            "403 forbidden: permission_denied",
            "invalid x-api-key",
            "404 not_found_error: model: claude-fable-5",
            "insufficient_quota: your credit balance is too low",
            "400 bad request: context length exceeded",
            "content_policy_violation",
        ];
        for error in deterministic {
            assert!(
                classify_message(error).is_none(),
                "should NOT classify as network interruption: {error}"
            );
        }
    }

    #[test]
    fn classifies_wrapped_provider_send_errors() {
        let wrapped = [
            "Failed to send Antigravity generateContent request",
            "Failed to send request to Anthropic API",
            "Failed to send OpenAI-compatible chat request",
            "Failed to send request to OpenAI API",
            "Failed to send request to Claude API",
            "Failed to send Cursor agent request headers",
            "Network is down",
            "Software caused connection abort",
            "Network unstable: retrying",
        ];
        for error in wrapped {
            assert!(
                classify_message(error).is_some(),
                "should classify wrapped transport send failure: {error}"
            );
        }
    }

    #[test]
    fn excludes_stream_idle_timeouts_and_http_errors() {
        let excluded = [
            "Stream read timeout: no data received for 180s",
            "Timed out waiting for next WebSocket activity after 180s",
            "Request did not complete within 180s",
            "429 Too Many Requests: upstream timeout",
            "401 Unauthorized: not connected",
            "400 Bad Request: parameter timeout invalid",
        ];
        for error in excluded {
            assert!(
                classify_message(error).is_none(),
                "should NOT classify as network disconnect: {error}"
            );
        }
    }

    #[test]
    fn waiting_for_network_connection_phase_roundtrip() {
        let phase = waiting_for_network_connection_phase("listening via ip monitor");
        assert_eq!(
            parse_waiting_for_network_connection_phase(&phase).as_deref(),
            Some("listening via ip monitor")
        );
        assert!(
            parse_waiting_for_network_connection_phase(WAITING_FOR_NETWORK_PHASE_PREFIX).is_some()
        );
        assert!(parse_waiting_for_network_connection_phase("connecting").is_none());
    }

    #[tokio::test]
    async fn recovery_tracker_waits_for_offline_then_resets_online_budget() {
        let hooks = test_hooks::TestNetworkHooks::from_probe_sequence(vec![
            true,  // online retry 1
            true,  // online retry 2
            false, // offline detected
            false, // still offline after first wait
            true,  // restored!
            true,  // post-restore online retry 1 (budget was reset!)
            true,  // post-restore online retry 2
            true,  // post-restore online retry 3
            true,  // 4th consecutive online failure -> exhausted
        ]);

        test_hooks::with_test_network_hooks(hooks, async {
            let mut tracker = NetworkRecoveryTracker::new();
            let mut notices = Vec::new();

            assert_eq!(
                tracker
                    .wait_for_recovery(
                        false,
                        std::future::pending(),
                        |n| notices.push(n),
                        Duration::from_secs(10),
                        || {},
                    )
                    .await,
                NetworkRecoveryOutcome::RetryingWhileOnline {
                    attempt: 1,
                    max: MAX_ONLINE_NETWORK_RETRIES,
                }
            );
            assert_eq!(
                tracker
                    .wait_for_recovery(
                        false,
                        std::future::pending(),
                        |n| notices.push(n),
                        Duration::from_secs(10),
                        || {},
                    )
                    .await,
                NetworkRecoveryOutcome::RetryingWhileOnline {
                    attempt: 2,
                    max: MAX_ONLINE_NETWORK_RETRIES,
                }
            );
            assert_eq!(
                tracker
                    .wait_for_recovery(
                        false,
                        std::future::pending(),
                        |n| notices.push(n),
                        Duration::from_secs(10),
                        || {},
                    )
                    .await,
                NetworkRecoveryOutcome::RestoredAfterOffline
            );
            assert!(
                notices
                    .iter()
                    .any(|n| matches!(n, NetworkWaitNotice::Offline { .. })),
                "offline transition should emit NetworkWaitNotice::Offline"
            );

            for expected_attempt in 1..=MAX_ONLINE_NETWORK_RETRIES {
                assert_eq!(
                    tracker
                        .wait_for_recovery(
                            false,
                            std::future::pending(),
                            |_| {},
                            Duration::from_secs(10),
                            || {},
                        )
                        .await,
                    NetworkRecoveryOutcome::RetryingWhileOnline {
                        attempt: expected_attempt,
                        max: MAX_ONLINE_NETWORK_RETRIES,
                    }
                );
            }
            assert_eq!(
                tracker
                    .wait_for_recovery(
                        false,
                        std::future::pending(),
                        |_| {},
                        Duration::from_secs(10),
                        || {},
                    )
                    .await,
                NetworkRecoveryOutcome::OnlineRetryBudgetExhausted {
                    max: MAX_ONLINE_NETWORK_RETRIES,
                }
            );
        })
        .await;
    }

    #[tokio::test]
    async fn recovery_tracker_honors_cancellation_mid_offline_wait() {
        let waiting_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let waiting_started_for_wait = waiting_started.clone();
        let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancel_for_wait = cancel.clone();

        let hooks = test_hooks::TestNetworkHooks {
            probe: std::sync::Arc::new(|| false),
            wait: std::sync::Arc::new(move |_| {
                let started = waiting_started_for_wait.clone();
                Box::pin(async move {
                    started.notify_one();
                    std::future::pending::<bool>().await
                })
            }),
            online_backoff: Duration::from_millis(1),
        };

        let handle = tokio::spawn(test_hooks::with_test_network_hooks(hooks, async move {
            let mut tracker = NetworkRecoveryTracker::new();
            let cancel_fut = cancel_for_wait.notified();
            tokio::pin!(cancel_fut);
            tracker
                .wait_for_recovery(false, cancel_fut, |_| {}, Duration::from_secs(10), || {})
                .await
        }));

        waiting_started.notified().await;
        cancel.notify_one();
        let outcome = handle.await.expect("recovery task should join cleanly");
        assert_eq!(outcome, NetworkRecoveryOutcome::Cancelled);
    }
}
