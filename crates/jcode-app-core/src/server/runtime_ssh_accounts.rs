//! Host-only account operations for SSH clients. Never serialize credentials or pending resets.
use crate::agent::Agent;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub(super) fn handles(command: &str) -> bool {
    command == "ssh_account"
}

enum Pending {
    OpenAi(Box<crate::usage::PendingOpenAiUsageReset>),
    Claude(crate::usage::PendingAnthropicLimitReset),
}
impl Pending {
    fn provider(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "openai",
            Self::Claude(_) => "claude",
        }
    }
    fn message(&self) -> String {
        match self {
            Self::OpenAi(p) => p.confirmation_message(),
            Self::Claude(p) => format!(
                "Claude account: {}\n{}\nThis spends this week's session-limit reset and cannot be undone.\nConfirm: /reset usage limits claude confirm\nCancel: /reset usage limits claude cancel",
                p.account_display(),
                p.confirmation_details().join("\n")
            ),
        }
    }
}
#[derive(Default)]
struct Review {
    pending: Option<Pending>,
}
type Reviews = HashMap<String, (Instant, Arc<Mutex<Review>>)>;
static REVIEWS: OnceLock<Mutex<Reviews>> = OnceLock::new();
static REMOVALS: OnceLock<Mutex<HashMap<String, (Instant, String, String)>>> = OnceLock::new();

async fn remove_account(
    session: &str,
    provider: &str,
    label: &str,
    confirmed: bool,
) -> Result<String> {
    if !matches!(provider, "claude" | "openai") {
        bail!("Provider does not support named account removal")
    }
    let mut reviews = REMOVALS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .await;
    reviews.retain(|_, (at, _, _)| at.elapsed() < Duration::from_secs(600));
    if !confirmed {
        if reviews.len() >= 128 && !reviews.contains_key(session) {
            bail!("Too many pending removal reviews")
        }
        reviews.insert(
            session.to_string(),
            (Instant::now(), provider.to_string(), label.to_string()),
        );
        return Ok(format!(
            "Remove {provider} account {label} from the runtime host? This cannot be undone.\nConfirm: /account {provider} remove {label} confirm\nNothing was removed."
        ));
    }
    if !reviews
        .get(session)
        .is_some_and(|(_, p, l)| p == provider && l == label)
    {
        bail!(
            "No matching removal review. First run /account {provider} remove {label}. Nothing was removed."
        )
    }
    match provider {
        "claude" => crate::auth::claude::remove_account(label)?,
        "openai" => crate::auth::codex::remove_account(label)?,
        _ => unreachable!(),
    }
    reviews.remove(session);
    crate::auth::AuthStatus::invalidate_cache();
    Ok(format!(
        "Removed {provider} account {label} on runtime host."
    ))
}

