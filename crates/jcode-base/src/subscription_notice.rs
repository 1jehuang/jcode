//! Plan-limit notices from the Jcode subscription gateway.
//!
//! When an included feature (memory recall, browser handoff) exhausts its
//! daily plan allowance, the gateway says which plan would help. This module
//! turns that into one clear, user-facing upgrade prompt and remembers the
//! most recent one so every UI (TUI, Desktop, tool results) can surface it
//! instead of failing silently.

use std::fmt;
use std::sync::Mutex;

/// A daily plan allowance was exhausted. Fields are already sanitized by the
/// caller: bounded length, and `upgrade_url` is restricted to jcode.sh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaExceeded {
    /// Which included feature hit its limit, e.g. "memory" or "browser".
    pub feature: String,
    pub tier: Option<String>,
    pub upgrade_tier: Option<String>,
    pub upgrade_url: Option<String>,
    pub resets_at: Option<String>,
}

fn plan_name(tier: &str) -> String {
    match tier.trim().to_ascii_lowercase().as_str() {
        "plus" => "Plus".into(),
        "pro" => "Pro".into(),
        "max" => "Max".into(),
        "ultra" => "Ultra".into(),
        "flagship" | "solo" => "Solo".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn feature_name(feature: &str) -> &str {
    match feature {
        "memory" => "memory recall",
        "browser" => "browser automation",
        other => other,
    }
}

impl QuotaExceeded {
    /// Short headline suitable for a banner or status line.
    pub fn headline(&self) -> String {
        format!(
            "Daily {} limit reached{}",
            feature_name(&self.feature),
            self.tier
                .as_deref()
                .map(|tier| format!(" on your {} plan", plan_name(tier)))
                .unwrap_or_default()
        )
    }

    /// Call to action, including the upgrade link when a higher plan exists.
    pub fn call_to_action(&self) -> String {
        match (&self.upgrade_tier, &self.upgrade_url) {
            (Some(tier), Some(url)) => format!(
                "Upgrade to {} for a higher daily limit: {url}. Otherwise it resets within 24 hours.",
                plan_name(tier)
            ),
            _ => "It resets within 24 hours.".into(),
        }
    }
}

impl fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}. {}", self.headline(), self.call_to_action())
    }
}

impl std::error::Error for QuotaExceeded {}

static LATEST: Mutex<Option<QuotaExceeded>> = Mutex::new(None);

/// Remember the most recent plan-limit hit so UIs can show an upgrade prompt.
pub fn record(notice: QuotaExceeded) {
    crate::logging::info(&format!("Subscription plan limit: {notice}"));
    if let Ok(mut latest) = LATEST.lock() {
        *latest = Some(notice);
    }
}

/// Take the pending notice, if any. Each hit is shown once per take so a busy
/// background feature does not spam the user.
pub fn take() -> Option<QuotaExceeded> {
    LATEST.lock().ok().and_then(|mut latest| latest.take())
}

/// Find a plan-limit notice anywhere in an error chain.
pub fn from_error(error: &anyhow::Error) -> Option<&QuotaExceeded> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<QuotaExceeded>())
}

/// Stable per-session id so a later hit updates the same card instead of
/// stacking duplicates in the transcript.
pub const UPGRADE_CARD_ID: &str = "jcode-plan-limit";

