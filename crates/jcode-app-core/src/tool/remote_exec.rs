//! Remote execution seam: run workspace tools in a separate execution
//! environment while the agent runtime (model loop, session state, hooks,
//! telemetry, context guard) stays where it is.
//!
//! The runtime sets `JCODE_EXEC_WORKER` to a shell command whose stdin/stdout
//! speak the newline-delimited JSON protocol below. The command is the whole
//! transport contract, so the same runtime can target a local process
//! (`jcode server exec-worker`), a container (`docker exec -i box jcode server
//! exec-worker`), an SSH host, or a serverless sandbox behind a relay.
//!
//! Only tools that touch the workspace are routed. Everything else (todo,
//! swarm, memory, web search, MCP, batch fanout) keeps running in the runtime,
//! and `batch` subcalls are routed individually because they dispatch through
//! the registry.
//!
//! Prototype limits, see `docs/SERVERLESS.md`: no interactive stdin
//! forwarding, one worker per runtime process rather than per session, and a
//! command that runs past ten minutes is promoted inside the worker.
use super::{Registry, Tool, ToolContext, ToolExecutionMode, ToolImage, ToolOutput};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Shell command that starts the execution worker.
pub const WORKER_ENV: &str = "JCODE_EXEC_WORKER";
/// Working directory inside the execution environment. Replaces the runtime's
/// own working directory, which usually does not exist there.
pub const WORKDIR_ENV: &str = "JCODE_EXEC_WORKDIR";

/// Tools whose effects belong to the workspace.
///
/// `bg` is deliberately absent. Background tasks are owned by the runtime:
/// a slow remote call is an ordinary future there, so the runtime's manager
/// adopts it, and `bg` must look in that manager, not in the worker's.
const ROUTED: &[&str] = &["bash", "read", "write", "edit", "apply_patch", "agentgrep"];

/// Longest foreground command the worker runs before `bash` promotes it into
/// the worker's own background manager, where the runtime cannot follow.
const BACKGROUND_COMMAND_TIMEOUT_MS: u64 = 600_000;

/// Set in the worker process so it executes locally instead of re-routing.
static WORKER_MODE: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    Call {
        id: u64,
        name: String,
        input: Value,
        session_id: String,
        message_id: String,
        tool_call_id: String,
        working_dir: Option<PathBuf>,
    },
    Cancel {
        id: u64,
    },
    Result {
        id: u64,
        #[serde(default)]
        output: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        metadata: Option<Value>,
        #[serde(default)]
        images: Vec<WireImage>,
        #[serde(default)]
        error: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct WireImage {
    media_type: String,
    data: String,
    #[serde(default)]
    label: Option<String>,
}

/// The configured worker command, unless this process is itself a worker.
fn worker_command() -> Option<String> {
    if WORKER_MODE.load(Ordering::Relaxed) {
        return None;
    }
    std::env::var(WORKER_ENV)
        .ok()
        .filter(|command| !command.trim().is_empty())
}

/// Whether this tool call should leave the runtime.
pub(crate) fn routes(tool: &str) -> bool {
    ROUTED.contains(&tool) && worker_command().is_some()
}

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<ToolOutput>>>>>;

struct Client {
    frames: mpsc::UnboundedSender<Frame>,
    pending: Pending,
}

static CLIENT: tokio::sync::Mutex<Option<Arc<Client>>> = tokio::sync::Mutex::const_new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn fail_all(pending: &Pending, reason: &str) {
    let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(anyhow::anyhow!("{reason}")));
    }
}

async fn client() -> Result<Arc<Client>> {
    let mut slot = CLIENT.lock().await;
    if let Some(client) = slot.as_ref()
        && !client.frames.is_closed()
    {
        return Ok(Arc::clone(client));
    }
    let command = worker_command().context("no execution worker is configured")?;
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start execution worker `{command}`"))?;
    let mut stdin = child.stdin.take().context("worker has no stdin")?;
    let stdout = child.stdout.take().context("worker has no stdout")?;
    let pending: Pending = Arc::default();
    let (frames, mut outgoing) = mpsc::unbounded_channel::<Frame>();

    // The writer task owns the child, so closing the channel ends the worker.
    tokio::spawn(async move {
        let _child = child;
        while let Some(frame) = outgoing.recv().await {
            let Ok(mut line) = serde_json::to_string(&frame) else {
                continue;
            };
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });

    let reader_pending = Arc::clone(&pending);
    let reader_frames = frames.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(Frame::Result {
                id,
                output,
                title,
                metadata,
                images,
                error,
            }) = serde_json::from_str::<Frame>(&line)
            else {
                continue;
            };
            let tx = reader_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(match error {
                    Some(error) => Err(anyhow::anyhow!("{error}")),
                    None => Ok(ToolOutput {
                        output,
                        title,
                        metadata,
                        images: images
                            .into_iter()
                            .map(|image| ToolImage {
                                media_type: image.media_type,
                                data: image.data,
                                label: image.label,
                            })
                            .collect(),
                    }),
                });
            }
        }
        fail_all(&reader_pending, "execution worker disconnected");
        // Dropping the last sender stops the writer and reaps the child. A
        // later call sees the closed channel and starts a fresh worker.
        drop(reader_frames);
    });

    let client = Arc::new(Client { frames, pending });
    *slot = Some(Arc::clone(&client));
    Ok(client)
}

