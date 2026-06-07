use crate::ambient_runner::AmbientRunnerHandle;
use crate::config::SafetyConfig;
use crate::logging;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait MessageChannel: Send + Sync {
    fn name(&self) -> &str;

    fn is_send_enabled(&self) -> bool;

    fn is_reply_enabled(&self) -> bool;

    async fn send(&self, text: &str) -> anyhow::Result<()>;

    async fn reply_loop(&self, runner: AmbientRunnerHandle);
}

#[derive(Clone)]
pub struct ChannelRegistry {
    channels: Vec<Arc<dyn MessageChannel>>,
}

impl ChannelRegistry {
    pub fn from_config(config: &SafetyConfig) -> Self {
        let mut channels: Vec<Arc<dyn MessageChannel>> = Vec::new();

        if config.telegram_enabled
            && let (Some(token), Some(chat_id)) = (
                config.telegram_bot_token.clone(),
                config.telegram_chat_id.clone(),
            )
        {
            logging::info(&format!(
                "registering telegram notification channel reply_enabled={}",
                config.telegram_reply_enabled
            ));
            channels.push(Arc::new(TelegramChannel::new(
                token,
                chat_id,
                config.telegram_reply_enabled,
            )));
        }

        if config.discord_enabled
            && let (Some(token), Some(channel_id)) = (
                config.discord_bot_token.clone(),
                config.discord_channel_id.clone(),
            )
        {
            logging::info(&format!(
                "registering discord notification channel reply_enabled={}",
                config.discord_reply_enabled
            ));
            channels.push(Arc::new(DiscordChannel::new(
                token,
                channel_id,
                config.discord_reply_enabled,
                config.discord_bot_user_id.clone(),
            )));
        }

        if config.jade_relay_enabled {
            match (
                config.jade_relay_api_base.clone(),
                config.jade_relay_token.clone(),
                config.jade_relay_session_id.clone(),
            ) {
                (Some(api_base), Some(token), Some(session_id)) => {
                    // user_id defaults to the token id when not explicitly set.
                    let user_id = config
                        .jade_relay_user_id
                        .clone()
                        .or_else(|| config.jade_relay_token_id.clone())
                        .unwrap_or_else(|| "default".to_string());
                    logging::info(&format!(
                        "registering jade relay channel user={} session={} reply_enabled={}",
                        user_id, session_id, config.jade_relay_reply_enabled
                    ));
                    channels.push(Arc::new(JadeRelayChannel::new(
                        api_base,
                        token,
                        config.jade_relay_token_id.clone(),
                        user_id,
                        session_id,
                        config.jade_relay_reply_enabled,
                    )));
                }
                _ => {
                    logging::warn(
                        "jade_relay_enabled but api_base/token/session_id incomplete; skipping",
                    );
                }
            }
        }

        logging::debug(&format!(
            "channel registry initialized channel_count={}",
            channels.len()
        ));
        Self { channels }
    }

    pub fn send_all(&self, text: &str) {
        if tokio::runtime::Handle::try_current().is_err() {
            logging::warn("skipping channel send_all because no Tokio runtime is active");
            return;
        }
        for ch in self.channels.iter().filter(|c| c.is_send_enabled()) {
            let ch = Arc::clone(ch);
            let text = text.to_string();
            tokio::spawn(async move {
                logging::debug(&format!("sending notification via {}", ch.name()));
                if let Err(e) = ch.send(&text).await {
                    logging::error(&format!("{} notification failed: {}", ch.name(), e));
                }
            });
        }
    }

    pub fn spawn_reply_loops(&self, runner: &AmbientRunnerHandle) {
        for ch in self.channels.iter().filter(|c| c.is_reply_enabled()) {
            let ch = Arc::clone(ch);
            let runner = runner.clone();
            tokio::spawn(async move {
                logging::info(&format!("{} reply loop spawned", ch.name()));
                ch.reply_loop(runner).await;
            });
        }
    }

    pub fn channel_names(&self) -> Vec<String> {
        self.channels.iter().map(|c| c.name().to_string()).collect()
    }

    pub fn find_by_name(&self, name: &str) -> Option<Arc<dyn MessageChannel>> {
        let channel = self.channels.iter().find(|c| c.name() == name).cloned();
        if channel.is_none() {
            logging::debug(&format!("channel lookup missed name={name}"));
        }
        channel
    }