/// An inline chat card that shows the limit and, when a higher plan exists, a
/// Subscribe/Upgrade button that opens jcode.sh pricing in the browser.
/// Nothing is purchased in-app.
pub fn upgrade_card(notice: &QuotaExceeded, session_id: &str) -> jcode_applet_types::Instance {
    use jcode_applet_types::{Anchor, Lifetime, Placement, Scope};
    let mut children = vec![
        serde_json::json!({"type": "text", "text": notice.headline(), "style": "heading"}),
        serde_json::json!({"type": "text", "tone": "dim", "text": match (&notice.upgrade_tier, &notice.upgrade_url) {
            (Some(tier), Some(_)) => format!("Upgrade to {} for a higher daily limit, or wait for it to reset within 24 hours.", plan_name(tier)),
            _ => "It resets within 24 hours.".to_string(),
        }}),
    ];
    let mut buttons = Vec::new();
    if let (Some(tier), Some(url)) = (&notice.upgrade_tier, &notice.upgrade_url) {
        buttons.push(serde_json::json!({
            "type": "button", "variant": "primary",
            "label": format!("Upgrade to {}", plan_name(tier)),
            "on_press": {"action": "host.open_url", "args": {"url": url}}
        }));
    }
    buttons.push(serde_json::json!({
        "type": "button", "variant": "secondary", "label": "Dismiss",
        "on_press": {"action": "host.close"}
    }));
    children.push(serde_json::json!({"type": "stack", "direction": "horizontal", "gap": "sm", "children": buttons}));
    let document = serde_json::from_value(serde_json::json!({
        "revision": 1,
        "title": "Jcode plan limit",
        "view": {"type": "card", "title": "Jcode subscription", "children": [
            {"type": "stack", "gap": "sm", "children": children}
        ]}
    }))
    .expect("static upgrade card document");
    jcode_applet_types::Instance {
        id: UPGRADE_CARD_ID.to_string(),
        applet: jcode_applet_types::agent::APPLET_ID.to_string(),
        placement: Placement::Inline {
            session_id: session_id.to_string(),
            anchor: Anchor::End,
        },
        scope: Scope::Session {
            session_id: session_id.to_string(),
        },
        lifetime: Lifetime::Session,
        document,
    }
}

/// Mount the upgrade card in a session's transcript and notify connected UIs.
/// Best effort: a failure only loses the card, never the turn.
pub fn show_upgrade_card(notice: &QuotaExceeded, session_id: &str) {
    match crate::applets::mount(session_id, upgrade_card(notice, session_id)) {
        Ok(snapshot) => crate::applets::publish(session_id, snapshot),
        Err(error) => crate::logging::warn(&format!("Could not show plan-limit card: {error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(upgrade: bool) -> QuotaExceeded {
        QuotaExceeded {
            feature: "browser".into(),
            tier: Some("plus".into()),
            upgrade_tier: upgrade.then(|| "pro".into()),
            upgrade_url: upgrade.then(|| "https://jcode.sh/pricing".into()),
            resets_at: Some("2026-09-27T00:00:00.000Z".into()),
        }
    }

    #[test]
    fn upgrade_card_is_a_valid_applet_with_upgrade_button() {
        let card = upgrade_card(&notice(true), "sess1");
        jcode_applet_types::validate_document(
            &card.document,
            &jcode_applet_types::agent::manifest(),
            &jcode_applet_types::Limits::default(),
        )
        .expect("card passes the same validation mount() applies");
        let json = serde_json::to_string(&card.document).unwrap();
        assert!(json.contains("Upgrade to Pro"));
        assert!(json.contains("host.open_url"));
        assert!(json.contains("https://jcode.sh/pricing"));
        assert!(json.contains("Daily browser automation limit reached on your Plus plan"));
        assert_eq!(card.id, UPGRADE_CARD_ID);
        assert!(matches!(
            card.placement,
            jcode_applet_types::Placement::Inline {
                anchor: jcode_applet_types::Anchor::End,
                ..
            }
        ));
    }

    #[test]
    fn top_tier_card_has_no_upgrade_button() {
        let card = upgrade_card(&notice(false), "sess1");
        jcode_applet_types::validate_document(
            &card.document,
            &jcode_applet_types::agent::manifest(),
            &jcode_applet_types::Limits::default(),
        )
        .unwrap();
        let json = serde_json::to_string(&card.document).unwrap();
        assert!(!json.contains("host.open_url"));
        assert!(json.contains("Dismiss"));
    }

    #[test]
    fn upgrade_prompt_names_plan_feature_and_link() {
        let text = notice(true).to_string();
        assert_eq!(
            text,
            "Daily browser automation limit reached on your Plus plan. Upgrade to Pro for a higher daily limit: https://jcode.sh/pricing. Otherwise it resets within 24 hours."
        );
    }

    #[test]
    fn top_tier_gets_reset_message_without_upsell() {
        let text = notice(false).to_string();
        assert!(!text.contains("Upgrade"));
        assert!(text.contains("resets within 24 hours"));
    }

    #[test]
    fn notice_is_recoverable_from_anyhow_chain_and_taken_once() {
        let error = anyhow::Error::from(notice(true)).context("browser handoff failed");
        assert_eq!(from_error(&error), Some(&notice(true)));
        record(notice(true));
        assert_eq!(take(), Some(notice(true)));
        assert_eq!(take(), None);
    }
}
