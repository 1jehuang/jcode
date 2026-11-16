//! NDJSON framing helpers and a minimal blocking client.

use crate::{API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame};
use std::io::{self, BufRead, Read, Write};

// Match the TUI transport ceiling. Never allocate an unbounded NDJSON line
// while waiting for a newline in corrupt history or a misbehaving peer.
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

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
    read_frame_with_limit(reader, MAX_FRAME_BYTES)
}

fn read_frame_with_limit<R: BufRead, T: serde::de::DeserializeOwned>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<T, FrameError> {
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let n = (&mut *reader)
            .take(max_bytes as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if n > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("harness API frame exceeds {max_bytes} bytes"),
            )
            .into());
        }
        if n == 0 {
            return Err(FrameError::Eof);
        }
        // A socket drop during a large history frame is a transport failure,
        // not proof of corrupt data. Only validate complete NDJSON lines.
        if !bytes.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "harness API frame interrupted before newline",
            )
            .into());
        }
        let line = std::str::from_utf8(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if line.trim().is_empty() {
            continue;
        }
        return Ok(serde_json::from_str(line.trim())?);
    }
}

#[cfg(test)]
mod framing_limits {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn unterminated_frames_are_retryable_transport_failures() {
        for bytes in [
            b"{\"history\":".as_slice(),
            b"{\"text\":\"\xe2\x82".as_slice(),
            b"{}".as_slice(),
        ] {
            let error = read_frame::<_, serde_json::Value>(&mut Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(error, FrameError::Io(ref error) if error.kind() == io::ErrorKind::UnexpectedEof)
            );
        }
    }

    #[test]
    fn newline_terminated_invalid_json_is_a_protocol_failure() {
        let error =
            read_frame::<_, serde_json::Value>(&mut Cursor::new(b"{broken}\n")).unwrap_err();
        assert!(matches!(error, FrameError::Json(_)));
    }

    #[test]
    fn over_limit_frame_stops_reading_without_waiting_for_newline() {
        let mut reader = Cursor::new(vec![b'x'; 1024]);
        let error = read_frame_with_limit::<_, serde_json::Value>(&mut reader, 16).unwrap_err();
        assert!(
            matches!(error, FrameError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData)
        );
        assert_eq!(
            reader.position(),
            17,
            "do not consume the entire oversized payload"
        );
    }

    #[test]
    fn valid_frame_at_limit_and_blank_lines_are_supported() {
        let mut reader = Cursor::new(b"\n \n{\"ok\":true}\n".as_slice());
        let value: serde_json::Value = read_frame_with_limit(&mut reader, 12).unwrap();
        assert_eq!(value, serde_json::json!({"ok": true}));
    }

    #[test]
    fn large_whitespace_frame_is_not_allowed_to_bypass_the_limit() {
        let mut reader = Cursor::new(b"                 \n{}\n".as_slice());
        assert!(matches!(
            read_frame_with_limit::<_, serde_json::Value>(&mut reader, 16),
            Err(FrameError::Io(_))
        ));
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
            supports_user_response: false,
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
