//! Payload types for the `ask_user` tool, shared by the tool and the wire.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AskUserOption>,
    #[serde(rename = "multiSelect", default)]
    pub multi_select: bool,
}

/// The user's choice for one question: option labels, or the custom text typed
/// into "Other". Empty means the question was left unanswered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserAnswer {
    pub question: String,
    pub answers: Vec<String>,
}
