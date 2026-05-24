use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;
use carpai_internal::*;

pub struct MockEventBus {
    events: Arc<RwLock<Vec<BusEventEnvelope>>>,
}

impl Default for MockEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEventBus {
    pub fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn collected_events(&self) -> Vec<BusEventEnvelope> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventBus for MockEventBus {
    async fn publish_json(&self, event_type: &str, payload: &str) -> Result<(), EventBusError> {
        let envelope = BusEventEnvelope {
            event_type: event_type.into(),
            payload: payload.into(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };
        self.events.write().await.push(envelope);
        Ok(())
    }

    async fn subscribe(
        &self,
        _event_type: &str,
    ) -> Result<Box<dyn BusSubscriber + Send>, EventBusError> {
        #[derive(Debug)]
        struct NoopSubscriber;
        #[async_trait]
        impl BusSubscriber for NoopSubscriber {
            async fn recv(&mut self) -> Result<BusEventEnvelope, EventBusError> {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                Err(EventBusError::ChannelClosed)
            }

            fn try_recv(&mut self) -> Result<Option<BusEventEnvelope>, EventBusError> {
                Ok(None)
            }

            fn len(&self) -> usize { 0 }
        }
        Ok(Box::new(NoopSubscriber))
    }

    fn subscriber_count(&self, _event_type: &str) -> usize {
        0
    }

    fn health_check(&self) -> BusHealth {
        BusHealth {
            healthy: true,
            backend: "mock".into(),
            total_subscribers: 0,
            events_published_total: 0,
            events_dropped_total: 0,
            uptime_secs: 0,
        }
    }

    fn clone_box(&self) -> Arc<dyn EventBus> {
        Arc::new(MockEventBus {
            events: self.events.clone(),
        })
    }
}
