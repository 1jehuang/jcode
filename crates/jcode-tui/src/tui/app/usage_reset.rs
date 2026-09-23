//! Session-local confirmation state. Credentials and redemption keys never enter the bus.
use super::{App, DisplayMessage};
use crate::tui::TuiState;
use crate::usage::PendingOpenAiUsageReset;
use tokio::sync::oneshot;

const USAGE: &str = "Usage: /reset usage limits openai [confirm|cancel]";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Prepare,
    Confirm,
    Cancel,
    Invalid,
}

fn parse(input: &str) -> Option<Action> {
    let words: Vec<_> = input.split_whitespace().collect();
    if words.first().copied() != Some("/reset") {
        return None;
    }
    Some(match words.as_slice() {
        // The command palette offers /reset itself. It must open the read-only
        // review, not leave the user in a usage/confirm-without-pending loop.
        ["/reset"] => Action::Prepare,
        ["/reset", "usage", "limits", "openai"] => Action::Prepare,
        ["/reset", "usage", "limits", "openai", "confirm"] => Action::Confirm,
        ["/reset", "usage", "limits", "openai", "cancel"] => Action::Cancel,
        _ => Action::Invalid,
    })
}

#[derive(Debug)]
enum Reply<P> {
    Prepared(anyhow::Result<Option<P>>),
    Redeemed(anyhow::Result<String>),
}

#[derive(Debug)]
pub(super) struct ResetState<P = PendingOpenAiUsageReset> {
    pending: Option<P>,
    receiver: Option<oneshot::Receiver<Reply<P>>>,
    redeeming: bool,
    cancelled: bool,
    redeem_account: Option<String>,
    pub(super) refresh_usage: bool,
    quota_refresh: Option<oneshot::Receiver<()>>,
    // Outer Some means invalidation is needed, inner None means the default account.
    pub(super) invalidate_account: Option<Option<String>>,
    pub(super) invalidate_requests: std::collections::HashMap<u64, Option<std::time::Instant>>,
}

impl<P> Default for ResetState<P> {
    fn default() -> Self {
        Self {
            pending: None,
            receiver: None,
            redeeming: false,
            cancelled: false,
            redeem_account: None,
            refresh_usage: false,
            quota_refresh: None,
            invalidate_account: None,
            invalidate_requests: Default::default(),
        }
    }
}

impl<P> ResetState<P> {
    fn finish(&mut self, reply: Reply<P>) -> String {
        self.receiver = None;
        self.redeeming = false;
        match reply {
            Reply::Prepared(Ok(pending)) => {
                if self.cancelled {
                    return "OpenAI usage reset preparation cancelled.".into();
                }
                self.pending = pending;
                "No banked OpenAI usage resets are available. No limits were changed.".into()
            }
            Reply::Prepared(Err(error)) => format!("Could not prepare OpenAI usage reset: {error}"),
            Reply::Redeemed(Ok(message)) => {
                self.pending = None;
                message
            }
            Reply::Redeemed(Err(error)) => format!(
                "OpenAI reset result is uncertain: {error}\n{}",
                if self.pending.is_some() {
                    "The same reset is still pending. Retry /reset usage limits openai confirm to reuse the same request, or cancel explicitly."
                } else {
                    "The pending reset was cancelled. Check /usage before preparing another reset."
                }
            ),
        }
    }
}

fn is_hard_openai_quota_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "usage_limit_reached",
        "insufficient_quota",
        "usage limit has been reached",
        "usage limit reached",
        "you have hit your usage limit",
        "you've hit your usage limit",
        "you’ve hit your usage limit",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

