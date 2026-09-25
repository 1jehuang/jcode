//! The declarative view tree: a closed set of native, theme-aware components.
//!
//! Providers describe *what* to show. The host decides pixels, colors, fonts,
//! shapes (buttons and chips are always pills), motion, focus and scrolling.
//! Layout uses spacing tokens, never raw pixel values, except for explicit
//! image and panel size hints, which the host clamps.
use crate::asset::{ImageFit, ImageSource};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// A root node plus the local state its inputs bind to.
pub type View = Node;

/// One node. `key` gives stable identity across patches, so the host can keep
/// focus, scroll and animation state for keyed items when a list reorders.
/// `fallback` is shown by hosts that do not know `kind`.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub key: Option<String>,
    pub fallback: Option<String>,
    pub kind: NodeKind,
}

impl Node {
    pub fn new(kind: NodeKind) -> Self {
        Self {
            key: None,
            fallback: None,
            kind,
        }
    }

    pub fn keyed(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Direct children, in render order.
    pub fn children(&self) -> Vec<&Node> {
        use NodeKind::*;
        match &self.kind {
            Stack { children, .. }
            | Grid { children, .. }
            | Scroll { children, .. }
            | List { children, .. }
            | Card { children, .. } => children.iter().collect(),
            ListItem { leading, .. } => leading.iter().map(|n| n.as_ref()).collect(),
            Tabs { tabs, .. } => tabs.iter().flat_map(|tab| tab.children.iter()).collect(),
            _ => Vec::new(),
        }
    }

    /// Every action this node can emit directly.
    pub fn actions(&self) -> Vec<&Action> {
        use NodeKind::*;
        match &self.kind {
            Button { on_press, .. } => vec![on_press],
            Chip { on_press, .. } | ListItem { on_press, .. } | Image { on_press, .. } => {
                on_press.iter().collect()
            }
            Toggle { on_change, .. } | Select { on_change, .. } | Tabs { on_change, .. } => {
                on_change.iter().collect()
            }
            Input { on_submit, .. } => on_submit.iter().collect(),
            Error { retry, .. } => retry.iter().collect(),
            _ => Vec::new(),
        }
    }
}

/// Known node types. New variants are additive. Hosts that do not know a
/// variant receive [`NodeKind::Unknown`] and render the node's fallback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    // Layout
    Stack {
        #[serde(default)]
        direction: Axis,
        #[serde(default)]
        gap: Space,
        /// Defaults to none: containers, not stacks, own insets.
        #[serde(default = "no_space")]
        padding: Space,
        #[serde(default)]
        align: Align,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// A wrapping grid, e.g. an image gallery. Columns are as many as fit
    /// `min_column_width`, clamped by the host.
    Grid {
        #[serde(default = "default_grid_column")]
        min_column_width: u32,
        #[serde(default)]
        gap: Space,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// A vertically scrolling region. The host persists its offset by key.
    Scroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_height: Option<u32>,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// A genuinely multi-line container (`rounded_xl`). Do not nest cards.
    Card {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default)]
        children: Vec<Node>,
    },
    Tabs {
        /// State key holding the selected tab id.
        bind: String,
        tabs: Vec<Tab>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_change: Option<Action>,
    },
    Spacer,
    Divider,

