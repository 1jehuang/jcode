//! Elicitation overlay types for structured agent-to-user input.
//!
//! Defines the data model for elicitation requests (multi-choice, text input,
//! boolean prompts) that agents can present to users via a native TUI overlay,
//! replacing the external phinbox popup dependency.

use ratatui::style::Color;
use tokio::sync::oneshot;

/// Request from agent to display elicitation overlay.
#[derive(Debug, Clone)]
pub struct ElicitRequest {
    pub title: String,
    pub field: FieldSpec,
    pub intent: String,
    pub question: Option<String>,
    pub notes: Option<NotesSpec>,
    pub buttons: Option<ButtonSpec>,
    pub request_id: Option<String>,
    pub timeout_secs: u32,
    pub urgency: Urgency,
}

/// Discriminated union for input field types.
#[derive(Debug, Clone)]
pub enum FieldSpec {
    Text {
        label: String,
        default: Option<String>,
        placeholder: Option<String>,
        max_length: Option<u32>,
        secret: bool,
    },
    LongText {
        label: String,
        default: Option<String>,
        max_length: Option<u32>,
    },
    Integer {
        label: String,
        default: Option<i64>,
        min: Option<i64>,
        max: Option<i64>,
    },
    Choice {
        label: String,
        options: Vec<ChoiceOption>,
        default_index: Option<usize>,
    },
    Boolean {
        label: String,
        default: Option<bool>,
    },
    DateTime {
        label: String,
        default: Option<String>,
        picker_kind: DateTimeKind,
    },
}

/// A single selectable option within a `FieldSpec::Choice` field.
#[derive(Debug, Clone)]
pub struct ChoiceOption {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
}

/// Which calendar/time picker widget to render.
#[derive(Debug, Clone)]
pub enum DateTimeKind {
    Date,
    Time,
    DateTime,
}

/// Optional free-text notes area attached to an elicitation request.
#[derive(Debug, Clone)]
pub struct NotesSpec {
    pub label: String,
    pub default: Option<String>,
    pub max_length: Option<u32>,
    pub required: bool,
}

/// Button labels for the elicitation overlay footer.
#[derive(Debug, Clone)]
pub struct ButtonSpec {
    pub confirm: String,
    pub cancel: String,
    pub default_is_cancel: bool,
}

/// Urgency level controlling border color and icon.
#[derive(Debug, Clone)]
pub enum Urgency {
    Info,
    Warning,
    Error,
    Secret,
}

impl Urgency {
    /// Border color for the overlay panel.
    pub fn border_color(&self) -> Color {
        match self {
            Urgency::Info => Color::Blue,
            Urgency::Warning => Color::Yellow,
            Urgency::Error => Color::Red,
            Urgency::Secret => Color::DarkGray,
        }
    }

    /// Icon glyph shown next to the title.
    pub fn icon(&self) -> &'static str {
        match self {
            Urgency::Info => "●",
            Urgency::Warning => "◆",
            Urgency::Error => "✗",
            Urgency::Secret => "○",
        }
    }
}

/// Response sent back to the agent after user interaction.
#[derive(Debug, Clone)]
pub struct ElicitResponse {
    pub action: ElicitAction,
    pub value: Option<String>,
    pub notes: Option<String>,
}

/// Whether the user confirmed or cancelled the elicitation.
#[derive(Debug, Clone)]
pub enum ElicitAction {
    Confirm,
    Cancel,
}

/// Mutable state of the elicitation overlay within the TUI.
///
/// Held in `App::elicit_overlay` as `Some(...)` while an elicitation is
/// active. The overlay renders this state and, on confirm/cancel, sends the
/// response through `response_tx` and clears the slot.
pub struct ElicitOverlayState {
    pub request: ElicitRequest,
    /// Currently focused UI element index (fields, notes, buttons).
    pub cursor: usize,
    /// Selected option index for `FieldSpec::Choice`.
    pub selected: usize,
    /// Text buffer for `FieldSpec::Text` / `FieldSpec::LongText`.
    pub text_buffer: String,
    /// Channel to send the user's response back to the waiting tool call.
    pub response_tx: Option<oneshot::Sender<ElicitResponse>>,
}
