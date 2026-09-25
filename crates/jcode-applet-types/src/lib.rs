//! Jcode applets: declarative, sandboxed custom UI hosted by Jcode Desktop.
//!
//! An applet is a provider (the agent, a local process, an MCP server, or
//! native Desktop code) that describes UI as a [`View`] tree of a closed set of
//! native components. The host renders it, owns layout, theme and input, and
//! sends user intents back as [`HostMessage::Action`]. No provider code runs in
//! the host process.
//!
//! Applets are independent of tool calls. Each mounted [`Instance`] declares a
//! [`Placement`]: a standalone panel, a sidebar section, an overlay, a strip
//! above the composer, or inline in a transcript anchored to a tool call, a
//! message, or simply the end of the conversation.
//!
//! Everything a host receives must pass [`validate::validate_document`] before
//! rendering. Unknown future node types deserialize to [`NodeKind::Unknown`]
//! and render their fallback text, so older hosts degrade gracefully.

pub mod asset;
pub mod manifest;
pub mod message;
pub mod patch;
pub mod placement;
pub mod validate;
pub mod view;

pub use asset::{AssetDecl, ImageFit, ImageSource};
pub use manifest::{Capability, Launcher, Manifest, ToolCardClaim};
pub use message::{Document, HostMessage, Instance, ProviderMessage};
pub use patch::{PatchError, PatchOp, apply_patch};
pub use placement::{Anchor, Lifetime, Placement, Scope};
pub use validate::{Limits, ValidationError, validate_document};
pub use view::{Action, Node, NodeKind, View};

/// Wire schema identifier. Bump the suffix only for breaking changes. Additive
/// changes (new nodes, new optional fields) keep the same schema.
pub const SCHEMA: &str = "jcode.applet/1";

/// Reverse-DNS or simple slug identifying an applet, such as `com.example.linear`.
pub type AppletId = String;
/// Host-unique identity of one mounted applet instance.
pub type InstanceId = String;
