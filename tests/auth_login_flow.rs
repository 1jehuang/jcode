use anyhow::Result;
use jcode::auth::{AuthState, AuthStatus};
use jcode::provider::Provider;
use jcode::provider::openrouter::OpenRouterProvider;
use jcode::provider_catalog::{
    OPENAI_COMPAT_LOGIN_PROVIDER, login_providers, openai_compatible_profiles,
};
use jcode::tui::login_picker::{LoginPicker, LoginPickerItem, LoginPickerSummary};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tracked_env_vars() -> Vec<String> {
    let mut keys: HashSet<String> = [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "JCODE_HOME",
        "NO_PROXY",
        "no_proxy",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_AUTH_HEADER_NAME",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
        "OPENROUTER_API_KEY",
        "AUTH_FLOW_TEST_KEY",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();

    for profile in openai_compatible_profiles() {
        keys.insert(profile.api_key_env.to_string());
    }

    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort();
    keys
}

struct TestEnv {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(String, Option<String>)>,
    temp: tempfile::TempDir,
}

impl TestEnv {
    fn new() -> Result<Self> {
        let lock = lock_env();
        let temp = tempfile::Builder::new()
            .prefix("jcode-auth-flow-")
            .tempdir()?;
        let saved = tracked_env_vars()
            .into_iter()
            .map(|key| {
                let value = std::env::var(&key).ok();
                (key, value)
            })
            .collect::<Vec<_>>();

        for (key, _) in &saved {
            jcode::env::remove_var(key);
        }

        jcode::env::set_var("HOME", temp.path());
        jcode::env::set_var("XDG_CONFIG_HOME", temp.path().join("config"));
        jcode::env::set_var("APPDATA", temp.path().join("AppData").join("Roaming"));
        jcode::env::set_var("JCODE_HOME", temp.path().join("jcode-home"));
        jcode::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        jcode::env::set_var("no_proxy", "127.0.0.1,localhost");
        AuthStatus::invalidate_cache();

        Ok(Self {
            _lock: lock,
            saved,
            temp,
        })
    }

    fn configure_openai_compatible_runtime(
        &self,
        api_base: &str,
        cache_namespace: &str,
        key: Option<&str>,
        allow_no_auth: bool,
    ) {
        let _ = self.temp.path();
        jcode::env::set_var("JCODE_OPENROUTER_API_BASE", api_base);
        jcode::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "AUTH_FLOW_TEST_KEY");
        jcode::env::set_var("JCODE_OPENROUTER_ENV_FILE", "auth-flow-test.env");
        jcode::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", cache_namespace);
        jcode::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
        jcode::env::set_var("JCODE_OPENROUTER_MODEL_CATALOG", "1");
        if let Some(key) = key {
            jcode::env::set_var("AUTH_FLOW_TEST_KEY", key);
        } else {
            jcode::env::remove_var("AUTH_FLOW_TEST_KEY");
        }
        if allow_no_auth {
            jcode::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
        } else {
            jcode::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
        }
        AuthStatus::invalidate_cache();
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        AuthStatus::invalidate_cache();
        for (key, value) in &self.saved {
            if let Some(value) = value {
                jcode::env::set_var(key, value);
            } else {
                jcode::env::remove_var(key);
            }
        }
        AuthStatus::invalidate_cache();
    }
}

struct FakeModelsServer {
    api_base: String,
    requests: mpsc::Receiver<String>,
    request_count: Arc<AtomicUsize>,
}

fn spawn_models_server(
    max_requests: usize,
    body: impl Into<String>,
    delay: Duration,
) -> FakeModelsServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    listener
        .set_nonblocking(true)
        .expect("set fake server nonblocking");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_thread = Arc::clone(&request_count);
    let body = body.into();

    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while request_count_thread.load(Ordering::SeqCst) < max_requests
            && Instant::now() < deadline
        {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .expect("set read timeout");
                    let mut request = vec![0u8; 8192];
                    let n = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..n]).into_owned();
                    request_count_thread.fetch_add(1, Ordering::SeqCst);
                    let _ = request_tx.send(request);

                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    FakeModelsServer {
        api_base: format!("http://{addr}/v1"),
        requests: request_rx,
        request_count,
    }
}

fn assert_models_request(request: &str) {
    assert!(
        request.starts_with("GET /v1/models "),
        "expected GET /v1/models request, got: {request}"
    );
}

fn lower_headers(request: &str) -> String {
    request.to_ascii_lowercase()
}

fn run_current_thread<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

#[test]
fn every_login_provider_has_auth_status_and_tui_copy_metadata() -> Result<()> {
    let _env = TestEnv::new()?;
    let status = AuthStatus::default();

    for provider in login_providers() {
        assert!(!provider.id.trim().is_empty(), "provider id must be set");
        assert!(
            !provider.display_name.trim().is_empty(),
            "{} display name must be set",
            provider.id
        );
        assert!(
            !provider.menu_detail.trim().is_empty(),
            "{} setup copy must be set",
            provider.id
        );
        assert!(
            !provider.auth_kind.label().trim().is_empty(),
            "{} auth kind label must be set",
            provider.id
        );
        assert!(
            !status
                .method_detail_for_provider(*provider)
                .trim()
                .is_empty(),
            "{} detected setup copy must be renderable",
            provider.id
        );
    }

    Ok(())
}

