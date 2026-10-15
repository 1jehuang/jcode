use super::*;

impl App {
    pub(in crate::tui::app) fn show_jcode_subscription_status(&mut self) {
        let configured_key = crate::subscription_catalog::configured_api_key().is_some();
        let configured_base = crate::subscription_catalog::configured_api_base()
            .unwrap_or_else(|| crate::subscription_catalog::DEFAULT_JCODE_API_BASE.to_string());
        let runtime_mode = crate::subscription_catalog::is_runtime_mode_enabled();

        let mut message = String::from("Jcode Hosted Model Status\n\n");
        message.push_str(&format!(
            "  - Credentials: {}\n",
            if configured_key {
                "configured"
            } else {
                "not configured (/login jcode)"
            }
        ));
        message.push_str(&format!(
            "  - Router base: {}{}\n",
            configured_base,
            if crate::subscription_catalog::has_router_base() {
                ""
            } else {
                " (default)"
            }
        ));
        message.push_str("  - Billing: pay as you go, no subscription fee\n");
        message.push_str(&format!(
            "  - Runtime mode: {}\n\n",
            if runtime_mode {
                "active for this session"
            } else {
                "inactive for this session"
            }
        ));

        message.push_str("Catalog\n\n");
        for model in crate::subscription_catalog::curated_models() {
            let default_suffix = if model.default_enabled {
                " (default)"
            } else {
                ""
            };
            let tier_suffix = String::new();
            message.push_str(&format!(
                "  - {} - {}{}{}\n      - {}\n      - {}\n",
                model.display_name,
                model.id,
                default_suffix,
                tier_suffix,
                crate::subscription_catalog::routing_policy_detail(model),
                model.note
            ));
        }

        message.push_str("\nBilling\n\n");
        message.push_str("  - Set the monthly spending limit you control in your Jcode account\n");
        message.push_str("  - Email and account warnings are sent at usage milestones\n");
        message.push_str("  - Warning milestones do not rate limit hosted requests\n");
        message.push_str("  - Charges begin at $20, then use progressively larger tranches\n");
        message.push_str("  - Any unbilled remainder is collected at your limit or month end\n");

        if configured_key {
            message.push_str("\nFetching hosted usage and spending limit...");
        } else {
            message.push_str(
                "\nLog in with /login jcode to set a spending limit and connect hosted models.",
            );
        }

        self.push_display_message(DisplayMessage::system(message));

        // With credentials present, fetch live account status (/v1/me) in the
        // background and surface it via a UiActivity card. Short timeout keeps
        // this responsive; offline failures degrade to a quiet log line.
        if configured_key {
            let session_id = self.session.id.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    match crate::subscription_api::fetch_subscription_me().await {
                        Ok(me) => {
                            let resets = me
                                .usage
                                .resets_at
                                .as_deref()
                                .map_or_else(String::new, |at| format!(", resets {}", at));
                            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                                crate::bus::UiActivity::background(
                                    Some(session_id),
                                    format!(
                                        "Jcode Hosted Model Account\n\n  - Email: {}\n  - Billing: {}\n  - Spend: ${:.2} of ${:.2} monthly limit\n  - Billed in tranches: ${:.2}{}{}",
                                        me.email,
                                        me.status,
                                        me.usage.used_usd,
                                        me.usage.budget_usd,
                                        me.usage.billed_usd,
                                        me.usage
                                            .next_charge_at_usd
                                            .map_or_else(String::new, |amount| format!("\n  - Next tranche at: ${amount:.2}")),
                                        resets
                                    ),
                                    Some("Hosted usage: account status loaded"),
                                ),
                            ));
                        }
                        Err(error) => {
                            let message = if error
                                .downcast_ref::<crate::subscription_api::AccountApiError>()
                                == Some(&crate::subscription_api::AccountApiError::Unauthorized)
                            {
                                revoked_account_status_message(crate::subscription_catalog::clear_account_credentials())
                            } else {
                                format!(
                                    "Jcode Account Status\n\nCould not load /v1/me: {}\n\nThe local credential was retained. Retry /account jcode status, open /account jcode manage, or use /account jcode logout.",
                                    error
                                )
                            };
                            crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                                crate::bus::UiActivity::background(
                                    Some(session_id),
                                    message,
                                    Some("Jcode account status unavailable"),
                                ),
                            ));
                        }
                    }
                });
            }
        }
    }
}

fn revoked_account_status_message(cleanup: anyhow::Result<()>) -> String {
    match cleanup {
                                    Ok(()) => "Jcode Account Status\n\nThe saved account key was revoked or expired. Local credentials were cleared. Use /account jcode login to sign in again.".to_string(),
                                    Err(clear_error) => format!(
                                        "Jcode Account Status\n\nThe saved account key was revoked or expired, but local credentials could not be cleared: {clear_error}\n\nUse /account jcode logout to retry cleanup before signing in again."
                                    ),
                                }
}

#[cfg(test)]
#[path = "auth_subscription_status_tests.rs"]
mod tests;
