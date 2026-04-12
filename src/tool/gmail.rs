use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::google;
use crate::gmail::{self, GmailClient, MessageFormat};

pub struct GmailTool {
    client: GmailClient,
}

impl GmailTool {
    pub fn new() -> Self {
        Self {
            client: GmailClient::new(),
        }
    }
}

#[derive(Deserialize)]
struct GmailInput {
    action: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    draft_id: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    max_results: Option<u32>,
    #[serde(default)]
    label_ids: Option<Vec<String>>,
    #[serde(default)]
    add_labels: Option<Vec<String>>,
    #[serde(default)]
    remove_labels: Option<Vec<String>>,
    #[serde(default)]
    confirmed: Option<bool>,
}

#[async_trait]
impl Tool for GmailTool {
    fn name(&self) -> &str {
        "gmail"
    }

    fn description(&self) -> &str {
        "Use Gmail."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "read", "list", "draft", "send", "send_draft", "threads", "thread", "labels", "trash", "modify_labels"],
                    "description": "Action."
                },
                "query": { "type": "string" },
                "message_id": { "type": "string" },
                "thread_id": { "type": "string" },
                "draft_id": { "type": "string" },
                "to": { "type": "string" },
                "subject": { "type": "string" },
                "body": { "type": "string" },
                "in_reply_to": { "type": "string" },
                "max_results": { "type": "integer" },
                "label_ids": { "type": "array", "items": { "type": "string" } },
                "add_labels": { "type": "array", "items": { "type": "string" } },
                "remove_labels": { "type": "array", "items": { "type": "string" } },
                "confirmed": {
                    "type": "boolean",
                    "description": "Confirm."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        if !google::has_tokens() {
            return Ok(ToolOutput::new(
                "Gmail is not configured. Run `jcode login google` to set up Gmail access.",
            ));
        }

        let params: GmailInput = serde_json::from_value(input)?;
        let max = params.max_results.unwrap_or(10).min(50);

        match params.action.as_str() {
            "search" | "list" => {
                let query = params.query.as_deref();
                let label_refs: Vec<&str> = params
                    .label_ids
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                let labels = if label_refs.is_empty() {
                    None
                } else {
                    Some(label_refs.as_slice())
                };

                let list = self.client.list_messages(query, labels, max).await?;
                let msgs = list.messages.unwrap_or_default();

                if msgs.is_empty() {
                    return Ok(ToolOutput::new("No messages found."));
                }

                let mut results = Vec::new();
                for (i, msg_ref) in msgs.iter().enumerate().take(max as usize) {
                    match self
                        .client
                        .get_message(&msg_ref.id, MessageFormat::Metadata)
                        .await
                    {
                        Ok(msg) => {
                            results.push(format!(
                                "{}. {}\n   From: {}\n   Date: {}\n   ID: {}",
                                i + 1,
                                msg.subject().unwrap_or("(no subject)"),
                                msg.from().unwrap_or("(unknown)"),
                                msg.date().unwrap_or(""),
                                msg.id,
                            ));
                        }
                        Err(e) => {
                            results.push(format!(
                                "{}. [error fetching {}: {}]",
                                i + 1,
                                msg_ref.id,
                                e
                            ));
                        }
                    }
                }

                let header = if let Some(q) = query {
                    format!("Search results for \"{}\" ({} found):", q, msgs.len())
                } else {
                    format!("Recent messages ({} shown):", results.len())
                };

                Ok(ToolOutput::new(format!(
                    "{}\n\n{}",
                    header,
                    results.join("\n\n")
                )))
            }

            "read" => {
                let id = params
                    .message_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("message_id is required for read action"))?;

                let msg = self.client.get_message(id, MessageFormat::Full).await?;
                Ok(ToolOutput::new(gmail::format_message_full(&msg)))
            }

            "threads" => {
                let query = params.query.as_deref();
                let list = self.client.list_threads(query, max).await?;
                let threads = list.threads.unwrap_or_default();

                if threads.is_empty() {
                    return Ok(ToolOutput::new("No threads found."));
                }

                let mut results = Vec::new();
                for (i, t) in threads.iter().enumerate() {
                    results.push(format!(
                        "{}. {}\n   ID: {}",
                        i + 1,
                        t.snippet.as_deref().unwrap_or("(no snippet)"),
                        t.id,
                    ));
                }

                Ok(ToolOutput::new(format!(
                    "Threads ({}):\n\n{}",
                    threads.len(),
                    results.join("\n\n")
                )))
            }

            "thread" => {
                let id = params
                    .thread_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("thread_id is required for thread action"))?;

                let thread = self.client.get_thread(id).await?;
                let messages = thread.messages.unwrap_or_default();

                if messages.is_empty() {
                    return Ok(ToolOutput::new("Thread has no messages."));
                }

                let mut results = Vec::new();
                for (i, msg) in messages.iter().enumerate() {
                    results.push(format!(
                        "--- Message {} ---\nFrom: {}\nDate: {}\nSubject: {}\nSnippet: {}",
                        i + 1,
                        msg.from().unwrap_or("(unknown)"),
                        msg.date().unwrap_or(""),
                        msg.subject().unwrap_or("(no subject)"),
                        msg.snippet.as_deref().unwrap_or(""),
                    ));
                }

                Ok(ToolOutput::new(format!(
                    "Thread {} ({} messages):\n\n{}",
                    id,
                    messages.len(),
                    results.join("\n\n")
                )))
            }

            "labels" => {
                let labels = self.client.list_labels().await?;
                let mut results = Vec::new();
                for label in &labels {
                    let unread = label
                        .messages_unread
                        .map(|u| format!(" ({} unread)", u))
                        .unwrap_or_default();
                    let total = label
                        .messages_total
                        .map(|t| format!(" [{} total]", t))
                        .unwrap_or_default();
                    results.push(format!(
                        "- {} (id: {}){}{}",
                        label.name, label.id, unread, total
                    ));
                }
                Ok(ToolOutput::new(format!("Labels:\n{}", results.join("\n"))))
            }

            "draft" => {
                let to = params
                    .to
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'to' is required for draft action"))?;
                let subject = params.subject.as_deref().unwrap_or("");
                let body = params.body.as_deref().unwrap_or("");

                let draft = self
                    .client
                    .create_draft(
                        to,
                        subject,
                        body,
                        params.in_reply_to.as_deref(),
                        params.thread_id.as_deref(),
                    )
                    .await?;

                Ok(ToolOutput::new(format!(
                    "Draft created successfully.\nDraft ID: {}\nTo: {}\nSubject: {}\n\nTo send this draft, use action 'send_draft' with draft_id '{}' and confirmed: true.",
                    draft.id, to, subject, draft.id
                )))
            }

            "send" => {
                let tokens = google::load_tokens()?;
                if !tokens.tier.can_send() {
                    return Ok(ToolOutput::new(
                        "Send is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         The draft has been created - open Gmail to send it manually.\n\
                         To enable sending, run `jcode login google --upgrade`.",
                    ));
                }

                let to = params
                    .to
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'to' is required for send action"))?;
                let subject = params.subject.as_deref().unwrap_or("");
                let body = params.body.as_deref().unwrap_or("");

                if params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "CONFIRMATION REQUIRED: Send this email?\n\n\
                         To: {}\n\
                         Subject: {}\n\
                         Body:\n{}\n\n\
                         To confirm, call gmail again with the same parameters and confirmed: true.",
                        to, subject, body
                    )));
                }

                let msg = self
                    .client
                    .send_message(
                        to,
                        subject,
                        body,
                        params.in_reply_to.as_deref(),
                        params.thread_id.as_deref(),
                    )
                    .await?;

                Ok(ToolOutput::new(format!(
                    "Email sent successfully.\nMessage ID: {}\nTo: {}\nSubject: {}",
                    msg.id, to, subject
                )))
            }

            "send_draft" => {
                let tokens = google::load_tokens()?;
                if !tokens.tier.can_send() {
                    return Ok(ToolOutput::new(
                        "Send is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         Open Gmail to send the draft manually.\n\
                         To enable sending, run `jcode login google --upgrade`.",
                    ));
                }

                let draft_id = params.draft_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("'draft_id' is required for send_draft action")
                })?;

                if params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "CONFIRMATION REQUIRED: Send draft {}?\n\n\
                         To confirm, call gmail again with action 'send_draft', draft_id '{}', and confirmed: true.",
                        draft_id, draft_id
                    )));
                }

                let msg = self.client.send_draft(draft_id).await?;
                Ok(ToolOutput::new(format!(
                    "Draft sent successfully.\nMessage ID: {}",
                    msg.id
                )))
            }

            "trash" => {
                let tokens = google::load_tokens()?;
                if !tokens.tier.can_delete() {
                    return Ok(ToolOutput::new(
                        "Trash is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         To enable delete, run `jcode login google --upgrade`.",
                    ));
                }

                let id = params
                    .message_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'message_id' is required for trash action"))?;

                if params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "CONFIRMATION REQUIRED: Move message {} to trash?\n\n\
                         To confirm, call gmail again with action 'trash', message_id '{}', and confirmed: true.",
                        id, id
                    )));
                }

                self.client.trash_message(id).await?;
                Ok(ToolOutput::new(format!("Message {} moved to trash.", id)))
            }

            "modify_labels" => {
                let id = params
                    .message_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'message_id' is required for modify_labels"))?;

                let add: Vec<&str> = params
                    .add_labels
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();
                let remove: Vec<&str> = params
                    .remove_labels
                    .as_ref()
                    .map(|v| v.iter().map(|s| s.as_str()).collect())
                    .unwrap_or_default();

                self.client.modify_labels(id, &add, &remove).await?;
                Ok(ToolOutput::new(format!(
                    "Labels modified on message {}.\nAdded: {:?}\nRemoved: {:?}",
                    id, add, remove
                )))
            }

            other => Ok(ToolOutput::new(format!(
                "Unknown gmail action: '{}'. Valid actions: search, read, list, draft, send, send_draft, threads, thread, labels, trash, modify_labels",
                other
            ))),
        }
    }
}
