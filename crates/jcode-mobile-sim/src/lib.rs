use anyhow::{Context, Result, anyhow, bail};
use jcode_mobile_core::{DispatchReport, ScenarioName, SimulatorAction, SimulatorStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationResponse {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSummary {
    pub socket_path: String,
    pub started_at_ms: u64,
    pub screen: String,
    pub connection_state: String,
    pub message_count: usize,
    pub transition_count: usize,
}

pub fn default_socket_path() -> PathBuf {
    runtime_dir().join("jcode-mobile-sim.sock")
}

pub async fn request_status(socket_path: &Path) -> Result<StatusSummary> {
    let response = send_request(
        socket_path,
        AutomationRequest {
            id: "status".to_string(),
            method: "status".to_string(),
            params: Value::Null,
        },
    )
    .await?;
    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| "status request failed".to_string())
        );
    }
    Ok(serde_json::from_value(response.result)?)
}

#[cfg(unix)]
pub async fn run_server(socket_path: &Path, initial_scenario: ScenarioName) -> Result<()> {
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::sync::Mutex;

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let _ = tokio::fs::remove_file(socket_path).await;

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind unix socket {}", socket_path.display()))?;
    let store = Arc::new(Mutex::new(SimulatorStore::new(
        jcode_mobile_core::SimulatorState::for_scenario(initial_scenario),
    )));
    let started_at_ms = now_ms();
    let socket_path_string = socket_path.display().to_string();

    loop {
        let (stream, _) = listener.accept().await?;
        let store = Arc::clone(&store);
        let socket_path_string = socket_path_string.clone();

        let shutdown = async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(false);
            }
            let request: AutomationRequest =
                serde_json::from_str(&line).with_context(|| "decode automation request JSON")?;
            let (response, should_shutdown) =
                handle_request(store, request, started_at_ms, &socket_path_string).await;
            let mut json = serde_json::to_string(&response)?;
            json.push('\n');
            writer.write_all(json.as_bytes()).await?;
            writer.flush().await?;
            Ok::<bool, anyhow::Error>(should_shutdown)
        }
        .await?;

        if shutdown {
            break;
        }
    }

    let _ = tokio::fs::remove_file(socket_path).await;
    Ok(())
}

#[cfg(not(unix))]
pub async fn run_server(_socket_path: &Path, _initial_scenario: ScenarioName) -> Result<()> {
    bail!("jcode-mobile-sim currently supports Unix socket automation only")
}

#[cfg(unix)]
pub async fn send_request(
    socket_path: &Path,
    request: AutomationRequest,
) -> Result<AutomationResponse> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connect to {}", socket_path.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        bail!("simulator disconnected before responding");
    }
    Ok(serde_json::from_str(&line)?)
}

#[cfg(not(unix))]
pub async fn send_request(
    _socket_path: &Path,
    _request: AutomationRequest,
) -> Result<AutomationResponse> {
    bail!("jcode-mobile-sim currently supports Unix socket automation only")
}

