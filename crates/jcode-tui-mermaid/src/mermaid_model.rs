use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// Stable identity for a Mermaid diagram occurrence.
///
/// `source_hash` identifies equivalent content, while `origin` + `ordinal`
/// distinguish separate occurrences for UI selection/registry ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagramId {
    pub source_hash: u64,
    pub origin: DiagramOrigin,
    pub ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiagramOrigin {
    Chat,
    SidePanel { page_id: String },
    StreamingPreview,
    DebugProbe,
}

#[derive(Debug, Clone)]
pub struct DiagramBlock {
    pub id: DiagramId,
    pub source: Arc<str>,
}

/// Explicit render profile for the next Mermaid pipeline.
///
/// This is intentionally independent from the legacy thread-local profile in
/// `lib.rs`. New code should pass this through request objects instead of using
/// ambient context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DiagramRenderProfile {
    pub width_cells: Option<u16>,
    pub preferred_aspect_per_mille: Option<u16>,
    pub theme: MermaidTheme,
}

impl DiagramRenderProfile {
    pub fn new(width_cells: Option<u16>, preferred_aspect_ratio: Option<f32>) -> Self {
        Self {
            width_cells,
            preferred_aspect_per_mille: normalize_aspect_ratio(preferred_aspect_ratio),
            theme: MermaidTheme::TerminalDark,
        }
    }

    pub fn cache_key(self, source_hash: u64) -> DiagramCacheKey {
        DiagramCacheKey {
            source_hash,
            width_cells: self.width_cells,
            preferred_aspect_per_mille: self.preferred_aspect_per_mille,
            theme: self.theme,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MermaidTheme {
    #[default]
    TerminalDark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagramCacheKey {
    pub source_hash: u64,
    pub width_cells: Option<u16>,
    pub preferred_aspect_per_mille: Option<u16>,
    pub theme: MermaidTheme,
}

impl fmt::Display for DiagramCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.source_hash)?;
        if let Some(width_cells) = self.width_cells {
            write!(f, ":w{width_cells}")?;
        }
        if let Some(aspect) = self.preferred_aspect_per_mille {
            write!(f, ":a{aspect}")?;
        }
        write!(f, ":{:?}", self.theme)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderTarget {
    InlineMarkdown,
    SidePanel,
    PinnedPane,
    DebugProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderPriority {
    Interactive,
    Visible,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderMode {
    CacheOnly,
    EnqueueIfMissing,
    Blocking,
}

#[derive(Debug, Clone)]
pub struct DiagramRenderRequest {
    pub diagram_id: DiagramId,
    pub source: Arc<str>,
    pub target: RenderTarget,
    pub profile: DiagramRenderProfile,
    pub priority: RenderPriority,
    pub mode: RenderMode,
}

impl DiagramRenderRequest {
    pub fn cache_key(&self) -> DiagramCacheKey {
        self.profile.cache_key(self.diagram_id.source_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderArtifact {
    pub cache_key: DiagramCacheKey,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderStatus {
    Ready(RenderArtifact),
    Pending { cache_key: DiagramCacheKey },
    Failed(RenderError),
    ProtocolUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError {
    pub cache_key: Option<DiagramCacheKey>,
    pub message: String,
}

pub fn normalize_aspect_ratio(ratio: Option<f32>) -> Option<u16> {
    ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .map(|ratio| (ratio * 1000.0).round().clamp(100.0, 10_000.0) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_ratio_normalization_matches_legacy_bucket_policy() {
        assert_eq!(normalize_aspect_ratio(None), None);
        assert_eq!(normalize_aspect_ratio(Some(0.75)), Some(750));
        assert_eq!(normalize_aspect_ratio(Some(1.2345)), Some(1235));
        assert_eq!(normalize_aspect_ratio(Some(0.001)), Some(100));
        assert_eq!(normalize_aspect_ratio(Some(999.0)), Some(10_000));
        assert_eq!(normalize_aspect_ratio(Some(f32::NAN)), None);
        assert_eq!(normalize_aspect_ratio(Some(-1.0)), None);
    }

    #[test]
    fn cache_key_is_explicit_and_stable() {
        let profile = DiagramRenderProfile::new(Some(96), Some(0.5));
        let key = profile.cache_key(0xabc);
        assert_eq!(key.source_hash, 0xabc);
        assert_eq!(key.width_cells, Some(96));
        assert_eq!(key.preferred_aspect_per_mille, Some(500));
        assert_eq!(key.to_string(), "0000000000000abc:w96:a500:TerminalDark");
    }

    #[test]
    fn render_request_derives_cache_key_from_source_hash_and_profile_only() {
        let source: Arc<str> = "flowchart TD\nA-->B".into();
        let request = DiagramRenderRequest {
            diagram_id: DiagramId {
                source_hash: 42,
                origin: DiagramOrigin::Chat,
                ordinal: 7,
            },
            source,
            target: RenderTarget::PinnedPane,
            profile: DiagramRenderProfile::new(Some(120), Some(2.0)),
            priority: RenderPriority::Visible,
            mode: RenderMode::EnqueueIfMissing,
        };

        assert_eq!(request.cache_key(), request.profile.cache_key(42));
        assert_eq!(
            request.cache_key().to_string(),
            "000000000000002a:w120:a2000:TerminalDark"
        );
    }
}
