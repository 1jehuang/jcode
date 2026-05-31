use super::*;
use std::ffi::OsString;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

async fn mock_token_server(
    status: u16,
    response_body: &str,
) -> (
    u16,
    tokio::task::JoinHandle<(
        String,
        String,
        std::collections::HashMap<String, String>,
        String,
    )>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let resp_body = response_body.to_string();

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        let mut request_line = String::new();
        reader.read_line(&mut request_line).await.unwrap();
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        let method = parts.first().unwrap_or(&"").to_string();
        let path = parts.get(1).unwrap_or(&"").to_string();

        let mut headers = std::collections::HashMap::new();
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                let k = key.trim().to_lowercase();
                let v = value.trim().to_string();
                if k == "content-length" {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.insert(k, v);
            }
        }

        let mut body_bytes = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body_bytes).await.unwrap();
        }
        let body = String::from_utf8(body_bytes).unwrap_or_default();

        let response = format!(
            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status,
            resp_body.len(),
            resp_body
        );
        writer.write_all(response.as_bytes()).await.unwrap();

        (method, path, headers, body)
    });

    (port, handle)
}

// ========================
// REGRESSION: Content-Type must be form-urlencoded, not JSON
// ========================

mod basic;
mod flow;
