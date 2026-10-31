//! Keep explicit provider overload reports visible across transport recovery.
//!
//! A later socket timeout is not evidence that the earlier overload report was
//! wrong. Retain both facts without claiming that overload caused the timeout.

#[derive(Default)]
pub(super) struct OpenAIRetryDiagnostics {
    overload_notice: Option<&'static str>,
}

impl OpenAIRetryDiagnostics {
    /// Returns a user-facing notice once per complete() call, never for a
    /// generic timeout, 503, or rate limit that does not explicitly say overload.
    pub(super) fn record(&mut self, error: &str) -> Option<&'static str> {
        if self.overload_notice.is_some() {
            return None;
        }
        let lower = error.to_ascii_lowercase();
        let notice = if lower.contains("server_is_overloaded") {
            "OpenAI reported server overload (server_is_overloaded)."
        } else if lower.contains("servers are currently overloaded")
            || lower.contains("server is overloaded")
        {
            "OpenAI reported server overload: its servers are currently overloaded."
        } else {
            return None;
        };
        self.overload_notice = Some(notice);
        Some(notice)
    }

    /// The latest failure stays explicit, with the original anyhow chain and
    /// structured retry-after metadata preserved underneath the added context.
    pub(super) fn annotate(&self, error: anyhow::Error) -> anyhow::Error {
        let Some(notice) = self.overload_notice else {
            return error;
        };
        let latest = format!("{error:#}");
        error.context(format!(
            "{notice} Earlier in this turn. Latest attempt failed: {latest}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overload_notice_is_explicit_and_deduplicated() {
        let mut diagnostics = OpenAIRetryDiagnostics::default();
        let notice = diagnostics.record("stream error: service_unavailable_error (server_is_overloaded): Our servers are currently overloaded.").unwrap();
        assert!(notice.starts_with("OpenAI reported server overload"));
        assert!(notice.contains("server_is_overloaded"));
        assert!(diagnostics.record("server_is_overloaded").is_none());
    }

    #[test]
    fn generic_failures_do_not_invent_overload() {
        for error in [
            "Stream read timeout: no data received for 180 seconds",
            "HTTP 503 Service Unavailable",
            "service_unavailable_error",
            "HTTP 429 rate limit exceeded",
            "WebSocket connection reset",
        ] {
            let mut diagnostics = OpenAIRetryDiagnostics::default();
            assert!(diagnostics.record(error).is_none(), "{error}");
            assert_eq!(
                diagnostics.annotate(anyhow::anyhow!("latest")).to_string(),
                "latest"
            );
        }
    }

    #[test]
    fn overload_message_without_code_is_not_given_an_invented_code() {
        let notice = OpenAIRetryDiagnostics::default()
            .record("Our servers are currently overloaded. Please try again later.")
            .unwrap();
        assert!(!notice.contains("server_is_overloaded"));
    }

    #[test]
    fn later_timeout_keeps_both_facts_and_error_chain() {
        let mut diagnostics = OpenAIRetryDiagnostics::default();
        diagnostics.record("server_is_overloaded");
        let error = diagnostics.annotate(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Stream read timeout: no data received for 180 seconds",
        )));
        let visible = error.to_string();
        assert!(visible.contains("server_is_overloaded"));
        assert!(visible.contains("Latest attempt failed: Stream read timeout"));
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
