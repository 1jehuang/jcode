//! Shared runner for synchronous hooks (gates, transformers, feedback).
//!
//! The child runs in its own process group. Stdin is written then closed,
//! stdout and stderr are read by bounded reader tasks, and everything runs
//! under one deadline. When the deadline passes the whole process group is
//! killed, so a grandchild that inherited the output pipes cannot keep a
//! reader blocked past the timeout.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Bytes kept per output stream. Anything beyond is drained and discarded.
const OUTPUT_CAPTURE_LIMIT: usize = 1024 * 1024;

/// What to do with the child's stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StderrMode {
    Capture,
    Discard,
}

/// Completed run of a synchronous hook command.
#[derive(Debug)]
pub(super) struct RunOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub(super) enum RunError {
    Spawn(std::io::Error),
    Io(std::io::Error),
    Timeout,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "failed to start: {error}"),
            Self::Io(error) => write!(f, "failed: {error}"),
            Self::Timeout => write!(f, "timed out"),
        }
    }
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(kept);
        }
        let room = OUTPUT_CAPTURE_LIMIT.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..read.min(room)]);
    }
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: killpg only sends a signal to the process group we created.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

/// Run `std_cmd` with `stdin` bytes, waiting at most `timeout` for the whole
/// exchange. On timeout the process group is killed.
pub(super) async fn run_command(
    std_cmd: std::process::Command,
    stdin: &[u8],
    timeout: Duration,
    stderr_mode: StderrMode,
) -> Result<RunOutput, RunError> {
    let mut std_cmd = std_cmd;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std_cmd.process_group(0);
    }
    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(match stderr_mode {
            StderrMode::Capture => Stdio::piped(),
            StderrMode::Discard => Stdio::null(),
        })
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;
    let pid = child.id();

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdin_pipe = child.stdin.take();

    let exchange = async {
        let write_stdin = async {
            if let Some(mut pipe) = stdin_pipe {
                // A hook that exits without reading stdin closes the pipe.
                let _ = pipe.write_all(stdin).await;
            }
        };
        let read_stdout = async {
            match stdout_pipe {
                Some(pipe) => read_bounded(pipe).await,
                None => Ok(Vec::new()),
            }
        };
        let read_stderr = async {
            match stderr_pipe {
                Some(pipe) => read_bounded(pipe).await,
                None => Ok(Vec::new()),
            }
        };
        let (_, stdout, stderr, status) =
            tokio::join!(write_stdin, read_stdout, read_stderr, child.wait());
        Ok::<_, std::io::Error>(RunOutput {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    };

    match tokio::time::timeout(timeout, exchange).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            kill_group(pid);
            Err(RunError::Io(error))
        }
        Err(_elapsed) => {
            kill_group(pid);
            Err(RunError::Timeout)
        }
    }
}
