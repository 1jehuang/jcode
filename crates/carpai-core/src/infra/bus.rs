use std::sync::Arc;
use std::collections::VecDeque;
use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};
use carpai_internal::*;
use tracing::{debug};

pub struct InProcessEventBus {
    sender: broadcast::Sender<BusEventEnvelope>,
    capacity: usize,
    history: Arc<RwLock<VecDeque<BusEventEnvelope>>>,
    events_published: Arc<std::sync::atomic::AtomicU64>,
    events_dropped: Arc<std::sync::atomic::AtomicU64>,
    start_instant: std::time::Instant,
}

impl Clone for InProcessEventBus {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            capacity: self.capacity,
            history: self.history.clone(),
            events_published: self.events_published.clone(),
            events_dropped: self.events_dropped.clone(),
            start_instant: self.start_instant,
        }
    }
}

impl InProcessEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            history: Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
            events_published: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            events_dropped: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            start_instant: std::time::Instant::now(),
        }
    }
}

#[async_trait]
impl EventBus for InProcessEventBus {
    async fn publish_json(
        &self,
        event_type: &str,
        payload: &str,
    ) -> Result<(), EventBusError> {
        let envelope = BusEventEnvelope {
            event_type: event_type.to_string(),
            payload: payload.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };

        {
            let mut history = self.history.write().await;
            if history.len() >= self.capacity {
                history.pop_front();
            }
            history.push_back(envelope.clone());
        }

        self.events_published.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        match self.sender.send(envelope) {
            Ok(_) => {}
            Err(broadcast::error::SendError(_)) => {
                self.events_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!("No subscribers for published event, event dropped");
            }
        }

        Ok(())
    }

    async fn subscribe(
        &self,
        event_type: &str,
    ) -> Result<Box<dyn BusSubscriber + Send>, EventBusError> {
        let receiver = self.sender.subscribe();

        let subscriber = BroadcastSubscriber {
            receiver,
            event_filter: event_type.to_string(),
            buffer: Vec::new(),
        };

        Ok(Box::new(subscriber))
    }

    fn subscriber_count(&self, _event_type: &str) -> usize {
        self.sender.receiver_count()
    }

    fn health_check(&self) -> BusHealth {
        BusHealth {
            healthy: true,
            backend: "in-process".to_string(),
            total_subscribers: self.sender.receiver_count(),
            events_published_total: self.events_published.load(std::sync::atomic::Ordering::Relaxed),
            events_dropped_total: self.events_dropped.load(std::sync::atomic::Ordering::Relaxed),
            uptime_secs: self.start_instant.elapsed().as_secs(),
        }
    }

    fn clone_box(&self) -> Arc<dyn EventBus> {
        Arc::new(self.clone())
    }
}

#[derive(Debug)]
struct BroadcastSubscriber {
    receiver: broadcast::Receiver<BusEventEnvelope>,
    event_filter: String,
    buffer: Vec<BusEventEnvelope>,
}

#[async_trait]
impl BusSubscriber for BroadcastSubscriber {
    async fn recv(&mut self) -> Result<BusEventEnvelope, EventBusError> {
        loop {
            match self.receiver.recv().await {
                Ok(envelope) => {
                    if envelope.event_type == self.event_filter || self.event_filter == "*" {
                        return Ok(envelope);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(lagged = n, "Subscriber lagged, catching up");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
            }
        }
    }

    fn try_recv(&mut self) -> Result<Option<BusEventEnvelope>, EventBusError> {
        match self.receiver.try_recv() {
            Ok(envelope) => {
                if envelope.event_type == self.event_filter || self.event_filter == "*" {
                    Ok(Some(envelope))
                } else {
                    Ok(None)
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                Ok(None)
            }
            Err(broadcast::error::TryRecvError::Closed) => {
                Err(EventBusError::ChannelClosed)
            }
        }
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }
}
