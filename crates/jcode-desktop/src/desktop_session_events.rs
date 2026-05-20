use crate::{DesktopUserEvent, desktop_log, session_launch};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

pub(crate) const BACKEND_EVENT_FORWARD_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS: usize = 512;
pub(crate) const BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub(crate) struct DesktopSessionEventBatch {
    pub(crate) events: Vec<session_launch::DesktopSessionEvent>,
    pub(crate) raw_event_count: usize,
    pub(crate) raw_payload_bytes: usize,
    pub(crate) first_received_at: Instant,
    pub(crate) forwarded_at: Instant,
}

impl DesktopSessionEventBatch {
    pub(crate) fn accumulated_for(&self) -> Duration {
        self.forwarded_at
            .saturating_duration_since(self.first_received_at)
    }
}

pub(crate) fn spawn_session_event_forwarder(
    session_event_rx: mpsc::Receiver<session_launch::DesktopSessionEvent>,
    event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
) {
    if let Err(error) = std::thread::Builder::new()
        .name("jcode-desktop-session-event-forwarder".to_string())
        .spawn(move || {
            let mut next_forward_at = Instant::now();
            while let Ok(first_event) = session_event_rx.recv() {
                let now = Instant::now();
                if now < next_forward_at {
                    std::thread::sleep(next_forward_at.saturating_duration_since(now));
                }
                let batch = collect_desktop_session_event_batch(first_event, &session_event_rx);
                if batch.events.is_empty() {
                    continue;
                }
                next_forward_at = Instant::now() + BACKEND_EVENT_FORWARD_INTERVAL;
                if event_loop_proxy
                    .send_event(DesktopUserEvent::SessionEvents(batch))
                    .is_err()
                {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: failed to forward session events, event loop is closed"
                    ));
                    break;
                }
            }
        })
    {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to start session event forwarder: {error:#}"
        ));
    }
}