#[test]
fn live_models_contract_uses_models_endpoint_and_bearer_auth() -> Result<()> {
    let env = TestEnv::new()?;
    let server = spawn_models_server(
        1,
        r#"{"data":[{"id":"bearer-contract-model","object":"model"}]}"#,
        Duration::ZERO,
    );
    env.configure_openai_compatible_runtime(
        &server.api_base,
        "auth-flow-bearer-contract",
        Some("sk-bearer-contract"),
        false,
    );

    let provider = OpenRouterProvider::new()?;
    let models = run_current_thread(provider.fetch_models())?;
    assert_eq!(models[0].id, "bearer-contract-model");

    let request = server.requests.recv_timeout(Duration::from_secs(2))?;
    assert_models_request(&request);
    let headers = lower_headers(&request);
    assert!(
        headers.contains("authorization: bearer sk-bearer-contract"),
        "missing bearer auth header: {request}"
    );
    assert!(
        provider
            .available_models_display()
            .iter()
            .any(|model| model == "bearer-contract-model"),
        "fetched model should be visible immediately after refresh"
    );

    Ok(())
}

#[test]
fn live_models_contract_supports_api_key_header_mode() -> Result<()> {
    let env = TestEnv::new()?;
    let server = spawn_models_server(
        1,
        r#"{"data":[{"id":"header-contract-model","object":"model"}]}"#,
        Duration::ZERO,
    );
    env.configure_openai_compatible_runtime(
        &server.api_base,
        "auth-flow-header-contract",
        Some("sk-header-contract"),
        false,
    );
    jcode::env::set_var("JCODE_OPENROUTER_AUTH_HEADER", "api-key");
    jcode::env::set_var("JCODE_OPENROUTER_AUTH_HEADER_NAME", "x-api-key");

    let provider = OpenRouterProvider::new()?;
    let models = run_current_thread(provider.fetch_models())?;
    assert_eq!(models[0].id, "header-contract-model");

    let request = server.requests.recv_timeout(Duration::from_secs(2))?;
    assert_models_request(&request);
    let headers = lower_headers(&request);
    assert!(
        headers.contains("x-api-key: sk-header-contract"),
        "missing x-api-key auth header: {request}"
    );
    assert!(
        !headers.contains("authorization: bearer"),
        "api-key mode must not also send bearer auth: {request}"
    );

    Ok(())
}

#[test]
fn local_no_auth_models_contract_sends_no_auth_header() -> Result<()> {
    let env = TestEnv::new()?;
    let server = spawn_models_server(
        1,
        r#"{"data":[{"id":"local-no-auth-model","object":"model"}]}"#,
        Duration::ZERO,
    );
    env.configure_openai_compatible_runtime(
        &server.api_base,
        "auth-flow-local-no-auth-contract",
        None,
        true,
    );

    let provider = OpenRouterProvider::new()?;
    let models = run_current_thread(provider.fetch_models())?;
    assert_eq!(models[0].id, "local-no-auth-model");

    let request = server.requests.recv_timeout(Duration::from_secs(2))?;
    assert_models_request(&request);
    let headers = lower_headers(&request);
    assert!(
        !headers.contains("authorization:"),
        "local no-auth endpoint should not send Authorization: {request}"
    );
    assert!(
        !headers.contains("api-key:") && !headers.contains("x-api-key:"),
        "local no-auth endpoint should not send API-key headers: {request}"
    );

    Ok(())
}

#[test]
fn model_picker_cache_miss_schedules_single_background_refresh_and_updates_routes() -> Result<()> {
    let env = TestEnv::new()?;
    let server = spawn_models_server(
        2,
        r#"{"data":[{"id":"background-race-live-model","object":"model"}]}"#,
        Duration::from_millis(25),
    );
    env.configure_openai_compatible_runtime(
        &server.api_base,
        "auth-flow-background-race",
        None,
        true,
    );
    jcode::env::set_var("JCODE_OPENROUTER_MODEL", "background-race-selected-model");

    let provider = OpenRouterProvider::new()?;
    run_current_thread(async {
        let first = provider.available_models_display();
        let second = provider.available_models_display();
        assert!(
            first
                .iter()
                .any(|model| model == "background-race-selected-model")
        );
        assert!(
            second
                .iter()
                .any(|model| model == "background-race-selected-model")
        );

        for _ in 0..100 {
            if provider
                .available_models_display()
                .iter()
                .any(|model| model == "background-race-live-model")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let display = provider.available_models_display();
        assert!(
            display
                .iter()
                .any(|model| model == "background-race-live-model"),
            "background refresh should update picker models; display={display:?}"
        );
        let routes = provider.model_routes();
        assert!(
            routes.iter().any(|route| {
                route.model == "background-race-live-model"
                    && route.api_method == "openai-compatible"
                    && route.available
            }),
            "background refresh should update model routes; routes={routes:?}"
        );
    });

    let request = server.requests.recv_timeout(Duration::from_secs(2))?;
    assert_models_request(&request);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        server.request_count.load(Ordering::SeqCst),
        1,
        "concurrent picker renders should coalesce into one background /models request"
    );

    Ok(())
}

fn buffer_to_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn login_picker_renders_openai_compatible_setup_copy() -> Result<()> {
    let mut picker = LoginPicker::with_summary(
        " Login ",
        vec![LoginPickerItem::new(
            1,
            OPENAI_COMPAT_LOGIN_PROVIDER,
            AuthState::Available,
            "http://localhost:11434/v1 configured; no API key required",
        )],
        LoginPickerSummary {
            ready_count: 1,
            recommended_count: 0,
            ..LoginPickerSummary::default()
        },
    );

    let backend = TestBackend::new(120, 44);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| picker.render(frame))?;
    let text = buffer_to_text(terminal.backend().buffer());

    for expected in [
        "OpenAI-compatible",
        "/login openai-compatible",
        "API key / CLI",
        "Detected setup",
        "http://localhost:11434/v1 configured; no API key required",
        "custom endpoint setup: base URL first, then API key",
        "Press Enter to begin login.",
    ] {
        assert!(
            text.contains(expected),
            "login picker should render expected copy {expected:?}; rendered:\n{text}"
        );
    }

    Ok(())
}
