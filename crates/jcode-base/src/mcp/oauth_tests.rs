use super::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// These tests serve real HTTP on ephemeral 127.0.0.1 sockets. No mocked client,
// copied OAuth implementation, external provider, browser executable or secrets.
struct Sandbox {
    _lock: crate::storage::TestEnvGuard,
    temp: tempfile::TempDir,
    saved: Option<std::ffi::OsString>,
}

impl Sandbox {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let saved = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        Self {
            _lock: lock,
            temp,
            saved,
        }
    }

    fn path(&self) -> PathBuf {
        self.temp.path().join(STORE_FILE)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(value) = &self.saved {
            crate::env::set_var("JCODE_HOME", value);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[derive(Clone, Default)]
struct Options {
    no_challenge: bool,
    oidc_append_only: bool,
    wrong_resource: bool,
    wrong_issuer: bool,
    plain_pkce: bool,
    metadata_redirect: bool,
    token_redirect: bool,
    token_error: bool,
    invalid_token: bool,
    no_new_refresh_token: bool,
    same_access_token: bool,
    no_metadata: bool,
    delay_refresh: bool,
    metadata_trailing_slash: bool,
}

#[derive(Default)]
struct State {
    requests: Vec<(String, String, String)>,
    registration_redirect: Option<String>,
    challenge: Option<String>,
    authorization_state: Option<String>,
    refresh_count: usize,
    code_exchanges: usize,
    accepted_refresh: String,
}

struct Fixture {
    origin: String,
    state: Arc<Mutex<State>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(options: Options) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            accepted_refresh: "refresh-0".to_string(),
            ..Default::default()
        }));
        let shared = state.clone();
        let base = origin.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let shared = shared.clone();
                        let base = base.clone();
                        let options = options.clone();
                        connections.spawn(async move {
                            serve(stream, &base, shared, options).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => (),
                }
            }
        });
        Self {
            origin,
            state,
            server,
        }
    }

    fn resource(&self) -> String {
        format!("{}/mcp", self.origin)
    }

    fn entry(&self, expired: bool) -> StoredOAuth {
        StoredOAuth {
            client_id: "test-public-client".into(),
            token_endpoint: format!("{}/token", self.origin),
            access_token: Some("access-0".into()),
            refresh_token: Some("refresh-0".into()),
            expires_at: Some(now_secs() + if expired { -100 } else { 3600 }),
            resource: Some(self.resource()),
            ..Default::default()
        }
    }

    fn seed(&self, expired: bool) {
        seed(&self.resource(), self.entry(expired));
    }
}

fn seed(resource: &str, entry: StoredOAuth) {
    let path = store_path().unwrap();
    let file = open_store_lock(&path).unwrap();
    file.try_lock().unwrap();
    let mut store = load_store_at(&path).unwrap();
    let key = canonical_resource(resource);
    store.servers.insert(key.clone(), entry);
    advance_generation(&mut store, &key).unwrap();
    save_store_at(&path, &store).unwrap();
}

async fn read_request(
    stream: &mut tokio::net::TcpStream,
) -> Option<(String, String, String, String)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_JSON_BYTES {
            return None;
        }
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = String::from_utf8(bytes[..end].to_vec()).ok()?;
    let mut parts = header.lines().next()?.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let length = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if length > MAX_JSON_BYTES {
        return None;
    }
    while bytes.len() < end + length {
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = String::from_utf8(bytes[end..end + length].to_vec()).ok()?;
    Some((method, target, header, body))
}

