use super::*;

pub fn openai_auth_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    openai_auth_url_with_prompt(redirect_uri, challenge, state, None)
}

pub fn openai_auth_url_with_prompt(
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    prompt: Option<&str>,
) -> String {
    let prompt_param = prompt
        .map(|p| format!("&prompt={}", urlencoding::encode(p)))
        .unwrap_or_default();
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs{}",
        openai::AUTHORIZE_URL,
        openai::CLIENT_ID,
        urlencoding::encode(redirect_uri),
        urlencoding::encode(openai::SCOPES),
        challenge,
        state,
        prompt_param
    )
}

pub fn callback_listener_available(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .map(|listener| {
            drop(listener);
            true
        })
        .unwrap_or(false)
}

pub(super) async fn exchange_openai_code_at_url(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let client = crate::provider::shared_http_client();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
            openai::CLIENT_ID,
            code,
            verifier,
            urlencoding::encode(redirect_uri)
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token exchange failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        id_token: Option<String>,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: tokens.id_token,
        scopes: Vec::new(),
    })
}

pub async fn exchange_openai_code(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    exchange_openai_code_at_url(openai::TOKEN_URL, code, verifier, redirect_uri).await
}

pub async fn exchange_openai_callback_input(
    verifier: &str,
    input: &str,
    expected_state: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens> {
    let (code, callback_state) = parse_callback_input_with_state(input)?;
    if callback_state != expected_state {
        anyhow::bail!("OAuth state mismatch. Start login again and use the latest callback URL.");
    }
    exchange_openai_code(&code, verifier, redirect_uri).await
}

pub async fn start_openai_device_login() -> Result<OpenAiDeviceCode> {
    start_openai_device_login_at(openai::AUTH_BASE).await
}

pub async fn start_openai_device_login_at(base: &str) -> Result<OpenAiDeviceCode> {
    #[derive(Deserialize)]
    struct UserCodeResponse {
        device_auth_id: String,
        #[serde(alias = "usercode")]
        user_code: String,
        #[serde(default)]
        interval: Option<serde_json::Value>,
        #[serde(default)]
        expires_at: Option<String>,
    }
    let resp = crate::provider::shared_http_client()
        .post(format!("{base}/api/accounts/deviceauth/usercode"))
        .json(&OpenAiDeviceStartRequest {
            client_id: openai::CLIENT_ID,
        })
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        if status.as_u16() == 404 {
            anyhow::bail!(
                "OpenAI device-code login is not enabled for this account. Turn on device code authorization in ChatGPT security settings, then try again."
            );
        }
        anyhow::bail!("OpenAI device-code request failed ({status}): {text}");
    }
    let body: UserCodeResponse = resp.json().await?;
    // The server sends `interval` as a string ("5"); accept a number too.
    let interval = match body.interval {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => match s.trim().parse() {
            Ok(interval) => Some(interval),
            Err(error) => {
                crate::logging::debug(&format!("Invalid device-code polling interval: {error}"));
                None
            }
        },
        _ => None,
    }
    .unwrap_or(5)
    .clamp(1, 60);
    let expires_at_ms = body
        .expires_at
        .as_deref()
        .and_then(|s| match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(expires_at) => Some(expires_at),
            Err(error) => {
                crate::logging::debug(&format!("Invalid device-code expiry: {error}"));
                None
            }
        })
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() + 15 * 60 * 1000);
    Ok(OpenAiDeviceCode {
        device_auth_id: body.device_auth_id,
        user_code: body.user_code,
        interval,
        verification_url: openai::DEVICE_VERIFICATION_URL.to_string(),
        expires_at_ms,
    })
}

/// Poll once. `Ok(None)` while the user has not approved the code yet.
pub async fn poll_openai_device_login_at(
    base: &str,
    device_auth_id: &str,
    user_code: &str,
) -> Result<Option<(String, String)>> {
    #[derive(Deserialize)]
    struct TokenResponse {
        authorization_code: String,
        code_verifier: String,
    }
    let resp = crate::provider::shared_http_client()
        .post(format!("{base}/api/accounts/deviceauth/token"))
        .json(&OpenAiDevicePollRequest {
            device_auth_id,
            user_code,
        })
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    let status = resp.status();
    // Pending approval is reported as 403 or 404.
    if status.as_u16() == 403 || status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI device-code login failed ({status}): {text}");
    }
    let body: TokenResponse = resp.json().await?;
    Ok(Some((body.authorization_code, body.code_verifier)))
}

/// Wait for approval and exchange the code for tokens.
pub async fn finish_openai_device_login(device: &OpenAiDeviceCode) -> Result<OAuthTokens> {
    finish_openai_device_login_at(openai::AUTH_BASE, openai::TOKEN_URL, device).await
}

pub async fn finish_openai_device_login_at(
    base: &str,
    token_url: &str,
    device: &OpenAiDeviceCode,
) -> Result<OAuthTokens> {
    loop {
        if let Some((code, verifier)) =
            poll_openai_device_login_at(base, &device.device_auth_id, &device.user_code).await?
        {
            return exchange_openai_code_at_url(
                token_url,
                &code,
                &verifier,
                openai::DEVICE_REDIRECT_URI,
            )
            .await;
        }
        if chrono::Utc::now().timestamp_millis() >= device.expires_at_ms {
            anyhow::bail!("The OpenAI sign-in code expired. Start sign-in again.");
        }
        tokio::time::sleep(Duration::from_secs(device.interval)).await;
    }
}

/// Perform OAuth login for OpenAI/Codex
pub async fn login_openai(no_browser: bool) -> Result<OAuthTokens> {
    let (verifier, challenge) = generate_pkce();
    let state = generate_state();

    let port = openai::DEFAULT_PORT;
    let redirect_uri = openai::redirect_uri(port);
    let auth_url = openai_auth_url_with_prompt(&redirect_uri, &challenge, &state, Some("login"));

    eprintln!("\nOpen this URL in your browser:\n");
    eprintln!("{}\n", auth_url);
    if let Some(qr) = crate::auth::login_ui::qr_section(
        &auth_url,
        "Scan this QR on another device if this machine has no browser:",
        "    ",
        crate::auth::browser_suppressed(no_browser),
    ) {
        eprintln!("{qr}\n");
    }

    let callback_listener = bind_callback_listener(port).ok();
    let browser_opened = if crate::auth::browser_suppressed(no_browser) {
        false
    } else {
        crate::companion_client::open_url(&auth_url)
    };

    if browser_opened {
        if let Some(listener) = callback_listener {
            eprintln!(
                "Waiting up to 300s for automatic callback on {}",
                redirect_uri
            );
            match tokio::time::timeout(
                std::time::Duration::from_secs(300),
                wait_for_callback_async_on_listener(listener, &state),
            )
            .await
            {
                Ok(Ok(code)) => return exchange_openai_code(&code, &verifier, &redirect_uri).await,
                Ok(Err(err)) => {
                    eprintln!("Automatic callback failed ({err}). Falling back to manual paste.");
                }
                Err(_) => {
                    eprintln!("Timed out waiting for callback. Falling back to manual paste.");
                }
            }
        } else {
            eprintln!(
                "Local callback port {} is unavailable. Finish login in any browser, then paste the full callback URL here.\n",
                port
            );
        }
    } else if !browser_opened {
        eprintln!(
            "Couldn't open a browser on this machine. Use the QR code above, then paste the full callback URL here.\n"
        );
    }

    eprintln!("Paste the full callback URL (or query string) here:\n");
    eprint!("> ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("No callback URL entered.");
    }
    exchange_openai_callback_input(&verifier, trimmed, &state, &redirect_uri).await
}
