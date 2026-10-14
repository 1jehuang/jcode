use jcode_provider_protocol::{
    DEFAULT_MAX_FRAME_SIZE, Frame, PROTOCOL_VERSION, ProtocolError, ProviderInfo, decode, encode,
    negotiate_capabilities,
};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

mod confine;

const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct SubprocessConfig {
    pub max_frame_size: usize,
    pub handshake_timeout: Duration,
    pub request_timeout: Duration,
    /// When set, replace the inherited environment with exactly these values.
    /// `None` preserves the legacy behavior for direct adapter callers.
    pub environment: Option<Vec<(OsString, OsString)>>,
    /// When true, the child is started only if filesystem access can be
    /// confined. Callers set this when the provider was not granted the
    /// filesystem permission.
    pub confine_filesystem: bool,
}

impl Default for SubprocessConfig {
    fn default() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            environment: None,
            confine_filesystem: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Handshake {
    pub provider: ProviderInfo,
    pub capabilities: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("process I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("provider process exited with status {0:?}")]
    ProcessExited(Option<i32>),
    #[error("provider request timed out")]
    Timeout,
    #[error("provider transport failed and cannot be reused")]
    TransportFailed,
    #[error("unexpected protocol frame: {0}")]
    UnexpectedFrame(String),
}

struct StdoutState {
    reader: BufReader<ChildStdout>,
    /// Bytes of the current JSONL frame already pulled from stdout.
    ///
    /// A read deadline drops the in-flight read future. Keeping this buffer in
    /// the mutex (instead of on that future's stack) lets the next read resume
    /// the same frame instead of decoding the leftover suffix as a new one.
    partial: Vec<u8>,
}

/// A bounded JSONL subprocess transport for external provider implementations.
/// The adapter owns the child and kills it on drop, so abandoned sessions do not
/// leave provider processes behind.
pub struct SubprocessProvider {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<StdoutState>,
    /// Serializes a full request exchange so two collectors cannot consume one
    /// stdout stream. Cancellation writes only need the stdin mutex.
    exchange: Mutex<()>,
    broken: AtomicBool,
    config: SubprocessConfig,
    requested_capabilities: Vec<String>,
}

impl SubprocessProvider {
    pub async fn spawn<P, A>(
        program: P,
        args: &[A],
        client: &str,
        capabilities: Vec<String>,
    ) -> Result<Self, AdapterError>
    where
        P: AsRef<OsStr>,
        A: AsRef<OsStr>,
    {
        Self::spawn_with_config(
            program,
            args,
            client,
            capabilities,
            SubprocessConfig::default(),
        )
        .await
    }

    pub async fn spawn_with_config<P, A>(
        program: P,
        args: &[A],
        client: &str,
        capabilities: Vec<String>,
        config: SubprocessConfig,
    ) -> Result<Self, AdapterError>
    where
        P: AsRef<OsStr>,
        A: AsRef<OsStr>,
    {
        if config.max_frame_size == 0 {
            return Err(AdapterError::Protocol(ProtocolError::TooLarge));
        }

        let program = program.as_ref();
        let mut command = Command::new(program);
        command
            .args(args.iter().map(AsRef::as_ref))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(environment) = &config.environment {
            command.env_clear();
            command.envs(environment.iter().map(|(key, value)| (key, value)));
        }
        if config.confine_filesystem {
            confine::install(&mut command, program)?;
        }
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("missing provider stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("missing provider stdout"))?;
        let adapter = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(StdoutState {
                reader: BufReader::new(stdout),
                partial: Vec::with_capacity(1024),
            }),
            exchange: Mutex::new(()),
            broken: AtomicBool::new(false),
            config: config.clone(),
            requested_capabilities: capabilities.clone(),
        };
        let hello_deadline = Instant::now() + config.handshake_timeout;
        adapter
            .send_until(
                Frame::Hello {
                    protocol_version: PROTOCOL_VERSION.into(),
                    client: client.into(),
                    capabilities,
                    max_frame_size: Some(config.max_frame_size),
                },
                Some(hello_deadline),
            )
            .await?;
        Ok(adapter)
    }

    pub async fn send(&self, frame: Frame) -> Result<(), AdapterError> {
        self.send_until(frame, None).await
    }

    async fn send_until(
        &self,
        frame: Frame,
        deadline: Option<Instant>,
    ) -> Result<(), AdapterError> {
        if self.broken.load(Ordering::Acquire) {
            return Err(AdapterError::TransportFailed);
        }
        let bytes = encode(&frame)?;
        let mut stdin = self.stdin.lock().await;
        let write = async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await?;
            Ok::<(), std::io::Error>(())
        };
        let write_result = if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdapterError::Timeout);
            }
            match tokio::time::timeout(remaining, write).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    self.broken.store(true, Ordering::Release);
                    drop(stdin);
                    let _ = self.child.lock().await.start_kill();
                    return Err(AdapterError::Timeout);
                }
            }
        } else {
            write.await
        };
        write_result.map_err(AdapterError::Io)
    }

    pub async fn next(&self) -> Result<Frame, AdapterError> {
        self.next_with_timeout(None).await
    }

    pub async fn next_with_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Frame, AdapterError> {
        let _exchange = self.exchange.lock().await;
        self.read_frame_with_timeout(timeout).await
    }

    async fn read_frame_with_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Frame, AdapterError> {
        if self.broken.load(Ordering::Acquire) {
            return Err(AdapterError::TransportFailed);
        }
        let read = self.next_inner();
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, read)
                .await
                .map_err(|_| AdapterError::Timeout)?,
            None => read.await,
        }
    }

    async fn next_inner(&self) -> Result<Frame, AdapterError> {
        let mut stdout = self.stdout.lock().await;
        loop {
            let byte = match stdout.reader.read_u8().await {
                Ok(byte) => byte,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    let mut child = self.child.lock().await;
                    let status = child.try_wait()?.and_then(|status| status.code());
                    return Err(AdapterError::ProcessExited(status));
                }
                Err(error) => return Err(AdapterError::Io(error)),
            };
            if byte == b'\n' {
                let line = std::mem::take(&mut stdout.partial);
                return Ok(decode(&line, self.config.max_frame_size)?);
            }
            stdout.partial.push(byte);
            if stdout.partial.len() > self.config.max_frame_size {
                stdout.partial.clear();
                return Err(AdapterError::Protocol(ProtocolError::TooLarge));
            }
        }
    }

    pub async fn handshake(&self) -> Result<Handshake, AdapterError> {
        let _exchange = self.exchange.lock().await;
        let frame = self
            .read_frame_with_timeout(Some(self.config.handshake_timeout))
            .await?;
        match frame {
            Frame::HelloOk {
                provider,
                capabilities,
                ..
            } => Ok(Handshake {
                provider,
                capabilities: negotiate_capabilities(&self.requested_capabilities, &capabilities),
            }),
            Frame::Response { error, .. } if error.is_some() => Err(AdapterError::UnexpectedFrame(
                "provider rejected handshake".into(),
            )),
            other => Err(AdapterError::UnexpectedFrame(format!(
                "expected hello_ok, received {other:?}"
            ))),
        }
    }

    pub async fn request(
        &self,
        id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
    ) -> Result<(), AdapterError> {
        self.send(Frame::Request {
            protocol_version: PROTOCOL_VERSION.into(),
            id: id.into(),
            method: method.into(),
            params,
        })
        .await
    }

    /// Send a request and collect its stream until the matching response.
    /// Events belonging to the request, including native tool calls and tool
    /// results, are returned in wire order.
    pub async fn request_and_collect(
        &self,
        id: impl Into<String>,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Vec<Frame>, AdapterError> {
        let id = id.into();
        let _exchange = self.exchange.lock().await;
        let deadline = Instant::now() + self.config.request_timeout;
        self.send_until(
            Frame::Request {
                protocol_version: PROTOCOL_VERSION.into(),
                id: id.clone(),
                method: method.into(),
                params,
            },
            Some(deadline),
        )
        .await?;
        let mut frames = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AdapterError::Timeout);
            }
            let frame = self.read_frame_with_timeout(Some(remaining)).await?;
            match &frame {
                Frame::Event { request_id, .. } if request_id == &id => frames.push(frame),
                Frame::Response {
                    id: response_id, ..
                } if response_id == &id => {
                    frames.push(frame);
                    return Ok(frames);
                }
                Frame::HelloOk { .. } => {
                    return Err(AdapterError::UnexpectedFrame(
                        "received hello_ok after handshake".into(),
                    ));
                }
                Frame::Event { request_id, .. } => {
                    return Err(AdapterError::UnexpectedFrame(format!(
                        "event for another request while waiting for {id}: {request_id}"
                    )));
                }
                other => {
                    return Err(AdapterError::UnexpectedFrame(format!(
                        "unexpected frame while waiting for {id}: {other:?}"
                    )));
                }
            }
        }
    }

    pub async fn cancel(&self, request_id: impl Into<String>) -> Result<(), AdapterError> {
        self.send(Frame::Cancel {
            protocol_version: PROTOCOL_VERSION.into(),
            request_id: request_id.into(),
        })
        .await
    }

    pub async fn kill(&self) -> Result<(), AdapterError> {
        self.child.lock().await.kill().await.map_err(Into::into)
    }

    pub async fn wait(&self) -> Result<std::process::ExitStatus, AdapterError> {
        Ok(self.child.lock().await.wait().await?)
    }
}

