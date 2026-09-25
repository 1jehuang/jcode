//! Static applet description: identity, entry points and requested capabilities.
use crate::placement::Placement;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Must equal [`crate::SCHEMA`] or share its major version.
    pub schema: String,
    pub id: crate::AppletId,
    pub title: String,
    /// A built-in icon name such as `mail`, or an asset id prefixed `asset:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How users start the applet without a tool call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launchers: Vec<Launcher>,
    /// Tool calls this applet renders as cards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_cards: Vec<ToolCardClaim>,
    /// Everything beyond rendering and emitting actions back to the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
}

impl Manifest {
    pub fn allows(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// A user-visible entry point. Launching mounts a new instance at `placement`,
/// or focuses the existing one when `singleton` is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launcher {
    #[serde(flatten)]
    pub trigger: Trigger,
    pub placement: Placement,
    #[serde(default = "default_true")]
    pub singleton: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "trigger", rename_all = "snake_case")]
pub enum Trigger {
    /// A pill in the sidebar launcher row.
    Sidebar,
    /// An entry in the command palette.
    Command { label: String },
    /// A keybinding request such as `super-shift-l`. The host may refuse
    /// bindings that collide with its own.
    Shortcut { keys: String },
    /// Mounted automatically when the provider connects.
    Startup,
}

/// Claims tool calls by tool name and, optionally, the `action` input field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCardClaim {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

impl ToolCardClaim {
    pub fn matches(&self, tool: &str, action: Option<&str>) -> bool {
        self.tool == tool
            && (self.actions.is_empty()
                || action.is_some_and(|action| self.actions.iter().any(|a| a == action)))
    }
}

/// Host services an applet must declare. The user approves them once per
/// applet. Validation rejects views that use undeclared capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// `host.open_url` actions.
    OpenUrl,
    /// `host.copy` actions.
    Clipboard,
    /// `host.start_chat` actions: open a new session with a prompt.
    StartChat,
    /// `host.send_prompt` actions: submit into an existing session.
    SendPrompt,
    /// `host.open_file` actions and `path` image sources.
    ReadFiles,
    /// `url` image sources.
    RemoteImages,
    /// Desktop notifications from toasts.
    Notifications,
    /// The sandboxed `html` node.
    Html,
}