async fn reset(agent: &Agent, args: &[String]) -> Result<String> {
    let (provider, action) = match args {
        [] => (
            if agent
                .provider_name()
                .to_ascii_lowercase()
                .contains("openai")
            {
                "openai"
            } else {
                "claude"
            },
            "prepare",
        ),
        [usage, limits, provider] if usage == "usage" && limits == "limits" => {
            (provider.as_str(), "prepare")
        }
        [usage, limits, provider, action] if usage == "usage" && limits == "limits" => {
            (provider.as_str(), action.as_str())
        }
        _ => bail!("Usage: /reset usage limits <openai|claude> [confirm|cancel]"),
    };
    let provider = match provider {
        "openai" | "codex" | "chatgpt" => "openai",
        "claude" | "anthropic" => "claude",
        _ => bail!("Reset provider must be openai or claude"),
    };
    if !matches!(action, "prepare" | "confirm" | "cancel") {
        bail!("Reset action must be confirm or cancel")
    }
    let state = {
        let mut reviews = REVIEWS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .await;
        reviews.retain(|_, (at, state)| {
            at.elapsed() < Duration::from_secs(1800) || Arc::strong_count(state) > 1
        });
        if !reviews.contains_key(agent.session_id()) && reviews.len() >= 128 {
            bail!("Too many pending reset reviews. Cancel an existing review first.")
        }
        let entry = reviews
            .entry(agent.session_id().to_string())
            .or_insert_with(|| (Instant::now(), Arc::new(Mutex::new(Review::default()))));
        entry.0 = Instant::now();
        entry.1.clone()
    };
    let mut review = state.lock().await;
    // Bare /reset repeats the pending review even after a provider switch.
    let provider = if args.is_empty() {
        review
            .pending
            .as_ref()
            .map(Pending::provider)
            .unwrap_or(provider)
    } else {
        provider
    };
    if let Some(pending) = &review.pending {
        if pending.provider() != provider {
            bail!(
                "A {} reset is pending. Cancel it before reviewing another provider.",
                pending.provider()
            )
        }
    }
    match action {
        "cancel" => {
            review.pending = None;
            Ok("Reset confirmation cleared. This does not undo a submitted reset.".into())
        }
        "confirm" => {
            let Some(pending) = &review.pending else {
                bail!(
                    "No reset is pending. Run /reset usage limits {provider} and review it first."
                )
            };
            let result = match pending {
                Pending::OpenAi(p) => crate::usage::consume_openai_usage_reset(p)
                    .await
                    .map(|o| o.message()),
                Pending::Claude(p) => crate::usage::consume_anthropic_limit_reset(p)
                    .await
                    .map(|o| o.message()),
            };
            match result {
                Ok(message) => {
                    review.pending = None;
                    Ok(message)
                }
                Err(_) => bail!(
                    "Reset outcome is uncertain. The same account-pinned request remains pending. Check /usage, then retry /reset usage limits {provider} confirm or cancel explicitly."
                ),
            }
        }
        _ => {
            if let Some(pending) = &review.pending {
                return Ok(pending.message());
            }
            let handle = agent.provider_handle();
            let session_provider = handle.name().to_ascii_lowercase();
            let session_matches = if provider == "openai" {
                session_provider == "openai"
            } else {
                matches!(session_provider.as_str(), "claude" | "anthropic")
            };
            let account = if session_matches {
                handle
                    .serving_oauth_account_label()
                    .or_else(|| handle.oauth_account_label())
            } else {
                None
            };
            review.pending = if provider == "openai" {
                crate::usage::prepare_openai_usage_reset_for_account(
                    account.or_else(crate::auth::codex::active_account_label),
                )
                .await?
                .map(|p| Pending::OpenAi(Box::new(p)))
            } else {
                match crate::usage::prepare_anthropic_limit_reset(
                    account.or_else(crate::auth::claude::active_account_label),
                )
                .await?
                {
                    Ok(p) => Some(Pending::Claude(p)),
                    Err(unavailable) => return Ok(unavailable.message()),
                }
            };
            Ok(review
                .pending
                .as_ref()
                .map(Pending::message)
                .unwrap_or_else(|| {
                    "No banked OpenAI usage resets are available. Nothing changed.".into()
                }))
        }
    }
}

fn clear(value: &str) -> Option<&str> {
    (!matches!(value, "clear" | "unset" | "auto")).then_some(value)
}

fn doctor(provider: Option<&str>) -> Result<String> {
    let status = crate::auth::AuthStatus::check_fast();
    let providers = match provider {
        Some(id) => vec![
            crate::provider_catalog::resolve_login_provider(id)
                .ok_or_else(|| anyhow::anyhow!("Unknown provider"))?,
        ],
        None => crate::provider_catalog::auth_status_login_providers().to_vec(),
    };
    let mut lines = vec!["Authentication on runtime host (read-only)".to_string()];
    for provider in providers {
        let assessment = status.assessment_for_provider(provider);
        // Only typed state and locally generated recommendations. Never return credential payloads.
        lines.push(format!(
            "{}: {:?} ({})",
            provider.display_name,
            assessment.state,
            assessment.health_summary()
        ));
        lines.extend(crate::auth::doctor::recommended_actions(
            provider,
            &assessment,
            None,
        ));
    }
    Ok(lines.join("\n"))
}

