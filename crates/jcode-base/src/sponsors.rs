//! Integration discovery: shared constants and provenance tracking.
//!
//! Integration discovery makes third-party developer tools discoverable to
//! the agent through the `discover_tools` tool, backed by a hosted manifest.
//! All agent-facing guidance lives in that tool's schema rather than the
//! system prompt.
//! Some providers may share revenue with Jcode when users choose their tools.
//! Partnership status never influences recommendations. The relationship is
//! disclosed in the UI with a link to <https://jcode.sh/discovery-tools>.
//!
//! Design constraints (see the discovery-tools page for the public
//! version of this policy):
//! - Solo Systems vets every listing and enforces one-tool-call setup at the
//!   sponsor-platform admission layer for seamless harness integration.
//! - Discovery is on by default and can be opted out of with
//!   `[sponsors] enabled = false` in config.toml.
//! - The category list below is a shipped constant, so building the tool schema
//!   never requires a network request.
//! - Tools within a category live server-side and are fetched on demand by
//!   `discover_tools`. If the request fails, the tool fails plainly. There is
//!   no cache and no offline fallback.
//! - Requests carry only discovery fields (category, query, tool, and reason),
//!   never session content.

/// Public URL explaining Jcode's tool-provider partnerships.
pub const DISCOVERY_PARTNERS_URL: &str = "https://jcode.sh/discovery-tools";

/// Provenance tagging and coarse usage metering for MCP servers connected
/// as a result of a discovery listing.
pub mod provenance;

/// Categories in which discoverable tools exist. Shipped as a constant so the
/// tool schema never depends on the network. The tools within each category are
/// served by the discovery endpoint.
pub const DISCOVERY_CATEGORIES: &[&str] = &[
    "payments",
    "git",
    "code-review",
    "databases",
    "browser-automation",
    "deployment",
    "observability",
    "authentication",
    "security",
    "storage",
    "analytics",
    "web-search",
    "web-data",
    "financial-data",
    "cloud-infrastructure",
    "compliance-and-privacy",
    "integration-platforms",
    "email-messaging",
    "ai-models",
    "other",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_are_nonempty_and_lowercase() {
        assert!(!DISCOVERY_CATEGORIES.is_empty());
        for cat in DISCOVERY_CATEGORIES {
            assert!(!cat.is_empty());
            assert_eq!(cat.to_ascii_lowercase(), *cat);
            assert!(!cat.contains(' '), "categories are slugs: {cat}");
        }
    }

    #[test]
    fn categories_match_the_public_discovery_taxonomy() {
        assert_eq!(
            DISCOVERY_CATEGORIES,
            &[
                "payments",
                "git",
                "code-review",
                "databases",
                "browser-automation",
                "deployment",
                "observability",
                "authentication",
                "security",
                "storage",
                "analytics",
                "web-search",
                "web-data",
                "financial-data",
                "cloud-infrastructure",
                "compliance-and-privacy",
                "integration-platforms",
                "email-messaging",
                "ai-models",
                "other",
            ]
        );
    }

    #[test]
    fn discovery_is_enabled_by_default() {
        let config = crate::config::Config::default();
        assert!(config.sponsors.enabled);
    }
}
