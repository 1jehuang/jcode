// wasm-bindgen bindings for CarpAI SDK with full HTTP client

use wasm_bindgen::prelude::*;
use js_sys::Promise;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response, console};

use crate::types::ChatMessage;
use crate::session_api::SessionResponse;

/// Initialize the SDK (called from JavaScript)
#[wasm_bindgen(start)]
pub fn init() {
    console::log_1(&"[carpai-sdk] Initialized".into());
}

/// Get SDK version
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Send a chat completion request to CarpAI server
#[wasm_bindgen]
pub async fn chat_completion(
    server_url: &str,
    api_key: &str,
    messages_json: &str,
    model: Option<String>,
) -> Result<String, JsValue> {
    console::log_1(&"[carpai-sdk] Sending chat completion".into());

    let body = serde_json::json!({
        "messages": serde_json::from_str::<Vec<ChatMessage>>(messages_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid messages: {}", e)))?,
        "model": model.unwrap_or_else(|| "claude-sonnet-4".to_string()),
        "stream": false
    });

    fetch_post(&format!("{}/v1/chat/completions", server_url), &body.to_string(), api_key).await
}

/// Create a new session
#[wasm_bindgen]
pub async fn create_session(
    server_url: &str,
    api_key: &str,
    title: Option<String>,
) -> Result<String, JsValue> {
    console::log_1(&"[carpai-sdk] Creating session".into());

    let body = serde_json::json!({
        "title": title.unwrap_or_else(|| "New Session".to_string())
    });

    fetch_post(&format!("{}/v1/sessions", server_url), &body.to_string(), api_key).await
}

/// Append a message to session
#[wasm_bindgen]
pub async fn append_message(
    server_url: &str,
    api_key: &str,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<String, JsValue> {
    let body = serde_json::json!({
        "message": { "role": role, "content": content }
    });

    fetch_post(
        &format!("{}/v1/sessions/{}/messages", server_url, session_id),
        &body.to_string(),
        api_key,
    ).await
}

/// GET session messages
#[wasm_bindgen]
pub async fn get_messages(
    server_url: &str,
    api_key: &str,
    session_id: &str,
) -> Result<String, JsValue> {
    fetch_get(&format!("{}/v1/sessions/{}/messages", server_url, session_id), api_key).await
}

// === HTTP Helpers using web-sys Fetch API ===

async fn fetch_post(url: &str, body: &str, api_key: &str) -> Result<String, JsValue> {
    let mut opts = RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(body)));
    opts.mode(RequestMode::Cors);

    let request = Request::new_with_str_and_init(url, &opts)?;
    let headers = request.headers();
    headers.set("Content-Type", "application/json")?;
    if !api_key.is_empty() {
        headers.set("Authorization", &format!("Bearer {}", api_key))?;
    }

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
    let resp: Response = JsFuture::from(window.fetch_with_request(&request))
        .await?
        .dyn_into()?;

    if !resp.ok() {
        return Err(JsValue::from_str(&format!("HTTP {}", resp.status())));
    }

    let text = JsFuture::from(resp.text()?).await?;
    Ok(text.as_string().unwrap_or_default())
}

async fn fetch_get(url: &str, api_key: &str) -> Result<String, JsValue> {
    let mut opts = RequestInit::new();
    opts.method("GET");
    opts.mode(RequestMode::Cors);

    let request = Request::new_with_str_and_init(url, &opts)?;
    if !api_key.is_empty() {
        request.headers().set("Authorization", &format!("Bearer {}", api_key))?;
    }

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
    let resp: Response = JsFuture::from(window.fetch_with_request(&request))
        .await?
        .dyn_into()?;

    if !resp.ok() {
        return Err(JsValue::from_str(&format!("HTTP {}", resp.status())));
    }

    let text = JsFuture::from(resp.text()?).await?;
    Ok(text.as_string().unwrap_or_default())
}
