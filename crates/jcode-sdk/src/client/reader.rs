use super::*;

/// The reader thread: correlates replies, fans stream events out.
pub(super) fn spawn_reader(inner: Arc<Inner>, mut reader: Box<dyn BufRead + Send>) {
    std::thread::spawn(move || {
        loop {
            let frame = match read_frame::<_, ServerFrame>(&mut reader) {
                Ok(frame) => frame,
                Err(error) => {
                    // Publish before closing pending requests/subscriptions, so
                    // awakened callers observe the actual cause, not generic EOF.
                    if let Ok(mut reason) = inner.disconnect_reason.lock() {
                        *reason = Some(error.into());
                    }
                    break;
                }
            };
            // Unknown kinds are skipped silently, per the protocol's
            // forward-compatibility rule.
            if matches!(frame.event, ApiEvent::Unknown) {
                continue;
            }
            if let Some(reply_to) = frame.reply_to {
                let waiter = inner
                    .pending
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.remove(&reply_to));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(frame);
                    continue;
                }
            }
            // Not a reply anyone is waiting on: a stream event. Dead
            // subscriptions are dropped here as well as in `EventStream::drop`,
            // so a leaked stream cannot grow the fan-out list forever.
            if let Ok(mut subs) = inner.subscribers.lock() {
                let session = event_session(&frame.event);
                subs.retain(|(_, filter, sink)| {
                    match (filter, session) {
                        // A filtered subscription only wants its own session's
                        // events. Events that name *no* session still go to it:
                        // `error` is the one that matters, since the harness
                        // sends it instead of `turn_done`, and dropping it
                        // leaves a turn waiting forever for an end that will
                        // never come.
                        (Some(want), Some(got)) if want != got => true,
                        _ => sink.send(frame.event.clone()).is_ok(),
                    }
                });
            }
        }
        if let Some(shutdown) = &inner.shutdown {
            shutdown();
        }
        inner.closed.store(true, Ordering::Relaxed);
        // Fail everything in flight rather than leaving callers blocked on a
        // reply that can never arrive.
        if let Ok(mut pending) = inner.pending.lock() {
            pending.clear();
        }
        if let Ok(mut subs) = inner.subscribers.lock() {
            subs.clear();
        }
    });
}

/// The session an event belongs to, when it names one.
fn event_session(event: &ApiEvent) -> Option<&str> {
    use ApiEvent::*;
    match event {
        TextDelta { session_id, .. }
        | TextDone { session_id, .. }
        | TextReplace { session_id, .. }
        | ReasoningDelta { session_id, .. }
        | ReasoningDone { session_id, .. }
        | ToolStart { session_id, .. }
        | ToolInputDelta { session_id, .. }
        | ToolExec { session_id, .. }
        | ToolDone { session_id, .. }
        | ToolCall { session_id, .. }
        | Tools { session_id, .. }
        | SidePanelState { session_id, .. }
        | TokenUsage { session_id, .. }
        | ContextCapChanged { session_id, .. }
        | KvCacheMiss { session_id, .. }
        | TurnDone { session_id, .. }
        | SoftInterruptInjected { session_id, .. }
        | TurnStopped { session_id, .. }
        | BackgroundProgress { session_id, .. }
        | MessageAccepted { session_id, .. }
        | PermissionRequest { session_id, .. }
        | SessionStatus { session_id, .. }
        | SessionRecovery { session_id, .. }
        | ModelInfo { session_id, .. }
        | RuntimeInfo { session_id, .. }
        | ConnectionPhase { session_id, .. }
        | Models { session_id, .. }
        | Compacted { session_id, .. }
        | SessionRenamed { session_id, .. }
        | RewindCheckpoints { session_id, .. }
        | RewindComplete { session_id, .. }
        | History { session_id, .. } => Some(session_id.as_str()),
        Attached { session } => Some(session.session_id.as_str()),
        _ => None,
    }
}
