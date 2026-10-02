use jcode_provider_protocol::{
    DEFAULT_MAX_FRAME_SIZE, Frame, PROTOCOL_VERSION, ProtocolError, ProviderInfo, decode, encode,
    negotiate_capabilities,
};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

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
}

impl Default for SubprocessConfig {
    fn default() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            environment: None,
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
    #[error("unexpected protocol frame: {0}")]
    UnexpectedFrame(String),
}

/// A bounded JSONL subprocess transport for external provider implementations.
/// The adapter owns the child and kills it on drop, so abandoned sessions do not
/// leave provider processes behind.
pub struct SubprocessProvider {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
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
            stdout: Mutex::new(BufReader::new(stdout)),
            config: config.clone(),
            requested_capabilities: capabilities.clone(),
        };
        adapter
            .send(Frame::Hello {
                protocol_version: PROTOCOL_VERSION.into(),
                client: client.into(),
                capabilities,
                max_frame_size: Some(config.max_frame_size),
            })
            .await?;
        Ok(adapter)
    }

    pub async fn send(&self, frame: Frame) -> Result<(), AdapterError> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&encode(&frame)?).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn next(&self) -> Result<Frame, AdapterError> {
        self.next_with_timeout(None).await
    }

    pub async fn next_with_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Frame, AdapterError> {
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
        let mut line = Vec::with_capacity(1024);
        loop {
            let byte = match stdout.read_u8().await {
                Ok(byte) => byte,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    let mut child = self.child.lock().await;
                    let status = child.try_wait()?.map(|status| status.code()).flatten();
                    return Err(AdapterError::ProcessExited(status));
                }
                Err(error) => return Err(AdapterError::Io(error)),
            };
            if byte == b'\n' {
                return Ok(decode(&line, self.config.max_frame_size)?);
            }
            line.push(byte);
            if line.len() > self.config.max_frame_size {
                return Err(AdapterError::Protocol(ProtocolError::TooLarge));
            }
        }
    }

    pub async fn handshake(&self) -> Result<Handshake, AdapterError> {
        let frame = self
            .next_with_timeout(Some(self.config.handshake_timeout))
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
        self.request(id.clone(), method, params).await?;
        let deadline = tokio::time::Instant::now() + self.config.request_timeout;
        let mut frames = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(AdapterError::Timeout);
            }
            let frame = self.next_with_timeout(Some(remaining)).await?;
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
}
