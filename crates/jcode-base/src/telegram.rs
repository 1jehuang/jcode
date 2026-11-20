use crate::logging;
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.telegram.org/bot";

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub text: Option<String>,
    pub chat: Chat,
    #[serde(rename = "date")]
    pub _date: i64,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

// Sorted fields preserve the legacy serde_json map's exact wire representation.
#[derive(Serialize)]
struct MessageRequest<'a> {
    chat_id: &'a str,
    disable_web_page_preview: bool,
    parse_mode: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct UpdatesRequest {
    allowed_updates: [&'static str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    timeout: u64,
}

fn message_request(
    client: &reqwest::Client,
    url: &str,
    chat_id: &str,
    text: &str,
) -> reqwest::RequestBuilder {
    client.post(url).json(&MessageRequest {
        chat_id,
        disable_web_page_preview: true,
        parse_mode: "Markdown",
        text,
    })
}

fn updates_request(
    client: &reqwest::Client,
    url: &str,
    offset: Option<i64>,
    timeout_secs: u64,
) -> reqwest::RequestBuilder {
    client
        .post(url)
        .json(&UpdatesRequest {
            allowed_updates: ["message"],
            offset,
            timeout: timeout_secs,
        })
        .timeout(std::time::Duration::from_secs(timeout_secs + 5))
}

pub async fn send_message(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: &str,
    text: &str,
) -> anyhow::Result<()> {
    let url = format!("{}{}/sendMessage", API_BASE, bot_token);
    let resp = message_request(client, &url, chat_id, text).send().await?;

    let status = resp.status();
    let body: TelegramResponse<serde_json::Value> = resp.json().await?;

    if !body.ok {
        anyhow::bail!(
            "Telegram API error ({}): {}",
            status,
            body.description.unwrap_or_default()
        );
    }

    logging::info("Telegram notification sent");
    Ok(())
}

pub async fn get_updates(
    client: &reqwest::Client,
    bot_token: &str,
    offset: Option<i64>,
    timeout_secs: u64,
) -> anyhow::Result<Vec<Update>> {
    let url = format!("{}{}/getUpdates", API_BASE, bot_token);
    let resp = updates_request(client, &url, offset, timeout_secs)
        .send()
        .await?;

    let body: TelegramResponse<Vec<Update>> = resp.json().await?;

    if !body.ok {
        anyhow::bail!(
            "Telegram getUpdates error: {}",
            body.description.unwrap_or_default()
        );
    }

    Ok(body.result.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_message_request_preserves_legacy_bytes_after_inputs_drop() {
        let client = reqwest::Client::new();
        for text in [
            String::new(),
            "line\n\"quoted\" café 🦀".into(),
            "界🦀".repeat(8192),
        ] {
            let chat = String::from("-123456");
            let expected = serde_json::to_vec(&serde_json::json!({
                "chat_id": chat, "text": text, "parse_mode": "Markdown",
                "disable_web_page_preview": true
            }))
            .unwrap();
            let builder = message_request(&client, "http://127.0.0.1/unused", &chat, &text);
            drop(chat);
            drop(text);
            let request = builder.build().unwrap();
            assert_eq!(request.method(), reqwest::Method::POST);
            assert_eq!(request.url().as_str(), "http://127.0.0.1/unused");
            assert_eq!(
                request.headers()[reqwest::header::CONTENT_TYPE],
                "application/json"
            );
            assert_eq!(request.body().unwrap().as_bytes().unwrap(), expected);
        }
    }

    #[test]
    fn borrowed_updates_request_preserves_optional_offset_and_timeout() {
        let client = reqwest::Client::new();
        for offset in [None, Some(-1), Some(0), Some(i64::MAX)] {
            for timeout in [0, 1, 60] {
                let mut expected = serde_json::json!({
                    "timeout": timeout, "allowed_updates": ["message"]
                });
                if let Some(offset) = offset {
                    expected["offset"] = serde_json::json!(offset);
                }
                let request = updates_request(&client, "http://127.0.0.1/unused", offset, timeout)
                    .build()
                    .unwrap();
                assert_eq!(request.method(), reqwest::Method::POST);
                assert_eq!(
                    request.timeout(),
                    Some(&std::time::Duration::from_secs(timeout + 5))
                );
                assert_eq!(
                    request.body().unwrap().as_bytes().unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
            }
        }
    }

    #[test]
    fn test_parse_update() {
        let json = r#"{
            "update_id": 123,
            "message": {
                "text": "hello",
                "chat": {"id": 456},
                "date": 1700000000
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        assert_eq!(update.update_id, 123);
        assert_eq!(update.message.unwrap().text.unwrap(), "hello");
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{"ok": true, "result": []}"#;
        let resp: TelegramResponse<Vec<Update>> = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert!(resp.result.unwrap().is_empty());
    }
}