async fn serve(
    mut stream: tokio::net::TcpStream,
    origin: &str,
    shared: Arc<Mutex<State>>,
    options: Options,
) {
    let Some((method, target, headers, body)) = read_request(&mut stream).await else {
        return;
    };
    let response = {
        let mut state = shared.lock().unwrap();
        state
            .requests
            .push((method.clone(), target.clone(), body.clone()));
        route(
            origin, &method, &target, &headers, &body, &mut state, &options,
        )
    };
    if options.delay_refresh
        && method == "POST"
        && target == "/token"
        && body.contains("grant_type=refresh_token")
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn json_response(value: Value) -> String {
    response("200 OK", "", &value.to_string())
}

fn route(
    origin: &str,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
    state: &mut State,
    options: &Options,
) -> String {
    let url = url::Url::parse(&format!("{origin}{target}")).unwrap();
    let path = url.path();
    let resource = if options.metadata_trailing_slash {
        format!("{origin}/mcp/")
    } else {
        format!("{origin}/mcp")
    };
    let issuer = format!("{origin}/tenant");
    if path == "/mcp" && method == "POST" {
        if state.refresh_count > 0
            && headers.contains(&format!("Bearer access-{}", state.refresh_count))
        {
            return json_response(json!({"jsonrpc":"2.0", "id":1, "result":{}}));
        }
        let challenge = if options.no_challenge {
            String::new()
        } else {
            format!(
                "WWW-Authenticate: Basic realm=\"ignored\"\r\nWWW-Authenticate: Bearer realm=\"a,b\", resource_metadata = \"{origin}/resource\", scope=\"read\"\r\n"
            )
        };
        return response("401 Unauthorized", &challenge, "{}");
    }
    if path == "/resource" || path == "/.well-known/oauth-protected-resource" {
        if options.no_metadata {
            return response("404 Not Found", "", "{}");
        }
        if options.metadata_redirect {
            return response(
                "302 Found",
                &format!("Location: {origin}/credential-sink\r\n"),
                "{}",
            );
        }
        return json_response(json!({
            "resource": if options.wrong_resource { "https://other.example/mcp".to_string() } else { resource },
            "authorization_servers": [issuer],
            "scopes_supported": ["metadata-scope"],
        }));
    }
    if path == "/.well-known/oauth-authorization-server/tenant"
        || path == "/.well-known/openid-configuration/tenant"
        || path == "/tenant/.well-known/openid-configuration"
    {
        if options.oidc_append_only && path != "/tenant/.well-known/openid-configuration" {
            return response("404 Not Found", "", "{}");
        }
        return json_response(json!({
            "issuer": if options.wrong_issuer { format!("{origin}/attacker") } else { issuer },
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "registration_endpoint": format!("{origin}/register"),
            "scopes_supported": ["excessive-global-scope"],
            "code_challenge_methods_supported": [if options.plain_pkce { "plain" } else { "S256" }],
            "authorization_response_iss_parameter_supported": true,
            "token_endpoint_auth_methods_supported": ["none"],
        }));
    }
    if path == "/register" && method == "POST" {
        let value: Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["token_endpoint_auth_method"], "none");
        assert_eq!(
            value["grant_types"],
            json!(["authorization_code", "refresh_token"])
        );
        state.registration_redirect = value["redirect_uris"][0].as_str().map(str::to_owned);
        return json_response(
            json!({"client_id":"test-public-client", "token_endpoint_auth_method":"none"}),
        );
    }
    if path == "/authorize" && method == "GET" {
        let values: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(values.get("client_id").unwrap(), "test-public-client");
        assert_eq!(values.get("response_type").unwrap(), "code");
        assert_eq!(values.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(values.get("resource").unwrap(), &resource);
        assert_eq!(
            values.get("redirect_uri"),
            state.registration_redirect.as_ref()
        );
        state.challenge = values.get("code_challenge").cloned();
        state.authorization_state = values.get("state").cloned();
        let mut callback = url::Url::parse(values.get("redirect_uri").unwrap()).unwrap();
        callback
            .query_pairs_mut()
            .append_pair("code", "one-time-auth-code")
            .append_pair("state", values.get("state").unwrap())
            .append_pair("iss", &issuer);
        return response("303 See Other", &format!("Location: {callback}\r\n"), "{}");
    }
    if path == "/token" && method == "POST" {
        if options.token_redirect {
            return response(
                "307 Temporary Redirect",
                &format!("Location: {origin}/credential-sink?secret=REFLECTED-SECRET\r\n"),
                "REFLECTED-SECRET",
            );
        }
        if options.token_error {
            return response(
                "400 Bad Request",
                "",
                "{\"error_description\":\"REFLECTED-SECRET refresh-0 client-secret\"}",
            );
        }
        if options.invalid_token {
            return json_response(
                json!({"access_token":"REFLECTED-SECRET", "token_type":"unexpected"}),
            );
        }
        let values: BTreeMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(values.get("resource").unwrap(), &resource);
        assert_eq!(values.get("client_id").unwrap(), "test-public-client");
        if values.get("grant_type").map(String::as_str) == Some("authorization_code") {
            assert_eq!(values.get("code").unwrap(), "one-time-auth-code");
            assert_eq!(
                values.get("redirect_uri"),
                state.registration_redirect.as_ref()
            );
            let verifier = values.get("code_verifier").unwrap();
            assert_eq!(
                Some(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))),
                state.challenge
            );
            state.code_exchanges += 1;
            return json_response(
                json!({"access_token":"access-0", "refresh_token":"refresh-0", "token_type":"Bearer", "expires_in":3600, "scope":"read"}),
            );
        }
        assert_eq!(values.get("grant_type").unwrap(), "refresh_token");
        assert_eq!(
            values.get("refresh_token").unwrap(),
            &state.accepted_refresh
        );
        state.refresh_count += 1;
        let number = state.refresh_count;
        let access = if options.same_access_token {
            "access-0".to_string()
        } else {
            format!("access-{number}")
        };
        let mut token = json!({"access_token":access, "token_type":"bearer", "expires_in":3600});
        if !options.no_new_refresh_token {
            state.accepted_refresh = format!("refresh-{number}");
            token["refresh_token"] = json!(state.accepted_refresh);
        }
        return json_response(token);
    }
    if path == "/credential-sink" {
        panic!("OAuth followed a redirect with credentials");
    }
    response("404 Not Found", "", "{}")
}

