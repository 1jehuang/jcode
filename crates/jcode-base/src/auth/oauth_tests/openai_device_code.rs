//! OpenAI device-code login against a loopback server that speaks the real
//! auth.openai.com schema (string `interval`, 403 while pending). Dummy values only.
use super::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, String) {
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).await.unwrap();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.unwrap();
    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
    (path, String::from_utf8(body).unwrap())
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn device_code_flow_polls_until_approved_then_exchanges() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut polls = 0;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (path, body) = read_request(&mut stream).await;
            seen.push((path.clone(), body.clone()));
            let (status, reply) = match path.as_str() {
                "/api/accounts/deviceauth/usercode" => (
                    200,
                    r#"{"device_auth_id":"dev-1","user_code":"ABCD-1234","interval":"1","expires_at":"2999-01-01T00:00:00+00:00"}"#.to_string(),
                ),
                "/api/accounts/deviceauth/token" => {
                    polls += 1;
                    if polls == 1 {
                        (403, r#"{"error":"authorization_pending"}"#.to_string())
                    } else {
                        (200, r#"{"authorization_code":"code-1","code_verifier":"ver-1"}"#.to_string())
                    }
                }
                "/oauth/token" => (
                    200,
                    r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,"id_token":"idt"}"#.to_string(),
                ),
                _ => (500, "{}".to_string()),
            };
            stream
                .write_all(http_response(status, &reply).as_bytes())
                .await
                .unwrap();
            if path == "/oauth/token" {
                return seen;
            }
        }
    });

    let device = start_openai_device_login_at(&base).await.unwrap();
    assert_eq!(device.user_code, "ABCD-1234");
    assert_eq!(device.interval, 1, "string interval is parsed");
    assert_eq!(device.verification_url, openai::DEVICE_VERIFICATION_URL);
    let tokens = finish_openai_device_login_at(&base, &format!("{base}/oauth/token"), &device)
        .await
        .unwrap();
    assert_eq!(tokens.access_token, "at");
    assert_eq!(tokens.refresh_token, "rt");

    let seen = server.await.unwrap();
    assert!(seen[0].1.contains(openai::CLIENT_ID));
    assert!(seen[1].1.contains("\"device_auth_id\":\"dev-1\""));
    let exchange = &seen.last().unwrap().1;
    assert!(exchange.contains("code=code-1"));
    assert!(exchange.contains("code_verifier=ver-1"));
    assert!(exchange.contains(&*urlencoding::encode(openai::DEVICE_REDIRECT_URI)));
}

#[tokio::test]
async fn device_code_disabled_account_gets_actionable_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(http_response(404, "{}").as_bytes())
            .await
            .unwrap();
    });
    let err = start_openai_device_login_at(&base).await.unwrap_err();
    assert!(
        err.to_string().contains("ChatGPT security settings"),
        "{err}"
    );
}
