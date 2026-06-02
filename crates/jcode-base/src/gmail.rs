use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::auth::google;

const GMAIL_API_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

pub struct GmailClient {
    http: reqwest::Client,
}

impl Default for GmailClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GmailClient {
    pub fn new() -> Self {
        Self {
            http: crate::provider::shared_http_client(),
        }
    }

    async fn token(&self) -> Result<String> {
        google::get_valid_token().await
    }

    pub async fn list_messages(
        &self,
        query: Option<&str>,
        label_ids: Option<&[&str]>,
        max_results: u32,
    ) -> Result<MessageList> {
        let token = self.token().await?;
        let mut url = format!("{}/messages?maxResults={}", GMAIL_API_BASE, max_results);

        if let Some(q) = query {
            url.push_str(&format!("&q={}", urlencoding::encode(q)));
        }
        if let Some(labels) = label_ids {
            for label in labels {
                url.push_str(&format!("&labelIds={}", label));
            }
        }

        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;
        let list: MessageList = resp.json().await?;
        Ok(list)
    }

    pub async fn get_message(&self, id: &str, format: MessageFormat) -> Result<Message> {
        let token = self.token().await?;
        let url = format!(
            "{}/messages/{}?format={}",
            GMAIL_API_BASE,
            id,
            format.as_str()
        );
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;
        let msg: Message = resp.json().await?;
        Ok(msg)
    }

    pub async fn list_threads(&self, query: Option<&str>, max_results: u32) -> Result<ThreadList> {
        let token = self.token().await?;
        let mut url = format!("{}/threads?maxResults={}", GMAIL_API_BASE, max_results);

        if let Some(q) = query {
            url.push_str(&format!("&q={}", urlencoding::encode(q)));
        }

        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;
        let list: ThreadList = resp.json().await?;
        Ok(list)
    }

    pub async fn get_thread(&self, id: &str) -> Result<Thread> {
        let token = self.token().await?;
        let url = format!("{}/threads/{}?format=metadata", GMAIL_API_BASE, id);
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;
        let thread: Thread = resp.json().await?;
        Ok(thread)
    }

    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        let token = self.token().await?;
        let url = format!("{}/labels", GMAIL_API_BASE);
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;

        #[derive(Deserialize)]
        struct LabelList {
            labels: Option<Vec<Label>>,
        }

        let list: LabelList = resp.json().await?;
        Ok(list.labels.unwrap_or_default())
    }

    pub async fn create_draft(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        in_reply_to: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<Draft> {
        let token = self.token().await?;
        let url = format!("{}/drafts", GMAIL_API_BASE);

        let mut headers = format!(
            "To: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=utf-8\r\n",
            to, subject
        );
        if let Some(reply_to) = in_reply_to {
            headers.push_str(&format!(
                "In-Reply-To: {}\r\nReferences: {}\r\n",
                reply_to, reply_to
            ));
        }

        let raw = format!("{}\r\n{}", headers, body);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes());

        let mut message = serde_json::json!({ "raw": encoded });
        if let Some(tid) = thread_id {
            message["threadId"] = serde_json::Value::String(tid.to_string());
        }

        let payload = serde_json::json!({ "message": message });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await?;
        handle_error(&resp).await?;
        let draft: Draft = resp.json().await?;
        Ok(draft)
    }

    pub async fn send_draft(&self, draft_id: &str) -> Result<Message> {
        let token = self.token().await?;
        let url = format!("{}/drafts/send", GMAIL_API_BASE);
        let payload = serde_json::json!({ "id": draft_id });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await?;
        handle_error(&resp).await?;
        let msg: Message = resp.json().await?;
        Ok(msg)
    }

    pub async fn send_message(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        in_reply_to: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<Message> {
        let token = self.token().await?;
        let url = format!("{}/messages/send", GMAIL_API_BASE);

        let mut headers = format!(
            "To: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=utf-8\r\n",
            to, subject
        );
        if let Some(reply_to) = in_reply_to {
            headers.push_str(&format!(
                "In-Reply-To: {}\r\nReferences: {}\r\n",
                reply_to, reply_to
            ));
        }

        let raw = format!("{}\r\n{}", headers, body);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes());

        let mut message = serde_json::json!({ "raw": encoded });
        if let Some(tid) = thread_id {
            message["threadId"] = serde_json::Value::String(tid.to_string());
        }

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&message)
            .send()
            .await?;
        handle_error(&resp).await?;
        let msg: Message = resp.json().await?;
        Ok(msg)
    }

    pub async fn trash_message(&self, id: &str) -> Result<()> {
        let token = self.token().await?;
        let url = format!("{}/messages/{}/trash", GMAIL_API_BASE, id);
        let resp = self.http.post(&url).bearer_auth(&token).send().await?;
        handle_error(&resp).await?;
        Ok(())
    }

    pub async fn modify_labels(
        &self,
        id: &str,
        add_labels: &[&str],
        remove_labels: &[&str],
    ) -> Result<()> {
        let token = self.token().await?;
        let url = format!("{}/messages/{}/modify", GMAIL_API_BASE, id);
        let payload = serde_json::json!({
            "addLabelIds": add_labels,
            "removeLabelIds": remove_labels,
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await?;
        handle_error(&resp).await?;
        Ok(())
    }
}

async fn handle_error(resp: &reqwest::Response) -> Result<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "Gmail API error {}: check token permissions",
        resp.status()
    ))
}

