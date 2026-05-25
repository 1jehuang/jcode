use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AbortController {
    inner: Arc<AbortSignalInner>,
}

struct AbortSignalInner {
    aborted: std::sync::atomic::AtomicBool,
    notify: Notify,
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AbortSignalInner {
                aborted: std::sync::atomic::AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    pub fn abort(&self) {
        self.inner
            .aborted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn signal(&self) -> AbortSignal {
        AbortSignal {
            inner: self.inner.clone(),
        }
    }
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct AbortSignal {
    inner: Arc<AbortSignalInner>,
}

impl AbortSignal {
    pub fn aborted(&self) -> bool {
        self.inner
            .aborted
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn notified(&self) {
        if !self.aborted() {
            self.inner.notify.notified().await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    UserCancelled,
    Timeout,
    ToolError,
    InternalError,
}