    pub fn send_enabled(&self) -> Vec<Arc<dyn MessageChannel>> {
        self.channels
            .iter()
            .filter(|c| c.is_send_enabled())
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Telegram channel
// ---------------------------------------------------------------------------

pub struct TelegramChannel {
    token: String,
    chat_id: String,
    reply_enabled: bool,
    client: reqwest::Client,
}

impl TelegramChannel {
    pub fn new(token: String, chat_id: String, reply_enabled: bool) -> Self {
        Self {
            token,
            chat_id,
            reply_enabled,
            client: crate::provider::shared_http_client(),
        }
    }
}

#[async_trait]
impl MessageChannel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    fn is_send_enabled(&self) -> bool {
        true
    }

    fn is_reply_enabled(&self) -> bool {
        self.reply_enabled
    }

    async fn send(&self, text: &str) -> anyhow::Result<()> {
        logging::debug(&format!(
            "sending telegram notification bytes={}",
            text.len()
        ));
        crate::telegram::send_message(&self.client, &self.token, &self.chat_id, text).await
    }

    async fn reply_loop(&self, runner: AmbientRunnerHandle) {
        let mut offset: Option<i64> = None;

        loop {
            match crate::telegram::get_updates(&self.client, &self.token, offset, 30).await {
                Ok(updates) => {
                    if !updates.is_empty() {
                        logging::debug(&format!(
                            "telegram reply loop received update_count={}",
                            updates.len()
                        ));
                    }
                    for update in updates {
                        offset = Some(update.update_id + 1);

                        let msg = match update.message {
                            Some(m) => m,
                            None => continue,
                        };

                        if msg.chat.id.to_string() != self.chat_id {
                            continue;
                        }

                        let text = match msg.text {
                            Some(t) => t,
                            None => continue,
                        };

                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Some(req_id) = crate::notifications::extract_permission_id(trimmed) {
                            let (approved, message) =
                                crate::notifications::parse_permission_reply(trimmed);
                            if let Err(e) = crate::safety::record_permission_via_file(
                                &req_id,
                                approved,
                                "telegram_reply",
                                message,
                            ) {
                                logging::error(&format!(
                                    "Failed to record permission from Telegram for {}: {}",
                                    req_id, e
                                ));
                            } else {
                                logging::info(&format!(
                                    "Permission {} via Telegram: {}",
                                    if approved { "approved" } else { "denied" },
                                    req_id
                                ));
                                let _ = self
                                    .send(&format!(
                                        "✅ Permission {} for `{}`",
                                        if approved { "approved" } else { "denied" },
                                        req_id
                                    ))
                                    .await;
                            }
                        } else {
                            let injected = runner.inject_message(trimmed, "telegram").await;
                            logging::info(&format!(
                                "telegram reply injected into session injected={}",
                                injected
                            ));
                            let ack = if injected {
                                format!("💬 Message sent to active session: _{}_", trimmed)
                            } else {
                                format!("📋 Message queued, waking agent: _{}_", trimmed)
                            };
                            let _ = self.send(&ack).await;
                        }
                    }
                }
                Err(e) => {
                    logging::error(&format!("Telegram poll error: {}", e));
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Discord channel
// ---------------------------------------------------------------------------

pub struct DiscordChannel {
    token: String,
    channel_id: String,
    reply_enabled: bool,
    bot_user_id: Option<String>,
    client: reqwest::Client,
}

impl DiscordChannel {
    pub fn new(
        token: String,
        channel_id: String,
        reply_enabled: bool,
        bot_user_id: Option<String>,
    ) -> Self {
        Self {
            token,
            channel_id,
            reply_enabled,
            bot_user_id,
            client: crate::provider::shared_http_client(),
        }
    }

    async fn poll_messages(&self, after: Option<&str>) -> anyhow::Result<Vec<DiscordMessage>> {
        logging::debug(&format!(
            "polling discord messages after_present={}",
            after.is_some()
        ));
        let mut url = format!(
            "https://discord.com/api/v10/channels/{}/messages?limit=10",
            self.channel_id
        );
        if let Some(after_id) = after {
            url.push_str(&format!("&after={}", after_id));
        }

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            logging::warn(&format!("discord message poll returned status={status}"));
            anyhow::bail!("Discord messages error ({}): {}", status, body);
        }

        let messages: Vec<DiscordMessage> = resp.json().await?;
        logging::debug(&format!(
            "discord message poll returned count={}",
            messages.len()
        ));
        Ok(messages)
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DiscordMessage {
    pub id: String,
    pub content: String,
    pub author: DiscordAuthor,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DiscordAuthor {
    pub id: String,
    pub bot: Option<bool>,
}

#[async_trait]
impl MessageChannel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    fn is_send_enabled(&self) -> bool {
        true
    }

    fn is_reply_enabled(&self) -> bool {
        self.reply_enabled
    }

    async fn send(&self, text: &str) -> anyhow::Result<()> {
        let url = format!(
            "https://discord.com/api/v10/channels/{}/messages",
            self.channel_id
        );
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .json(&serde_json::json!({ "content": text }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord API error ({}): {}", status, body);
        }

        logging::info("Discord notification sent");
        Ok(())
    }

    async fn reply_loop(&self, runner: AmbientRunnerHandle) {
        let mut last_seen_id: Option<String> = None;

        // Get the latest message ID on startup so we don't replay old messages
        match self.poll_messages(None).await {
            Ok(msgs) => {
                if let Some(latest) = msgs.first() {
                    last_seen_id = Some(latest.id.clone());
                }
            }
            Err(e) => {
                logging::error(&format!("Discord initial poll error: {}", e));
            }
        }

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            match self.poll_messages(last_seen_id.as_deref()).await {
                Ok(msgs) => {
                    // Discord returns newest first, reverse for chronological order
                    let mut msgs = msgs;
                    msgs.reverse();

                    for msg in msgs {
                        last_seen_id = Some(msg.id.clone());

                        // Skip messages from bots (including ourselves)
                        if msg.author.bot.unwrap_or(false) {
                            continue;
                        }

                        // If we know our bot user ID, also skip our own messages
                        if let Some(ref bot_id) = self.bot_user_id
                            && msg.author.id == *bot_id
                        {
                            continue;
                        }

                        let trimmed = msg.content.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Some(req_id) = crate::notifications::extract_permission_id(trimmed) {
                            let (approved, message) =
                                crate::notifications::parse_permission_reply(trimmed);
                            if let Err(e) = crate::safety::record_permission_via_file(
                                &req_id,
                                approved,
                                "discord_reply",
                                message,
                            ) {
                                logging::error(&format!(
                                    "Failed to record permission from Discord for {}: {}",
                                    req_id, e
                                ));
                            } else {
                                logging::info(&format!(
                                    "Permission {} via Discord: {}",
                                    if approved { "approved" } else { "denied" },
                                    req_id
                                ));
                                let _ = self
                                    .send(&format!(
                                        "✅ Permission {} for `{}`",
                                        if approved { "approved" } else { "denied" },
                                        req_id
                                    ))
                                    .await;
                            }
                        } else {
                            let injected = runner.inject_message(trimmed, "discord").await;
                            logging::info(&format!(
                                "discord reply injected into session injected={}",
                                injected
                            ));
                            let ack = if injected {
                                format!("💬 Message sent to active session: *{}*", trimmed)
                            } else {
                                format!("📋 Message queued, waking agent: *{}*", trimmed)
                            };
                            let _ = self.send(&ack).await;
                        }
                    }
                }
                Err(e) => {
                    logging::error(&format!("Discord poll error: {}", e));
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Jade cloud relay channel
// ---------------------------------------------------------------------------

/// Remote control via the Jade cloud relay (an append-only per-session event
/// log in AWS). Unlike the WebSocket gateway, nothing listens on this machine:
/// the laptop only makes outbound long-poll requests, so there is no inbound
/// port to attack. A cloud client posts `prompt` events; this channel injects
/// them into the live session and posts the agent's reply back as a `response`
/// event for the cloud client to read.
pub struct JadeRelayChannel {
    /// API base URL, normalized to end with a single '/'.
    api_base: String,
    token: String,
    token_id: Option<String>,
    user_id: String,
    session_id: String,
    reply_enabled: bool,
    client: reqwest::Client,
}

impl JadeRelayChannel {
    pub fn new(
        api_base: String,
        token: String,
        token_id: Option<String>,
        user_id: String,
        session_id: String,
        reply_enabled: bool,
    ) -> Self {
        let api_base = if api_base.ends_with('/') {
            api_base
        } else {
            format!("{}/", api_base)
        };
        Self {
            api_base,
            token,
            token_id,
            user_id,
            session_id,
            reply_enabled,
            client: crate::provider::shared_http_client(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path.trim_start_matches('/'))
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut req = req.header("Authorization", format!("Bearer {}", self.token));
        if let Some(id) = &self.token_id {
            req = req.header("x-jade-token-id", id);
        }
        req
    }

    /// Register/heartbeat this device so the cloud can show it as online.
    async fn heartbeat(&self, device_id: &str) {
        let body = serde_json::json!({
            "user_id": self.user_id,
            "device_id": device_id,
            "label": device_id,
            "platform": std::env::consts::OS,
        });
        let req = self.auth(self.client.post(self.url("v1/devices")).json(&body));
        if let Err(e) = req.send().await {
            logging::debug(&format!("jade relay heartbeat failed: {}", e));
        }
    }

    /// Long-poll for new prompt events after `after`. Returns (events, next_after).
    /// `wait` is the server-side long-poll window in seconds (capped at 25 by the relay).
    async fn poll_prompts(&self, after: i64, wait: u32) -> anyhow::Result<(Vec<RelayEvent>, i64)> {
        let session = urlencoding_encode(&self.session_id);
        let url = self.url(&format!(
            "v1/sessions/{}/events?user_id={}&after={}&types=prompt&wait={}",
            session,
            urlencoding_encode(&self.user_id),
            after,
            wait
        ));
        let resp = self.auth(self.client.get(&url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("jade relay poll error ({}): {}", status, body);
        }
        let parsed: RelayEventsResponse = resp.json().await?;
        Ok((parsed.events, parsed.next_after))
    }

    /// Post a response event back to the relay for the cloud client to read.
    async fn post_response(&self, text: &str, request_seq: i64) -> anyhow::Result<()> {
        let session = urlencoding_encode(&self.session_id);
        let body = serde_json::json!({
            "user_id": self.user_id,
            "type": "response",
            "text": text,
            "request_seq": request_seq,
            "origin": "jcode",
        });
        let resp = self
            .auth(
                self.client
                    .post(self.url(&format!("v1/sessions/{}/events", session)))
                    .json(&body),
            )
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            anyhow::bail!("jade relay post error ({}): {}", status, detail);
        }
        Ok(())
    }
}

#[derive(Debug, serde::Deserialize)]
struct RelayEventsResponse {
    #[serde(default)]
    events: Vec<RelayEvent>,
    #[serde(default)]
    next_after: i64,
}

#[derive(Debug, serde::Deserialize)]
struct RelayEvent {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    text: Option<String>,
}

/// Minimal percent-encoding for path/query segments (alnum and -_.~ pass through).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[async_trait]
impl MessageChannel for JadeRelayChannel {
    fn name(&self) -> &str {
        "jade_relay"
    }

    fn is_send_enabled(&self) -> bool {
        true
    }

    fn is_reply_enabled(&self) -> bool {
        // Inbound Jade relay prompts are delivered by server::jade_relay so they
        // work even when ambient mode is disabled and target the configured live
        // Jcode session directly. Keep this channel for outbound notifications
        // only; otherwise ambient mode would start a second poller.
        let _configured_for_server_listener = self.reply_enabled;
        false
    }

    async fn send(&self, text: &str) -> anyhow::Result<()> {
        // Cloud notifications (e.g. ambient cycle summaries) are posted as a
        // response event with request_seq=0 (not tied to a specific prompt).
        self.post_response(text, 0).await
    }

    async fn reply_loop(&self, runner: AmbientRunnerHandle) {
        let host = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "laptop".to_string());
        let device_id = format!("jcode-{}", host);
        logging::info(&format!(
            "jade relay reply loop started channel={}/{}",
            self.user_id, self.session_id
        ));
        // Start after the latest existing prompt so we don't replay history.
        let mut after: i64 = match self.poll_prompts(0, 0).await {
            Ok((_, next)) => next,
            Err(e) => {
                logging::error(&format!("jade relay init poll failed: {}", e));
                0
            }
        };
        let mut last_heartbeat = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(60))
            .unwrap_or_else(std::time::Instant::now);

        loop {
            if last_heartbeat.elapsed() >= std::time::Duration::from_secs(30) {
                self.heartbeat(&device_id).await;
                last_heartbeat = std::time::Instant::now();
            }
            match self.poll_prompts(after, 20).await {
                Ok((events, next_after)) => {
                    after = next_after;
                    for ev in events {
                        let text = ev.text.unwrap_or_default();
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Some(req_id) = crate::notifications::extract_permission_id(trimmed) {
                            let (approved, message) =
                                crate::notifications::parse_permission_reply(trimmed);
                            if let Err(e) = crate::safety::record_permission_via_file(
                                &req_id,
                                approved,
                                "jade_relay",
                                message,
                            ) {
                                logging::error(&format!(
                                    "Failed to record permission from jade relay for {}: {}",
                                    req_id, e
                                ));
                            } else {
                                let _ = self
                                    .post_response(
                                        &format!(
                                            "Permission {} for {}",
                                            if approved { "approved" } else { "denied" },
                                            req_id
                                        ),
                                        ev.seq,
                                    )
                                    .await;
                            }
                            continue;
                        }
                        let injected = runner.inject_message(trimmed, "jade_relay").await;
                        logging::info(&format!(
                            "jade relay prompt injected seq={} injected={}",
                            ev.seq, injected
                        ));
                        let ack = if injected {
                            "Message delivered to active session."
                        } else {
                            "Message queued; waking agent."
                        };
                        if let Err(e) = self.post_response(ack, ev.seq).await {
                            logging::error(&format!("jade relay ack post failed: {}", e));
                        }
                    }
                }
                Err(e) => {
                    logging::error(&format!("jade relay poll error: {}", e));
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_message_parse() {
        let json = r#"{
            "id": "123456",
            "content": "hello agent",
            "author": {"id": "789", "bot": false}
        }"#;
        let msg: DiscordMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "123456");
        assert_eq!(msg.content, "hello agent");
        assert!(!msg.author.bot.unwrap());
    }

    #[test]
    fn test_discord_bot_message_parse() {
        let json = r#"{
            "id": "999",
            "content": "bot response",
            "author": {"id": "111", "bot": true}
        }"#;
        let msg: DiscordMessage = serde_json::from_str(json).unwrap();
        assert!(msg.author.bot.unwrap());
    }

    #[test]
    fn test_relay_events_parse() {
        let json = r#"{
            "events": [
                {"seq": 5, "type": "prompt", "text": "run the tests"},
                {"seq": 6, "type": "prompt", "text": "now lint"}
            ],
            "next_after": 6
        }"#;
        let parsed: RelayEventsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.events[0].seq, 5);
        assert_eq!(parsed.events[0].text.as_deref(), Some("run the tests"));
        assert_eq!(parsed.next_after, 6);
    }

    #[test]
    fn test_relay_events_empty() {
        let json = r#"{"events": [], "next_after": 0}"#;
        let parsed: RelayEventsResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.events.is_empty());
        assert_eq!(parsed.next_after, 0);
    }

    #[test]
    fn test_relay_url_encoding() {
        assert_eq!(urlencoding_encode("sess-relay-test"), "sess-relay-test");
        assert_eq!(urlencoding_encode("a/b c"), "a%2Fb%20c");
        assert_eq!(urlencoding_encode("user.name~1_2"), "user.name~1_2");
    }

    #[test]
    fn test_relay_url_join() {
        let ch = JadeRelayChannel::new(
            "https://example.com/api".to_string(),
            "tok".to_string(),
            Some("jeremy".to_string()),
            "jeremy".to_string(),
            "sess-1".to_string(),
            true,
        );
        assert_eq!(ch.url("v1/devices"), "https://example.com/api/v1/devices");
        assert_eq!(ch.url("/v1/devices"), "https://example.com/api/v1/devices");
    }

    #[test]
    fn test_relay_registry_wiring() {
        // Disabled: not registered.
        let cfg = SafetyConfig::default();
        let reg = ChannelRegistry::from_config(&cfg);
        assert!(!reg.channel_names().iter().any(|n| n == "jade_relay"));

        // Enabled but incomplete: skipped with a warning.
        let mut cfg = SafetyConfig {
            jade_relay_enabled: true,
            ..SafetyConfig::default()
        };
        let reg = ChannelRegistry::from_config(&cfg);
        assert!(!reg.channel_names().iter().any(|n| n == "jade_relay"));

        // Enabled and complete: registered.
        cfg.jade_relay_api_base = Some("https://example.com/".to_string());
        cfg.jade_relay_token = Some("tok".to_string());
        cfg.jade_relay_session_id = Some("sess-1".to_string());
        let reg = ChannelRegistry::from_config(&cfg);
        assert!(reg.channel_names().iter().any(|n| n == "jade_relay"));
    }

    /// Live end-to-end test against the real Jade relay. Ignored by default;
    /// run with the relay env vars set:
    ///   JADE_RELAY_API_BASE, JADE_RELAY_TOKEN, JADE_RELAY_TOKEN_ID,
    ///   JADE_RELAY_USER_ID, JADE_RELAY_SESSION_ID
    ///   cargo test -p jcode-app-core relay_live -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires live Jade relay credentials"]
    async fn test_relay_live_roundtrip() {
        let api_base = match std::env::var("JADE_RELAY_API_BASE") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: JADE_RELAY_API_BASE not set");
                return;
            }
        };
        let token = std::env::var("JADE_RELAY_TOKEN").expect("JADE_RELAY_TOKEN");
        let token_id = std::env::var("JADE_RELAY_TOKEN_ID").ok();
        let user_id = std::env::var("JADE_RELAY_USER_ID").unwrap_or_else(|_| "jeremy".to_string());
        let session_id = std::env::var("JADE_RELAY_SESSION_ID")
            .unwrap_or_else(|_| format!("rust-live-{}", chrono::Utc::now().timestamp()));

        let ch = JadeRelayChannel::new(
            api_base,
            token,
            token_id.clone(),
            user_id.clone(),
            session_id.clone(),
            true,
        );

        // 1) heartbeat (device register)
        ch.heartbeat("jcode-test-device").await;

        // 2) baseline cursor: no prompts yet
        let (events, after) = ch.poll_prompts(0, 0).await.expect("baseline poll");
        eprintln!("baseline: {} events, next_after={}", events.len(), after);

        // 3) simulate a cloud client posting a prompt by POSTing a prompt event
        let prompt_text = format!(
            "hello from rust live test {}",
            chrono::Utc::now().timestamp()
        );
        let prompt_body = serde_json::json!({
            "user_id": user_id,
            "type": "prompt",
            "text": prompt_text,
            "origin": "rust-test-client",
        });
        let resp = ch
            .auth(
                ch.client
                    .post(ch.url(&format!(
                        "v1/sessions/{}/events",
                        urlencoding_encode(&session_id)
                    )))
                    .json(&prompt_body),
            )
            .send()
            .await
            .expect("post prompt");
        assert!(
            resp.status().is_success(),
            "post prompt status {}",
            resp.status()
        );

        // 4) the channel polls and sees the prompt
        let (events, after2) = ch.poll_prompts(after, 5).await.expect("poll after prompt");
        assert!(!events.is_empty(), "expected at least one prompt event");
        let prompt_ev = events
            .iter()
            .find(|e| e.text.as_deref() == Some(prompt_text.as_str()))
            .expect("our prompt event present");
        eprintln!("received prompt seq={} after2={}", prompt_ev.seq, after2);

        // 5) the channel posts a response tied to that prompt's seq
        let reply = format!("rust live reply to seq {}", prompt_ev.seq);
        ch.post_response(&reply, prompt_ev.seq)
            .await
            .expect("post response");

        // 6) verify the response is visible (poll all event types via raw GET)
        let verify_url = ch.url(&format!(
            "v1/sessions/{}/events?user_id={}&after=0&types=response&wait=5",
            urlencoding_encode(&session_id),
            urlencoding_encode(&user_id)
        ));
        let verify: RelayEventsResponse = ch
            .auth(ch.client.get(&verify_url))
            .send()
            .await
            .expect("verify get")
            .json()
            .await
            .expect("verify json");
        assert!(
            verify
                .events
                .iter()
                .any(|e| e.text.as_deref() == Some(reply.as_str())),
            "response event should be readable back from the relay"
        );
        eprintln!("LIVE ROUNDTRIP OK: prompt -> poll -> response verified");
    }
}
