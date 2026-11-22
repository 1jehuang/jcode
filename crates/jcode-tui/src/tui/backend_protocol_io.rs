//! Backend protocol IO and request dispatch.

use super::*;

impl RemoteConnection {
    /// Decide what a single decoded protocol line means: a real event, a line
    /// to skip (blank/stray), or a fatal protocol/transport error.
    pub(super) fn classify_protocol_line(
        &self,
        line: Vec<u8>,
        stray_lines: &mut usize,
    ) -> LineOutcome {
        let text = match String::from_utf8(line) {
            Ok(text) => text,
            Err(error) => {
                *stray_lines += 1;
                let preview: String = String::from_utf8_lossy(error.as_bytes())
                    .chars()
                    .take(240)
                    .collect();
                crate::logging::warn(&format!(
                    "RemoteConnection::next_event: skipping stray non-UTF-8 protocol line {}/{} preview={:?} (session_id={:?}, client_instance_id={:?})",
                    *stray_lines,
                    MAX_STRAY_REMOTE_PROTOCOL_LINES,
                    preview,
                    self.session_id,
                    self.client_instance_id
                ));
                if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                    return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(
                        "too many stray non-JSON protocol lines".to_string(),
                    ));
                }
                return LineOutcome::Skip;
            }
        };
        let trimmed = text.trim_start();
        if trimmed.trim().is_empty() {
            crate::logging::warn(&format!(
                "RemoteConnection::next_event: skipping blank line (session_id={:?}, client_instance_id={:?})",
                self.session_id, self.client_instance_id
            ));
            return LineOutcome::Skip;
        }
        if !trimmed.starts_with('{') {
            *stray_lines += 1;
            let preview: String = text.chars().take(240).collect();
            crate::logging::warn(&format!(
                "RemoteConnection::next_event: skipping stray non-JSON protocol line {}/{} preview={:?} (session_id={:?}, client_instance_id={:?})",
                *stray_lines,
                MAX_STRAY_REMOTE_PROTOCOL_LINES,
                preview,
                self.session_id,
                self.client_instance_id
            ));
            if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(
                    "too many stray non-JSON protocol lines".to_string(),
                ));
            }
            return LineOutcome::Skip;
        }
        match self.request_timings.parse_event(&text) {
            Ok(ServerEvent::Done { id })
                if self
                    .control_done_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&id) =>
            {
                LineOutcome::Skip
            }
            Ok(event) => LineOutcome::Event(Box::new(event)),
            Err(error) => {
                // A single unparseable JSON line (e.g. the tail half of a frame
                // split by a lost write, or an event variant this client build
                // doesn't know) must not kill the whole session. Count it
                // against the stray-line budget and resync on the next line;
                // only give up if the stream keeps failing to parse (which
                // indicates a real protocol/version mismatch). See issue #422:
                // huge sessions used to die permanently on one corrupt frame.
                *stray_lines += 1;
                let preview: String = text.chars().take(240).collect();
                crate::logging::warn(&format!(
                    "RemoteConnection::next_event: skipping unparseable protocol line {}/{} error={} preview={:?} (session_id={:?}, client_instance_id={:?})",
                    *stray_lines,
                    MAX_STRAY_REMOTE_PROTOCOL_LINES,
                    error,
                    preview,
                    self.session_id,
                    self.client_instance_id
                ));
                if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                    return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(format!(
                        "too many unparseable protocol lines; last error: {}",
                        error
                    )));
                }
                LineOutcome::Skip
            }
        }
    }
}

