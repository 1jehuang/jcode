//! Claude Code subscription usage per configured instance.
//!
//! The runtime crate registers an async probe (init-only `claude` child plus
//! `get_usage`, no prompt) at startup. `/usage` calls it once per instance and
//! shows one report per Claude Code login, deduplicated by account email, the
//! same way t3code pools Claude instances.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use super::{ProviderUsage, UsageLimit, attach_activity};
use crate::config::ClaudeCodeInstanceConfig;

/// One usage window reported by the probe.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCodeUsageWindow {
    pub name: String,
    pub percent: f32,
    pub resets_at: Option<String>,
}

/// What the probe found for one instance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeCodeUsageProbe {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub windows: Vec<ClaudeCodeUsageWindow>,
    /// The CLI could not report limits (API key or proxy logins).
    pub limits_unavailable: bool,
}

type ProbeFuture = Pin<Box<dyn Future<Output = anyhow::Result<ClaudeCodeUsageProbe>> + Send>>;
type ProbeFn = Arc<dyn Fn(ClaudeCodeInstanceConfig) -> ProbeFuture + Send + Sync>;

fn probe_slot() -> &'static RwLock<Option<ProbeFn>> {
    static SLOT: OnceLock<RwLock<Option<ProbeFn>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Register the Claude Code usage probe (called by the binary at startup).
pub fn register_claude_code_usage_probe<F, Fut>(probe: F)
where
    F: Fn(ClaudeCodeInstanceConfig) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<ClaudeCodeUsageProbe>> + Send + 'static,
{
    let probe: ProbeFn = Arc::new(move |instance| Box::pin(probe(instance)));
    *probe_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(probe);
}

fn registered_probe() -> Option<ProbeFn> {
    probe_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// `/usage` title for an instance.
pub(super) fn report_name(instance: &ClaudeCodeInstanceConfig, is_default: bool) -> String {
    match instance.display_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => format!("Claude Code ({name})"),
        _ if is_default => "Claude Code".to_string(),
        _ => format!("Claude Code ({})", instance.id),
    }
}

/// Build the `/usage` report for one instance from its probe result.
pub(super) fn report_from_probe(
    instance: &ClaudeCodeInstanceConfig,
    is_default: bool,
    probe: anyhow::Result<ClaudeCodeUsageProbe>,
    login_hint: &str,
) -> ProviderUsage {
    let mut report = ProviderUsage {
        provider_name: report_name(instance, is_default),
        ..Default::default()
    };
    match probe {
        Ok(probe) => {
            if let Some(email) = probe.email.as_deref() {
                report
                    .extra_info
                    .push(("Account".to_string(), email.to_string()));
            }
            if let Some(plan) = probe.plan.as_deref() {
                report
                    .extra_info
                    .push(("Plan".to_string(), plan.to_string()));
            }
            report.extra_info.push((
                "Login".to_string(),
                match instance.resolved_home() {
                    Some(home) => format!("Claude Code CLI, CLAUDE_CONFIG_DIR={}", home.display()),
                    None => "Claude Code CLI, default login".to_string(),
                },
            ));
            if probe.email.is_none() && probe.windows.is_empty() {
                report.error = Some(format!("Not signed in. Run `{login_hint}`."));
            } else if probe.limits_unavailable {
                report.extra_info.push((
                    "Limits".to_string(),
                    "Not reported for this login (API key or proxy)".to_string(),
                ));
            }
            report.hard_limit_reached = probe.windows.iter().any(|w| w.percent >= 100.0);
            report.limits = probe
                .windows
                .into_iter()
                .map(|window| UsageLimit {
                    name: window.name,
                    usage_percent: window.percent,
                    resets_at: window.resets_at,
                })
                .collect();
        }
        Err(error) => {
            report.error = Some(format!("Claude Code probe failed: {error:#}"));
        }
    }
    report
}

fn account_email(report: &ProviderUsage) -> Option<String> {
    report
        .extra_info
        .iter()
        .find(|(key, _)| key == "Account")
        .map(|(_, email)| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty())
}

fn is_claude_code_report(report: &ProviderUsage) -> bool {
    report.provider_name == "Claude Code" || report.provider_name.starts_with("Claude Code (")
}

/// Drop Claude Code reports whose account (same email, case-insensitive) is
/// already listed by an earlier Claude Code report. Other reports are kept.
pub(super) fn dedupe_claude_code_by_email(reports: &mut Vec<ProviderUsage>) {
    let mut seen: Vec<String> = Vec::new();
    reports.retain(|report| {
        if !is_claude_code_report(report) {
            return true;
        }
        match account_email(report) {
            Some(email) if seen.contains(&email) => false,
            Some(email) => {
                seen.push(email);
                true
            }
            None => true,
        }
    });
}