/// Tells the worker to stop a call whose runtime future was dropped, for
/// example when the user interrupts a turn.
struct CancelOnDrop {
    client: Arc<Client>,
    id: u64,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.client
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.id);
            let _ = self.client.frames.send(Frame::Cancel { id: self.id });
        }
    }
}

/// Run one routed tool call in the execution environment.
pub(crate) async fn execute(name: &str, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
    if name == "bash" && input.get("run_in_background").and_then(Value::as_bool) == Some(true) {
        return execute_background(input, ctx).await;
    }
    call(name, input, ctx).await
}

/// `bash` with `run_in_background`: the command runs in the foreground of the
/// worker while the runtime's background manager owns the pending call, so
/// `bg`, completion notices and wakeups all work without leaving the runtime.
async fn execute_background(mut input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let flag =
        |key: &str, default: bool| input.get(key).and_then(Value::as_bool).unwrap_or(default);
    let wake = flag("wake", false);
    let notify = flag("notify", true) || wake;
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let display_name =
        crate::util::truncate_str(text("intent").or(text("command")).unwrap_or("bash"), 28)
            .to_string();
    if let Some(fields) = input.as_object_mut() {
        fields.insert("run_in_background".into(), Value::Bool(false));
        fields.insert("timeout".into(), BACKGROUND_COMMAND_TIMEOUT_MS.into());
    }
    let task_ctx = ctx.clone();
    let handle = tokio::spawn(async move { call("bash", input, &task_ctx).await });
    let info = crate::background::global()
        .adopt_with_options(
            "bash",
            Some(display_name.clone()),
            &ctx.session_id,
            notify,
            wake,
            handle,
        )
        .await;
    let delivery = if wake {
        "The agent will be woken when the task completes."
    } else if notify {
        "You will be notified when the task completes."
    } else {
        "Notifications disabled."
    };
    Ok(ToolOutput::new(format!(
        "Command started in background as requested.\n\nTask ID: {}\nName: {}\n\n{}\n\n{}",
        info.task_id,
        display_name,
        delivery,
        super::BACKGROUND_WAIT_GUIDANCE
    )))
}

async fn call(name: &str, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let client = client().await?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    client
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, tx);
    let mut guard = CancelOnDrop {
        client: Arc::clone(&client),
        id,
        armed: true,
    };
    let working_dir = std::env::var_os(WORKDIR_ENV)
        .map(PathBuf::from)
        .or_else(|| ctx.working_dir.clone());
    client
        .frames
        .send(Frame::Call {
            id,
            name: name.into(),
            input,
            session_id: ctx.session_id.clone(),
            message_id: ctx.message_id.clone(),
            tool_call_id: ctx.tool_call_id.clone(),
            working_dir,
        })
        .map_err(|_| anyhow::anyhow!("execution worker disconnected"))?;
    let result = rx
        .await
        .map_err(|_| anyhow::anyhow!("execution worker disconnected"))?;
    guard.armed = false;
    result
}

struct NoModel;
#[async_trait::async_trait]
impl crate::provider::Provider for NoModel {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[jcode_message_types::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("the execution worker never calls a model")
    }
    fn name(&self) -> &str {
        "exec-worker"
    }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(NoModel)
    }
}