async fn browser_login(auth_url: String, inject_bad_callbacks: bool) -> Result<()> {
    let url = url::Url::parse(&auth_url)?;
    let query: BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let callback = url::Url::parse(query.get("redirect_uri").unwrap())?;
    let client = http()?;
    if inject_bad_callbacks {
        let state = query.get("state").unwrap();
        for target in [
            "/callback?error=REFLECTED-SECRET&state=wrong".to_string(),
            format!("/callback?code=bad&state={state}&state=duplicate"),
            format!("/not-callback?code=bad&state={state}"),
            "/callback?code=bad".to_string(),
        ] {
            let mut bad = callback.clone();
            bad.set_path(target.split('?').next().unwrap());
            bad.set_query(target.split_once('?').map(|(_, query)| query));
            assert_eq!(
                client.get(bad).send().await?.status(),
                reqwest::StatusCode::BAD_REQUEST
            );
        }
        let mut bad = callback.clone();
        bad.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("code", "bad");
        assert_eq!(
            client.post(bad).send().await?.status(),
            reqwest::StatusCode::BAD_REQUEST
        );
    }
    let authorization = client.get(auth_url).send().await?;
    assert_eq!(authorization.status(), reqwest::StatusCode::SEE_OTHER);
    let location = authorization
        .headers()
        .get(reqwest::header::LOCATION)
        .unwrap()
        .to_str()?;
    assert_eq!(
        client.get(location).send().await?.status(),
        reqwest::StatusCode::OK
    );
    Ok(())
}

async fn do_login(fixture: &Fixture, inject_bad_callbacks: bool) -> Result<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let resource = fixture.resource();
    let login = login(&resource, BrowserMode::Print, move |auth_url| {
        let auth_url = auth_url.to_string();
        tokio::spawn(async move {
            let _ = sender.send(browser_login(auth_url, inject_bad_callbacks).await);
        });
    });
    tokio::time::timeout(Duration::from_secs(10), login).await??;
    receiver.await??;
    Ok(())
}

#[test]
fn canonical_resource_redacts_query_fragment_and_userinfo() {
    assert_eq!(
        canonical_resource("https://user:password@MCP.example.com/mcp/?token=secret#x"),
        "https://mcp.example.com/mcp"
    );
    assert_eq!(
        canonical_resource("https://MCP.example.com"),
        "https://mcp.example.com"
    );
    assert_eq!(canonical_resource("bad?secret=secret"), "");
    for opaque in ["mailto:user:secret@example.com", "data:text/plain,secret"] {
        assert_eq!(canonical_resource(opaque), "");
    }
}

