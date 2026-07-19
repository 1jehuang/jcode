//! Centralized session "facts" formatting and a small ledger that tracks which
//! facts are already visible on screen this frame.
//!
//! Several surfaces (info widgets, the overscroll status line, the idle input
//! hint, and the status line) all want to show the same handful of facts: the
//! model, reasoning effort, context usage, the working directory, the provider,
//! and so on. Historically each surface formatted these independently, which
//! led to duplication (the model shown in three places) and inconsistency (raw
//! vs pretty model ids).
//!
//! This module is the single source of truth for:
//! 1. How each fact is formatted (`pretty_model`, `dir_label`, ...).
//! 2. Which facts are currently visible (`FactLedger`), so idle fallback
//!    surfaces can show only what is *not* already on screen.

/// A distinct piece of session information that can be surfaced in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Fact {
    Model,
    ReasoningEffort,
    Context,
    Provider,
    Auth,
    Dir,
    Session,
}

/// Tracks which facts have already been claimed (rendered) this frame so that
/// lower-priority fallback surfaces can fill in only the gaps.
#[derive(Clone, Debug, Default)]
pub(crate) struct FactLedger {
    shown: u32,
}

impl FactLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn bit(fact: Fact) -> u32 {
        1u32 << (fact as u32)
    }

    /// Record that `fact` is being rendered by some surface this frame.
    pub(crate) fn claim(&mut self, fact: Fact) {
        self.shown |= Self::bit(fact);
    }

    /// Whether `fact` is already visible somewhere this frame.
    pub(crate) fn is_shown(&self, fact: Fact) -> bool {
        self.shown & Self::bit(fact) != 0
    }

    /// Whether `fact` still needs a home (not yet shown anywhere).
    pub(crate) fn is_missing(&self, fact: Fact) -> bool {
        !self.is_shown(fact)
    }
}

/// Render `claude-opus-4-8` as `Opus 4.8`, `gpt-5.5` as `GPT-5.5`, etc. Single
/// source of truth for the human-friendly model name across every compact UI
/// surface. The redundant `Claude ` family prefix is dropped: the provider
/// fact already says Claude/Anthropic, so the model reads as `Fable 5` or
/// `Sonnet 4.5` rather than `Claude Fable 5`.
pub(crate) fn pretty_model(model: &str) -> String {
    let pretty = crate::tui::app::helpers::pretty_model_display_name(model);
    match pretty.strip_prefix("Claude ") {
        Some(rest) if !rest.trim().is_empty() => rest.to_string(),
        _ => pretty,
    }
}

/// Home-relative directory label, e.g. `/home/me/jcode` -> `~/jcode`. Does not
/// shorten intermediate path segments.
pub(crate) fn dir_label(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty() && (trimmed == home || trimmed.starts_with(&format!("{home}/"))) {
            let rest = &trimmed[home.len()..];
            return if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~{rest}")
            };
        }
    }
    trimmed.to_string()
}

/// Compact home-relative directory label that elides intermediate segments,
/// e.g. `/home/me/a/b/c` -> `…/b/c` and `~/a/b/c` -> `~/…/c`. Used where space
/// is tight (status line, overscroll, idle input hint).
pub(crate) fn dir_label_short(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let display = dir_label(trimmed);
    let segs: Vec<&str> = display.split('/').filter(|s| !s.is_empty()).collect();
    let short = if display.starts_with('~') {
        if segs.len() <= 2 {
            display.clone()
        } else {
            format!("~/…/{}", segs[segs.len() - 1])
        }
    } else if segs.len() <= 2 {
        format!("/{}", segs.join("/"))
    } else {
        format!("…/{}/{}", segs[segs.len() - 2], segs[segs.len() - 1])
    };
    Some(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_tracks_claimed_facts() {
        let mut ledger = FactLedger::new();
        assert!(ledger.is_missing(Fact::Model));
        ledger.claim(Fact::Model);
        assert!(ledger.is_shown(Fact::Model));
        assert!(ledger.is_missing(Fact::Dir));
        ledger.claim(Fact::Dir);
        ledger.claim(Fact::Context);
        assert!(ledger.is_shown(Fact::Dir));
        assert!(ledger.is_shown(Fact::Context));
        assert!(ledger.is_missing(Fact::Provider));
    }

    #[test]
    fn pretty_model_drops_redundant_claude_prefix() {
        assert_eq!(pretty_model("claude-opus-4-8"), "Opus 4.8");
        assert_eq!(pretty_model("claude-sonnet-4-5"), "Sonnet 4.5");
        assert_eq!(pretty_model("claude-fable-5"), "Fable 5");
        // Non-Claude ids are untouched.
        assert_eq!(pretty_model("gpt-5.5"), "GPT-5.5");
        assert_eq!(pretty_model("gemini-2.5-pro"), "Gemini 2.5 Pro");
    }

    #[test]
    fn dir_label_is_home_relative() {
        // Avoid depending on the real HOME by checking the non-home branch and
        // the trailing-slash normalization.
        assert_eq!(dir_label("/var/log/"), "/var/log");
        assert_eq!(dir_label("/"), "/");
        assert_eq!(dir_label("   "), "/");
    }

    #[test]
    fn dir_label_short_elides_middle_segments() {
        assert_eq!(dir_label_short("/a/b"), Some("/a/b".to_string()));
        assert_eq!(dir_label_short("/a/b/c/d"), Some("…/c/d".to_string()));
        assert_eq!(dir_label_short(""), None);
    }
}
