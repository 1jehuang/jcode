use crate::model::{BackendCommand, BackendEvent};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct BackendBridge {
    events: Arc<Mutex<VecDeque<BackendEvent>>>,
}

impl BackendBridge {
    pub fn spawn() -> Self {
        let mut queue = VecDeque::new();
        queue.push_back(BackendEvent::Status(
            "Web target is running in preview mode. Native jcode socket transport is desktop-only right now.".to_string(),
        ));
        queue.push_back(BackendEvent::Disconnected {
            reason: "No browser transport is implemented yet.".to_string(),
        });

        Self {
            events: Arc::new(Mutex::new(queue)),
        }
    }

    pub fn send(&self, _command: BackendCommand) {}

    pub async fn next_event(&self) -> Option<BackendEvent> {
        self.events
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front())
    }
}