use base64::Engine;

#[derive(Debug, Clone, Copy)]
pub enum MessageFormat {
    Full,
    Metadata,
}

impl MessageFormat {
    fn as_str(&self) -> &'static str {
        match self {
            MessageFormat::Full => "full",
            MessageFormat::Metadata => "metadata",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageList {
    pub messages: Option<Vec<MessageRef>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    pub result_size_estimate: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageRef {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: Option<String>,
    #[serde(rename = "labelIds")]
    pub label_ids: Option<Vec<String>>,
    pub snippet: Option<String>,
    pub payload: Option<MessagePayload>,
    #[serde(rename = "internalDate")]
    pub internal_date: Option<String>,
    #[serde(rename = "sizeEstimate")]
    pub size_estimate: Option<u32>,
}

impl Message {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload.as_ref().and_then(|p| {
            p.headers.as_ref().and_then(|headers| {
                headers
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case(name))
                    .map(|h| h.value.as_str())
            })
        })
    }

    pub fn subject(&self) -> Option<&str> {
        self.header("Subject")
    }

    pub fn from(&self) -> Option<&str> {
        self.header("From")
    }

    pub fn date(&self) -> Option<&str> {
        self.header("Date")
    }

    pub fn body_text(&self) -> Option<String> {
        self.payload.as_ref().and_then(|p| p.extract_text())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessagePayload {
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub headers: Option<Vec<Header>>,
    pub body: Option<MessageBody>,
    pub parts: Option<Vec<MessagePayload>>,
}

impl MessagePayload {
    #[expect(
        clippy::collapsible_if,
        reason = "Nested MIME/body decoding is kept explicit for readability"
    )]
    fn extract_text(&self) -> Option<String> {
        if let Some(ref mime) = self.mime_type {
            if mime == "text/plain" {
                if let Some(ref body) = self.body {
                    if let Some(ref data) = body.data {
                        if let Ok(bytes) =
                            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
                        {
                            return String::from_utf8(bytes).ok();
                        }
                        if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE.decode(data) {
                            return String::from_utf8(bytes).ok();
                        }
                    }
                }
            }
        }

        if let Some(ref parts) = self.parts {
            for part in parts {
                if let Some(text) = part.extract_text() {
                    return Some(text);
                }
            }
        }

        None
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MessageBody {
    pub size: Option<u32>,
    pub data: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadList {
    pub threads: Option<Vec<ThreadRef>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    pub result_size_estimate: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadRef {
    pub id: String,
    pub snippet: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Thread {
    pub id: String,
    pub messages: Option<Vec<Message>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub label_type: Option<String>,
    #[serde(rename = "messagesTotal")]
    pub messages_total: Option<u32>,
    #[serde(rename = "messagesUnread")]
    pub messages_unread: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Draft {
    pub id: String,
    pub message: Option<MessageRef>,
}

pub fn format_message_summary(msg: &Message) -> String {
    let from = msg.from().unwrap_or("(unknown)");
    let subject = msg.subject().unwrap_or("(no subject)");
    let date = msg.date().unwrap_or("");
    let snippet = msg.snippet.as_deref().unwrap_or("");
    let labels = msg
        .label_ids
        .as_ref()
        .map(|l| l.join(", "))
        .unwrap_or_default();

    format!(
        "From: {}\nSubject: {}\nDate: {}\nLabels: {}\nSnippet: {}\nID: {}",
        from, subject, date, labels, snippet, msg.id
    )
}

pub fn format_message_full(msg: &Message) -> String {
    let mut out = format_message_summary(msg);
    if let Some(body) = msg.body_text() {
        out.push_str("\n\n--- Body ---\n");
        out.push_str(&body);
    }
    out
}