fn settings(provider: &str) -> Result<String> {
    if crate::provider_catalog::resolve_login_provider(provider).is_none() {
        bail!("Unknown provider")
    }
    let cfg = crate::config::Config::load();
    let mut lines = vec![
        format!("{provider} settings on runtime host"),
        format!(
            "Default provider: {}",
            cfg.provider.default_provider.as_deref().unwrap_or("auto")
        ),
        format!(
            "Default model: {}",
            cfg.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)")
        ),
    ];
    match provider {
        "openai" => lines.extend([
            format!(
                "Transport: {}",
                cfg.provider.openai_transport.as_deref().unwrap_or("auto")
            ),
            format!(
                "Reasoning effort: {}",
                cfg.provider
                    .openai_reasoning_effort
                    .as_deref()
                    .unwrap_or("(provider default)")
            ),
            format!(
                "Fast: {}",
                cfg.provider.openai_service_tier.as_deref().unwrap_or("off")
            ),
        ]),
        "copilot" => lines.push(format!(
            "Premium: {}",
            cfg.provider.copilot_premium.as_deref().unwrap_or("normal")
        )),
        "openai-compatible" => {
            let compat = crate::provider_catalog::resolve_openai_compatible_profile(
                crate::provider_catalog::OPENAI_COMPAT_PROFILE,
            );
            lines.push(format!("API key variable: {}", compat.api_key_env));
            lines.push(format!("Env filename: {}", compat.env_file));
            lines.push(format!(
                "Default model hint: {}",
                compat.default_model.as_deref().unwrap_or("(unset)")
            ));
            lines.push("API base is configured on the runtime host. Use /account openai-compatible api-base <url|clear> to change it.".into());
        }
        _ => {}
    }
    Ok(lines.join("\n"))
}

fn save_compat(setting: &str, raw: &str) -> Result<String> {
    let value = clear(raw);
    let old = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    );
    let key =
        crate::provider_catalog::load_api_key_from_env_or_config(&old.api_key_env, &old.env_file);
    let (name, value) = match setting {
        "api-base" => (
            "JCODE_OPENAI_COMPAT_API_BASE",
            match value {
                Some(value) => Some(
                    crate::provider_catalog::normalize_api_base(value).ok_or_else(|| {
                        anyhow::anyhow!("API base must use HTTPS or localhost HTTP")
                    })?,
                ),
                None => None,
            },
        ),
        "api-key-name" => {
            if value.is_some_and(|v| !crate::provider_catalog::is_safe_env_key_name(v)) {
                bail!("Invalid API key variable name")
            }
            (
                "JCODE_OPENAI_COMPAT_API_KEY_NAME",
                value.map(str::to_string),
            )
        }
        "env-file" => {
            if value.is_some_and(|v| !crate::provider_catalog::is_safe_env_file_name(v)) {
                bail!("Env file must be a simple filename")
            }
            ("JCODE_OPENAI_COMPAT_ENV_FILE", value.map(str::to_string))
        }
        "default-model" => (
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
            value.map(str::to_string),
        ),
        _ => bail!("Unsupported compatible-provider setting"),
    };
    crate::provider_catalog::save_env_value_to_env_file(
        name,
        crate::provider_catalog::OPENAI_COMPAT_PROFILE.env_file,
        value.as_deref(),
    )?;
    let new = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    );
    if let Some(key) = key {
        if old.api_key_env != new.api_key_env || old.env_file != new.env_file {
            crate::provider_catalog::save_named_api_key(&new.env_file, &new.api_key_env, &key)?;
        }
    }
    crate::auth::AuthStatus::invalidate_cache();
    Ok("Saved OpenAI-compatible preference on runtime host. Credentials were not returned.".into())
}