impl Drop for SubprocessProvider {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_provider_protocol::{CAP_CANCELLATION, CAP_NATIVE_TOOLS, CAP_STREAMING};

    fn provider_script() -> OsString {
        OsString::from(concat!(
            "import sys,json\n",
            "for line in sys.stdin:\n",
            " f=json.loads(line)\n",
            " if f['kind']=='hello':\n",
            "  print(json.dumps({'kind':'hello_ok','protocol_version':'0.1','provider':{'id':'p','name':'P','version':'1'},'capabilities':['streaming','native_tools']}),flush=True)\n",
            " elif f['kind']=='request':\n",
            "  print(json.dumps({'kind':'event','protocol_version':'0.1','request_id':f['id'],'event':'native_tool_call','payload':{'call_id':'call-1'}}),flush=True)\n",
            "  print(json.dumps({'kind':'response','protocol_version':'0.1','id':f['id'],'ok':True,'result':{'text':'ok'}}),flush=True)\n",
            " elif f['kind']=='cancel':\n",
            "  print(json.dumps({'kind':'response','protocol_version':'0.1','id':f['request_id'],'ok':False,'error':{'code':'cancelled','message':'cancelled'}}),flush=True)\n",
        ))
    }

    #[tokio::test]
    async fn handshake_stream_correlation_and_cancel() {
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", provider_script().to_string_lossy().as_ref()],
            "test",
            vec![
                CAP_STREAMING.into(),
                CAP_NATIVE_TOOLS.into(),
                CAP_CANCELLATION.into(),
            ],
            SubprocessConfig {
                request_timeout: Duration::from_secs(2),
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        let handshake = provider.handshake().await.unwrap();
        assert_eq!(handshake.provider.id, "p");
        assert_eq!(
            handshake.capabilities,
            vec![CAP_STREAMING, CAP_NATIVE_TOOLS]
        );
        let frames = provider
            .request_and_collect("r1", "complete", serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            matches!(frames.first(), Some(Frame::Event { request_id, .. }) if request_id == "r1")
        );
        assert!(matches!(frames.last(), Some(Frame::Response { id, ok: true, .. }) if id == "r1"));
        provider.cancel("r2").await.unwrap();
        provider.kill().await.unwrap();
    }

    #[tokio::test]
    async fn read_timeout_and_drop_are_bounded() {
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", "import time; time.sleep(5)"],
            "test",
            vec![],
            SubprocessConfig {
                handshake_timeout: Duration::from_millis(20),
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            provider.handshake().await,
            Err(AdapterError::Timeout)
        ));
        provider.kill().await.unwrap();
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_unbounded_growth() {
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", "print('x' * 10000, flush=True)"],
            "test",
            vec![],
            SubprocessConfig {
                max_frame_size: 64,
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            provider.next().await,
            Err(AdapterError::Protocol(ProtocolError::TooLarge))
        ));
    }

