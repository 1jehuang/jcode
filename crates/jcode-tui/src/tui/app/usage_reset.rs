//! Session-local confirmation state. Credentials and redemption keys never enter the bus.
//!
//! Both OpenAI banked resets and Claude session-limit resets follow the same
//! contract: a read-only check pins one login, the user reviews it, and only an
//! explicit `confirm` spends the reset.
use super::{App, DisplayMessage};
use crate::tui::TuiState;
use crate::usage::{PendingAnthropicLimitReset, PendingOpenAiUsageReset};
use tokio::sync::oneshot;

const USAGE: &str = "Usage: /reset usage limits <openai|claude> [confirm|cancel]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provider {
    OpenAi,
    Claude,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Claude => "Claude",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::OpenAi => "/reset usage limits openai",
            Self::Claude => "/reset usage limits claude",
        }
    }

    fn reset_noun(self) -> &'static str {
        match self {
            Self::OpenAi => "usage reset",
            Self::Claude => "session-limit reset",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// `None` means bare `/reset`: pick the provider of the active login.
    Prepare(Option<Provider>),
    Confirm(Provider),
    Cancel(Provider),
    Invalid,
}

fn parse(input: &str) -> Option<Action> {
    let words: Vec<_> = input.split_whitespace().collect();
    if words.first().copied() != Some("/reset") {
        return None;
    }
    let provider = |word: &str| match word.to_ascii_lowercase().as_str() {
        "openai" | "codex" | "chatgpt" => Some(Provider::OpenAi),
        "claude" | "anthropic" => Some(Provider::Claude),
        _ => None,
    };
    Some(match words.as_slice() {
        // The command palette offers /reset itself. It must open the read-only
        // review, not leave the user in a usage/confirm-without-pending loop.
        ["/reset"] => Action::Prepare(None),
        ["/reset", "usage", "limits", name] => match provider(name) {
            Some(provider) => Action::Prepare(Some(provider)),
            None => Action::Invalid,
        },
        ["/reset", "usage", "limits", name, "confirm"] => match provider(name) {
            Some(provider) => Action::Confirm(provider),
            None => Action::Invalid,
        },
        ["/reset", "usage", "limits", name, "cancel"] => match provider(name) {
            Some(provider) => Action::Cancel(provider),
            None => Action::Invalid,
        },
        _ => Action::Invalid,
    })
}

/// A prepared, account-pinned redemption. Holds credentials, so it never
/// leaves this module.
#[derive(Clone, Debug)]
pub(super) enum Pending {
    OpenAi(PendingOpenAiUsageReset),
    Claude(PendingAnthropicLimitReset),
}

impl Pending {
    fn account_label(&self) -> Option<&str> {
        match self {
            Self::OpenAi(pending) => pending.account_label(),
            Self::Claude(pending) => pending.account_label(),
        }
    }

    fn confirmation_message(&self) -> String {
        match self {
            Self::OpenAi(pending) => pending.confirmation_message(),
            Self::Claude(pending) => claude_confirmation_message(pending),
        }
    }
}

fn claude_confirmation_message(pending: &PendingAnthropicLimitReset) -> String {
    let mut message = format!("Claude account: {}", pending.account_display());
    for line in pending.confirmation_details() {
        message.push('\n');
        message.push_str(&line);
    }
    let command = Provider::Claude.command();
    message.push_str(&format!(
        "\nThis spends this week's session-limit reset and cannot be undone. It does not buy credits or raise your plan limits.\nConfirm: {command} confirm\nCancel: {command} cancel"
    ));
    message
}

#[derive(Debug)]
enum Reply<P> {
    /// `Err(message)` means nothing can be claimed right now. Nothing changed.
    Prepared(anyhow::Result<Result<P, String>>),
    Redeemed(anyhow::Result<String>),
}

#[derive(Debug)]
pub(super) struct ResetState<P = Pending> {
    pending: Option<P>,
    /// Provider of the pending confirmation or the operation in flight.
    provider: Provider,
    receiver: Option<oneshot::Receiver<Reply<P>>>,
    redeeming: bool,
    cancelled: bool,
    redeem_account: Option<String>,
    pub(super) refresh_usage: bool,
    quota_refresh: Option<oneshot::Receiver<()>>,
    // Outer Some means invalidation is needed, inner None means the default account.
    pub(super) invalidate_account: Option<(Provider, Option<String>)>,
    pub(super) invalidate_requests: std::collections::HashMap<u64, Option<std::time::Instant>>,
}