impl App {
    pub(super) fn refresh_openai_usage_after_quota_error(&mut self, error: &str) {
        if !is_hard_openai_quota_error(error)
            || crate::tui::is_ssh_remote()
            || self.info_widget_data().auth_method
                != crate::tui::info_widget::AuthMethod::OpenAIOAuth
            || self.usage_reset.quota_refresh.is_some()
        {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let account = crate::auth::codex::active_account_label();
        let (sender, receiver) = oneshot::channel();
        self.usage_reset.quota_refresh = Some(receiver);
        runtime.spawn(async move {
            // This invalidates quota snapshots only, never account cooldowns.
            crate::usage::invalidate_openai_usage_cache(account.as_deref()).await;
            let _ = sender.send(());
        });
    }

    pub(super) fn handle_usage_reset_command(&mut self, input: &str) -> bool {
        let Some(action) = parse(input) else {
            return false;
        };
        let message = match action {
            Action::Invalid => USAGE.to_string(),
            Action::Cancel if self.usage_reset.redeeming => {
                "The confirmed reset is already in flight and cannot be cancelled. Wait for its result before cancelling the pending confirmation.".into()
            }
            Action::Cancel => {
                self.usage_reset.pending = None;
                self.usage_reset.cancelled = true;
                "OpenAI usage reset confirmation cleared. This does not undo any reset already requested. Check /usage before preparing another reset.".into()
            }
            Action::Prepare if self.usage_reset.pending.is_some() => {
                self.usage_reset.pending.as_ref().unwrap().confirmation_message()
            }
            _ if self.usage_reset.receiver.is_some() => {
                "An OpenAI usage reset operation is already in flight. Please wait.".into()
            }
            Action::Confirm if self.usage_reset.pending.is_none() => {
                "No OpenAI reset is pending. First run /reset usage limits openai and review the confirmation.".into()
            }
            Action::Prepare | Action::Confirm => {
                let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                    self.push_display_message(DisplayMessage::error("OpenAI usage reset requires an active async runtime."));
                    return true;
                };
                let (sender, receiver) = oneshot::channel();
                self.usage_reset.receiver = Some(receiver);
                self.usage_reset.cancelled = false;
                let pending = self.usage_reset.pending.clone();
                let redeeming = action == Action::Confirm;
                self.usage_reset.redeeming = redeeming;
                self.usage_reset.redeem_account = pending.as_ref().and_then(|pending| pending.account_label().map(str::to_owned));
                runtime.spawn(async move {
                    let reply = if let Some(pending) = pending {
                        Reply::Redeemed(crate::usage::consume_openai_usage_reset(&pending).await.map(|outcome| outcome.message()))
                    } else {
                        Reply::Prepared(crate::usage::prepare_openai_usage_reset().await)
                    };
                    let _ = sender.send(reply);
                });
                if redeeming { "Requesting the confirmed OpenAI usage reset...".into() }
                else { "Checking available banked OpenAI usage resets (read-only)...".into() }
            }
        };
        self.push_display_message(DisplayMessage::system(message));
        true
    }

    pub(super) fn poll_usage_reset(&mut self) -> bool {
        if let Some(receiver) = self.usage_reset.quota_refresh.as_mut()
            && !matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            )
        {
            self.usage_reset.quota_refresh = None;
            self.usage_reset.refresh_usage = true;
        }
        // A report already in flight may have read pre-reset state. Queue a fresh
        // report after it completes instead of silently dropping the refresh.
        if self.usage_reset.refresh_usage && !self.usage_report_refreshing {
            self.usage_reset.refresh_usage = false;
            self.request_usage_report();
        }
        let Some(receiver) = self.usage_reset.receiver.as_mut() else {
            return false;
        };
        let reply = match receiver.try_recv() {
            Ok(reply) => reply,
            Err(oneshot::error::TryRecvError::Empty) => return false,
            Err(oneshot::error::TryRecvError::Closed) => {
                let error = anyhow::anyhow!("The reset worker stopped before returning a result");
                if self.usage_reset.redeeming {
                    Reply::Redeemed(Err(error))
                } else {
                    Reply::Prepared(Err(error))
                }
            }
        };
        let redeemed = matches!(reply, Reply::Redeemed(_));
        if redeemed && self.is_remote {
            self.usage_reset.invalidate_account = Some(self.usage_reset.redeem_account.take());
        }
        let message = self.usage_reset.finish(reply);
        let message = if !redeemed {
            self.usage_reset
                .pending
                .as_ref()
                .map(|pending| pending.confirmation_message())
                .unwrap_or(message)
        } else {
            message
        };
        self.push_display_message(DisplayMessage::system(message));
        if redeemed {
            self.push_usage_loading_card();
            self.usage_reset.refresh_usage = true;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_reset_parser_claims_malformed_commands() {
        for input in [
            "/reset usage",
            "/reset usage limits claude",
            "/reset usage limits openai confirm extra",
        ] {
            assert_eq!(parse(input), Some(Action::Invalid));
        }
        assert_eq!(parse("/reset"), Some(Action::Prepare));
        assert_eq!(parse("/reset usage limits openai"), Some(Action::Prepare));
        assert_eq!(
            parse("/reset usage limits openai confirm"),
            Some(Action::Confirm)
        );
        assert_eq!(
            parse("/reset usage limits openai cancel"),
            Some(Action::Cancel)
        );
        assert_eq!(parse("/resetting"), None);
    }

    #[test]
    fn usage_reset_uncertain_result_preserves_exact_pending_request() {
        let mut state = ResetState::<String>::default();
        state.pending = Some("same-account-credit-and-uuid".into());
        state.finish(Reply::Redeemed(Err(anyhow::anyhow!("timeout"))));
        assert_eq!(
            state.pending.as_deref(),
            Some("same-account-credit-and-uuid")
        );
        state.finish(Reply::Redeemed(Ok("already redeemed".into())));
        assert!(state.pending.is_none());
    }

    #[test]
    fn usage_reset_cancelled_prepare_cannot_restore_pending() {
        let mut state = ResetState::<String>::default();
        state.cancelled = true;
        state.finish(Reply::Prepared(Ok(Some("discarded".into()))));
        assert!(state.pending.is_none());
        assert!(state.receiver.is_none());
    }

    #[test]
    fn usage_reset_confirm_requires_pending_and_malformed_never_dispatches() {
        let mut app = crate::tui::app::tests::create_test_app();
        assert!(app.handle_usage_reset_command("/reset usage limits openai confirm"));
        assert!(app.usage_reset.receiver.is_none());
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("No OpenAI reset is pending")
        );
        assert!(super::super::commands_dispatch::dispatch_local_command(
            &mut app,
            "/reset usage typo"
        ));
        assert!(app.display_messages.last().unwrap().content.contains(USAGE));
        assert!(!app.is_processing);
    }

