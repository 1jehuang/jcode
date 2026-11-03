use super::*;

pub(super) fn bad_request_response(message: &str) -> String {
    let body = format!(
        "<html><body><h1>Authentication not completed</h1><p>{}</p><p>You can close this tab and return to jcode.</p></body></html>",
        message
    );
    format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

pub(super) fn is_socket_read_timeout(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

pub(super) fn read_http_request_line_blocking<R: BufRead>(
    reader: &mut R,
) -> Result<Option<String>> {
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(request_line)),
        Err(err) if is_socket_read_timeout(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn drain_http_headers_blocking<R: BufRead>(reader: &mut R) -> Result<bool> {
    let mut header_line = String::new();
    loop {
        header_line.clear();
        match reader.read_line(&mut header_line) {
            Ok(0) => return Ok(false),
            Ok(_) if header_line.trim().is_empty() => return Ok(true),
            Ok(_) => {}
            Err(err) if is_socket_read_timeout(&err) => return Ok(false),
            Err(err) => return Err(err.into()),
        }
    }
}

pub(super) async fn read_http_request_line_async<R>(
    reader: &mut tokio::io::BufReader<R>,
) -> Result<Option<String>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut request_line = String::new();
    match tokio::time::timeout(
        Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS),
        tokio::io::AsyncBufReadExt::read_line(reader, &mut request_line),
    )
    .await
    {
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(request_line)),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Ok(None),
    }
}

pub(super) async fn drain_http_headers_async<R>(
    reader: &mut tokio::io::BufReader<R>,
) -> Result<bool>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut header_line = String::new();
    loop {
        header_line.clear();
        match tokio::time::timeout(
            Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS),
            tokio::io::AsyncBufReadExt::read_line(reader, &mut header_line),
        )
        .await
        {
            Ok(Ok(0)) => return Ok(false),
            Ok(Ok(_)) if header_line.trim().is_empty() => return Ok(true),
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(err.into()),
            Err(_) => return Ok(false),
        }
    }
}

/// Start local server and wait for OAuth callback
pub fn wait_for_callback(port: u16, expected_state: &str) -> Result<String> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    eprintln!("Waiting for OAuth callback on port {}...", port);

    loop {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(CALLBACK_READ_TIMEOUT_SECS)))?;
        let mut reader = BufReader::new(&stream);
        let Some(request_line) = read_http_request_line_blocking(&mut reader)? else {
            continue;
        };
        if !drain_http_headers_blocking(&mut reader)? {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            let _ = stream.write_all(bad_request_response("Invalid HTTP request.").as_bytes());
            continue;
        }

        let path = parts[1];
        let url = match url::Url::parse(&format!("http://localhost{}", path)) {
            Ok(url) => url,
            Err(_) => {
                let _ = stream.write_all(
                    bad_request_response("Could not parse OAuth callback URL.").as_bytes(),
                );
                continue;
            }
        };

        if let Some(error) = url
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.to_string())
        {
            let _ = stream.write_all(
                bad_request_response("Authentication was denied or cancelled.").as_bytes(),
            );
            anyhow::bail!("OAuth provider returned error: {}", error);
        }

        let code = match url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
        {
            Some(code) => code,
            None => {
                let _ = stream.write_all(
                    bad_request_response("No authorization code was included in this request.")
                        .as_bytes(),
                );
                continue;
            }
        };

        let state = match url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string())
        {
            Some(state) => state,
            None => {
                let _ = stream.write_all(
                    bad_request_response("No OAuth state was included in this request.").as_bytes(),
                );
                continue;
            }
        };

        if state != expected_state {
            let _ = stream.write_all(
                bad_request_response("OAuth state mismatch. Please retry the latest login flow.")
                    .as_bytes(),
            );
            continue;
        }

        let body = "<html><body><h1>Success!</h1><p>You can close this window.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())?;

        return Ok(code);
    }
}

/// Async version of wait_for_callback using tokio (for use from TUI context)
pub async fn wait_for_callback_async(port: u16, expected_state: &str) -> Result<String> {
    let listener = bind_callback_listener(port)?;
    wait_for_callback_async_on_listener(listener, expected_state).await
}

pub fn bind_callback_listener(port: u16) -> Result<tokio::net::TcpListener> {
    let std_listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}"))?;
    std_listener.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(std_listener)?)
}

pub async fn wait_for_callback_async_on_listener(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String> {
    let expected_state = expected_state.to_string();

    use tokio::io::{AsyncWriteExt, BufReader};

    loop {
        let (stream, _) = listener.accept().await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let Some(request_line) = read_http_request_line_async(&mut reader).await? else {
            continue;
        };
        if !drain_http_headers_async(&mut reader).await? {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            let _ = writer
                .write_all(bad_request_response("Invalid HTTP request.").as_bytes())
                .await;
            continue;
        }

        let path = parts[1];
        let url = match url::Url::parse(&format!("http://localhost{}", path)) {
            Ok(url) => url,
            Err(_) => {
                let _ = writer
                    .write_all(
                        bad_request_response("Could not parse OAuth callback URL.").as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        if let Some(error) = url
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.to_string())
        {
            let _ = writer
                .write_all(
                    bad_request_response("Authentication was denied or cancelled.").as_bytes(),
                )
                .await;
            anyhow::bail!("OAuth provider returned error: {}", error);
        }

        let code = match url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
        {
            Some(code) => code,
            None => {
                let _ = writer
                    .write_all(
                        bad_request_response("No authorization code was included in this request.")
                            .as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        let state = match url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string())
        {
            Some(state) => state,
            None => {
                let _ = writer
                    .write_all(
                        bad_request_response("No OAuth state was included in this request.")
                            .as_bytes(),
                    )
                    .await;
                continue;
            }
        };

        if state != expected_state {
            let _ = writer
                .write_all(
                    bad_request_response(
                        "OAuth state mismatch. Please retry the latest login flow.",
                    )
                    .as_bytes(),
                )
                .await;
            continue;
        }

        let body = "<html><body><h1>Success!</h1><p>You can close this window and return to jcode.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        writer.write_all(response.as_bytes()).await?;

        return Ok(code);
    }
}
