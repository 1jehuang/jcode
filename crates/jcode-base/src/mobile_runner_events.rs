//! Optional command wakeup hints. HTTP claims remain the only queue authority.
//! A disconnected, disabled or old API never disables the five-second fallback.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, protocol::WebSocketConfig,
};

#[cfg(test)]
#[path = "mobile_runner_events_tests.rs"]
mod tests;

type Source = Arc<dyn Fn() -> Option<(String, String)> + Send + Sync>;

pub(super) struct Wakeups {
    receiver: watch::Receiver<u64>,
    task: JoinHandle<()>,
}

impl Drop for Wakeups {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Wakeups {
    pub(super) async fn wait(&mut self, delay: Duration) {
        // A notification must not bypass the runner's error backoff.
        if delay >= Duration::from_secs(20) {
            tokio::time::sleep(delay).await;
            return;
        }
        let deadline = tokio::time::Instant::now() + delay;
        tokio::select! {
            changed = self.receiver.changed() => {
                if changed.is_err() {
                    tokio::time::sleep_until(deadline).await;
                }
            }
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

pub(super) fn spawn() -> Wakeups {
    spawn_with(
        Arc::new(|| {
            let key = crate::subscription_catalog::configured_api_key()?;
            Some((crate::subscription_api::configured_api_base(), key))
        }),
        Duration::from_secs(30),
    )
}

fn event_request(
    base: &str,
    key: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, ()> {
    let mut url = reqwest::Url::parse(base).map_err(|_| ())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => return Err(()),
    };
    url.set_scheme(scheme).map_err(|_| ())?;
    url.set_path(&format!(
        "{}/mobile/commands/events",
        url.path().trim_end_matches('/')
    ));
    let mut request = url.as_str().into_client_request().map_err(|_| ())?;
    let value = format!("Bearer {key}").parse().map_err(|_| ())?;
    request.headers_mut().insert("Authorization", value);
    Ok(request)
}

fn available(message: &str) -> bool {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Hint {
        #[serde(rename = "type")]
        kind: String,
    }
    matches!(serde_json::from_str::<Hint>(message), Ok(hint) if hint.kind == "commands_available")
}

fn spawn_with(source: Source, retry: Duration) -> Wakeups {
    let (sender, receiver) = watch::channel(0u64);
    let task = tokio::spawn(async move {
        loop {
            let scope = source();
            let Some((base, key)) = scope.as_ref() else {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };
            let mut cooldown = retry;
            if let Ok(request) = event_request(base, key) {
                let config = WebSocketConfig {
                    max_message_size: Some(4096),
                    max_frame_size: Some(4096),
                    max_write_buffer_size: 16384,
                    write_buffer_size: 0,
                    ..Default::default()
                };
                let mut timing = super::StageTimer::new("event_connect");
                let connection = tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio_tungstenite::connect_async_with_config(request, Some(config), true),
                )
                .await;
                match connection {
                    Ok(Ok((mut socket, _))) => {
                        timing.outcome = "ok";
                        drop(timing);
                        let mut lifetime = super::StageTimer::new("event_connection");
                        // Recheck the queue after subscribing, covering enqueue between
                        // the previous empty claim and WebSocket registration.
                        sender.send_modify(|version| *version = version.wrapping_add(1));
                        let mut check = tokio::time::interval(Duration::from_secs(5));
                        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        let mut heartbeat = tokio::time::Instant::now();
                        loop {
                            tokio::select! {
                                message = socket.next() => match message {
                                    Some(Ok(Message::Text(text))) if available(&text) => {
                                        // A watch channel coalesces bursts, with constant memory.
                                        sender.send_modify(|version| *version = version.wrapping_add(1));
                                    }
                                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                                    _ => { lifetime.outcome = "error"; break; },
                                },
                                _ = check.tick() => {
                                    if source() != scope { lifetime.outcome = "scope_changed"; break; }
                                    if heartbeat.elapsed() >= Duration::from_secs(60) {
                                        if !matches!(tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Ping(Vec::new()))).await, Ok(Ok(()))) {
                                            lifetime.outcome = "error";
                                            break;
                                        }
                                        heartbeat = tokio::time::Instant::now();
                                    }
                                }
                            }
                        }
                    }
                    Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                        if matches!(response.status().as_u16(), 404 | 426 | 503) =>
                    {
                        timing.outcome = "unavailable";
                        cooldown = Duration::from_secs(300);
                    }
                    _ => timing.outcome = "error",
                }
            }
            // Reconnects are bounded independently of normal HTTP queue claims.
            // Credential or endpoint changes invalidate the cooldown within 5s.
            let deadline = tokio::time::Instant::now() + cooldown;
            while source() == scope && tokio::time::Instant::now() < deadline {
                tokio::time::sleep_until(
                    (tokio::time::Instant::now() + Duration::from_secs(5)).min(deadline),
                )
                .await;
            }
        }
    });
    Wakeups { receiver, task }
}
