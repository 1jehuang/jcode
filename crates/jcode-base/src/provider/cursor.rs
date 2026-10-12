//! Cursor pure model-catalog data (compatibility shim).
//!
//! The Cursor provider *runtime* (`CursorCliProvider`) now lives in the
//! downstream `jcode-provider-cursor-runtime` crate so provider edits do not
//! rebuild the base -> app-core -> tui spine. The binary's composition root
//! registers it via [`crate::provider::external`]. Base keeps only the pure
//! model-catalog data that its routing logic (`provider::models`) needs.

// Cursor deprecated `composer-1.5` ("no longer available; use Composer 2.5").
// Default to a model Cursor currently serves; the live catalog overrides this
// whenever it is reachable.
pub const DEFAULT_MODEL: &str = "composer-2.5";

pub const AVAILABLE_MODELS: &[&str] = &[
    "composer-2.5",
    // Cursor's /v0/models catalog lags its backend: these serve fine but are
    // not advertised. Verified live against api2.cursor.sh on 2026-09-22.
    // Only the bare slugs resolve; `-high`/`-fast` suffixed forms 404.
    "grok-4.7",
    "grok-4.6",
    "composer-2-fast",
    "composer-2",
    "gpt-5.4-high",
    "gpt-5.4-medium",
    "gpt-5.4-low",
    "gpt-5",
    "sonnet-4.6",
    "sonnet-4.6-thinking",
    "opus-4.6",
    "gemini-3.1-pro",
];

pub fn is_known_model(model: &str) -> bool {
    let trimmed = model.trim();
    AVAILABLE_MODELS.contains(&trimmed)
}
