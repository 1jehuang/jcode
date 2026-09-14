use jcode_provider_protocol::{encode, decode, Frame, ProtocolError, PROTOCOL_VERSION};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum AdapterError { #[error("process I/O: {0}")] Io(#[from] std::io::Error), #[error("protocol: {0}")] Protocol(#[from] ProtocolError), #[error("unexpected frame")] UnexpectedFrame }

pub struct SubprocessProvider { child: Child, stdin: Mutex<ChildStdin>, stdout: Mutex<BufReader<ChildStdout>>, max_frame_size: usize }

impl SubprocessProvider {
    pub async fn spawn(program: impl AsRef<std::ffi::OsStr>, args: &[impl AsRef<std::ffi::OsStr>], client: &str, capabilities: Vec<String>) -> Result<Self, AdapterError> {
        let mut command = Command::new(program); command.args(args.iter().map(|a| a.as_ref())).stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn()?; let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("missing stdin"))?; let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("missing stdout"))?;
        let adapter = Self { child, stdin: Mutex::new(stdin), stdout: Mutex::new(BufReader::new(stdout)), max_frame_size: 16 * 1024 * 1024 };
        adapter.send(Frame::Hello { protocol_version: PROTOCOL_VERSION.into(), client: client.into(), capabilities }).await?;
        Ok(adapter)
    }
    pub async fn send(&self, frame: Frame) -> Result<(), AdapterError> { let mut stdin=self.stdin.lock().await; stdin.write_all(&encode(&frame)?).await?; stdin.flush().await?; Ok(()) }
    pub async fn next(&self) -> Result<Frame, AdapterError> { let mut line=Vec::new(); let n=self.stdout.lock().await.read_until(b'\n', &mut line).await?; if n==0 { return Err(AdapterError::UnexpectedFrame); } Ok(decode(line.strip_suffix(b"\n").unwrap_or(&line), self.max_frame_size)?) }
    pub async fn request(&self, id: impl Into<String>, method: impl Into<String>, params: serde_json::Value) -> Result<(), AdapterError> { self.send(Frame::Request { protocol_version: PROTOCOL_VERSION.into(), id:id.into(), method:method.into(), params }).await }
    pub async fn cancel(&self, request_id: impl Into<String>) -> Result<(), AdapterError> { self.send(Frame::Cancel { protocol_version: PROTOCOL_VERSION.into(), request_id:request_id.into() }).await }
    pub async fn kill(&mut self) -> Result<(), AdapterError> { self.child.kill().await.map_err(Into::into) }
}

#[cfg(test)]
mod tests { use super::*; #[tokio::test] async fn handshake_stream_and_cancel() { let script="import sys,json\nfor l in sys.stdin:\n f=json.loads(l); k=f['kind'];\n if k=='hello': print(json.dumps({'kind':'hello_ok','protocol_version':'1.0','provider':{'id':'p','name':'P','version':'1'},'capabilities':['streaming']}),flush=True)\n elif k=='request': print(json.dumps({'kind':'event','protocol_version':'1.0','request_id':f['id'],'event':'chunk','payload':{'text':'ok'}}),flush=True)"; let mut p=SubprocessProvider::spawn("python3", &["-c",script], "test", vec!["streaming".into()]).await.unwrap(); assert!(matches!(p.next().await.unwrap(),Frame::HelloOk{..})); p.request("r1","complete",serde_json::json!({})).await.unwrap(); assert!(matches!(p.next().await.unwrap(),Frame::Event{request_id,..} if request_id=="r1")); p.cancel("r1").await.unwrap(); p.kill().await.unwrap(); } }