pub(super) async fn execute(
    agent: &mut Agent,
    command: &str,
    args: &[String],
) -> Result<(Value, String)> {
    if !handles(command) {
        bail!("Unknown account runtime command")
    }
    let Some((operation, args)) = args.split_first() else {
        bail!("Missing account operation")
    };
    let message = match (operation.as_str(), args) {
        ("reset", args) => reset(agent, args).await?,
        ("usage", []) => {
            let usage = crate::usage::fetch_all_provider_usage().await;
            let mut lines = vec!["Usage on runtime host".to_string()];
            for provider in usage {
                lines.push(provider.provider_name);
                for limit in provider.limits {
                    lines.push(format!(
                        "  {}: {:.0}%{}",
                        limit.name,
                        limit.usage_percent,
                        limit
                            .resets_at
                            .map(|at| format!(" (resets {at})"))
                            .unwrap_or_default()
                    ));
                }
                if provider.error.is_some() {
                    lines.push("  Usage unavailable. Run /account doctor on this host.".into());
                }
                if let Some(credits) = provider.openai_reset_credits {
                    lines.push(format!(
                        "  {} banked resets. Review /reset usage limits openai",
                        credits.available_count
                    ));
                }
                if provider
                    .anthropic_limit_reset
                    .is_some_and(|offer| offer.available)
                {
                    lines.push(
                        "  Session-limit reset available. Review /reset usage limits claude".into(),
                    );
                }
            }
            lines.join("\n")
        }
        ("doctor", []) => doctor(None)?,
        ("doctor", [provider]) => doctor(Some(provider))?,
        ("settings", [provider]) => settings(provider)?,
        ("remove", [provider, label]) => {
            remove_account(agent.session_id(), provider, label, false).await?
        }
        ("remove", [provider, label, confirm]) if confirm == "confirm" => {
            remove_account(agent.session_id(), provider, label, true).await?
        }
        ("default-provider", [value]) => {
            let value = clear(value);
            if value.is_some_and(|v| {
                !matches!(v, "claude" | "openai" | "copilot" | "gemini" | "openrouter")
                    && jcode_provider_core::AuthRoute::parse(v).is_none()
            }) {
                bail!("Unsupported default provider")
            }
            crate::config::Config::set_default_provider(value)?;
            "Saved default provider on runtime host for future sessions.".into()
        }
        ("default-model", [value]) => {
            crate::config::Config::set_default_model_only(clear(value))?;
            "Saved default model on runtime host for future sessions.".into()
        }
        ("openai", [setting, value]) => {
            let provider = agent.provider_handle();
            match setting.as_str() {
                "transport" => {
                    let value = clear(value).unwrap_or("auto");
                    if !matches!(value, "auto" | "https" | "websocket") {
                        bail!("Transport must be auto, https, or websocket")
                    }
                    crate::config::Config::set_openai_transport(Some(value))?;
                    if agent
                        .provider_name()
                        .to_ascii_lowercase()
                        .contains("openai")
                    {
                        provider.set_transport(value)?;
                    }
                }
                "effort" => {
                    let value = if value == "none" {
                        Some(value.as_str())
                    } else {
                        clear(value)
                    };
                    if value.is_some_and(|v| {
                        !matches!(
                            v,
                            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                        )
                    }) {
                        bail!("Invalid reasoning effort")
                    }
                    crate::config::Config::set_openai_reasoning_effort(value)?;
                    if let Some(value) = value {
                        if agent
                            .provider_name()
                            .to_ascii_lowercase()
                            .contains("openai")
                        {
                            provider.set_reasoning_effort(value)?;
                        }
                    }
                }
                "fast" => {
                    let tier = match value.as_str() {
                        "on" => Some("priority"),
                        "off" => Some("off"),
                        _ => bail!("Fast must be on or off"),
                    };
                    crate::config::Config::set_openai_service_tier(tier)?;
                    if agent
                        .provider_name()
                        .to_ascii_lowercase()
                        .contains("openai")
                    {
                        provider.set_service_tier(tier.unwrap_or("off"))?;
                    }
                }
                _ => bail!("Unsupported OpenAI account setting"),
            }
            "Saved OpenAI preference on runtime host.".into()
        }
        ("compat", [setting, value]) => save_compat(setting, value)?,
        ("copilot", [setting, value]) if setting == "premium" => {
            let value = match value.as_str() {
                "normal" | "clear" => None,
                "one" | "zero" => Some(value.as_str()),
                _ => bail!("Premium must be normal, one, or zero"),
            };
            crate::config::Config::set_copilot_premium(value)?;
            "Saved Copilot premium preference on runtime host.".into()
        }
        _ => bail!("Unsupported account operation or arguments. Nothing changed."),
    };
    Ok((
        json!({"scope": "runtime host", "operation": operation}),
        message,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoInference;
    #[async_trait::async_trait]
    impl crate::provider::Provider for NoInference {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            panic!("Account commands must not invoke inference")
        }
        fn name(&self) -> &str {
            "test"
        }
        fn fork(&self) -> Arc<dyn crate::provider::Provider> {
            Arc::new(Self)
        }
    }
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn host_account_defaults_and_reset_confirmation_do_not_invoke_inference() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::TempDir::new().unwrap();
        let old = std::env::var_os("JCODE_HOME");
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(old) => crate::env::set_var("JCODE_HOME", old),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore = Restore(old);
        crate::env::set_var("JCODE_HOME", home.path());
        let provider: Arc<dyn crate::provider::Provider> = Arc::new(NoInference);
        let registry = crate::tool::Registry::new(provider.clone()).await;
        let mut agent =
            Agent::new_with_initial_working_dir(provider, registry, home.path().to_str());
        let (data, message) = execute(
            &mut agent,
            "ssh_account",
            &["default-model".into(), "future-model-test".into()],
        )
        .await
        .unwrap();
        assert_eq!(data["scope"], "runtime host");
        assert!(message.contains("future sessions"));
        assert_eq!(
            crate::config::Config::load()
                .provider
                .default_model
                .as_deref(),
            Some("future-model-test")
        );
        let (_, settings) = execute(
            &mut agent,
            "ssh_account",
            &["settings".into(), "openai".into()],
        )
        .await
        .unwrap();
        assert!(settings.contains("future-model-test"));
        for args in [
            vec!["reset", "usage", "limits", "openai", "confirm"],
            vec!["reset", "usage", "limits", "claude", "confirm"],
            vec!["remove", "openai", "missing", "confirm"],
            vec!["openai", "transport", "invalid"],
        ] {
            let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(execute(&mut agent, "ssh_account", &args).await.is_err());
        }
        let (_, message) = execute(
            &mut agent,
            "ssh_account",
            &[
                "reset".into(),
                "usage".into(),
                "limits".into(),
                "openai".into(),
                "cancel".into(),
            ],
        )
        .await
        .unwrap();
        assert!(message.contains("confirmation cleared"));
        REVIEWS
            .get()
            .unwrap()
            .lock()
            .await
            .remove(agent.session_id());
    }

    #[test]
    fn string_handler_is_narrow() {
        assert!(handles("ssh_account"));
        for command in ["account", "auth", "reset", "ssh_account_extra"] {
            assert!(!handles(command));
        }
    }
    #[tokio::test]
    async fn removal_requires_matching_session_provider_and_label_review() {
        let session = "ssh-account-removal-review-test";
        assert!(
            remove_account(session, "claude", "reviewed", true)
                .await
                .is_err()
        );
        let message = remove_account(session, "claude", "reviewed", false)
            .await
            .unwrap();
        assert!(message.contains("Nothing was removed"));
        assert!(
            remove_account(session, "claude", "different", true)
                .await
                .is_err()
        );
        assert!(
            remove_account(session, "openai", "reviewed", true)
                .await
                .is_err()
        );
        assert!(
            remove_account("different-session", "claude", "reviewed", true)
                .await
                .is_err()
        );
        REMOVALS.get().unwrap().lock().await.remove(session);
    }
    #[test]
    fn clearing_settings_preserves_explicit_none_effort() {
        assert_eq!(clear("none"), Some("none"));
        assert_eq!(clear("clear"), None);
        assert_eq!(clear("auto"), None);
    }
}