    #[test]
    fn usage_reset_in_flight_cancel_keeps_single_worker_and_preserves_input() {
        let mut app = crate::tui::app::tests::create_test_app();
        let (sender, receiver) = oneshot::channel();
        app.usage_reset.receiver = Some(receiver);
        assert!(app.handle_usage_reset_command("/reset usage limits openai confirm"));
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("already in flight")
        );
        assert!(app.handle_usage_reset_command("/reset usage limits openai cancel"));
        assert!(app.handle_usage_reset_command("/reset usage limits openai"));
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("already in flight")
        );
        app.input = "unfinished user draft".into();
        sender.send(Reply::Prepared(Ok(None))).unwrap();
        assert!(app.poll_usage_reset());
        assert_eq!(app.input, "unfinished user draft");
        assert!(app.usage_reset.pending.is_none());
        assert!(app.usage_reset.receiver.is_none());
    }

    #[test]
    fn usage_reset_refresh_ack_does_not_complete_active_turn() {
        let mut app = crate::tui::app::tests::create_test_app();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.usage_reset
            .invalidate_requests
            .insert(42, Some(std::time::Instant::now()));
        app.usage_reset.invalidate_requests.insert(43, None);
        app.is_processing = true;
        app.usage_report_refreshing = true;
        app.current_message_id = Some(7);
        assert!(
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote)
        );
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(7));
        assert!(app.usage_reset.invalidate_requests.contains_key(&43));
        assert!(app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 43,
                message: "cache refresh rejected".into(),
                retry_after_secs: None,
            },
            &mut remote
        ));
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(7));
        assert!(app.usage_reset.invalidate_requests.is_empty());
    }

    #[test]
    fn usage_reset_registered_with_subcommand_completions() {
        let app = crate::tui::app::tests::create_test_app();
        assert!(App::command_accepts_args("/reset"));
        let suggestions = app.get_suggestions_for("/reset usage limits openai ");
        assert!(
            suggestions
                .iter()
                .any(|(command, _)| command.ends_with(" confirm"))
        );
        assert!(
            suggestions
                .iter()
                .any(|(command, _)| command.ends_with(" cancel"))
        );
    }

    #[test]
    fn usage_reset_palette_command_opens_read_only_review() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.input = "/rese".into();
        assert!(app.accept_selected_command_suggestion());
        assert_eq!(app.input, "/reset");
        assert!(!app.accept_selected_command_suggestion());
        assert_eq!(parse(&app.input), Some(Action::Prepare));
        // Without a runtime this exercises dispatch without any network I/O.
        assert!(super::super::commands_dispatch::dispatch_local_command(
            &mut app, "/reset"
        ));
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("active async runtime")
        );
        assert!(app.usage_reset.receiver.is_none());
        assert!(!app.usage_reset.redeeming);
    }

    #[test]
    fn usage_reset_enter_preserves_review_with_trailing_whitespace() {
        let mut app = crate::tui::app::tests::create_test_app();
        for input in ["/reset usage limits openai", "/reset usage limits openai "] {
            app.input = input.into();
            assert!(!app.accept_selected_command_suggestion());
            assert_eq!(parse(&app.input), Some(Action::Prepare));
        }
    }

    #[test]
    fn usage_reset_quota_refresh_excludes_transient_rate_limits() {
        assert!(is_hard_openai_quota_error("usage_limit_reached"));
        assert!(is_hard_openai_quota_error("INSUFFICIENT_QUOTA"));
        assert!(is_hard_openai_quota_error(
            "Rate limited: The usage limit has been reached. Plan: team. Resets in 30d 4h 29m (2026-08-21 04:31 UTC)."
        ));
        assert!(is_hard_openai_quota_error("You’ve hit your usage limit"));
        assert!(!is_hard_openai_quota_error(
            "Rate limited: Too many requests. Resets in 1m."
        ));
        assert!(!is_hard_openai_quota_error("429 too many requests"));
        assert!(!is_hard_openai_quota_error("tokens per minute rate limit"));
        assert!(!is_hard_openai_quota_error("TPM quota exceeded"));
    }

    #[test]
    fn usage_reset_cannot_cancel_already_submitted_redemption() {
        let mut app = crate::tui::app::tests::create_test_app();
        let (_sender, receiver) = oneshot::channel();
        app.usage_reset.receiver = Some(receiver);
        app.usage_reset.redeeming = true;
        app.handle_usage_reset_command("/reset usage limits openai cancel");
        assert!(!app.usage_reset.cancelled);
        assert!(app.usage_reset.receiver.is_some());
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("cannot be cancelled")
        );
    }
}
