use crate::logging;
use serde::Deserialize;

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
    pub date: i64,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

pub async fn send_message(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: &str,
    text: &str,
) -> anyhow::Result<()> {
    let url = format!("{}{}/sendMessage", API_BASE, bot_token);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
            "disable_web_page_preview": true,
        }))
        .send()
        .await?;

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
    let mut params = serde_json::json!({
        "timeout": timeout_secs,
        "allowed_updates": ["message"],
    });

    if let Some(off) = offset {
        params["offset"] = serde_json::json!(off);
    }

    let resp = client
        .post(&url)
        .json(&params)
        .timeout(std::time::Duration::from_secs(timeout_secs + 5))
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
