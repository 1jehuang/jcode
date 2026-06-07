#![allow(dead_code)]

use crate::desktop_ipc::{
    DesktopIpcFrameError, DesktopWorkerToHostEnvelope, read_desktop_ipc_frame,
    write_desktop_ipc_frame,
};
use crate::desktop_protocol::{
    DesktopHostToWorkerMessage, DesktopProtocolEnvelope, DesktopWorkerToHostMessage,
};
use anyhow::{Context, Result, anyhow};
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};

pub(crate) struct DesktopWorkerConnection {
    child: Child,
    writer: DesktopWorkerIpcWriter<ChildStdin>,
    events: Receiver<Result<DesktopWorkerToHostEnvelope, DesktopIpcFrameError>>,
    reader_thread: Option<JoinHandle<()>>,
    initialized: bool,
}

impl DesktopWorkerConnection {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command
            .spawn()
            .context("failed to spawn desktop app worker")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("desktop app worker stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("desktop app worker stdout was not piped"))?;
        let (tx, events) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_desktop_ipc_frame::<DesktopWorkerToHostEnvelope>(&mut reader) {
                    Ok(Some(envelope)) => {
                        if tx.send(Ok(envelope)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            writer: DesktopWorkerIpcWriter::new(stdin),
            events,
            reader_thread: Some(reader_thread),
            initialized: false,
        })
    }

    pub(crate) fn child_id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn send(&mut self, message: DesktopHostToWorkerMessage) -> Result<()> {
        let is_initialize = matches!(message, DesktopHostToWorkerMessage::Initialize(_));
        self.writer.send(message).map_err(anyhow::Error::from)?;
        if is_initialize {
            self.initialized = true;
        }
        Ok(())
    }

    pub(crate) fn initialized(&self) -> bool {
        self.initialized
    }

    pub(crate) fn try_recv(
        &self,
    ) -> Option<Result<DesktopWorkerToHostMessage, DesktopIpcFrameError>> {
        match self.events.try_recv() {
            Ok(Ok(envelope)) => match envelope.validate_version() {
                Ok(()) => Some(Ok(envelope.payload)),
                Err(error) => Some(Err(error.into())),
            },
            Ok(Err(error)) => Some(Err(error)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to poll desktop app worker")
    }

    pub(crate) fn kill(mut self) -> Result<()> {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => self
                .child
                .kill()
                .context("failed to kill desktop app worker")?,
            Err(error) => {
                return Err(error).context("failed to poll desktop app worker before kill");
            }
        }
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        Ok(())
    }
}

pub(crate) struct DesktopWorkerIpcWriter<W> {
    writer: W,
    next_sequence: u64,
}

impl<W: Write> DesktopWorkerIpcWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer,
            next_sequence: 1,
        }
    }

    pub(crate) fn send(
        &mut self,
        message: DesktopHostToWorkerMessage,
    ) -> Result<(), DesktopIpcFrameError> {
        let envelope = DesktopProtocolEnvelope::new(self.next_sequence, message);
        self.next_sequence += 1;
        write_desktop_ipc_frame(&mut self.writer, &envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_protocol::{DesktopInputEvent, DesktopKeyEvent, DesktopKeyModifiers};

    #[test]
    fn worker_ipc_writer_assigns_monotonic_sequences() {
        let mut bytes = Vec::new();
        {
            let mut writer = DesktopWorkerIpcWriter::new(&mut bytes);

            writer
                .send(DesktopHostToWorkerMessage::Input(DesktopInputEvent::Key(
                    DesktopKeyEvent {
                        key: "a".to_string(),
                        text: Some("a".to_string()),
                        pressed: true,
                        modifiers: DesktopKeyModifiers::default(),
                    },
                )))
                .expect("first frame");
            writer
                .send(DesktopHostToWorkerMessage::SnapshotRequest { request_id: 99 })
                .expect("second frame");
        }

        let encoded = String::from_utf8(bytes).expect("utf8 frames");
        let mut lines = encoded.lines();
        let first: crate::desktop_ipc::DesktopHostToWorkerEnvelope =
            crate::desktop_ipc::decode_desktop_protocol_frame(lines.next().expect("first line"))
                .expect("decode first");
        let second: crate::desktop_ipc::DesktopHostToWorkerEnvelope =
            crate::desktop_ipc::decode_desktop_protocol_frame(lines.next().expect("second line"))
                .expect("decode second");

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert!(lines.next().is_none());
    }
}
