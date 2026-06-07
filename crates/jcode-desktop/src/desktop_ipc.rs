#![allow(dead_code)]

use crate::desktop_protocol::{
    DesktopHostToWorkerMessage, DesktopProtocolCompatibilityError, DesktopProtocolEnvelope,
    DesktopWorkerToHostMessage,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, BufRead, Write};

pub(crate) type DesktopHostToWorkerEnvelope = DesktopProtocolEnvelope<DesktopHostToWorkerMessage>;
pub(crate) type DesktopWorkerToHostEnvelope = DesktopProtocolEnvelope<DesktopWorkerToHostMessage>;

#[derive(Debug)]
pub(crate) enum DesktopIpcFrameError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(DesktopProtocolCompatibilityError),
    EmptyFrame,
}

impl std::fmt::Display for DesktopIpcFrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "desktop IPC I/O error: {error}"),
            Self::Json(error) => write!(formatter, "desktop IPC JSON error: {error}"),
            Self::Protocol(error) => write!(formatter, "desktop IPC protocol error: {error}"),
            Self::EmptyFrame => write!(formatter, "desktop IPC frame was empty"),
        }
    }
}

impl std::error::Error for DesktopIpcFrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::EmptyFrame => None,
        }
    }
}

impl From<io::Error> for DesktopIpcFrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for DesktopIpcFrameError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<DesktopProtocolCompatibilityError> for DesktopIpcFrameError {
    fn from(error: DesktopProtocolCompatibilityError) -> Self {
        Self::Protocol(error)
    }
}

pub(crate) fn encode_desktop_ipc_frame<T: Serialize>(
    message: &T,
) -> Result<String, DesktopIpcFrameError> {
    let mut frame = serde_json::to_string(message)?;
    frame.push('\n');
    Ok(frame)
}

pub(crate) fn write_desktop_ipc_frame<T: Serialize>(
    writer: &mut impl Write,
    message: &T,
) -> Result<(), DesktopIpcFrameError> {
    writer.write_all(encode_desktop_ipc_frame(message)?.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn read_desktop_ipc_frame<T: DeserializeOwned>(
    reader: &mut impl BufRead,
) -> Result<Option<T>, DesktopIpcFrameError> {
    let mut line = String::new();
    let bytes = reader.read_line(&mut line)?;
    if bytes == 0 {
        return Ok(None);
    }
    decode_desktop_ipc_frame(&line).map(Some)
}

pub(crate) fn decode_desktop_ipc_frame<T: DeserializeOwned>(
    line: &str,
) -> Result<T, DesktopIpcFrameError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Err(DesktopIpcFrameError::EmptyFrame);
    }
    Ok(serde_json::from_str(trimmed)?)
}

pub(crate) fn decode_desktop_protocol_frame<T: DeserializeOwned>(
    line: &str,
) -> Result<DesktopProtocolEnvelope<T>, DesktopIpcFrameError> {
    let envelope: DesktopProtocolEnvelope<T> = decode_desktop_ipc_frame(line)?;
    envelope.validate_version()?;
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_protocol::{
        DesktopHostToWorkerMessage, DesktopInputEvent, DesktopKeyEvent, DesktopKeyModifiers,
    };
    use std::io::Cursor;

    #[test]
    fn ipc_frame_round_trips_host_to_worker_message() {
        let message = DesktopProtocolEnvelope::new(
            42,
            DesktopHostToWorkerMessage::Input(DesktopInputEvent::Key(DesktopKeyEvent {
                key: "Enter".to_string(),
                text: Some("\n".to_string()),
                pressed: true,
                modifiers: DesktopKeyModifiers::default(),
            })),
        );

        let encoded = encode_desktop_ipc_frame(&message).expect("encode frame");
        assert!(encoded.ends_with('\n'));

        let decoded: DesktopHostToWorkerEnvelope =
            decode_desktop_protocol_frame(&encoded).expect("decode frame");
        assert_eq!(decoded, message);
    }

    #[test]
    fn ipc_read_write_frame_round_trips() {
        let message = DesktopProtocolEnvelope::new(
            3,
            DesktopHostToWorkerMessage::SnapshotRequest { request_id: 7 },
        );
        let mut bytes = Vec::new();
        write_desktop_ipc_frame(&mut bytes, &message).expect("write frame");

        let mut reader = Cursor::new(bytes);
        let decoded: DesktopHostToWorkerEnvelope = read_desktop_ipc_frame(&mut reader)
            .expect("read frame")
            .expect("frame present");
        assert_eq!(decoded, message);

        let eof: Option<DesktopHostToWorkerEnvelope> =
            read_desktop_ipc_frame(&mut reader).expect("read eof");
        assert!(eof.is_none());
    }

    #[test]
    fn ipc_rejects_protocol_version_mismatch() {
        let mut message = DesktopProtocolEnvelope::new(
            1,
            DesktopHostToWorkerMessage::SnapshotRequest { request_id: 1 },
        );
        message.protocol_version += 1;
        let encoded = encode_desktop_ipc_frame(&message).expect("encode frame");

        let error = decode_desktop_protocol_frame::<DesktopHostToWorkerMessage>(&encoded)
            .expect_err("version mismatch");
        assert!(matches!(error, DesktopIpcFrameError::Protocol(_)));
    }
}