    // Content
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
        #[serde(default)]
        tone: Tone,
        /// Clamp to this many lines with an ellipsis.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
        #[serde(default = "default_true")]
        selectable: bool,
    },
    Markdown {
        text: String,
    },
    Code {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
    Image {
        source: ImageSource,
        /// Accessible description, also shown while loading or on failure.
        alt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        /// Reserve layout space before pixels arrive to avoid reflow.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aspect_ratio: Option<f32>,
        #[serde(default)]
        fit: ImageFit,
        #[serde(default)]
        shape: ImageShape,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_press: Option<Action>,
    },
    Icon {
        name: String,
        #[serde(default)]
        tone: Tone,
    },
    KeyValue {
        rows: Vec<KeyValueRow>,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Progress {
        /// 0.0 to 1.0. `None` is indeterminate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Empty {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<Action>,
    },

    // Controls (always pills)
    Button {
        label: String,
        #[serde(default)]
        variant: ButtonVariant,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
        on_press: Action,
        #[serde(default)]
        disabled: bool,
    },
    Chip {
        label: String,
        #[serde(default)]
        tone: Tone,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_press: Option<Action>,
    },
    Toggle {
        label: String,
        /// State key holding a bool.
        bind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_change: Option<Action>,
    },
    Input {
        /// State key holding the text.
        bind: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        multiline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_submit: Option<Action>,
    },
    Select {
        bind: String,
        options: Vec<SelectOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_change: Option<Action>,
    },
    /// A list of pill rows on a subtle fill. Virtualized by the host.
    List {
        #[serde(default)]
        children: Vec<Node>,
    },
    ListItem {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        /// Right-aligned secondary text, such as a time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<String>,
        /// A small leading visual: an `image` (avatar, thumbnail) or `icon`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        leading: Option<Box<Node>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        badges: Vec<String>,
        #[serde(default)]
        emphasized: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_press: Option<Action>,
    },

    /// Escape hatch: sandboxed HTML in an isolated process. Requires the `html`
    /// capability. Scripts may only post `applet.action` messages.
    Html {
        source: String,
        height: u32,
    },

    /// A node type this host does not know. Never sent by providers. The raw
    /// fields are preserved so relaying or persisting the node is lossless.
    #[serde(skip)]
    Unknown {
        type_name: String,
        raw: Map<String, Value>,
    },
}

/// Every `type` tag this build understands.
pub const KNOWN_TYPES: &[&str] = &[
    "stack",
    "grid",
    "scroll",
    "card",
    "tabs",
    "spacer",
    "divider",
    "text",
    "markdown",
    "code",
    "image",
    "icon",
    "key_value",
    "table",
    "progress",
    "empty",
    "error",
    "button",
    "chip",
    "toggle",
    "input",
    "select",
    "list",
    "list_item",
    "html",
];

fn default_true() -> bool {
    true
}
fn no_space() -> Space {
    Space::None
}
fn default_grid_column() -> u32 {
    160
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueRow {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
    /// Horizontal stacks only: push children apart.
    Between,
}

/// Spacing tokens mapped to the host's spacing scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    None,
    Xs,
    #[default]
    Sm,
    Md,
    Lg,
    Xl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    #[default]
    Body,
    Title,
    Heading,
    Caption,
    Mono,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Default,
    Dim,
    Accent,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonVariant {
    /// Filled accent pill.
    Primary,
    /// Subtle filled pill.
    #[default]
    Secondary,
    /// Small inline pill, such as Copy or Skip.
    Compact,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageShape {
    /// Softly rounded rectangle.
    #[default]
    Rounded,
    /// Circle, for avatars.
    Circle,
    Square,
}

/// A user intent. Names beginning with `host.` are handled by the host itself
/// and need the matching capability. All others go to the provider as
/// [`crate::HostMessage::Action`] with the instance's current state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub action: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
}

impl Action {
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            args: Value::Null,
        }
    }

    pub fn is_host(&self) -> bool {
        self.action.starts_with("host.")
    }
}

/// Host actions. Each maps to a capability in [`crate::Capability`].
pub mod host_action {
    pub const OPEN_URL: &str = "host.open_url";
    pub const COPY: &str = "host.copy";
    pub const START_CHAT: &str = "host.start_chat";
    pub const SEND_PROMPT: &str = "host.send_prompt";
    pub const OPEN_FILE: &str = "host.open_file";
    /// Close this instance. Always allowed.
    pub const CLOSE: &str = "host.close";
    /// Set a state key locally without a provider round trip. Always allowed.
    pub const SET_STATE: &str = "host.set_state";
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = match &self.kind {
            NodeKind::Unknown { type_name, raw } => {
                let mut map = raw.clone();
                map.insert("type".into(), Value::String(type_name.clone()));
                Value::Object(map)
            }
            kind => serde_json::to_value(kind).map_err(serde::ser::Error::custom)?,
        };
        if let Value::Object(map) = &mut value {
            if let Some(key) = &self.key {
                map.insert("key".into(), Value::String(key.clone()));
            }
            if let Some(fallback) = &self.fallback {
                map.insert("fallback".into(), Value::String(fallback.clone()));
            }
        }
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = Value::deserialize(deserializer)?;
        let map = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("applet node must be an object"))?;
        let key = take_string(map, "key").map_err(D::Error::custom)?;
        let fallback = take_string(map, "fallback").map_err(D::Error::custom)?;
        let type_name = map
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("applet node is missing a string `type`"))?
            .to_owned();
        let kind = if KNOWN_TYPES.contains(&type_name.as_str()) {
            // Known types are strict: a malformed known node is an error, not
            // silently unknown.
            serde_json::from_value(value)
                .map_err(|error| D::Error::custom(format!("invalid `{type_name}` node: {error}")))?
        } else {
            let mut raw = std::mem::take(map);
            raw.remove("type");
            NodeKind::Unknown { type_name, raw }
        };
        Ok(Node {
            key,
            fallback,
            kind,
        })
    }
}

fn take_string(map: &mut Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match map.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("applet node `{field}` must be a string")),
    }
}
