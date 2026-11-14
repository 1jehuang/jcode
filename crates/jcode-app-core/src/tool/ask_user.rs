//! Multiple-choice questions answered in the attached TUI.
use super::{AskUserInputRequest, ClientInputRequest, Tool, ToolContext, ToolOutput};
use anyhow::{Result, bail};
use async_trait::async_trait;
use jcode_message_types::ask_user::{AskUserAnswer, AskUserOption, AskUserQuestion};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub(super) struct AskUserTool;

pub(super) const DISMISSED: &str = "The user dismissed the questions without answering. Stop and wait for the user to tell you how to proceed.";
pub(super) const NO_USER: &str = "No interactive user is available to answer. Proceed with your best judgment and state the assumptions you made.";

/// Which attached clients can show questions, per session. The server updates
/// it on attach, detach and activity without taking the Agent lock, and the
/// tool reads it when it runs, so a TUI that reattaches mid-turn is reachable.
pub(crate) mod route {
    use super::ClientInputRequest;
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use tokio::sync::mpsc::UnboundedSender;

    type Tx = UnboundedSender<ClientInputRequest>;
    /// session id -> capable connections, least to most recently active.
    type Routes = HashMap<String, Vec<(String, Tx)>>;
    static ROUTES: LazyLock<Mutex<Routes>> = LazyLock::new(Default::default);

    fn routes() -> std::sync::MutexGuard<'static, Routes> {
        ROUTES.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// Make `capable` the session's capable connections. Known ones keep their
    /// recency, new ones become the most recent, and a connection routes for
    /// one session only.
    pub(crate) fn sync(session_id: &str, capable: Vec<(String, Tx)>) {
        let mut routes = routes();
        for (session, list) in routes.iter_mut() {
            if session != session_id {
                list.retain(|(id, _)| !capable.iter().any(|(other, _)| other == id));
            }
        }
        let list = routes.entry(session_id.to_string()).or_default();
        list.retain(|(id, _)| capable.iter().any(|(other, _)| other == id));
        for (id, tx) in capable {
            if !list.iter().any(|(known, _)| *known == id) {
                list.push((id, tx));
            }
        }
        routes.retain(|_, list| !list.is_empty());
    }

    /// The user just used this connection: ask there first.
    pub(crate) fn touch(connection_id: &str) {
        for list in routes().values_mut() {
            if let Some(index) = list.iter().position(|(id, _)| id == connection_id) {
                let entry = list.remove(index);
                list.push(entry);
            }
        }
    }

    pub(crate) fn current(session_id: &str) -> Option<Tx> {
        routes()
            .get(session_id)?
            .iter()
            .rev()
            .find(|(_, tx)| !tx.is_closed())
            .map(|(_, tx)| tx.clone())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    questions: Vec<QuestionInput>,
    #[serde(rename = "intent")]
    _intent: Option<String>,
    #[serde(rename = "accept_large_output")]
    _accept_large_output: Option<bool>,
}

/// Strict tool-input twins of the wire types, which accept unknown fields.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionInput {
    question: String,
    header: String,
    options: Vec<OptionInput>,
    #[serde(rename = "multiSelect", default)]
    multi_select: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionInput {
    label: String,
    description: String,
}

impl From<QuestionInput> for AskUserQuestion {
    fn from(input: QuestionInput) -> Self {
        Self {
            question: input.question,
            header: input.header,
            options: input
                .options
                .into_iter()
                .map(|option| AskUserOption {
                    label: option.label,
                    description: option.description,
                })
                .collect(),
            multi_select: input.multi_select,
        }
    }
}

/// Tells the client to close the dialog if the call ends without an answer
/// (interrupt, or the turn being dropped), and frees the pending reply.
struct CloseOnDrop {
    client: tokio::sync::mpsc::UnboundedSender<ClientInputRequest>,
    request_id: String,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if !self.request_id.is_empty() {
            let request_id = std::mem::take(&mut self.request_id);
            let _ = self
                .client
                .send(ClientInputRequest::AskUserClosed(request_id));
        }
    }
}