/// Serve tool calls from `reader`, answering on `writer`, until EOF.
///
/// The worker holds no model credentials and no conversation: it only sees
/// the tool name, its input, and the identifiers needed to key background
/// tasks and file locks.
pub async fn run_worker<R, W>(reader: R, mut writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    WORKER_MODE.store(true, Ordering::Relaxed);
    let registry = Registry::new(Arc::new(NoModel)).await;
    let (results, mut outgoing) = mpsc::unbounded_channel::<Frame>();
    let write_task = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            let Ok(mut line) = serde_json::to_string(&frame) else {
                continue;
            };
            line.push('\n');
            if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });

    let mut running: HashMap<u64, tokio::task::AbortHandle> = HashMap::new();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        running.retain(|_, task| !task.is_finished());
        match serde_json::from_str::<Frame>(&line) {
            Ok(Frame::Call {
                id,
                name,
                input,
                session_id,
                message_id,
                tool_call_id,
                working_dir,
            }) => {
                let results = results.clone();
                let registry = registry.clone();
                let task = tokio::spawn(async move {
                    let outcome = if ROUTED.contains(&name.as_str()) {
                        let ctx = ToolContext {
                            session_id,
                            message_id,
                            tool_call_id,
                            working_dir,
                            stdin_request_tx: None,
                            decision_request_tx: None,
                            graceful_shutdown_signal: None,
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        registry.execute_local(&name, input, ctx).await
                    } else {
                        Err(anyhow::anyhow!(
                            "tool `{name}` does not run in the execution environment"
                        ))
                    };
                    let frame = match outcome {
                        Ok(output) => Frame::Result {
                            id,
                            output: output.output,
                            title: output.title,
                            metadata: output.metadata,
                            images: output
                                .images
                                .into_iter()
                                .map(|image| WireImage {
                                    media_type: image.media_type,
                                    data: image.data,
                                    label: image.label,
                                })
                                .collect(),
                            error: None,
                        },
                        Err(error) => Frame::Result {
                            id,
                            output: String::new(),
                            title: None,
                            metadata: None,
                            images: Vec::new(),
                            error: Some(format!("{error:#}")),
                        },
                    };
                    let _ = results.send(frame);
                });
                running.insert(id, task.abort_handle());
            }
            Ok(Frame::Cancel { id }) => {
                if let Some(task) = running.remove(&id) {
                    task.abort();
                }
            }
            Ok(Frame::Result { .. }) | Err(_) => {}
        }
    }
    for task in running.into_values() {
        task.abort();
    }
    drop(results);
    let _ = write_task.await;
    Ok(())
}

/// `jcode server exec-worker`: the worker over this process's stdin/stdout.
pub async fn run_worker_stdio() -> Result<()> {
    run_worker(tokio::io::stdin(), tokio::io::stdout()).await
}

impl Registry {
    /// Execute a tool in this process with no hooks, policy, telemetry or
    /// context guard. The runtime applies all of those around the remote call.
    async fn execute_local(
        &self,
        name: &str,
        input: Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let tool: Arc<dyn Tool> = self
            .tools
            .read()
            .await
            .get(name)
            .cloned()
            .with_context(|| format!("unknown tool `{name}`"))?;
        tool.execute(input, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn call(
        to_worker: &mut (impl AsyncWrite + Unpin),
        from_worker: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
        id: u64,
        name: &str,
        input: Value,
        dir: &std::path::Path,
    ) -> Frame {
        let frame = Frame::Call {
            id,
            name: name.into(),
            input,
            session_id: "exec-test".into(),
            message_id: "m".into(),
            tool_call_id: format!("call-{id}"),
            working_dir: Some(dir.to_path_buf()),
        };
        let line = serde_json::to_string(&frame).unwrap() + "\n";
        to_worker.write_all(line.as_bytes()).await.unwrap();
        let line = from_worker.next_line().await.unwrap().unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// The worker performs file and shell work in its own directory, and
    /// refuses tools that belong to the runtime.
    #[tokio::test]
    async fn worker_runs_workspace_tools_and_refuses_runtime_tools() {
        let dir = tempfile::tempdir().unwrap();
        let (mut to_worker, worker_in) = tokio::io::duplex(1 << 16);
        let (worker_out, from_worker) = tokio::io::duplex(1 << 16);
        let worker = tokio::spawn(run_worker(worker_in, worker_out));
        let mut from_worker = BufReader::new(from_worker).lines();

        let wrote = call(
            &mut to_worker,
            &mut from_worker,
            1,
            "write",
            serde_json::json!({"file_path": "note.txt", "content": "from the worker\n"}),
            dir.path(),
        )
        .await;
        assert!(matches!(
            wrote,
            Frame::Result {
                id: 1,
                error: None,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("note.txt")).unwrap(),
            "from the worker\n"
        );

        let ran = call(
            &mut to_worker,
            &mut from_worker,
            2,
            "bash",
            serde_json::json!({"command": "cat note.txt && pwd"}),
            dir.path(),
        )
        .await;
        let Frame::Result { output, error, .. } = ran else {
            panic!("expected a result")
        };
        assert_eq!(error, None);
        assert!(output.contains("from the worker"), "{output}");
        let real = dir.path().canonicalize().unwrap();
        assert!(output.contains(&*real.to_string_lossy()), "{output}");

        let refused = call(
            &mut to_worker,
            &mut from_worker,
            3,
            "todo",
            serde_json::json!({}),
            dir.path(),
        )
        .await;
        let Frame::Result { error, .. } = refused else {
            panic!("expected a result")
        };
        assert!(error.unwrap().contains("does not run in the execution"));

        drop(to_worker);
        worker.await.unwrap().unwrap();
        WORKER_MODE.store(false, Ordering::Relaxed);
    }
}