    #[tokio::test]
    async fn explicit_environment_does_not_leak_parent_variables() {
        let script = concat!(
            "import json,os,sys\n",
            "for line in sys.stdin:\n",
            " f=json.loads(line)\n",
            " if f['kind']=='hello':\n",
            "  print(json.dumps({'kind':'hello_ok','protocol_version':'0.1','provider':{'id':'env','name':'Env','version':'1'},'capabilities':[]}),flush=True)\n",
            " elif f['kind']=='request':\n",
            "  print(json.dumps({'kind':'response','protocol_version':'0.1','id':f['id'],'ok':True,'result':{'has_home':'HOME' in os.environ,'has_path':'PATH' in os.environ}}),flush=True)\n",
        );
        let environment = std::env::var_os("PATH")
            .map(|path| vec![(OsString::from("PATH"), path)])
            .unwrap_or_default();
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", script],
            "test",
            vec![],
            SubprocessConfig {
                environment: Some(environment),
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        provider.handshake().await.unwrap();
        let frames = provider
            .request_and_collect("env-check", "complete", serde_json::json!({}))
            .await
            .unwrap();
        let Some(Frame::Response {
            result: Some(result),
            ..
        }) = frames.last()
        else {
            panic!("expected response result");
        };
        assert_eq!(result["has_home"], false);
        assert_eq!(result["has_path"], true);
    }

    #[tokio::test]
    async fn timed_out_partial_frame_resumes_instead_of_desynchronizing() {
        let script = "import sys,time\n\
            sys.stdout.write('{\"kind\":\"hello_ok\",\"protocol_version\":\"0.1\",\"provider\":{\"id\":\"p\",\"name\":\"P\",\"version\":\"1\"},\"capabilities\":[]}\\n')\n\
            sys.stdout.flush()\n\
            sys.stdin.readline()\n\
            sys.stdout.write('{\"kind\":\"response\",\"protocol_version\":\"0.1\",\"id\":\"r1\",\"ok\":true,\"result\":{}}')\n\
            sys.stdout.flush()\n\
            time.sleep(1.0)\n\
            sys.stdout.write('\\n')\n\
            sys.stdout.flush()\n";
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", script],
            "test",
            vec![],
            SubprocessConfig {
                request_timeout: Duration::from_millis(200),
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        provider.handshake().await.unwrap();
        assert!(matches!(
            provider
                .request_and_collect("r1", "complete", serde_json::json!({}))
                .await,
            Err(AdapterError::Timeout)
        ));
        let frame = provider.next().await.unwrap();
        assert!(matches!(frame, Frame::Response { ok: true, .. }));
    }

    #[tokio::test]
    async fn concurrent_collections_do_not_steal_frames() {
        let script = "import sys,json\n\
            for line in sys.stdin:\n\
            \tf=json.loads(line)\n\
            \tif f['kind']=='hello':\n\
            \t\tprint(json.dumps({'kind':'hello_ok','protocol_version':'0.1','provider':{'id':'p','name':'P','version':'1'},'capabilities':[]}),flush=True)\n\
            \telif f['kind']=='request':\n\
            \t\tprint(json.dumps({'kind':'response','protocol_version':'0.1','id':f['id'],'ok':True,'result':{'id':f['id']}}),flush=True)\n";
        let provider = std::sync::Arc::new(
            SubprocessProvider::spawn_with_config(
                "python3",
                &["-c", script],
                "test",
                vec![],
                SubprocessConfig {
                    request_timeout: Duration::from_secs(2),
                    ..SubprocessConfig::default()
                },
            )
            .await
            .unwrap(),
        );
        provider.handshake().await.unwrap();
        let first = {
            let provider = std::sync::Arc::clone(&provider);
            tokio::spawn(async move {
                provider
                    .request_and_collect("a", "complete", serde_json::json!({}))
                    .await
            })
        };
        let second = {
            let provider = std::sync::Arc::clone(&provider);
            tokio::spawn(async move {
                provider
                    .request_and_collect("b", "complete", serde_json::json!({}))
                    .await
            })
        };
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert!(matches!(first.last(), Some(Frame::Response { id, .. }) if id == "a"));
        assert!(matches!(second.last(), Some(Frame::Response { id, .. }) if id == "b"));
    }

    #[tokio::test]
    async fn request_write_respects_deadline_when_provider_stops_reading() {
        let provider = SubprocessProvider::spawn_with_config(
            "python3",
            &["-c", "import time; time.sleep(30)"],
            "test",
            vec![],
            SubprocessConfig {
                handshake_timeout: Duration::from_secs(2),
                request_timeout: Duration::from_millis(200),
                ..SubprocessConfig::default()
            },
        )
        .await
        .unwrap();
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            provider.request_and_collect(
                "r1",
                "complete",
                serde_json::json!({ "blob": "x".repeat(2 * 1024 * 1024) }),
            ),
        )
        .await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "request write blocked for {elapsed:?}"
        );
        assert!(matches!(
            result,
            Ok(Err(AdapterError::Timeout | AdapterError::TransportFailed))
        ));
    }
}
