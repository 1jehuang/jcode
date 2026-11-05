#[derive(Clone, Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

#[derive(Clone)]
struct Plan {
    legacy: bool,
    fallback_status: u16,
    post_sse: bool,
    endpoint: Option<String>,
    omit_endpoint: bool,
    initialize_status: Option<u16>,
    redirect: Option<String>,
    notification_status: u16,
    protocol_version: String,
    get_content_type: String,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            legacy: false,
            fallback_status: 405,
            post_sse: false,
            endpoint: None,
            omit_endpoint: false,
            initialize_status: None,
            redirect: None,
            notification_status: 202,
            protocol_version: "2025-06-18".to_string(),
            get_content_type: "text/event-stream".to_string(),
        }
    }
}

struct FixtureState {
    plan: Plan,
    requests: std::sync::Mutex<Vec<HttpRequest>>,
    initialized: AtomicBool,
    ordering_violation: AtomicBool,
    active_streams: AtomicUsize,
    disconnected_streams: AtomicUsize,
    sse_tx: std::sync::Mutex<Option<mpsc::Sender<Value>>>,
}

struct Fixture {
    url: String,
    state: Arc<FixtureState>,
    task: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn start(plan: Plan) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let state = Arc::new(FixtureState {
            plan,
            requests: std::sync::Mutex::new(Vec::new()),
            initialized: AtomicBool::new(false),
            ordering_violation: AtomicBool::new(false),
            active_streams: AtomicUsize::new(0),
            disconnected_streams: AtomicUsize::new(0),
            sse_tx: std::sync::Mutex::new(None),
        });
        let owned_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let state = Arc::clone(&owned_state);
                        connections.spawn(async move {
                            let _ = serve(socket, state).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
        });
        Self { url, state, task }
    }

    fn config(&self, transport: &str) -> McpServerConfig {
        serde_json::from_value(json!({
            "type": transport,
            "url": self.url,
            "headers": {"Authorization":"Bearer fixture-secret", "X-Api-Key":"custom-secret"},
            "timeout_secs": 3
        }))
        .unwrap()
    }

    async fn connect(&self, transport: &str) -> McpClient {
        tokio::time::timeout(
            Duration::from_secs(5),
            McpClient::connect("local-fixture".into(), &self.config(transport)),
        )
        .await
        .unwrap()
        .unwrap()
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}
