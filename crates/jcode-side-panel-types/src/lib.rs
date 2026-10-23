use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidePanelPageFormat {
    #[default]
    Markdown,
    Pdf,
}

impl SidePanelPageFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidePanelPageSource {
    #[default]
    Managed,
    LinkedFile,
    Ephemeral,
}

impl SidePanelPageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::LinkedFile => "linked_file",
            Self::Ephemeral => "ephemeral",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedSidePanelState {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub archived: Vec<SidePanelHistoryEntry>,
    #[serde(default)]
    pub focus_revision: u64,
    #[serde(default)]
    pub focused_page_id: Option<String>,
    #[serde(default)]
    pub pages: Vec<PersistedSidePanelPage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSidePanelPage {
    #[serde(default)]
    pub status: SidePanelPageStatus,
    #[serde(default)]
    pub cache_path: Option<String>,
    pub id: String,
    pub title: String,
    pub file_path: String,
    #[serde(default)]
    pub format: SidePanelPageFormat,
    #[serde(default)]
    pub source: SidePanelPageSource,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SidePanelPage {
    #[serde(default)]
    pub status: SidePanelPageStatus,
    pub id: String,
    pub title: String,
    pub file_path: String,
    #[serde(default)]
    pub format: SidePanelPageFormat,
    #[serde(default)]
    pub source: SidePanelPageSource,
    #[serde(default)]
    pub content: String,
    /// Base64 PDF bytes. Content remains a human-readable Markdown fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_data: Option<String>,
    #[serde(default)]
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SidePanelSnapshot {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub unavailable: Option<String>,
    #[serde(default)]
    pub archived: Vec<SidePanelHistoryEntry>,
    #[serde(default)]
    pub focus_revision: u64,
    #[serde(default)]
    pub focused_page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<SidePanelPage>,
}

impl SidePanelSnapshot {
    pub fn has_pages(&self) -> bool {
        !self.pages.is_empty()
    }

    pub fn focused_page(&self) -> Option<&SidePanelPage> {
        let focused_id = self.focused_page_id.as_deref()?;
        self.pages.iter().find(|page| page.id == focused_id)
    }
}

pub fn snapshot_is_empty(snapshot: &SidePanelSnapshot) -> bool {
    snapshot.session_id.is_none()
        && snapshot.revision == 0
        && !snapshot.has_pages()
        && snapshot.archived.is_empty()
        && snapshot.unavailable.is_none()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SidePanelPageStatus {
    #[serde(default)]
    pub last_successful_update_ms: u64,
    #[serde(default)]
    pub source_revision: u64,
    #[serde(default)]
    pub stale_reason: Option<String>,
    #[serde(default)]
    pub last_opened_at_ms: u64,
    #[serde(default)]
    pub restored_from_session: Option<String>,
}

/// Archived metadata only. Bodies are read only when reopening a page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidePanelHistoryEntry {
    pub id: String,
    pub title: String,
    pub file_path: String,
    #[serde(default)]
    pub format: SidePanelPageFormat,
    #[serde(default)]
    pub source: SidePanelPageSource,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub status: SidePanelPageStatus,
    #[serde(default)]
    pub archived_at_ms: u64,
    #[serde(default)]
    pub cache_path: Option<String>,
}
