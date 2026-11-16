//! One privacy-preserving outcome per real runtime turn, not per provider response.
use crate::protocol::{ServerEvent, TurnIdentity};

#[derive(Default)]
pub(super) struct StopLog {
    reason: Option<&'static str>,
    provider: Option<&'static str>,
    emitted: bool,
}

impl StopLog {
    fn provider_reason(reason: &str) -> &'static str {
        // Never copy arbitrary provider strings into logs.
        match reason.trim().to_ascii_lowercase().as_str() {
            "end_turn" | "stop" | "completed" | "stop_sequence" => "normal",
            "max_tokens" | "length" | "max_output_tokens" | "model_context_window_exceeded" => {
                "limit"
            }
            "tool_use" | "tool_calls" => "tool",
            "refusal" | "content_filter" | "safety" | "blocklist" | "prohibited_content"
            | "spii" | "recitation" => "guardrail",
            _ => "unknown",
        }
    }

    fn outcome(&mut self, event: &ServerEvent) -> Option<(u64, &'static str)> {
        if self.emitted {
            return None;
        }
        match event {
            ServerEvent::TurnStopped {
                reason,
                provider_stop_reason,
                ..
            } => {
                use crate::protocol::TurnStopReason;
                self.reason = Some(match reason {
                    TurnStopReason::Interrupted => "interrupted",
                    TurnStopReason::Failure => "failure",
                    TurnStopReason::Crash => "crash",
                    TurnStopReason::LimitReached => "limit",
                    TurnStopReason::ProviderGuardrail => "guardrail",
                    TurnStopReason::Unknown => "unknown",
                });
                if let Some(reason) = provider_stop_reason {
                    self.provider = Some(Self::provider_reason(reason));
                }
            }
            ServerEvent::Interrupted => self.reason = Some("interrupted"),
            ServerEvent::MessageEnd { stop_reason } => {
                self.provider = stop_reason.as_deref().map(Self::provider_reason);
            }
            ServerEvent::RetryRollback { .. } => {
                self.provider = None;
                self.reason = None;
            }
            ServerEvent::ProviderGuardrail { .. } => self.reason = Some("guardrail"),
            ServerEvent::Done { id } | ServerEvent::Error { id, .. } => {
                let reason = self.reason.unwrap_or_else(|| {
                    if matches!(event, ServerEvent::Error { .. }) {
                        "failure"
                    } else {
                        match self.provider {
                            Some("normal") => "normal",
                            Some("limit") => "limit",
                            Some("guardrail") => "guardrail",
                            _ => "unknown",
                        }
                    }
                });
                self.emitted = true;
                return Some((*id, reason));
            }
            _ => {}
        }
        None
    }

    pub(super) fn observe(&mut self, session: &str, identity: &TurnIdentity, event: &ServerEvent) {
        if let Some((request, reason)) = self.outcome(event) {
            self.log(session, identity, Some(request), reason);
        }
    }

    fn needs_investigation(reason: &str) -> bool {
        !matches!(reason, "normal" | "interrupted")
    }

    fn log(&self, session: &str, identity: &TurnIdentity, request: Option<u64>, reason: &str) {
        let needs_investigation = Self::needs_investigation(reason);
        let fields = vec![
            ("session", session.to_owned()),
            ("runtime", identity.runtime_generation.clone()),
            ("turn", identity.turn_id.to_string()),
            (
                "request",
                request
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "unknown".into()),
            ),
            ("reason", reason.to_owned()),
            ("abnormal", (reason != "normal").to_string()),
            ("needs_investigation", needs_investigation.to_string()),
            (
                "provider_stop",
                self.provider.unwrap_or("unknown").to_owned(),
            ),
        ];
        if needs_investigation {
            crate::logging::event_warn("TURN_STOP", fields);
        } else {
            crate::logging::event_info("TURN_STOP", fields);
        }
    }

    pub(super) fn finish(&mut self, session: &str, identity: &TurnIdentity) {
        if !self.emitted {
            // A missing terminal event is not evidence of a crash.
            self.log(session, identity, None, self.reason.unwrap_or("unknown"));
            self.emitted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_aliases_are_known_without_copying_untrusted_strings() {
        assert_eq!(StopLog::provider_reason(" STOP "), "normal");
        assert_eq!(StopLog::provider_reason("stop_sequence"), "normal");
        assert_eq!(
            StopLog::provider_reason("model_context_window_exceeded"),
            "limit"
        );
        assert_eq!(StopLog::provider_reason("SAFETY"), "guardrail");
        assert_eq!(StopLog::provider_reason("private provider text"), "unknown");
    }

    #[test]
    fn recovered_stop_reason_does_not_taint_later_success() {
        let mut log = StopLog::default();
        log.outcome(&ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::LimitReached,
            message: "private".into(),
            provider_stop_reason: Some("max_tokens".into()),
        });
        log.outcome(&ServerEvent::RetryRollback { attempt: 1, max: 2 });
        log.outcome(&ServerEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        });
        assert_eq!(
            log.outcome(&ServerEvent::Done { id: 3 }),
            Some((3, "normal"))
        );
    }

    #[test]
    fn missing_terminal_is_reported_once_and_is_not_assumed_normal() {
        let mut log = StopLog::default();
        let identity = TurnIdentity {
            runtime_generation: "test-runtime".into(),
            turn_id: 1,
        };
        log.outcome(&ServerEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        });
        log.finish("test-session", &identity);
        assert!(log.emitted);
        assert_eq!(log.outcome(&ServerEvent::Done { id: 1 }), None);
        assert!(StopLog::needs_investigation(
            log.reason.unwrap_or("unknown")
        ));
    }

    #[test]
    fn expected_intentional_stops_do_not_need_investigation() {
        assert!(!StopLog::needs_investigation("normal"));
        assert!(!StopLog::needs_investigation("interrupted"));
        for reason in ["failure", "crash", "limit", "guardrail", "unknown"] {
            assert!(StopLog::needs_investigation(reason));
        }
    }

    #[test]
    fn structured_reasons_override_provider_completion_without_logging_messages() {
        use crate::protocol::TurnStopReason;
        for (reason, expected) in [
            (TurnStopReason::Interrupted, "interrupted"),
            (TurnStopReason::Failure, "failure"),
            (TurnStopReason::Crash, "crash"),
            (TurnStopReason::LimitReached, "limit"),
            (TurnStopReason::Unknown, "unknown"),
        ] {
            let mut log = StopLog::default();
            assert_eq!(
                log.outcome(&ServerEvent::TurnStopped {
                    reason,
                    message: "private message must not be logged".into(),
                    provider_stop_reason: Some("end_turn".into()),
                }),
                None
            );
            assert_eq!(
                log.outcome(&ServerEvent::Done { id: 4 }),
                Some((4, expected))
            );
        }
    }

    #[test]
    fn retry_does_not_reuse_previous_provider_completion() {
        let mut log = StopLog::default();
        log.outcome(&ServerEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        });
        log.outcome(&ServerEvent::RetryRollback { attempt: 1, max: 2 });
        assert_eq!(
            log.outcome(&ServerEvent::Done { id: 3 }),
            Some((3, "unknown"))
        );
    }

    #[test]
    fn provider_subturns_do_not_log_and_final_reason_wins() {
        let mut log = StopLog::default();
        assert_eq!(
            log.outcome(&ServerEvent::MessageEnd {
                stop_reason: Some("tool_use".into())
            }),
            None
        );
        assert_eq!(
            log.outcome(&ServerEvent::MessageEnd {
                stop_reason: Some("end_turn".into())
            }),
            None
        );
        assert_eq!(
            log.outcome(&ServerEvent::Done { id: 9 }),
            Some((9, "normal"))
        );
        assert_eq!(log.outcome(&ServerEvent::Done { id: 9 }), None);
    }

    #[test]
    fn unknown_success_is_not_normal_and_errors_are_failures() {
        assert_eq!(
            StopLog::default().outcome(&ServerEvent::Done { id: 1 }),
            Some((1, "unknown"))
        );
        assert_eq!(
            StopLog::default().outcome(&ServerEvent::Error {
                id: 2,
                message: "private".into(),
                retry_after_secs: None
            }),
            Some((2, "failure"))
        );
    }

    #[test]
    fn interruption_and_output_limits_are_abnormal() {
        let mut log = StopLog::default();
        log.outcome(&ServerEvent::Interrupted);
        assert_eq!(
            log.outcome(&ServerEvent::Done { id: 1 }),
            Some((1, "interrupted"))
        );
        let mut log = StopLog::default();
        log.outcome(&ServerEvent::MessageEnd {
            stop_reason: Some("max_tokens".into()),
        });
        assert_eq!(
            log.outcome(&ServerEvent::Done { id: 2 }),
            Some((2, "limit"))
        );
    }

    #[test]
    fn provider_metadata_is_allowlisted() {
        assert_eq!(
            StopLog::provider_reason("private untrusted text"),
            "unknown"
        );
    }
}