pub(crate) fn collect_desktop_session_event_batch(
    first_event: session_launch::DesktopSessionEvent,
    session_event_rx: &mpsc::Receiver<session_launch::DesktopSessionEvent>,
) -> DesktopSessionEventBatch {
    let first_received_at = Instant::now();
    let mut events = vec![first_event];
    let mut raw_event_count = 1usize;
    let mut raw_payload_bytes = desktop_session_event_payload_bytes(&events[0]);

    'accumulate: loop {
        while let Ok(event) = session_event_rx.try_recv() {
            raw_event_count += 1;
            raw_payload_bytes += desktop_session_event_payload_bytes(&event);
            events.push(event);
            if should_flush_session_event_batch(
                &events,
                raw_event_count,
                raw_payload_bytes,
                first_received_at.elapsed(),
            ) {
                break 'accumulate;
            }
        }
        let elapsed = first_received_at.elapsed();
        if should_flush_session_event_batch(&events, raw_event_count, raw_payload_bytes, elapsed) {
            break;
        }
        let remaining = BACKEND_EVENT_FORWARD_INTERVAL.saturating_sub(elapsed);
        if remaining.is_zero() {
            break;
        }
        match session_event_rx.recv_timeout(remaining) {
            Ok(event) => {
                raw_event_count += 1;
                raw_payload_bytes += desktop_session_event_payload_bytes(&event);
                events.push(event);
                if should_flush_session_event_batch(
                    &events,
                    raw_event_count,
                    raw_payload_bytes,
                    first_received_at.elapsed(),
                ) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let events = coalesce_desktop_session_events(events);
    let forwarded_at = Instant::now();
    DesktopSessionEventBatch {
        events,
        raw_event_count,
        raw_payload_bytes,
        first_received_at,
        forwarded_at,
    }
}

fn should_flush_session_event_batch(
    events: &[session_launch::DesktopSessionEvent],
    raw_event_count: usize,
    raw_payload_bytes: usize,
    elapsed: Duration,
) -> bool {
    raw_event_count >= BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS
        || raw_payload_bytes >= BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES
        || elapsed >= BACKEND_EVENT_FORWARD_INTERVAL
        || events
            .iter()
            .any(|event| !desktop_session_event_can_wait_for_frame_tick(event))
}

fn desktop_session_event_can_wait_for_frame_tick(
    event: &session_launch::DesktopSessionEvent,
) -> bool {
    matches!(
        event,
        session_launch::DesktopSessionEvent::TextDelta(_)
            | session_launch::DesktopSessionEvent::ToolInput { .. }
            | session_launch::DesktopSessionEvent::ToolExecuting { .. }
            | session_launch::DesktopSessionEvent::Status(_)
    )
}

fn desktop_session_event_payload_bytes(event: &session_launch::DesktopSessionEvent) -> usize {
    match event {
        session_launch::DesktopSessionEvent::Status(status) => status.payload_bytes(),
        session_launch::DesktopSessionEvent::TextDelta(text)
        | session_launch::DesktopSessionEvent::TextReplace(text)
        | session_launch::DesktopSessionEvent::Error(text) => text.len(),
        session_launch::DesktopSessionEvent::ToolInput { delta } => delta.len(),
        session_launch::DesktopSessionEvent::ToolStarted { name }
        | session_launch::DesktopSessionEvent::ToolExecuting { name } => name.len(),
        session_launch::DesktopSessionEvent::ToolFinished { name, summary, .. } => {
            name.len() + summary.len()
        }
        session_launch::DesktopSessionEvent::SessionStarted { session_id }
        | session_launch::DesktopSessionEvent::Reloaded { session_id } => session_id.len(),
        session_launch::DesktopSessionEvent::SessionRenamed {
            title,
            display_title,
        } => title.as_deref().unwrap_or_default().len() + display_title.len(),
        session_launch::DesktopSessionEvent::ModelChanged {
            model,
            provider_name,
            error,
        } => {
            model.len()
                + provider_name.as_deref().unwrap_or_default().len()
                + error.as_deref().unwrap_or_default().len()
        }
        session_launch::DesktopSessionEvent::ModelCatalog {
            current_model,
            provider_name,
            models,
            ..
        } => {
            current_model.as_deref().unwrap_or_default().len()
                + provider_name.as_deref().unwrap_or_default().len()
                + models
                    .iter()
                    .map(|model| {
                        model.model.len()
                            + model.provider.as_deref().unwrap_or_default().len()
                            + model.detail.as_deref().unwrap_or_default().len()
                    })
                    .sum::<usize>()
        }
        session_launch::DesktopSessionEvent::ModelCatalogError { error } => error.len(),
        session_launch::DesktopSessionEvent::StdinRequest {
            request_id,
            prompt,
            tool_call_id,
            ..
        } => request_id.len() + prompt.len() + tool_call_id.len(),
        session_launch::DesktopSessionEvent::Reloading { new_socket } => {
            new_socket.as_deref().unwrap_or_default().len()
        }
        session_launch::DesktopSessionEvent::Done => 0,
    }
}

pub(crate) fn coalesce_desktop_session_events(
    events: Vec<session_launch::DesktopSessionEvent>,
) -> Vec<session_launch::DesktopSessionEvent> {
    let mut coalesced = Vec::with_capacity(events.len());
    for event in events {
        match event {
            session_launch::DesktopSessionEvent::TextDelta(delta) if !delta.is_empty() => {
                if let Some(session_launch::DesktopSessionEvent::TextDelta(existing)) =
                    coalesced.last_mut()
                {
                    existing.push_str(&delta);
                } else {
                    coalesced.push(session_launch::DesktopSessionEvent::TextDelta(delta));
                }
            }
            session_launch::DesktopSessionEvent::ToolInput { delta } if !delta.is_empty() => {
                if let Some(session_launch::DesktopSessionEvent::ToolInput { delta: existing }) =
                    coalesced.last_mut()
                {
                    existing.push_str(&delta);
                } else {
                    coalesced.push(session_launch::DesktopSessionEvent::ToolInput { delta });
                }
            }
            session_launch::DesktopSessionEvent::Status(status) => {
                if let Some(session_launch::DesktopSessionEvent::Status(existing)) =
                    coalesced.last_mut()
                {
                    *existing = status;
                } else {
                    coalesced.push(session_launch::DesktopSessionEvent::Status(status));
                }
            }
            event => coalesced.push(event),
        }
    }
    coalesced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_launch::{DesktopSessionEvent, DesktopSessionStatus};

    #[test]
    fn streaming_flood_is_split_before_try_recv_can_starve_ui() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..(BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS * 3) {
            tx.send(DesktopSessionEvent::TextDelta("x".to_string()))
                .unwrap();
        }

        let batch = collect_desktop_session_event_batch(
            DesktopSessionEvent::TextDelta("x".to_string()),
            &rx,
        );

        assert!(batch.raw_event_count <= BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS);
        assert!(batch.accumulated_for() < Duration::from_millis(100));
        assert_eq!(batch.events.len(), 1);
        let DesktopSessionEvent::TextDelta(text) = &batch.events[0] else {
            panic!("streaming flood should coalesce to one text delta");
        };
        assert_eq!(text.len(), batch.raw_event_count);
        assert!(
            rx.try_recv().is_ok(),
            "bounded batch collection should leave later stream chunks queued for the next UI wake"
        );
    }

    #[test]
    fn coalescing_preserves_event_domain_boundaries() {
        let events = coalesce_desktop_session_events(vec![
            DesktopSessionEvent::Status(DesktopSessionStatus::StartingSharedServer),
            DesktopSessionEvent::Status(DesktopSessionStatus::ConnectingSharedServer),
            DesktopSessionEvent::TextDelta("hel".to_string()),
            DesktopSessionEvent::TextDelta("lo".to_string()),
            DesktopSessionEvent::Done,
        ]);

        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            DesktopSessionEvent::Status(DesktopSessionStatus::ConnectingSharedServer)
        );
        assert_eq!(
            events[1],
            DesktopSessionEvent::TextDelta("hello".to_string())
        );
        assert_eq!(events[2], DesktopSessionEvent::Done);
    }
}
