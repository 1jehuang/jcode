//! Diagram generation preferences and local D2 renderer settings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramEngine {
    #[default]
    Mermaid,
    D2,
}

impl DiagramEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mermaid => "mermaid",
            Self::D2 => "d2",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum D2Layout {
    #[default]
    Dagre,
    Elk,
    Tala,
}

impl D2Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dagre => "dagre",
            Self::Elk => "elk",
            Self::Tala => "tala",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagramConfig {
    /// Syntax to request from the model. Explicit fences retain their language.
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub preferred_engine: DiagramEngine,
    /// Layout for local D2 rendering, independent of the generation preference.
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub d2_layout: D2Layout,
    /// Executable path or name, never a shell command.
    pub d2_binary: String,
    /// Host-enforced render deadline, clamped to 1 through 30 seconds.
    pub d2_timeout_secs: u64,
}

impl Default for DiagramConfig {
    fn default() -> Self {
        Self {
            preferred_engine: DiagramEngine::Mermaid,
            d2_layout: D2Layout::Dagre,
            d2_binary: "d2".to_string(),
            d2_timeout_secs: 10,
        }
    }
}

impl DiagramConfig {
    pub fn bounded_d2_timeout_secs(&self) -> u64 {
        self.d2_timeout_secs.clamp(1, 30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagram_defaults_preserve_mermaid_preference() {
        let config: DiagramConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, DiagramConfig::default());
        assert_eq!(config.preferred_engine, DiagramEngine::Mermaid);
        assert_eq!(config.d2_layout, D2Layout::Dagre);
        assert_eq!(config.d2_binary, "d2");
        assert_eq!(config.bounded_d2_timeout_secs(), 10);
    }

    #[test]
    fn diagram_engine_and_layout_names_round_trip() {
        for engine in [DiagramEngine::Mermaid, DiagramEngine::D2] {
            let encoded = serde_json::to_string(&engine).unwrap();
            assert_eq!(encoded, format!("\"{}\"", engine.as_str()));
            assert_eq!(
                serde_json::from_str::<DiagramEngine>(&encoded).unwrap(),
                engine
            );
        }
        for layout in [D2Layout::Dagre, D2Layout::Elk, D2Layout::Tala] {
            let encoded = serde_json::to_string(&layout).unwrap();
            assert_eq!(encoded, format!("\"{}\"", layout.as_str()));
            assert_eq!(serde_json::from_str::<D2Layout>(&encoded).unwrap(), layout);
        }
    }

    #[test]
    fn unknown_diagram_enums_do_not_discard_other_settings() {
        let config: DiagramConfig = serde_json::from_value(serde_json::json!({
            "preferred_engine": "future-engine", "d2_layout": "future-layout",
            "d2_binary": "/configured/d2", "d2_timeout_secs": 7
        }))
        .unwrap();
        assert_eq!(config.preferred_engine, DiagramEngine::Mermaid);
        assert_eq!(config.d2_layout, D2Layout::Dagre);
        assert_eq!(config.d2_binary, "/configured/d2");
        assert_eq!(config.bounded_d2_timeout_secs(), 7);
    }

    #[test]
    fn diagram_timeout_is_bounded() {
        for (configured, expected) in [(0, 1), (1, 1), (10, 10), (30, 30), (u64::MAX, 30)] {
            let config = DiagramConfig {
                d2_timeout_secs: configured,
                ..Default::default()
            };
            assert_eq!(config.bounded_d2_timeout_secs(), expected);
        }
    }
}
