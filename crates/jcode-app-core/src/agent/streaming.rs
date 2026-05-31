use super::STREAM_KEEPALIVE_PONG_ID;
use crate::protocol::ServerEvent;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{self, MissedTickBehavior};

fn stream_keepalive_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_secs(30)
    }
}

pub(super) fn stream_keepalive_ticker() -> time::Interval {
    let interval = stream_keepalive_interval();
    let mut ticker = time::interval_at(time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

pub(super) fn send_stream_keepalive_broadcast(event_tx: &broadcast::Sender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
    });
}

pub(super) fn send_stream_keepalive_mpsc(event_tx: &mpsc::UnboundedSender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
    });
}
