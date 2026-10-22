//! Device-flow TUI logins: GitHub Copilot and the native Muse subscription
//! (experimental opt-in).
//!
//! Extracted from `auth.rs` so device-flow polling loops stay in one cohesive
//! place. The Muse subscription background task never persists credentials: it
//! hands approved credentials to the main thread through the pending slot, and
//! `handle_login_completed` takes and saves them synchronously for the exact
//! pending attempt only.

use super::super::{App, DisplayMessage, helpers::copy_to_clipboard};
use super::auth_types::PendingLogin;
use crate::auth::muse;
use crate::bus::{Bus, BusEvent, LoginCompleted};

/// Device-code progress displays (`copilot_code`, `muse_subscription_code#…`)
/// are pending approval, never completed logins.
pub(crate) fn is_device_code_progress(provider: &str) -> bool {
    provider == "copilot_code" || provider.starts_with("muse_subscription_code#")
}

/// Attempt-scoped Muse approval events (`muse-subscription#…`) are consumed by
/// the pending-slot handoff in `handle_login_completed`, never as generic
/// success. Anything else with this prefix shape is not a completed login.
pub(crate) fn is_muse_subscription_attempt_event(provider: &str) -> bool {
    provider.starts_with("muse-subscription#")
}

/// Match an attempt identifier against a pending Muse attempt.
///
/// Both attempt IDs can be optionally prefixed with "muse-subscription#"
/// (from provider string stripping or test setups).
pub(crate) fn pending_attempt_matches(pending: &str, incoming: &str) -> bool {
    let p = pending
        .strip_prefix("muse-subscription#")
        .unwrap_or(pending);
    let i = incoming
        .strip_prefix("muse-subscription#")
        .unwrap_or(incoming);
    p == i
}

/// Compute the next polling interval after a `SlowDown` outcome.
///
/// RFC 8628 §3.5 requires that each slow_down response adds at least 5 seconds
/// to the interval and is carried forward into subsequent polling requests.
pub(crate) fn next_slow_down_interval(current_interval: u64, retry_after: u64) -> u64 {
    retry_after.max(current_interval.saturating_add(5))
}

/// Whether a bus login event counts as a completed login for success-gating
/// (remote forwarding, onboarding noise). Device-code progress displays and
/// attempt-scoped Muse approval events are pending approval or slot-handoff
/// only.
pub(crate) fn is_completed_login(login: &LoginCompleted) -> bool {
    login.success
        && !is_device_code_progress(&login.provider)
        && !is_muse_subscription_attempt_event(&login.provider)
}

/// Whether `profile` is the native Muse subscription profile, whose
/// credentials live in `auth::muse`'s own JSON store rather than profile env
/// files.
pub(super) fn is_muse_subscription_profile(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> bool {
    profile.id == crate::provider_catalog::MUSE_SUBSCRIPTION_PROFILE.id
}

/// Clear the native Muse subscription credentials for single-provider logout.
/// Returns the user-facing summary message.
pub(super) fn logout_provider() -> anyhow::Result<String> {
    muse::clear_credentials()?;
    Ok("Logged out of Muse subscription.".to_string())
}

/// Clear the native Muse subscription credentials for logout-all, recording
/// into the shared summary/errors exactly like the generic API-key path.
pub(super) fn logout_all(summary: &mut Vec<String>, errors: &mut Vec<String>) {
    match muse::clear_credentials() {
        Ok(()) => summary.push("Muse subscription".to_string()),
        Err(err) => errors.push(format!("Muse subscription: {}", err)),
    }
}

impl App {
    pub(super) fn start_copilot_login(&mut self) {
        self.set_status_notice("Login: copilot device flow...");
        self.begin_pending_login(PendingLogin::Copilot);

        tokio::spawn(async move {
            let client = crate::provider::shared_http_client();

            let device_resp = match crate::auth::copilot::initiate_device_flow(&client).await {
                Ok(resp) => resp,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot device flow failed: {}", e),
                    }));
                    return;
                }
            };

            let user_code = device_resp.user_code.clone();
            let verification_uri = device_resp.verification_uri.clone();

            let clipboard_ok = copy_to_clipboard(&user_code);
            let clipboard_msg = if clipboard_ok {
                " (copied to clipboard - just paste it!)"
            } else {
                ""
            };

            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: "copilot_code".to_string(),
                success: true,
                message: {
                    let qr_section = jcode_terminal_prompt::qr::markdown_section_for_tui(
                        &verification_uri,
                        "Scan this on another device to open the GitHub verification page:",
                    )
                    .map(|section| format!("\n\n{section}"))
                    .unwrap_or_default();
                    format!(
                        "GitHub Copilot Login\n\n\
                         Your code: {}{}\n\n\
                         Opening browser to {} ...\n\
                         Paste the code there and authorize.{}\n\n\
                         Waiting for authorization... (type /cancel to abort)",
                        user_code, clipboard_msg, verification_uri, qr_section
                    )
                },
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = Self::open_auth_browser(&verification_uri);

            let token = match crate::auth::copilot::poll_for_access_token(
                &client,
                &device_resp.device_code,
                device_resp.interval,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot login failed: {}", e),
                    }));
                    return;
                }
            };

            let username = crate::auth::copilot::fetch_github_username(&client, &token)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

            match crate::auth::copilot::save_github_token(&token, &username) {
                Ok(()) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: true,
                        message: format!(
                            "Authenticated as {} via GitHub Copilot.\n\n\
                             Copilot models are now available in /model.",
                            username
                        ),
                    }));
                }
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Failed to save Copilot token: {}", e),
                    }));
                }
            }
        });

        self.push_display_message(DisplayMessage::system(
            "GitHub Copilot Login\n\n\
             Starting device flow... please wait. Type /cancel to abort."
                .to_string(),
        ));
    }
}