impl RemoteConnection {
    /// Read the next event from the server.
    ///
    /// This is **cancellation safe** and may be used directly as a branch in a
    /// `tokio::select!`. Every byte pulled from the socket is moved into the
    /// persistent [`Self::read_buffer`] immediately, and the only `.await`
    /// point is [`AsyncBufReadExt::fill_buf`], which tokio guarantees reads
    /// nothing when its future is dropped. A previous implementation used
    /// `BufReader::read_line`, which tokio documents as **not** cancellation
    /// safe: when a large payload (e.g. a multi-megabyte `History` event for a
    /// session with images) was mid-read and a sibling `select!` branch (a
    /// redraw tick, terminal event, or bus event) completed first, the
    /// `read_line` future was dropped and the bytes it had already consumed
    /// from the socket were lost. That desynced the newline-framed protocol
    /// stream: the next read began mid-payload, surfaced as "stray non-JSON
    /// protocol line" warnings, the real `History` was discarded, and the
    /// session stayed stuck on "loading session…" until a manual `/restart`.
    pub async fn next_event(&mut self) -> RemoteRead {
        let mut stray_lines = 0usize;
        loop {
            // Serve any complete line already buffered before touching the
            // socket. This drains backlog left by a previous (possibly
            // cancelled) call before issuing a new read.
            if let Some(line) = self.take_buffered_line() {
                match self.classify_protocol_line(line, &mut stray_lines) {
                    LineOutcome::Event(event) => return RemoteRead::Event(*event),
                    LineOutcome::Skip => continue,
                    LineOutcome::Disconnect(reason) => return RemoteRead::Disconnected(reason),
                }
            }

            // No complete line yet: pull more bytes. `fill_buf` is cancel safe,
            // so if a `select!` peer wins the race here nothing is read or lost.
            let chunk = match self.reader.fill_buf().await {
                Ok(buf) => buf,
                Err(error) => {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: io error={} (session_id={:?}, client_instance_id={:?})",
                        error, self.session_id, self.client_instance_id
                    ));
                    return RemoteRead::Disconnected(RemoteDisconnectReason::Io(error.to_string()));
                }
            };
            if chunk.is_empty() {
                // EOF. Surface any trailing partial line for diagnostics, then
                // report a clean peer-closed disconnect.
                if self.read_buffer.is_empty() {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: peer closed (session_id={:?}, client_instance_id={:?})",
                        self.session_id, self.client_instance_id
                    ));
                } else {
                    let preview: String = String::from_utf8_lossy(&self.read_buffer)
                        .chars()
                        .take(240)
                        .collect();
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: peer closed mid-line, discarding {} buffered bytes preview={:?} (session_id={:?}, client_instance_id={:?})",
                        self.read_buffer.len(),
                        preview,
                        self.session_id,
                        self.client_instance_id
                    ));
                    self.read_buffer.clear();
                    self.read_buffer_scan_start = 0;
                }
                return RemoteRead::Disconnected(RemoteDisconnectReason::PeerClosed);
            }
            let len = chunk.len();
            if remote_protocol_frame_exceeds_limit(self.read_buffer.len(), len) {
                self.reader.consume(len);
                self.read_buffer.clear();
                self.read_buffer_scan_start = 0;
                return RemoteRead::Disconnected(RemoteDisconnectReason::Protocol(format!(
                    "remote protocol frame exceeded {} bytes",
                    MAX_REMOTE_PROTOCOL_FRAME_BYTES
                )));
            }
            self.read_buffer.extend_from_slice(chunk);
            self.reader.consume(len);
        }
    }
}

impl RemoteConnection {
    pub(super) async fn send_request_with_interrupt_trigger(
        &self,
        request: Request,
        interrupt_trigger: Option<&str>,
    ) -> Result<()> {
        let control_id = match &request {
            Request::Subscribe { id, .. }
            | Request::GetHistory { id }
            | Request::ResumeSession { id, .. }
            | Request::GetModelCatalog { id, .. }
            | Request::AccountUsage { id }
            | Request::GetState { id } => Some(*id),
            _ => None,
        };
        if let Some(id) = control_id {
            let mut ids = self
                .control_done_ids
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if ids.len() >= 256 {
                ids.pop_front();
            }
            ids.push_back(id);
        }
        let json = serde_json::to_string(&request)? + "\n";
        let interrupt_log = self.interrupt_request_log_fields(&request, interrupt_trigger);
        if let Some(fields) = &interrupt_log {
            crate::logging::info(&format!(
                "REMOTE_INTERRUPT_SEND_START {} json_bytes={}",
                fields,
                json.len()
            ));
        }

        let mut timing = super::super::cloud_latency::begin_request(&request);
        let total_start = Instant::now();
        let backlogged = self
            .outbound_pending
            .load(std::sync::atomic::Ordering::Acquire)
            > 0;
        let result = match self.enqueue_outbound(json.into_bytes()) {
            Err(error) => Err(error),
            Ok(_) if backlogged => Ok(()),
            // Surface prompt write failures (broken pipe) to the caller, but
            // never wait longer than a short bound: a full socket must not
            // stall the loop that drains server events.
            Ok(done) => match tokio::time::timeout(OUTBOUND_RESULT_WAIT, done).await {
                Ok(Ok(Err(error))) => Err(anyhow::anyhow!(error)),
                Ok(Err(_)) => Err(anyhow::anyhow!("remote connection writer closed")),
                Ok(Ok(Ok(()))) | Err(_) => Ok(()),
            },
        };
        if let Some(fields) = &interrupt_log {
            match &result {
                Ok(()) => crate::logging::info(&format!(
                    "REMOTE_INTERRUPT_SEND_OK {} queued total_ms={}",
                    fields,
                    total_start.elapsed().as_millis()
                )),
                Err(error) => crate::logging::warn(&format!(
                    "REMOTE_INTERRUPT_SEND_ERR {} total_ms={} error={}",
                    fields,
                    total_start.elapsed().as_millis(),
                    error
                )),
            }
        }
        if let Some(timing) = &mut timing {
            timing.finish(if result.is_ok() { "ok" } else { "error" });
        }
        result?;
        self.request_timings.request_sent(&request);
        Ok(())
    }
}