fn validate(questions: &[AskUserQuestion]) -> Result<()> {
    if !(1..=4).contains(&questions.len()) {
        bail!("ask_user takes 1-4 questions, got {}", questions.len());
    }
    let mut texts = HashSet::new();
    for question in questions {
        if !texts.insert(question.question.as_str()) {
            bail!("duplicate question text: {:?}", question.question);
        }
        if !(2..=4).contains(&question.options.len()) {
            bail!(
                "question {:?} needs 2-4 options, got {}",
                question.question,
                question.options.len()
            );
        }
        let mut labels = HashSet::new();
        for option in &question.options {
            if !labels.insert(option.label.as_str()) {
                bail!(
                    "duplicate option label {:?} in question {:?}",
                    option.label,
                    question.question
                );
            }
        }
    }
    Ok(())
}

pub(super) fn format_answers(questions: &[AskUserQuestion], answers: &[AskUserAnswer]) -> String {
    let pairs: Vec<String> = questions
        .iter()
        .map(|question| {
            let answer = answers
                .iter()
                .find(|answer| answer.question == question.question)
                .filter(|answer| !answer.answers.is_empty());
            let quoted = |text: &str| serde_json::to_string(text).unwrap_or_default();
            match answer {
                Some(answer) => {
                    // Quote a choice that would otherwise blur into its neighbours.
                    let choices: Vec<String> = answer
                        .answers
                        .iter()
                        .map(|choice| {
                            if choice.contains([',', '"']) {
                                quoted(choice)
                            } else {
                                choice.clone()
                            }
                        })
                        .collect();
                    format!(
                        "{}={}",
                        quoted(&question.question),
                        quoted(&choices.join(", "))
                    )
                }
                None => format!("{}=(no answer)", quoted(&question.question)),
            }
        })
        .collect();
    format!(
        "The user answered: {}. Follow what they actually said.",
        pairs.join(", ")
    )
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user 1-4 multiple-choice questions and wait for their answers. Use only when blocked on a decision that is genuinely the user's to make and that you cannot resolve from the request, the code, or sensible defaults. The user can always pick \"Other\" and type a custom answer, so never include an Other option. Put a recommended option first and suffix its label with \"(Recommended)\". Each question must stand on its own; the user may see only the question dialog."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "question": {"type": "string", "description": "Full question, ending with ?"},
                            "header": {"type": "string", "description": "Chip label, at most 12 chars."},
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": {
                                        "label": {"type": "string", "description": "1-5 words."},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {"type": "boolean", "default": false}
                        },
                        "required": ["question", "header", "options"]
                    }
                },
                "intent": super::intent_schema_property()
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let Input { questions, .. } = serde_json::from_value(input)?;
        let questions: Vec<AskUserQuestion> = questions.into_iter().map(Into::into).collect();
        validate(&questions)?;
        let output = |text: &str| Ok(ToolOutput::new(text).with_title("ask_user"));
        let Some(client) = route::current(&ctx.session_id) else {
            return output(NO_USER);
        };
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let request_id = crate::id::new_id("ask");
        let request = AskUserInputRequest {
            request_id: request_id.clone(),
            tool_call_id: ctx.tool_call_id.clone(),
            questions: questions.clone(),
            response_tx,
        };
        if client.send(ClientInputRequest::AskUser(request)).is_err() {
            return output(NO_USER);
        }
        let mut close = CloseOnDrop { client, request_id };
        let interrupted = async {
            match &ctx.graceful_shutdown_signal {
                Some(signal) if !signal.is_set() => signal.notified().await,
                Some(_) => {}
                None => std::future::pending::<()>().await,
            }
        };
        let reply = tokio::select! {
            reply = response_rx => reply,
            // Dropping `close` takes the dialog down.
            _ = interrupted => return output(DISMISSED),
        };
        // The client answered, dismissed or went away: nothing left to close.
        close.request_id.clear();
        match reply {
            Ok(Some(answers)) => output(&format_answers(&questions, &answers)),
            Ok(None) => output(DISMISSED),
            Err(_) => output(NO_USER),
        }
    }
}

#[cfg(test)]
#[path = "ask_user_tests.rs"]
mod tests;
