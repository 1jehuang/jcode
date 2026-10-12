//! Where an applet instance appears, who owns it, and how long it lives.
//!
//! Placement is deliberately independent of tool calls. A tool call is just one
//! possible [`Anchor`] for inline placement.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// A first-class workspace panel, like the Gmail inbox. It can be tiled,
    /// focused, and closed like any other panel.
    Panel {
        #[serde(default)]
        open: PanelOpen,
    },
    /// A section in the workspace sidebar. Keep these compact.
    Sidebar,
    /// Inside a conversation transcript, scrolled with the messages.
    Inline { session_id: String, anchor: Anchor },
    /// A strip directly above a session's composer, such as suggestions.
    Composer { session_id: String },
    /// A floating card over the workspace, such as a timer or status HUD.
    Overlay {
        #[serde(default)]
        corner: Corner,
    },
    /// No visible UI. The instance can still post toasts and launch panels.
    Background,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelOpen {
    /// Open beside the focused panel.
    #[default]
    Split,
    /// Replace the focused panel's slot, keeping it recoverable.
    Replace,
    /// Open without focusing, e.g. from a background provider.
    Background,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopRight,
    #[default]
    BottomRight,
    BottomLeft,
    TopLeft,
}

/// The transcript position of an inline instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Anchor {
    /// Replaces the generic row for this tool call (a tool card).
    ToolCall { call_id: String },
    /// Directly after a specific message.
    AfterMessage { message_id: String },
    /// After the latest item when mounted, then stays at that position.
    End,
}

/// Visibility scope. The host hides instances outside the active scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Global,
    Workspace {
        dir: String,
    },
    Session {
        session_id: String,
    },
}

/// How long the host keeps an instance and its last rendered document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Gone when the provider disconnects.
    Ephemeral,
    /// Kept while its scope exists. Survives UI reload (Ctrl+R).
    #[default]
    Session,
    /// Restored across app restarts from the last document, then reconciled
    /// when the provider reconnects.
    Persistent,
}