#[cfg(unix)]
async fn handle_request(
    store: std::sync::Arc<tokio::sync::Mutex<SimulatorStore>>,
    request: AutomationRequest,
    started_at_ms: u64,
    socket_path: &str,
) -> (AutomationResponse, bool) {
    let id = request.id.clone();
    let result = match request.method.as_str() {
        "status" => {
            let store = store.lock().await;
            let summary = StatusSummary {
                socket_path: socket_path.to_string(),
                started_at_ms,
                screen: format!("{:?}", store.state().screen).to_lowercase(),
                connection_state: format!("{:?}", store.state().connection_state).to_lowercase(),
                message_count: store.state().messages.len(),
                transition_count: store.transition_log().len(),
            };
            Ok((serde_json::to_value(summary).unwrap_or(Value::Null), false))
        }
        "state" => {
            let store = store.lock().await;
            Ok((
                serde_json::to_value(store.state()).unwrap_or(Value::Null),
                false,
            ))
        }
        "tree" => {
            let store = store.lock().await;
            Ok((
                serde_json::to_value(store.semantic_tree()).unwrap_or(Value::Null),
                false,
            ))
        }
        "log" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            let store = store.lock().await;
            let transitions = if let Some(limit) = limit {
                let len = store.transition_log().len();
                store.transition_log()[len.saturating_sub(limit)..].to_vec()
            } else {
                store.transition_log().to_vec()
            };
            Ok((
                json!({
                    "transitions": transitions,
                    "effects": store.effect_log(),
                }),
                false,
            ))
        }
        "dispatch" => {
            let action_value = request
                .params
                .get("action")
                .cloned()
                .ok_or_else(|| anyhow!("missing action field"));
            match action_value.and_then(|value| {
                serde_json::from_value::<SimulatorAction>(value).map_err(Into::into)
            }) {
                Ok(action) => {
                    let mut store = store.lock().await;
                    let report: DispatchReport = store.dispatch(action);
                    Ok((serde_json::to_value(report).unwrap_or(Value::Null), false))
                }
                Err(err) => Err(err),
            }
        }
        "reset" => {
            let mut store = store.lock().await;
            let report = store.dispatch(SimulatorAction::Reset);
            Ok((serde_json::to_value(report).unwrap_or(Value::Null), false))
        }
        "load_scenario" => {
            let scenario_name = request
                .params
                .get("scenario")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing scenario"));
            match scenario_name.and_then(|name| {
                ScenarioName::parse(name).ok_or_else(|| anyhow!("unknown scenario: {name}"))
            }) {
                Ok(scenario) => {
                    let mut store = store.lock().await;
                    let report = store.dispatch(SimulatorAction::LoadScenario { scenario });
                    Ok((serde_json::to_value(report).unwrap_or(Value::Null), false))
                }
                Err(err) => Err(err),
            }
        }
        "shutdown" => Ok((json!({"message": "shutting down"}), true)),
        _ => Err(anyhow!("unknown method: {}", request.method)),
    };

    match result {
        Ok((result, should_shutdown)) => (
            AutomationResponse {
                id,
                ok: true,
                result,
                error: None,
            },
            should_shutdown,
        ),
        Err(err) => (
            AutomationResponse {
                id,
                ok: false,
                result: Value::Null,
                error: Some(err.to_string()),
            },
            false,
        ),
    }
}

fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("JCODE_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    std::env::temp_dir().join(format!("jcode-mobile-sim-{}", user_discriminator()))
}

#[cfg(unix)]
fn user_discriminator() -> String {
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(not(unix))]
fn user_discriminator() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    async fn wait_for_socket(path: &Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("socket did not appear: {}", path.display());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn automation_round_trip_over_socket() {
        let dir = TempDir::new().expect("tempdir");
        let socket = dir.path().join("sim.sock");
        let server_socket = socket.clone();
        let server =
            tokio::spawn(async move { run_server(&server_socket, ScenarioName::Onboarding).await });
        wait_for_socket(&socket).await;

        let status = request_status(&socket).await.expect("status");
        assert_eq!(status.screen, "onboarding");

        let _ = send_request(
            &socket,
            AutomationRequest {
                id: "set-host".to_string(),
                method: "dispatch".to_string(),
                params: json!({
                    "action": {
                        "type": "set_host",
                        "value": "devbox.tailnet.ts.net"
                    }
                }),
            },
        )
        .await
        .expect("set host");

        let dispatch = send_request(
            &socket,
            AutomationRequest {
                id: "scenario".to_string(),
                method: "load_scenario".to_string(),
                params: json!({"scenario": "connected_chat"}),
            },
        )
        .await
        .expect("load scenario");
        assert!(dispatch.ok);

        let tree = send_request(
            &socket,
            AutomationRequest {
                id: "tree".to_string(),
                method: "tree".to_string(),
                params: Value::Null,
            },
        )
        .await
        .expect("tree");
        let tree_json = serde_json::to_string(&tree.result).expect("tree json");
        assert!(tree_json.contains("chat.send"));

        let _ = send_request(
            &socket,
            AutomationRequest {
                id: "shutdown".to_string(),
                method: "shutdown".to_string(),
                params: Value::Null,
            },
        )
        .await
        .expect("shutdown");

        server.await.expect("join").expect("server exit");
    }
}
