use super::{App, ServerEvent};
use crate::tui::backend::RemoteEventState;

pub(in crate::tui::app) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    let kind = crate::tui::cloud_latency::event_key(&event);
    let started = std::time::Instant::now();
    let redraw = super::server_events::handle_server_event(app, event, remote);
    if let Some((kind, _)) = kind {
        crate::tui::cloud_latency::applied(kind, started, redraw);
    }
    redraw
}