#[test]
fn bearer_challenge_parser_respects_schemes_escaping_and_whitespace() {
    let header = r#"Basic scope="wrong", Bearer realm="a,b", RESOURCE_METADATA = "https://x.test/metadata", scope = "read write""#;
    assert_eq!(
        www_authenticate_param(header, "scope").as_deref(),
        Some("read write")
    );
    assert_eq!(
        www_authenticate_param(header, "resource_metadata").as_deref(),
        Some("https://x.test/metadata")
    );
    assert_eq!(
        www_authenticate_param(r#"Bearer realm="a\"b", scope=read"#, "scope").as_deref(),
        Some("read")
    );
    assert_eq!(www_authenticate_param("Bearer xscope=a", "scope"), None);
    assert_eq!(
        www_authenticate_param("Bearer scope=a, scope=b", "scope"),
        None
    );
    assert_eq!(
        www_authenticate_param(r#"Bearer scope="unterminated"#, "scope"),
        None
    );
    assert_eq!(
        www_authenticate_param(r#"Basic scope="secret""#, "scope"),
        None
    );
}

#[test]
fn url_policy_requires_https_except_explicit_loopback() {
    for allowed in [
        "https://mcp.example/mcp",
        "http://127.0.0.1:1234/mcp",
        "http://[::1]:1234/mcp",
        "http://localhost:1234/mcp",
    ] {
        assert!(validate_mcp_url(allowed).is_ok(), "{allowed}");
    }
    for rejected in [
        "http://mcp.example/mcp",
        "http://192.168.0.1/mcp",
        "http://localhost.evil/mcp",
        "file:///secret",
        "https://user:secret@mcp.example/mcp",
        "https://mcp.example/mcp#secret",
    ] {
        assert!(validate_mcp_url(rejected).is_err(), "{rejected}");
    }
    assert!(checked_url("http://127.0.0.1:1234/token", false).is_err());
    assert!(checked_url("https://localhost/token", false).is_err());
    assert!(
        !format!(
            "{:#}",
            validate_mcp_url("https://user:TOP-SECRET@mcp.example").unwrap_err()
        )
        .contains("TOP-SECRET")
    );
}

#[test]
fn metadata_discovery_preserves_issuer_path_and_priority() {
    let issuer = url::Url::parse("https://auth.example/tenant/v1").unwrap();
    assert_eq!(
        auth_metadata_urls(&issuer),
        vec![
            "https://auth.example/.well-known/oauth-authorization-server/tenant/v1",
            "https://auth.example/.well-known/openid-configuration/tenant/v1",
            "https://auth.example/tenant/v1/.well-known/openid-configuration",
        ]
    );
    assert_eq!(
        well_known(&issuer, "oauth-protected-resource"),
        vec![
            "https://auth.example/.well-known/oauth-protected-resource/tenant/v1",
            "https://auth.example/.well-known/oauth-protected-resource",
        ]
    );
}

fn metadata() -> AuthServerMetadata {
    serde_json::from_value(json!({
        "issuer":"https://auth.example", "authorization_endpoint":"https://auth.example/authorize",
        "token_endpoint":"https://auth.example/token", "code_challenge_methods_supported":["S256"]
    }))
    .unwrap()
}

#[test]
fn callback_checks_state_before_errors_and_exact_issuer() {
    let mut meta = metadata();
    assert_eq!(
        callback_result("/callback?error=TOP-SECRET&state=wrong", "state", &meta).unwrap(),
        None
    );
    assert_eq!(
        callback_result("/callback?error=TOP-SECRET", "state", &meta).unwrap(),
        None
    );
    for target in [
        "/wrong?code=a&state=state",
        "/callback?code=a&state=state&state=state",
        "/callback?code=a&code=b&state=state",
        "/callback?code=&state=state",
    ] {
        assert_eq!(callback_result(target, "state", &meta).unwrap(), None);
    }
    let error =
        callback_result("/callback?error=TOP-SECRET&state=state", "state", &meta).unwrap_err();
    assert!(!format!("{error:#}").contains("TOP-SECRET"));
    assert_eq!(
        callback_result("/callback?code=a&state=state", "state", &meta)
            .unwrap()
            .as_deref(),
        Some("a")
    );
    assert!(
        callback_result(
            "/callback?code=a&state=state&iss=https%3A%2F%2FAUTH.example",
            "state",
            &meta
        )
        .is_err()
    );
    meta.authorization_response_iss_parameter_supported = true;
    assert!(callback_result("/callback?code=a&state=state", "state", &meta).is_err());
    assert_eq!(
        callback_result(
            "/callback?code=a&state=state&iss=https%3A%2F%2Fauth.example",
            "state",
            &meta
        )
        .unwrap()
        .as_deref(),
        Some("a")
    );
}

#[test]
fn pkce_pair_is_random_s256() {
    let (verifier, challenge) = pkce_pair();
    assert_eq!(verifier.len(), 43);
    assert_eq!(
        challenge,
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    );
    assert_ne!(verifier, pkce_pair().0);
}

#[test]
fn freshness_is_overflow_safe_and_honors_skew() {
    let mut entry = StoredOAuth {
        access_token: Some("t".into()),
        ..Default::default()
    };
    assert!(is_fresh(&entry));
    entry.expires_at = Some(now_secs() + 10);
    assert!(!is_fresh(&entry));
    entry.expires_at = Some(i64::MIN);
    assert!(!is_fresh(&entry));
    entry.expires_at = Some(i64::MAX);
    assert!(is_fresh(&entry));
    entry.access_token = Some(String::new());
    assert!(!is_fresh(&entry));
}

#[test]
fn token_validation_is_atomic_and_debug_is_redacted() {
    let mut entry = StoredOAuth {
        access_token: Some("TOP-SECRET-ACCESS".into()),
        refresh_token: Some("TOP-SECRET-REFRESH".into()),
        client_secret: Some("TOP-SECRET-CLIENT".into()),
        token_endpoint: "https://auth.example/token?secret=TOP-SECRET".into(),
        ..Default::default()
    };
    for value in [
        json!({"access_token":"bad", "token_type":"not-bearer"}),
        json!({"access_token":"bad\r\nvalue", "token_type":"Bearer"}),
        json!({"access_token":"bad", "token_type":"Bearer", "expires_in":-1}),
        json!({"access_token":"bad", "token_type":"Bearer", "expires_in":i64::MAX}),
    ] {
        assert!(apply_token(&mut entry, serde_json::from_value(value).unwrap()).is_err());
        assert_eq!(entry.access_token.as_deref(), Some("TOP-SECRET-ACCESS"));
    }
    assert!(!format!("{entry:?}").contains("TOP-SECRET"));
}

#[tokio::test(flavor = "current_thread")]
async fn localhost_full_login_pkce_callback_401_refresh_rotation_and_logout() {
    let sandbox = Sandbox::new();
    let fixture = Fixture::new(Options::default()).await;
    do_login(&fixture, true).await.unwrap();
    assert_eq!(fixture.state.lock().unwrap().code_exchanges, 1);
    let resource = fixture.resource();
    assert!(status(&resource).unwrap().logged_in);
    assert_eq!(
        access_token(&resource).await.unwrap().as_deref(),
        Some("access-0")
    );
    let rejected = http()
        .expect("loopback OAuth HTTP client")
        .post(&resource)
        .bearer_auth("access-0")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
    let (left, right) = tokio::join!(
        token_after_unauthorized(&resource, Some("access-0")),
        token_after_unauthorized(&resource, Some("access-0")),
    );
    assert_eq!(left.unwrap().as_deref(), Some("access-1"));
    assert_eq!(right.unwrap().as_deref(), Some("access-1"));
    assert_eq!(
        fixture.state.lock().unwrap().refresh_count,
        1,
        "401 must force exactly one refresh despite future expiry"
    );
    assert_eq!(
        stored(&resource).unwrap().refresh_token.as_deref(),
        Some("refresh-1")
    );
    assert_eq!(
        http()
            .expect("loopback OAuth HTTP client")
            .post(&resource)
            .bearer_auth("access-1")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let mut entry = stored(&resource).unwrap();
    entry.expires_at = Some(now_secs() - 100);
    seed(&resource, entry);
    assert_eq!(
        access_token(&resource).await.unwrap().as_deref(),
        Some("access-2")
    );
    assert_eq!(fixture.state.lock().unwrap().refresh_count, 2);
    assert_eq!(
        stored(&resource).unwrap().refresh_token.as_deref(),
        Some("refresh-2")
    );
    assert!(logout(&resource).unwrap());
    assert!(!logout(&resource).unwrap());
    assert_eq!(access_token(&resource).await.unwrap(), None);
    assert!(!status(&resource).unwrap().logged_in);
    let bytes = std::fs::read_to_string(sandbox.path()).unwrap();
    assert!(!bytes.contains("access-"));
    assert!(!bytes.contains("refresh-"));
    assert!(!sandbox.path().with_extension("bak").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [sandbox.path(), sandbox.path().with_extension("lock")] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            std::fs::metadata(sandbox.temp.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn localhost_well_known_fallback_and_oidc_path_append() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        no_challenge: true,
        oidc_append_only: true,
        ..Default::default()
    })
    .await;
    do_login(&fixture, false).await.unwrap();
    let state = fixture.state.lock().unwrap();
    let paths = state
        .requests
        .iter()
        .map(|(_, target, _)| target.as_str())
        .collect::<Vec<_>>();
    let expected = [
        "/mcp",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-authorization-server/tenant",
        "/.well-known/openid-configuration/tenant",
        "/tenant/.well-known/openid-configuration",
        "/register",
    ];
    assert_eq!(&paths[..expected.len()], &expected);
    assert_eq!(
        stored(&fixture.resource()).unwrap().scope.as_deref(),
        Some("read")
    );
    let authorize = state
        .requests
        .iter()
        .find(|(_, target, _)| target.starts_with("/authorize?"))
        .unwrap();
    assert!(authorize.1.contains("scope=metadata-scope"));
    assert!(!authorize.1.contains("excessive-global-scope"));
}

#[tokio::test(flavor = "current_thread")]
async fn localhost_metadata_mismatches_fail_before_registration() {
    for options in [
        Options {
            wrong_resource: true,
            ..Default::default()
        },
        Options {
            wrong_issuer: true,
            ..Default::default()
        },
        Options {
            plain_pkce: true,
            ..Default::default()
        },
        Options {
            no_metadata: true,
            ..Default::default()
        },
    ] {
        let fixture = Fixture::new(options).await;
        assert!(discover(&fixture.resource()).await.is_err());
        assert!(
            !fixture
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|(_, target, _)| target == "/register")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn localhost_redirects_and_provider_errors_do_not_leak_credentials() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        metadata_redirect: true,
        ..Default::default()
    })
    .await;
    assert!(discover(&fixture.resource()).await.is_err());
    assert!(
        !fixture
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(_, target, _)| target.starts_with("/credential-sink"))
    );
    for options in [
        Options {
            token_redirect: true,
            ..Default::default()
        },
        Options {
            token_error: true,
            ..Default::default()
        },
        Options {
            invalid_token: true,
            ..Default::default()
        },
    ] {
        let fixture = Fixture::new(options).await;
        fixture.seed(false);
        let error = token_after_unauthorized(&fixture.resource(), Some("access-0"))
            .await
            .unwrap_err();
        let text = format!("{error:#}");
        for secret in ["REFLECTED-SECRET", "refresh-0", "client-secret"] {
            assert!(!text.contains(secret), "{text}");
        }
        assert_eq!(
            stored(&fixture.resource()).unwrap().access_token.as_deref(),
            Some("access-0")
        );
        assert!(
            !fixture
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|(_, target, _)| target.starts_with("/credential-sink"))
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_preserves_omitted_refresh_token_and_never_retries_same_rejected_token() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        no_new_refresh_token: true,
        ..Default::default()
    })
    .await;
    fixture.seed(false);
    assert_eq!(
        token_after_unauthorized(&fixture.resource(), Some("access-0"))
            .await
            .unwrap()
            .as_deref(),
        Some("access-1")
    );
    assert_eq!(
        stored(&fixture.resource())
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("refresh-0")
    );
    assert_eq!(
        token_after_unauthorized(&fixture.resource(), Some("access-1"))
            .await
            .unwrap()
            .as_deref(),
        Some("access-2")
    );
    let fixture = Fixture::new(Options {
        same_access_token: true,
        ..Default::default()
    })
    .await;
    fixture.seed(false);
    assert_eq!(
        token_after_unauthorized(&fixture.resource(), Some("access-0"))
            .await
            .unwrap(),
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn expired_unrefreshable_token_is_not_returned_and_new_login_is_picked_up() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options::default()).await;
    let resource = fixture.resource();
    let mut entry = fixture.entry(true);
    entry.refresh_token = None;
    seed(&resource, entry);
    assert_eq!(access_token(&resource).await.unwrap(), None);
    assert_eq!(
        token_after_unauthorized(&resource, Some("access-0"))
            .await
            .unwrap(),
        None
    );
    let mut entry = fixture.entry(false);
    entry.access_token = Some("new-login-token".into());
    seed(&resource, entry);
    assert_eq!(
        token_after_unauthorized(&resource, Some("access-0"))
            .await
            .unwrap()
            .as_deref(),
        Some("new-login-token")
    );
    assert_eq!(fixture.state.lock().unwrap().refresh_count, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn logout_during_refresh_does_not_block_or_resurrect_credentials() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        delay_refresh: true,
        ..Default::default()
    })
    .await;
    fixture.seed(false);
    let resource = fixture.resource();
    let refresh_resource = resource.clone();
    let refresh =
        tokio::spawn(
            async move { token_after_unauthorized(&refresh_resource, Some("access-0")).await },
        );
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.lock().unwrap().refresh_count == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        logout(&resource).unwrap(),
        "store transaction lock must not span network I/O"
    );
    assert_eq!(refresh.await.unwrap().unwrap(), None);
    assert!(stored(&resource).is_none());
    assert!(!status(&resource).unwrap().logged_in);
}

