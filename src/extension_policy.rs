//! Issue #14: extension-load policy.
//!
//! Centralizes the rules for which "extensions" (MCP servers, future
//! plugin tools, side-loaded extensions) are permitted to load.
//!
//! Configured via:
//!   - `--extension-policy <all|trusted|none>` CLI flag, OR
//!   - `JCODE_EXTENSION_POLICY=<value>` env var (CLI flag sets this).
//!
//! Default is [`Policy::All`] (preserves existing behavior).

/// Extension-load policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Load every discovered extension. (Default — preserves prior
    /// behavior before this flag existed.)
    #[default]
    All,
    /// Only load extensions whose source path / spec has been
    /// explicitly trusted via `jcode mcp trust` (or future
    /// `jcode extension trust`).
    Trusted,
    /// Block all extension loading. Built-in tools still work.
    None,
}

impl Policy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" | "any" => Some(Self::All),
            "trusted" | "trust" | "trust-only" => Some(Self::Trusted),
            "none" | "off" | "no" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Trusted => "trusted",
            Self::None => "none",
        }
    }

    /// Convenience: should the loader even bother enumerating sources?
    pub fn allows_any_extension(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Convenience: is this entry permitted assuming it has been
    /// explicitly trusted?
    pub fn allows_trusted_entry(self) -> bool {
        matches!(self, Self::All | Self::Trusted)
    }

    /// Convenience: is this entry permitted assuming it is *not*
    /// trusted?
    pub fn allows_untrusted_entry(self) -> bool {
        matches!(self, Self::All)
    }
}

/// Read the active policy from `JCODE_EXTENSION_POLICY`.
///
/// Returns [`Policy::All`] when the env var is unset or unparseable.
pub fn current() -> Policy {
    std::env::var("JCODE_EXTENSION_POLICY")
        .ok()
        .and_then(|v| Policy::parse(&v))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognized_variants() {
        assert_eq!(Policy::parse("all"), Some(Policy::All));
        assert_eq!(Policy::parse("ALL"), Some(Policy::All));
        assert_eq!(Policy::parse("any"), Some(Policy::All));
        assert_eq!(Policy::parse("trusted"), Some(Policy::Trusted));
        assert_eq!(Policy::parse("trust"), Some(Policy::Trusted));
        assert_eq!(Policy::parse("trust-only"), Some(Policy::Trusted));
        assert_eq!(Policy::parse("none"), Some(Policy::None));
        assert_eq!(Policy::parse("off"), Some(Policy::None));
        assert_eq!(Policy::parse(" None "), Some(Policy::None));
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(Policy::parse("strict"), None);
        assert_eq!(Policy::parse(""), None);
    }

    #[test]
    fn as_str_round_trips() {
        for p in [Policy::All, Policy::Trusted, Policy::None] {
            assert_eq!(Policy::parse(p.as_str()), Some(p));
        }
    }

    #[test]
    fn allows_any_extension_blocks_only_none() {
        assert!(Policy::All.allows_any_extension());
        assert!(Policy::Trusted.allows_any_extension());
        assert!(!Policy::None.allows_any_extension());
    }

    #[test]
    fn allows_trusted_entry_under_all_and_trusted() {
        assert!(Policy::All.allows_trusted_entry());
        assert!(Policy::Trusted.allows_trusted_entry());
        assert!(!Policy::None.allows_trusted_entry());
    }

    #[test]
    fn allows_untrusted_entry_only_under_all() {
        assert!(Policy::All.allows_untrusted_entry());
        assert!(!Policy::Trusted.allows_untrusted_entry());
        assert!(!Policy::None.allows_untrusted_entry());
    }

    #[test]
    fn current_reads_env() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_EXTENSION_POLICY");

        crate::env::remove_var("JCODE_EXTENSION_POLICY");
        assert_eq!(current(), Policy::All);

        crate::env::set_var("JCODE_EXTENSION_POLICY", "trusted");
        assert_eq!(current(), Policy::Trusted);

        crate::env::set_var("JCODE_EXTENSION_POLICY", "none");
        assert_eq!(current(), Policy::None);

        crate::env::set_var("JCODE_EXTENSION_POLICY", "garbage");
        assert_eq!(current(), Policy::All, "fallback to default on bad value");

        if let Some(p) = prev {
            crate::env::set_var("JCODE_EXTENSION_POLICY", p);
        } else {
            crate::env::remove_var("JCODE_EXTENSION_POLICY");
        }
    }
}