/// Native Muse subscription device flow (experimental opt-in).
///
/// Mirrors the Copilot device flow: the experimental gate and warning come
/// from auth::muse, the challenge is shown with a QR, and a background task
/// polls the single-attempt `poll_login` to a terminal outcome. No billing
/// is claimed and no local Muse installation is required.
impl App {
    pub(super) fn start_muse_subscription_login(&mut self) {
        if let Err(err) = crate::auth::muse::ensure_enabled() {
            self.push_display_message(DisplayMessage::error(format!(
                "Muse subscription login is experimental and opt-in: {}",
                err
            )));
            self.set_status_notice("Login: muse-subscription gated");
            return;
        }
        self.set_status_notice("Login: muse-subscription device flow...");
        // Opaque per-attempt id: progress/completion events carry it in the
        // provider string so stale or cross-session events never match a
        // different pending attempt.
        let attempt = format!(
            "{:016x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos() as u64 ^ elapsed.as_secs())
                .unwrap_or(0)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        );
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let approved = std::sync::Arc::new(std::sync::Mutex::new(None));
        self.begin_pending_login(PendingLogin::MuseSubscription {
            attempt: attempt.clone(),
            cancelled: std::sync::Arc::clone(&cancelled),
            approved: std::sync::Arc::clone(&approved),
        });

        tokio::spawn(async move {
            use std::sync::atomic::Ordering;
            let is_cancelled = || cancelled.load(Ordering::Acquire);
            // Completion events carry the attempt id; the handler only accepts
            // events whose attempt matches the currently pending one.
            let completion_provider = format!("muse-subscription#{attempt}");
            let progress_provider = format!("muse_subscription_code#{attempt}");
            let fail = |message: String| {
                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                    provider: completion_provider.clone(),
                    success: false,
                    message,
                }));
            };
            let client = match crate::auth::muse::http_client() {
                Ok(client) => client,
                Err(err) => {
                    fail(format!(
                        "Muse subscription device flow failed to start: {}",
                        err
                    ));
                    return;
                }
            };
            if is_cancelled() {
                return;
            }
            let challenge = match crate::auth::muse::begin_login(&client).await {
                Ok(challenge) => challenge,
                Err(err) => {
                    fail(format!("Muse subscription device flow failed: {}", err));
                    return;
                }
            };
            // Check cancellation after the await and before any publish: a
            // /cancel during begin must not surface a stale code display.
            if is_cancelled() {
                return;
            }
            let url = challenge
                .verification_uri_complete
                .clone()
                .unwrap_or_else(|| challenge.verification_uri.clone());
            let user_code = challenge.user_code.clone();

            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: progress_provider,
                success: true,
                message: {
                    let warning = crate::auth::muse::experimental_warning_message();
                    let qr_section = jcode_terminal_prompt::qr::markdown_section_for_tui(
                        &url,
                        "Scan this on another device to open the Meta verification page:",
                    )
                    .map(|section| format!("\n\n{section}"))
                    .unwrap_or_default();
                    format!(
                        "{warning}\n\n\
                         Muse Subscription Login\n\n\
                         Your code: {user_code}\n\n\
                         Opening browser to {url} ...\n\
                         Paste the code there and authorize.{qr_section}\n\n\
                         No local Muse installation is required.\n\n\
                         Waiting for authorization... (type /cancel to abort)"
                    )
                },
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if is_cancelled() {
                return;
            }
            let _ = Self::open_auth_browser(&url);