#[tokio::test(flavor = "current_thread")]
async fn newer_login_during_refresh_is_not_overwritten() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        delay_refresh: true,
        ..Default::default()
    })
    .await;
    fixture.seed(false);
    let resource = fixture.resource();
    let refresh_resource = resource.clone();
    let refresh =
        tokio::spawn(
            async move { token_after_unauthorized(&refresh_resource, Some("access-0")).await },
        );
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.lock().unwrap().refresh_count == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut newer = fixture.entry(false);
    newer.access_token = Some("new-login-access".into());
    newer.refresh_token = Some("new-login-refresh".into());
    seed(&resource, newer);
    assert_eq!(
        refresh.await.unwrap().unwrap().as_deref(),
        Some("new-login-access")
    );
    assert_eq!(
        stored(&resource).unwrap().refresh_token.as_deref(),
        Some("new-login-refresh")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn exact_validated_metadata_resource_is_preserved_in_grants() {
    let _sandbox = Sandbox::new();
    let fixture = Fixture::new(Options {
        metadata_trailing_slash: true,
        ..Default::default()
    })
    .await;
    do_login(&fixture, false).await.unwrap();
    assert_eq!(
        stored(&fixture.resource()).unwrap().resource,
        Some(format!("{}/mcp/", fixture.origin))
    );
    assert_eq!(
        token_after_unauthorized(&fixture.resource(), Some("access-0"))
            .await
            .unwrap()
            .as_deref(),
        Some("access-1")
    );
}

#[test]
fn corrupt_store_is_not_silently_overwritten_or_exposed() {
    let sandbox = Sandbox::new();
    let bytes = b"{malformed TOP-SECRET";
    std::fs::write(sandbox.path(), bytes).unwrap();
    let error = logout("https://mcp.example/mcp").unwrap_err();
    assert!(!format!("{error:#}").contains("TOP-SECRET"));
    assert_eq!(std::fs::read(sandbox.path()).unwrap(), bytes);
    assert!(status("https://mcp.example/mcp").is_err());
}

#[cfg(unix)]
#[test]
fn store_rejects_symlinks_hardlinks_and_repairs_legacy_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let sandbox = Sandbox::new();
    let target = sandbox.temp.path().join("target");
    std::fs::write(&target, b"{\"version\":1,\"servers\":{}}").unwrap();
    symlink(&target, sandbox.path()).unwrap();
    assert!(load_store().is_err());
    std::fs::remove_file(sandbox.path()).unwrap();
    std::fs::hard_link(&target, sandbox.path()).unwrap();
    assert!(load_store().is_err());
    std::fs::remove_file(sandbox.path()).unwrap();
    std::fs::rename(&target, sandbox.path()).unwrap();
    std::fs::set_permissions(sandbox.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_store().is_ok());
    assert_eq!(
        std::fs::metadata(sandbox.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    symlink(sandbox.path(), sandbox.path().with_extension("lock")).unwrap();
    assert!(logout("https://mcp.example").is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn cross_process_lock_contends_and_releases_without_blocking_runtime() {
    let sandbox = Sandbox::new();
    let first = open_store_lock(&sandbox.path()).unwrap();
    first.try_lock().unwrap();
    let second = open_store_lock(&sandbox.path()).unwrap();
    assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
    assert!(logout("https://mcp.example").is_err());
    let path = sandbox.path();
    let pending = lock_store(&path);
    tokio::pin!(pending);
    tokio::select! {
        result = &mut pending => panic!("unexpected lock acquisition: {}", result.is_ok()),
        _ = tokio::time::sleep(Duration::from_millis(30)) => (),
    }
    drop(first);
    let acquired = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap();
    drop(acquired);
    second.try_lock().unwrap();
}
