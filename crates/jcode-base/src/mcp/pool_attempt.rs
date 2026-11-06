#[derive(Clone, Copy, PartialEq, Eq)]
enum SharedOperation {
    Connect,
    Disconnect,
}
type SharedConnecting = Arc<std::sync::Mutex<HashMap<String, Arc<SharedConnectionAttempt>>>>;
struct SharedConnectionAttempt {
    operation: SharedOperation,
    notify: Notify,
    outcome: std::sync::Mutex<Option<std::result::Result<(), String>>>,
}
impl SharedConnectionAttempt {
    fn new(operation: SharedOperation) -> Self {
        Self {
            operation,
            notify: Notify::new(),
            outcome: std::sync::Mutex::new(None),
        }
    }
    fn result(&self) -> Option<std::result::Result<(), String>> {
        self.outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    fn complete(&self, result: std::result::Result<(), String>) {
        let mut outcome = self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if outcome.is_none() {
            *outcome = Some(result);
        }
        drop(outcome);
        self.notify.notify_waiters();
    }
}
struct SharedConnectionAttemptGuard {
    connecting: SharedConnecting,
    name: String,
    attempt: Arc<SharedConnectionAttempt>,
    armed: bool,
}
impl SharedConnectionAttemptGuard {
    fn new(
        connecting: SharedConnecting,
        name: &str,
        attempt: Arc<SharedConnectionAttempt>,
    ) -> Self {
        Self {
            connecting,
            name: name.to_string(),
            attempt,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for SharedConnectionAttemptGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut connecting = self
            .connecting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if connecting
            .get(&self.name)
            .is_some_and(|current| Arc::ptr_eq(current, &self.attempt))
        {
            connecting.remove(&self.name);
        }
        self.attempt.complete(Err(format!(
            "MCP server '{}' connection attempt was cancelled; retry connection",
            self.name
        )));
    }
}