            let mut interval_secs = u64::try_from(challenge.interval).unwrap_or(5).max(1);
            let expires_secs = u64::try_from(challenge.expires_in).unwrap_or(1800).max(60);
            let mut wait = std::time::Duration::from_secs(interval_secs);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_secs);
            let credentials = loop {
                if is_cancelled() {
                    // /cancel clears the pending state; stay silent. An empty
                    // message keeps `fail` from publishing.
                    break Err(String::new());
                }
                match crate::auth::muse::poll_login_by_code(
                    &client,
                    &challenge.device_code,
                    interval_secs,
                )
                .await
                {
                    Ok(crate::auth::muse::PollOutcome::Approved(credentials)) => {
                        break Ok(*credentials);
                    }
                    Ok(crate::auth::muse::PollOutcome::Denied) => {
                        break Err(
                            "Muse subscription login was denied or cancelled in the browser."
                                .to_string(),
                        );
                    }
                    Ok(crate::auth::muse::PollOutcome::Expired) => {
                        break Err("The Muse device code expired. Start a new login.".to_string());
                    }
                    Ok(crate::auth::muse::PollOutcome::Pending) => {}
                    Ok(crate::auth::muse::PollOutcome::SlowDown { retry_after }) => {
                        interval_secs = next_slow_down_interval(interval_secs, retry_after);
                        wait = std::time::Duration::from_secs(interval_secs);
                    }
                    Err(err) => break Err(format!("Muse subscription login failed: {}", err)),
                }
                if std::time::Instant::now() + wait >= deadline {
                    break Err("Timed out waiting for Muse browser authorization.".to_string());
                }
                tokio::time::sleep(wait).await;
            };
            let credentials = match credentials {
                Ok(credentials) => credentials,
                Err(message) => {
                    if !message.is_empty() {
                        fail(message);
                    }
                    return;
                }
            };
            // Hand off to the main thread via the slot; the background task
            // never persists. A late /cancel between approval and handoff
            // drops the credentials instead of publishing success.
            if is_cancelled() {
                return;
            }
            if let Ok(mut slot) = approved.lock() {
                if is_cancelled() {
                    return;
                }
                *slot = Some(credentials);
            } else {
                return;
            }
            if is_cancelled() {
                if let Ok(mut slot) = approved.lock() {
                    *slot = None;
                }
                return;
            }
            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: completion_provider,
                success: true,
                // No secrets: the credentials travel via the pending slot, and
                // the handler takes and saves them synchronously.
                message: "Muse subscription approved in the browser; finalizing...".to_string(),
            }));
        });

        self.push_display_message(DisplayMessage::system(
            "Muse Subscription Login\n\n\
             Starting device flow... please wait. Type /cancel to abort."
                .to_string(),
        ));
    }
}

impl App {
    /// Clear the login input that the generic same-provider cleanup in
    /// `handle_login_completed` would have cleared.
    ///
    /// That cleanup is skipped for Muse because Muse pending state is
    /// attempt-owned, so the equivalent must run here instead, and only for the
    /// attempt that actually finished. Without it, an approval arriving while
    /// the user is still typing or pasting the device code leaves that text
    /// armed, and the next Enter submits it as a model prompt.
    fn clear_muse_login_input(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
        self.pasted_contents.clear();
        self.clear_input_undo_history();
    }

    fn clear_matching_muse_pending(&mut self, attempt: &str) {
        let is_match = match &self.pending_login {
            Some(PendingLogin::MuseSubscription {
                attempt: pending_attempt,
                ..
            }) => pending_attempt_matches(pending_attempt, attempt),
            _ => false,
        };
        if is_match {
            self.pending_login = None;
            self.clear_muse_login_input();
        }
    }

    /// Attempt-scoped Muse approval handoff.
    ///
    /// The background task places approved credentials in the pending slot and
    /// never persists. This takes them on the main thread and saves synchronously,
    /// but only when the event's attempt id matches the currently pending
    /// attempt. Anything else (cancelled, superseded, or cross-session) is
    /// ignored, and a missing slot can never switch the provider.
    ///
    /// Returns true when the event was consumed (matched or rejected as stale).
    pub(super) fn take_muse_subscription_approval(&mut self, login: &LoginCompleted) -> bool {
        let Some(attempt) = login
            .provider
            .strip_prefix("muse-subscription#")
            .map(str::to_string)
        else {
            return false;
        };
        let (slot, cancelled) = match &self.pending_login {
            Some(PendingLogin::MuseSubscription {
                attempt: pending_attempt,
                cancelled,
                approved,
            }) if pending_attempt_matches(pending_attempt, &attempt) => {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return true;
                }
                (
                    std::sync::Arc::clone(approved),
                    std::sync::Arc::clone(cancelled),
                )
            }
            _ => return true,
        };

        if !login.success {
            let _ = slot.lock().map(|mut guard| guard.take());
            self.handle_login_completed(LoginCompleted {
                provider: "muse-subscription".to_string(),
                success: false,
                message: login.message.clone(),
            });
            self.clear_matching_muse_pending(&attempt);
            return true;
        }

        let credentials = match slot.lock().map(|mut guard| guard.take()) {
            Ok(credentials) => credentials,
            Err(_) => return true,
        };
        let Some(credentials) = credentials else {
            // Approved event without slot credentials: never a login.
            return true;
        };
        // Re-check the captured flag: a /cancel that raced the event (or
        // fired between the take and now) must not save.
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return true;
        }
        match crate::auth::muse::save_credentials(&credentials) {
            Ok(()) => {
                crate::telemetry::record_auth_success("muse-subscription", "device_code");
                self.clear_matching_muse_pending(&attempt);
                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                    provider: "muse-subscription".to_string(),
                    success: true,
                    message: "Muse subscription login complete.\n\n\
                              Muse models are now available in /model."
                        .to_string(),
                }));
            }
            Err(err) => {
                self.handle_login_completed(LoginCompleted {
                    provider: "muse-subscription".to_string(),
                    success: false,
                    message: format!("Muse subscription was approved but saving failed: {}", err),
                });
                self.clear_matching_muse_pending(&attempt);
            }
        }
        true
    }
}
