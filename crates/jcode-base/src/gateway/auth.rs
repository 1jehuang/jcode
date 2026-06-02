#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WsAuth {
    pub(super) token: String,
    pub(super) source: WsAuthSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WsAuthSource {
    Header,
    Query,
}

#[expect(
    clippy::result_large_err,
    reason = "Tungstenite handshake APIs require returning ErrorResponse directly"
)]
pub(super) fn extract_ws_auth(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
) -> std::result::Result<WsAuth, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse> {
    let header_token = match request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        Some(auth) => Some(parse_bearer_token(auth).ok_or_else(|| {
            ws_error_response(
                401,
                "Unauthorized",
                "Authorization must be 'Bearer <token>'",
            )
        })?),
        None => None,
    };
    let query_token = request.uri().query().and_then(parse_query_token);

    let (token, source) = match (header_token, query_token) {
        (Some(header), Some(query)) if header != query => {
            return Err(ws_error_response(
                401,
                "Unauthorized",
                "Conflicting auth token sources",
            ));
        }
        (Some(header), _) => (header, WsAuthSource::Header),
        (None, Some(query)) => (query, WsAuthSource::Query),
        (None, None) => {
            return Err(ws_error_response(
                401,
                "Unauthorized",
                "Missing Authorization header or token query parameter",
            ));
        }
    };

    if !is_valid_hex_token(token) {
        return Err(ws_error_response(
            401,
            "Unauthorized",
            "Malformed auth token",
        ));
    }

    Ok(WsAuth {
        token: token.to_string(),
        source,
    })
}

pub(crate) fn parse_bearer_token(header_value: &str) -> Option<&str> {
    let mut parts = header_value.split_whitespace();
    let scheme = parts.next()?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if token.is_empty() {
        return None;
    }
    Some(token)
}

pub(crate) fn parse_query_token(query: &str) -> Option<&str> {
    for param in query.split('&') {
        if let Some(token) = param.strip_prefix("token=")
            && !token.is_empty()
        {
            return Some(token);
        }
    }
    None
}

pub(crate) fn is_valid_hex_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn ws_error_response(
    status: u16,
    reason: &str,
    body: &str,
) -> tokio_tungstenite::tungstenite::handshake::server::ErrorResponse {
    let primary = tokio_tungstenite::tungstenite::http::Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Connection", "close")
        .body(Some(format!("{}\n", body)));
    if let Ok(response) = primary {
        return response;
    }

    let fallback = tokio_tungstenite::tungstenite::http::Response::builder()
        .status(500)
        .body(Some(format!("{}\n", reason)));
    if let Ok(response) = fallback {
        return response;
    }

    let mut response =
        tokio_tungstenite::tungstenite::http::Response::new(Some(format!("{}\n", reason)));
    *response.status_mut() =
        tokio_tungstenite::tungstenite::http::StatusCode::INTERNAL_SERVER_ERROR;
    response
}

// ---------------------------------------------------------------------------
// HTTP handler for POST /pair and GET /health
// ---------------------------------------------------------------------------
