//! Local speech callback. The daemon receives text and queue status, never audio.
use super::*;
use crate::tui::backend::RemoteConnection;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeechInput {
    text: String,
    #[serde(rename = "intent")]
    _intent: Option<String>,
    #[serde(rename = "accept_large_output")]
    _accept_large_output: Option<bool>,
}

fn speech_text(input: serde_json::Value) -> anyhow::Result<String> {
    let input: SpeechInput = serde_json::from_value(input)?;
    anyhow::ensure!(
        !input.text.trim().is_empty()
            && input.text.chars().count() <= 2000
            && input.text.len() <= jcode_voice::speech::MAX_TEXT_BYTES
            && !input.text.contains('\0'),
        "Speech must be nonempty, NUL-free, at most 2000 characters and 4096 UTF-8 bytes"
    );
    Ok(input.text)
}

impl App {
    pub(super) async fn prepare_local_tools(
        &mut self,
        remote: &mut RemoteConnection,
    ) -> anyhow::Result<()> {
        if crate::tui::jarvis::enabled() && self.jarvis_speech.is_none() {
            self.jarvis_speech = Some(crate::tui::jarvis::create_speech_player()?);
            self.push_display_message(DisplayMessage::system(
                "Jarvis mode: preparing spoken replies. Use /voice or the voice key to talk. Starting voice input interrupts speech.",
            ));
        }
        self.prepare_cloud_environment(remote).await?;
        if crate::tui::jarvis::enabled()
            && !remote.jarvis_tools_configured
            && remote.local_tools_request.is_none()
            && self.remote_session_id.is_some()
            && remote.has_loaded_history()
            && !self.is_processing
        {
            remote.configure_cloud_tools(Default::default()).await?;
        }
        remote.prepare_screen_context().await?;
        Ok(())
    }

    pub(super) fn poll_jarvis_speech_error(&mut self) -> bool {
        let error = self
            .jarvis_speech
            .as_ref()
            .and_then(|player| player.take_error());
        if let Some(error) = error {
            self.push_display_message(DisplayMessage::error(error));
            self.set_status_notice("Jarvis speech failed");
            return true;
        }
        false
    }

    pub(super) fn cancel_jarvis_speech(&self) {
        if let Some(player) = &self.jarvis_speech {
            player.cancel();
        }
    }

    pub(super) fn jarvis_session_changed(&mut self, next: &str) {
        if self.remote_session_id.as_deref() != Some(next) {
            self.cancel_jarvis_speech();
            self.jarvis_listener.detach();
        }
    }

    pub(super) fn jarvis_call(
        &mut self,
        remote: &mut RemoteConnection,
        session: String,
        call_id: String,
        input: serde_json::Value,
    ) {
        let result = (|| -> anyhow::Result<String> {
            anyhow::ensure!(
                crate::tui::jarvis::enabled()
                    && self.remote_session_id.as_deref() == Some(session.as_str()),
                "Speech is not enabled for this session"
            );
            anyhow::ensure!(
                self.voice_input.is_none()
                    && !self.external_voice.as_ref().is_some_and(|(activity, at)| {
                        matches!(activity, crate::protocol::VoiceActivity::Recording { .. })
                            && at.elapsed() <= Duration::from_secs(3)
                    }),
                "Speech skipped while microphone input is active"
            );
            let text = speech_text(input)?;
            self.jarvis_listener.suppress_speech();
            self.jarvis_speech
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Local speech backend is unavailable"))?
                .enqueue(&text)?;
            Ok(format!(
                "Queued for local speech (playback is not confirmed):\n{text}"
            ))
        })();
        let reply = remote.cloud_callback_sender();
        match result {
            Ok(output) => reply(call_id, output, None),
            Err(error) => reply(call_id, String::new(), Some(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn speech_callback_rejects_other_sessions_without_playing_audio() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut app = crate::tui::app::tests::create_test_app();
        app.remote_session_id = Some("current-session".into());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let mut remote = RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = BufReader::new(reader);
            app.jarvis_call(
                &mut remote,
                "other-session".into(),
                "speech-call".into(),
                json!({"text":"Never play this"}),
            );
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let crate::protocol::Request::ToolResult {
                call_id,
                output,
                error,
                ..
            } = serde_json::from_str(&line).unwrap()
            else {
                panic!("Expected callback rejection");
            };
            assert_eq!(call_id, "speech-call");
            assert!(output.is_empty());
            assert!(error.unwrap().contains("not enabled"));
            assert!(app.jarvis_speech.is_none());
        });
    }

    #[test]
    fn speech_input_is_bounded_and_strict() {
        assert_eq!(
            speech_text(json!({"text":"Hello", "intent":"Reply"})).unwrap(),
            "Hello"
        );
        for input in [
            json!({}),
            json!({"text":"  "}),
            json!({"text":42}),
            json!({"text":"a".repeat(2001)}),
            json!({"text":"ok","command":"sh"}),
        ] {
            assert!(speech_text(input).is_err());
        }
        assert!(speech_text(json!({"text":"界".repeat(1000)})).is_ok());
        assert!(speech_text(json!({"text":"界".repeat(2000)})).is_err());
        assert!(speech_text(json!({"text":"a\u{0000}b"})).is_err());
    }
}
