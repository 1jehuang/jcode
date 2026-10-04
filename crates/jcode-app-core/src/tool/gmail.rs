use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

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

    /// Resolve reply parameters into a usable (In-Reply-To header, threadId).
    ///
    /// Models pass Gmail API message IDs (hex, from search/read output) as
    /// `in_reply_to`, but MIME threading needs the RFC 5322 Message-ID header
    /// and the Gmail API needs the containing threadId. Silently sending
    /// without either starts a new conversation, so look the message up and
    /// fail loudly when it cannot be resolved.
    async fn resolve_reply(
        &self,
        in_reply_to: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<(Option<String>, Option<String>)> {
        let Some(reply_ref) = in_reply_to else {
            return Ok((None, thread_id.map(str::to_string)));
        };
        // Already an RFC 5322 Message-ID (contains '@', usually in <...>).
        if reply_ref.contains('@') {
            return Ok((Some(reply_ref.to_string()), thread_id.map(str::to_string)));
        }
        let msg = self
            .client
            .get_message(reply_ref, MessageFormat::Metadata)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "in_reply_to '{}' is not an RFC 5322 Message-ID and could not be \
                     resolved as a Gmail message ID: {}. The reply was NOT sent.",
                    reply_ref,
                    e
                )
            })?;
        let header_id = msg.header("Message-ID").map(str::to_string);
        let resolved_thread = thread_id.map(str::to_string).or(msg.thread_id.clone());
        if header_id.is_none() && resolved_thread.is_none() {
            anyhow::bail!(
                "Message '{}' has no Message-ID header or threadId; cannot thread the reply. \
                 The reply was NOT sent.",
                reply_ref
            );
        }
        Ok((header_id, resolved_thread))
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
    #[serde(default)]
    attachments: Option<Vec<String>>,
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
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["connect", "search", "read", "list", "draft", "update_draft", "list_drafts", "delete_draft", "send", "send_draft", "threads", "thread", "labels", "trash", "modify_labels"],
                    "description": "Action. 'connect' sets up Gmail access via a browser OAuth screen the user approves. To revise a draft, use 'update_draft' with draft_id (omitted fields keep their current values) instead of creating a new draft."
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
                },
                "attachments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Absolute file paths to attach (for draft/send actions)."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: GmailInput = serde_json::from_value(input)?;
        let max = params.max_results.unwrap_or(10).min(50);

        // The connect action sets up the Composio managed backend by opening a
        // browser OAuth screen for the user to approve. It runs before the
        // is_configured gate so it can establish the very first connection.
        if params.action == "connect" {
            if !self.client.supports_connect() {
                return Ok(ToolOutput::new(
                    "The 'connect' action is only available with the Composio Gmail backend. \
                     Set JCODE_GMAIL_BACKEND=composio and COMPOSIO_API_KEY, then retry. \
                     For the default backend, run `jcode login google` instead.",
                ));
            }
            let no_browser = crate::auth::browser_suppressed(false);
            match self.client.connect(!no_browser).await {
                Ok(conn) => {
                    let who = conn
                        .email
                        .clone()
                        .unwrap_or_else(|| "your Gmail account".to_string());
                    return Ok(ToolOutput::new(format!(
                        "Gmail connected via Composio for {}. You can now search, read, draft, and send email.",
                        who
                    )));
                }
                Err(e) => {
                    return Ok(ToolOutput::new(format!("Gmail connect failed: {}", e)));
                }
            }
        }

        if !self.client.is_configured() {
            return Ok(ToolOutput::new(self.client.not_configured_message()));
        }

        if self.client.needs_connection() {
            return Ok(ToolOutput::new(
                "Gmail (Composio backend) has no connected account yet. Run the gmail tool with \
                 action 'connect' to authorize your Gmail account, then retry.",
            ));
        }

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

                // Accept a message ID too: if the thread lookup fails, try
                // resolving the ID as a message and use its containing thread.
                let thread = match self.client.get_thread(id).await {
                    Ok(t) => t,
                    Err(thread_err) => {
                        match self.client.get_message(id, MessageFormat::Metadata).await {
                            Ok(msg) => {
                                let tid = msg.thread_id.ok_or(thread_err)?;
                                self.client.get_thread(&tid).await?
                            }
                            Err(_) => return Err(thread_err),
                        }
                    }
                };
                let thread_id = thread.id.clone();
                let messages = thread.messages.unwrap_or_default();

                if messages.is_empty() {
                    return Ok(ToolOutput::new("Thread has no messages."));
                }

                let mut results = Vec::new();
                for (i, msg) in messages.iter().enumerate() {
                    let mut entry = format!(
                        "--- Message {} ---\nID: {}\nFrom: {}\nDate: {}\nSubject: {}\nSnippet: {}",
                        i + 1,
                        msg.id,
                        msg.from().unwrap_or("(unknown)"),
                        msg.date().unwrap_or(""),
                        msg.subject().unwrap_or("(no subject)"),
                        msg.snippet.as_deref().unwrap_or(""),
                    );
                    let attachments = msg.attachments();
                    if !attachments.is_empty() {
                        entry.push_str(&format!(
                            "\nAttachments ({}):\n{}",
                            attachments.len(),
                            gmail::format_attachment_lines(&attachments)
                        ));
                    }
                    results.push(entry);
                }

                Ok(ToolOutput::new(format!(
                    "Thread {} ({} messages):\n\n{}",
                    thread_id,
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

                let attachments: Vec<std::path::PathBuf> = params
                    .attachments
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                for path in &attachments {
                    if !path.is_file() {
                        return Ok(ToolOutput::new(format!(
                            "Attachment not found or not a file: {}",
                            path.display()
                        )));
                    }
                }

                let (reply_header, reply_thread) = self
                    .resolve_reply(params.in_reply_to.as_deref(), params.thread_id.as_deref())
                    .await?;
                let draft = self
                    .client
                    .create_draft_with_attachments(
                        to,
                        subject,
                        body,
                        reply_header.as_deref(),
                        reply_thread.as_deref(),
                        &attachments,
                    )
                    .await?;

                let attach_line = if attachments.is_empty() {
                    String::new()
                } else {
                    format!(
                        "Attachments ({}):\n{}\n",
                        attachments.len(),
                        attachments
                            .iter()
                            .map(|p| format!("  - {}", p.display()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                Ok(ToolOutput::new(format!(
                    "Draft created successfully.\nDraft ID: {}\nTo: {}\nSubject: {}\n{}\nTo send this draft, use action 'send_draft' with draft_id '{}' and confirmed: true.",
                    draft.id, to, subject, attach_line, draft.id
                )))
            }

            "send" => {
                if !self.client.can_send() {
                    return Ok(ToolOutput::new(
                        "Send is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         The draft has been created - open Gmail to send it manually.\n\
                         To enable sending, rerun `jcode login google --google-access-tier full`.",
                    ));
                }

                let to = params
                    .to
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'to' is required for send action"))?;
                let subject = params.subject.as_deref().unwrap_or("");
                let body = params.body.as_deref().unwrap_or("");

                let attachments: Vec<std::path::PathBuf> = params
                    .attachments
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                for path in &attachments {
                    if !path.is_file() {
                        return Ok(ToolOutput::new(format!(
                            "Attachment not found or not a file: {}",
                            path.display()
                        )));
                    }
                }

                if params.confirmed != Some(true) {
                    let attach_line = if attachments.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "Attachments:\n{}\n",
                            attachments
                                .iter()
                                .map(|p| format!("  - {}", p.display()))
                                .collect::<Vec<_>>()
                                .join("\n")
                        )
                    };
                    return Ok(ToolOutput::new(format!(
                        "CONFIRMATION REQUIRED: Send this email?\n\n\
                         To: {}\n\
                         Subject: {}\n\
                         {}\
                         Body:\n{}\n\n\
                         To confirm, call gmail again with the same parameters and confirmed: true.",
                        to, subject, attach_line, body
                    )));
                }

                let (reply_header, reply_thread) = self
                    .resolve_reply(params.in_reply_to.as_deref(), params.thread_id.as_deref())
                    .await?;
                let msg = self
                    .client
                    .send_message_with_attachments(
                        to,
                        subject,
                        body,
                        reply_header.as_deref(),
                        reply_thread.as_deref(),
                        &attachments,
                    )
                    .await?;

                Ok(ToolOutput::new(format!(
                    "Email sent successfully.\nMessage ID: {}\nThread ID: {}\nTo: {}\nSubject: {}\nAttachments: {}",
                    msg.id,
                    msg.thread_id.as_deref().unwrap_or("(new thread)"),
                    to,
                    subject,
                    attachments.len()
                )))
            }

            "update_draft" => {
                let draft_id = params.draft_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("'draft_id' is required for update_draft action")
                })?;

                let attachments: Vec<std::path::PathBuf> = params
                    .attachments
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                for path in &attachments {
                    if !path.is_file() {
                        return Ok(ToolOutput::new(format!(
                            "Attachment not found or not a file: {}",
                            path.display()
                        )));
                    }
                }

                // Merge: any field the caller omits keeps the draft's current
                // value, including reply threading, so a revision stays a reply.
                let existing = self.client.get_draft(draft_id).await?;
                let current = existing.message.as_ref();
                let merged = merge_draft_fields(
                    current,
                    params.to.as_deref(),
                    params.subject.as_deref(),
                    params.body.as_deref(),
                );
                let Some(to) = merged.to else {
                    anyhow::bail!(
                        "Draft {} has no recipient; pass 'to' to update it.",
                        draft_id
                    );
                };

                let (reply_header, reply_thread) = if params.in_reply_to.is_some() {
                    self.resolve_reply(params.in_reply_to.as_deref(), params.thread_id.as_deref())
                        .await?
                } else {
                    (
                        merged.in_reply_to,
                        params
                            .thread_id
                            .clone()
                            .or_else(|| current.and_then(|m| m.thread_id.clone())),
                    )
                };

                let dropped_attachments = attachments.is_empty()
                    && current
                        .map(|m| !m.attachments().is_empty())
                        .unwrap_or(false);

                let draft = self
                    .client
                    .update_draft(
                        draft_id,
                        &to,
                        &merged.subject,
                        &merged.body,
                        reply_header.as_deref(),
                        reply_thread.as_deref(),
                        &attachments,
                    )
                    .await?;

                let warn = if dropped_attachments {
                    "\nNote: the previous draft had attachments; they were not carried over. Pass 'attachments' to re-attach."
                } else {
                    ""
                };
                Ok(ToolOutput::new(format!(
                    "Draft updated in place.\nDraft ID: {}\nTo: {}\nSubject: {}\nBody:\n{}{}\n\nTo send this draft, use action 'send_draft' with draft_id '{}' and confirmed: true.",
                    draft.id, to, merged.subject, merged.body, warn, draft.id
                )))
            }

            "list_drafts" => {
                let drafts = self.client.list_drafts(max).await?;
                if drafts.is_empty() {
                    return Ok(ToolOutput::new("No drafts found."));
                }
                let mut results = Vec::new();
                for (i, d) in drafts.iter().enumerate() {
                    match self.client.get_draft(&d.id).await {
                        Ok(full) => {
                            let m = full.message.as_ref();
                            results.push(format!(
                                "{}. {}\n   To: {}\n   Snippet: {}\n   Draft ID: {}",
                                i + 1,
                                m.and_then(|m| m.subject()).unwrap_or("(no subject)"),
                                m.and_then(|m| m.header("To")).unwrap_or("(none)"),
                                m.and_then(|m| m.snippet.as_deref()).unwrap_or(""),
                                d.id,
                            ));
                        }
                        Err(e) => results.push(format!(
                            "{}. [error fetching draft {}: {}]",
                            i + 1,
                            d.id,
                            e
                        )),
                    }
                }
                Ok(ToolOutput::new(format!(
                    "Drafts ({}):\n\n{}",
                    drafts.len(),
                    results.join("\n\n")
                )))
            }

            "delete_draft" => {
                let draft_id = params.draft_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("'draft_id' is required for delete_draft action")
                })?;
                if params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "CONFIRMATION REQUIRED: Permanently delete draft {}? Drafts do not go to Trash.\n\n\
                         To confirm, call gmail again with action 'delete_draft', draft_id '{}', and confirmed: true.",
                        draft_id, draft_id
                    )));
                }
                self.client.delete_draft(draft_id).await?;
                Ok(ToolOutput::new(format!("Draft {} deleted.", draft_id)))
            }

            "send_draft" => {
                if !self.client.can_send() {
                    return Ok(ToolOutput::new(
                        "Send is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         Open Gmail to send the draft manually.\n\
                         To enable sending, rerun `jcode login google --google-access-tier full`.",
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
                if !self.client.can_delete() {
                    return Ok(ToolOutput::new(
                        "Trash is not available. Your Gmail access is configured as Read & Draft Only (API-level restriction).\n\
                         To enable delete, rerun `jcode login google --google-access-tier full`.",
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
                "Unknown gmail action: '{}'. Valid actions: search, read, list, draft, update_draft, list_drafts, delete_draft, send, send_draft, threads, thread, labels, trash, modify_labels",
                other
            ))),
        }
    }
}

/// Resolved fields for a draft update after merging caller input over the
/// draft's current content.
#[derive(Debug, PartialEq)]
struct MergedDraft {
    to: Option<String>,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
}

fn merge_draft_fields(
    current: Option<&gmail::Message>,
    to: Option<&str>,
    subject: Option<&str>,
    body: Option<&str>,
) -> MergedDraft {
    MergedDraft {
        to: to
            .map(str::to_string)
            .or_else(|| current.and_then(|m| m.header("To")).map(str::to_string)),
        subject: subject
            .map(str::to_string)
            .or_else(|| current.and_then(|m| m.subject()).map(str::to_string))
            .unwrap_or_default(),
        body: body
            .map(str::to_string)
            .or_else(|| current.and_then(|m| m.body_text()))
            .unwrap_or_default(),
        in_reply_to: current
            .and_then(|m| m.header("In-Reply-To"))
            .map(str::to_string),
    }
}

#[cfg(test)]
mod draft_merge_tests {
    use super::*;

    fn draft_message() -> gmail::Message {
        serde_json::from_value(json!({
            "id": "m1",
            "threadId": "t1",
            "payload": {
                "headers": [
                    {"name": "To", "value": "richard@varrock.vc"},
                    {"name": "Subject", "value": "Re: Intro"},
                    {"name": "In-Reply-To", "value": "<abc@mail.gmail.com>"}
                ],
                "mimeType": "text/plain",
                "body": {"data": "SGVsbG8"}
            }
        }))
        .unwrap()
    }

    #[test]
    fn omitted_fields_keep_current_values_and_threading() {
        let msg = draft_message();
        let merged = merge_draft_fields(Some(&msg), None, None, Some("New body"));
        assert_eq!(merged.to.as_deref(), Some("richard@varrock.vc"));
        assert_eq!(merged.subject, "Re: Intro");
        assert_eq!(merged.body, "New body");
        assert_eq!(merged.in_reply_to.as_deref(), Some("<abc@mail.gmail.com>"));
    }

    #[test]
    fn explicit_fields_override() {
        let msg = draft_message();
        let merged = merge_draft_fields(Some(&msg), Some("a@b.c"), Some("Hi"), None);
        assert_eq!(merged.to.as_deref(), Some("a@b.c"));
        assert_eq!(merged.subject, "Hi");
        assert_eq!(merged.body, "Hello");
    }

    #[test]
    fn no_current_message() {
        let merged = merge_draft_fields(None, None, None, None);
        assert_eq!(merged.to, None);
        assert_eq!(merged.subject, "");
    }
}
