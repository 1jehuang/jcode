//! NDJSON framing helpers and a minimal blocking client.

use crate::{API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame};
use std::io::{self, BufRead, Write};

/// Errors from reading or writing a frame.
#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    Json(serde_json::Error),
    /// The stream closed cleanly.
    Eof,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "harness API I/O error: {e}"),
            Self::Json(e) => write!(f, "harness API JSON error: {e}"),
            Self::Eof => write!(f, "harness API stream closed"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for FrameError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Write one frame as a single NDJSON line and flush.
pub fn write_frame<W: Write, T: serde::Serialize>(
    writer: &mut W,
    frame: &T,
) -> Result<(), FrameError> {
    let mut line = serde_json::to_string(frame)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    Ok(())
}

/// Read one frame from a buffered reader. Skips blank lines.
pub fn read_frame<R: BufRead, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> Result<T, FrameError> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(FrameError::Eof);
        }
        if line.trim().is_empty() {
            continue;
        }
        return Ok(serde_json::from_str(line.trim())?);
    }
}

/// Minimal blocking harness client over any read/write pair.
///
/// Handles id assignment and the version handshake. Transport-agnostic:
/// pass a Unix socket, TCP stream, or an in-memory pipe for tests.
pub struct HarnessClient<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    next_id: u64,
}

impl<R: BufRead, W: Write> HarnessClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: 1,
        }
    }

    /// Perform the version handshake. Must be called first.
    pub fn hello(&mut self, client_name: &str) -> Result<ServerFrame, FrameError> {
        self.send(ApiRequest::Hello {
            min_version: API_VERSION_MAJOR,
            max_version: API_VERSION_MAJOR,
            client: client_name.to_string(),
        })?;
        self.recv()
    }

    /// Send a request, returning the assigned frame id.
    pub fn send(&mut self, request: ApiRequest) -> Result<u64, FrameError> {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(&mut self.writer, &ClientFrame::new(id, request))?;
        Ok(id)
    }

    /// Receive the next server frame, skipping unknown event kinds.
    pub fn recv(&mut self) -> Result<ServerFrame, FrameError> {
        loop {
            let frame: ServerFrame = read_frame(&mut self.reader)?;
            if matches!(frame.event, ApiEvent::Unknown) {
                continue;
            }
            return Ok(frame);
        }
    }
}
