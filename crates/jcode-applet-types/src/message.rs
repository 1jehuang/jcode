//! Runtime messages between applet providers and the host.
use crate::asset::AssetDecl;
use crate::manifest::Manifest;
use crate::patch::PatchOp;
use crate::placement::{Lifetime, Placement, Scope};
use crate::view::{Action, View};
use crate::{AppletId, InstanceId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Everything needed to render an instance: the complete, validated unit the
/// host stores, persists and restores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Monotonic. Patches carry the revision they apply to.
    pub revision: u64,
    pub title: String,
    pub view: View,
    /// Values bound by inputs, toggles, selects and tabs, keyed by `bind`.
    #[serde(default = "empty_object")]
    pub state: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetDecl>,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// A mounted applet instance, as the host tracks it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    pub applet: AppletId,
    pub placement: Placement,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub lifetime: Lifetime,
    pub document: Document,
}

/// Provider to host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderMessage {
    /// Declare or update an applet. Must precede its first mount.
    Register {
        manifest: Manifest,
    },
    /// Show a new instance, or replace an existing one with the same id.
    Mount {
        instance: InstanceId,
        placement: Placement,
        #[serde(default)]
        scope: Scope,
        #[serde(default)]
        lifetime: Lifetime,
        document: Box<Document>,
    },
    /// Incremental update. Rejected unless `base_revision` matches the host's
    /// current revision. The host then sends [`HostMessage::Resync`].
    Patch {
        instance: InstanceId,
        base_revision: u64,
        ops: Vec<PatchOp>,
        /// Assets added alongside the patch.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        assets: Vec<AssetDecl>,
    },
    /// Move an instance, e.g. promote an inline card to a panel.
    Move {
        instance: InstanceId,
        placement: Placement,
    },
    /// Transient notification.
    Toast {
        instance: InstanceId,
        text: String,
        #[serde(default)]
        tone: crate::view::Tone,
    },
    /// Show busy state on an instance, e.g. while an action is processed.
    Busy {
        instance: InstanceId,
        busy: bool,
    },
    Close {
        instance: InstanceId,
    },
}

/// Host to provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// A user intent from a node, with the instance's current local state.
    Action {
        instance: InstanceId,
        revision: u64,
        action: Action,
        state: Value,
        /// Key of the node that emitted the action, when keyed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_key: Option<String>,
    },
    /// A launcher fired. The provider should mount an instance.
    Launch {
        applet: AppletId,
        launcher: usize,
        instance: InstanceId,
    },
    /// The host rejected a message. The provider should mount a full document.
    Resync {
        instance: InstanceId,
        revision: u64,
        reason: String,
    },
    /// Visibility changed. Providers may pause polling while hidden.
    Visibility { instance: InstanceId, visible: bool },
    /// The user or host closed the instance.
    Closed { instance: InstanceId },
    /// A message failed validation. Sent instead of rendering it.
    Rejected {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance: Option<InstanceId>,
        reason: String,
    },
}