impl<P> Default for ResetState<P> {
    fn default() -> Self {
        Self {
            pending: None,
            provider: Provider::OpenAi,
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
        let name = self.provider.name();
        let noun = self.provider.reset_noun();
        match reply {
            Reply::Prepared(Ok(result)) => {
                if self.cancelled {
                    return format!("{name} {noun} preparation cancelled.");
                }
                match result {
                    Ok(pending) => {
                        self.pending = Some(pending);
                        format!("{name} {noun} is ready for review.")
                    }
                    Err(message) => {
                        self.pending = None;
                        format!("{message} No limits were changed.")
                    }
                }
            }
            Reply::Prepared(Err(error)) => format!("Could not prepare {name} {noun}: {error}"),
            Reply::Redeemed(Ok(message)) => {
                self.pending = None;
                message
            }
            Reply::Redeemed(Err(error)) => format!(
                "{name} reset result is uncertain: {error}\n{}",
                if self.pending.is_some() {
                    format!(
                        "The same reset is still pending. Retry {} confirm to reuse the same request, or cancel explicitly.",
                        self.provider.command()
                    )
                } else {
                    "The pending reset was cancelled. Check /usage before preparing another reset."
                        .to_string()
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

async fn prepare(provider: Provider) -> anyhow::Result<Result<Pending, String>> {
    match provider {
        Provider::OpenAi => Ok(crate::usage::prepare_openai_usage_reset()
            .await?
            .map(Pending::OpenAi)
            .ok_or_else(|| "No banked OpenAI usage resets are available.".to_string())),
        Provider::Claude => Ok(crate::usage::prepare_anthropic_limit_reset(
            crate::auth::claude::active_account_label(),
        )
        .await?
        .map(Pending::Claude)
        .map_err(|unavailable| unavailable.message())),
    }
}

async fn redeem(pending: &Pending) -> anyhow::Result<String> {
    match pending {
        Pending::OpenAi(pending) => crate::usage::consume_openai_usage_reset(pending)
            .await
            .map(|outcome| outcome.message()),
        Pending::Claude(pending) => crate::usage::consume_anthropic_limit_reset(pending)
            .await
            .map(|outcome| outcome.message()),
    }
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

    /// Bare `/reset` targets the provider of the active login. Claude logins
    /// get the session-limit reset, everything else keeps the OpenAI default.
    fn default_reset_provider(&self) -> Provider {
        match self.info_widget_data().auth_method {
            crate::tui::info_widget::AuthMethod::AnthropicOAuth => Provider::Claude,
            _ => Provider::OpenAi,
        }
    }

    pub(super) fn handle_usage_reset_command(&mut self, input: &str) -> bool {
        let Some(action) = parse(input) else {
            return false;
        };
        let action = match action {
            Action::Prepare(None) => {
                // A pending review or in-flight check stays the target of a bare /reset.
                let provider = if self.usage_reset.pending.is_some()
                    || self.usage_reset.receiver.is_some()
                {
                    self.usage_reset.provider
                } else {
                    self.default_reset_provider()
                };
                Action::Prepare(Some(provider))
            }
            action => action,
        };
        let current = self.usage_reset.provider;
        let busy = self.usage_reset.receiver.is_some();
        let has_pending = self.usage_reset.pending.is_some();
        let message = match action {
            // Bare /reset is resolved to a provider above.
            Action::Invalid | Action::Prepare(None) => USAGE.to_string(),
            Action::Cancel(provider) if self.usage_reset.redeeming => format!(
                "The confirmed {} reset is already in flight and cannot be cancelled. Wait for its result before cancelling the pending confirmation.",
                if provider == current {
                    provider.name()
                } else {
                    current.name()
                }
            ),
            Action::Cancel(provider) if (has_pending || busy) && provider != current => format!(
                "No {} reset is pending. The pending {} reset is unchanged.",
                provider.name(),
                current.name()
            ),
            Action::Cancel(provider) => {
                self.usage_reset.pending = None;
                self.usage_reset.cancelled = true;
                format!(
                    "{} {} confirmation cleared. This does not undo any reset already requested. Check /usage before preparing another reset.",
                    provider.name(),
                    provider.reset_noun()
                )
            }
            Action::Prepare(Some(provider)) if has_pending && provider == current => self
                .usage_reset
                .pending
                .as_ref()
                .unwrap()
                .confirmation_message(),
            _ if busy => format!(
                "A {} reset operation is already in flight. Please wait.",
                current.name()
            ),
            Action::Prepare(Some(provider)) if has_pending => format!(
                "A {} reset is still pending review. Run {} cancel before checking {} resets.",
                current.name(),
                current.command(),
                provider.name()
            ),
            Action::Confirm(provider) if !has_pending || provider != current => format!(
                "No {} reset is pending. First run {} and review the confirmation.",
                provider.name(),
                provider.command()
            ),
            Action::Prepare(Some(provider)) | Action::Confirm(provider) => {
                let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                    self.push_display_message(DisplayMessage::error(format!(
                        "{} {} requires an active async runtime.",
                        provider.name(),
                        provider.reset_noun()
                    )));
                    return true;
                };
                let (sender, receiver) = oneshot::channel();
                self.usage_reset.provider = provider;
                self.usage_reset.receiver = Some(receiver);
                self.usage_reset.cancelled = false;
                let pending = self.usage_reset.pending.clone();
                let redeeming = matches!(action, Action::Confirm(_));
                self.usage_reset.redeeming = redeeming;
                self.usage_reset.redeem_account = pending
                    .as_ref()
                    .and_then(|pending| pending.account_label().map(str::to_owned));
                runtime.spawn(async move {
                    let reply = if let Some(pending) = pending {
                        Reply::Redeemed(redeem(&pending).await)
                    } else {
                        Reply::Prepared(prepare(provider).await)
                    };
                    let _ = sender.send(reply);
                });
                match (redeeming, provider) {
                    (true, provider) => {
                        format!("Requesting the confirmed {} reset...", provider.name())
                    }
                    (false, Provider::OpenAi) => {
                        "Checking available banked OpenAI usage resets (read-only)...".into()
                    }
                    (false, Provider::Claude) => {
                        "Checking the Claude session-limit reset for this login (read-only)..."
                            .into()
                    }
                }
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
            self.usage_reset.invalidate_account = Some((
                self.usage_reset.provider,
                self.usage_reset.redeem_account.take(),
            ));
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
            "/reset usage limits gemini",
            "/reset usage limits openai confirm extra",
            "/reset usage limits claude now",
        ] {
            assert_eq!(parse(input), Some(Action::Invalid));
        }
        assert_eq!(parse("/reset"), Some(Action::Prepare(None)));
        assert_eq!(
            parse("/reset usage limits openai"),
            Some(Action::Prepare(Some(Provider::OpenAi)))
        );
        assert_eq!(
            parse("/reset usage limits openai confirm"),
            Some(Action::Confirm(Provider::OpenAi))
        );
        assert_eq!(
            parse("/reset usage limits openai cancel"),
            Some(Action::Cancel(Provider::OpenAi))
        );
        assert_eq!(
            parse("/reset usage limits claude"),
            Some(Action::Prepare(Some(Provider::Claude)))
        );
        assert_eq!(
            parse("/reset usage limits Anthropic confirm"),
            Some(Action::Confirm(Provider::Claude))
        );
        assert_eq!(
            parse("/reset usage limits claude cancel"),
            Some(Action::Cancel(Provider::Claude))
        );
        assert_eq!(parse("/resetting"), None);
    }

    #[test]
    fn claude_confirm_requires_a_claude_review_not_an_openai_one() {
        let mut app = crate::tui::app::tests::create_test_app();
        assert!(app.handle_usage_reset_command("/reset usage limits claude confirm"));
        assert!(app.usage_reset.receiver.is_none());
        let last = &app.display_messages.last().unwrap().content;
        assert!(last.contains("No Claude reset is pending"), "{last}");
        assert!(last.contains("/reset usage limits claude"), "{last}");
    }

    #[test]
    fn unavailable_claude_reset_is_reported_without_pending_state() {
        let mut state = ResetState::<String> {
            provider: Provider::Claude,
            ..Default::default()
        };
        let message = state.finish(Reply::Prepared(Ok(Err(
            "A session-limit reset isn't available right now.".into(),
        ))));
        assert!(state.pending.is_none());
        assert!(message.contains("isn't available"));
        assert!(message.contains("No limits were changed"));
    }

    #[test]
    fn claude_uncertain_result_points_at_the_claude_retry_command() {
        let mut state = ResetState::<String> {
            provider: Provider::Claude,
            pending: Some("pinned".into()),
            ..Default::default()
        };
        let message = state.finish(Reply::Redeemed(Err(anyhow::anyhow!("timeout"))));
        assert_eq!(state.pending.as_deref(), Some("pinned"));
        assert!(
            message.contains("/reset usage limits claude confirm"),
            "{message}"
        );
    }

    #[test]
    fn usage_reset_uncertain_result_preserves_exact_pending_request() {
        let mut state = ResetState::<String> {
            pending: Some("same-account-credit-and-uuid".into()),
            ..Default::default()
        };
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
        let mut state = ResetState::<String> {
            cancelled: true,
            ..Default::default()
        };
        state.finish(Reply::Prepared(Ok(Ok("discarded".into()))));
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
        sender
            .send(Reply::Prepared(Ok(Err("none".into()))))
            .unwrap();
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
        assert_eq!(parse(&app.input), Some(Action::Prepare(None)));
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
            assert_eq!(
                parse(&app.input),
                Some(Action::Prepare(Some(Provider::OpenAi)))
            );
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