/// Enqueue one probe task per configured Claude Code instance. Returns the
/// number of tasks added.
pub(super) fn enqueue_claude_code_usage_tasks(
    tasks: &mut tokio::task::JoinSet<Option<ProviderUsage>>,
) -> usize {
    let Some(probe) = registered_probe() else {
        return 0;
    };
    if !crate::auth::claude_code::binary_available() {
        return 0;
    }
    let settings = crate::auth::claude_code::settings();
    let default_id = settings.resolved_default_instance();
    let instances = settings.effective_instances();
    let count = instances.len();
    for instance in instances {
        let probe = probe.clone();
        let is_default = instance.id == default_id;
        tasks.spawn(async move {
            let hint = crate::auth::claude_code::login_hint(&instance);
            let result = probe(instance.clone()).await;
            let mut report = report_from_probe(&instance, is_default, result, &hint);
            attach_activity(&mut report, &activity_source_key(&instance.id, is_default));
            Some(report)
        });
    }
    count
}

/// Activity ledger key for an instance (`claude-code` for the default).
pub(super) fn activity_source_key(instance_id: &str, is_default: bool) -> String {
    if is_default {
        "claude-code".to_string()
    } else {
        format!("claude-code:{instance_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: &str, home: Option<&str>, name: Option<&str>) -> ClaudeCodeInstanceConfig {
        ClaudeCodeInstanceConfig {
            id: id.to_string(),
            home: home.map(str::to_string),
            display_name: name.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn probe_result_becomes_named_limits() {
        let report = report_from_probe(
            &instance("default", None, None),
            true,
            Ok(ClaudeCodeUsageProbe {
                email: Some("a@b.c".into()),
                plan: Some("Claude Max".into()),
                windows: vec![
                    ClaudeCodeUsageWindow {
                        name: "5-hour window".into(),
                        percent: 100.0,
                        resets_at: Some("2026-10-05T02:00:00Z".into()),
                    },
                    ClaudeCodeUsageWindow {
                        name: "7-day window".into(),
                        percent: 23.0,
                        resets_at: None,
                    },
                ],
                limits_unavailable: false,
            }),
            "claude auth login",
        );
        assert_eq!(report.provider_name, "Claude Code");
        assert_eq!(report.limits.len(), 2);
        assert_eq!(report.limits[0].name, "5-hour window");
        assert!(report.hard_limit_reached);
        assert!(report.error.is_none());
        assert!(
            report
                .extra_info
                .iter()
                .any(|(k, v)| k == "Account" && v == "a@b.c")
        );
    }

    #[test]
    fn signed_out_instance_points_at_its_login() {
        let report = report_from_probe(
            &instance("work", Some("/tmp/cc-work"), Some("Work")),
            false,
            Ok(ClaudeCodeUsageProbe::default()),
            "CLAUDE_CONFIG_DIR=/tmp/cc-work claude auth login",
        );
        assert_eq!(report.provider_name, "Claude Code (Work)");
        assert!(
            report
                .error
                .as_deref()
                .unwrap()
                .contains("CLAUDE_CONFIG_DIR=/tmp/cc-work claude auth login")
        );
        assert!(report.limits.is_empty());
    }

    #[test]
    fn probe_errors_are_reported_not_hidden() {
        let report = report_from_probe(
            &instance("personal", None, None),
            false,
            Err(anyhow::anyhow!("Claude Code probe timed out")),
            "claude auth login",
        );
        assert_eq!(report.provider_name, "Claude Code (personal)");
        assert!(report.error.as_deref().unwrap().contains("timed out"));
    }

    #[test]
    fn instances_sharing_an_account_are_listed_once() {
        let report = |name: &str, email: Option<&str>| ProviderUsage {
            provider_name: name.into(),
            extra_info: email
                .map(|e| vec![("Account".to_string(), e.to_string())])
                .unwrap_or_default(),
            ..Default::default()
        };
        let mut reports = vec![
            report("Claude Code", Some("Me@X.com")),
            report("Anthropic (Claude)", Some("me@x.com")),
            report("Claude Code (alias)", Some("me@x.com")),
            report("Claude Code (work)", Some("work@x.com")),
            report("Claude Code (signed-out)", None),
        ];
        dedupe_claude_code_by_email(&mut reports);
        let names: Vec<_> = reports.iter().map(|r| r.provider_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Claude Code",
                "Anthropic (Claude)",
                "Claude Code (work)",
                "Claude Code (signed-out)"
            ]
        );
    }
}
