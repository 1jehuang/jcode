//! Detached observers have fixed workers and bounded retained work. Gates stay synchronous.
use super::*;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

const WORKERS: usize = 2;
const QUEUED_EVENTS: usize = 16;
const EVENT_BYTES: usize = 1024 * 1024;

struct Job {
    hooks: Vec<CommandHook>,
    event: HookEvent,
    terminal_env: Vec<(String, String)>,
}

// Count borrowed data before cloning it into a detached job.
struct Budget(usize);
impl std::io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("plugin observer event exceeds 1 MiB"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Job {
    fn new(event: &HookEvent, hooks: Vec<CommandHook>) -> Option<Box<Self>> {
        let mut terminal_env = Vec::new();
        if let Ok(env) = super::super::CLIENT_TERMINAL_ENV.try_with(Clone::clone) {
            terminal_env = env;
        }
        let mut budget = Budget(EVENT_BYTES);
        let mut measured = serde_json::to_writer(
            &mut budget,
            &(
                &event.session_id,
                &event.cwd,
                &event.fields,
                &event.plugin_tool_payload,
                &terminal_env,
            ),
        );
        for hook in &hooks {
            measured = measured.and_then(|()| {
                serde_json::to_writer(
                    &mut budget,
                    &(
                        &hook.command,
                        &hook.args,
                        &hook.root,
                        &hook.plugin,
                        hook.matcher.as_ref().map(regex::Regex::as_str),
                    ),
                )
            });
        }
        if measured.is_err() {
            crate::logging::warn("Plugin observer dropped: event/commands exceed 1 MiB");
            return None;
        }
        Some(Box::new(Self {
            hooks,
            event: event.clone(),
            terminal_env,
        }))
    }

    async fn run(self) {
        super::super::with_client_terminal_env(self.terminal_env, async {
            for hook in self.hooks {
                if let Some(output) = execute(&hook, &self.event, None).await
                    && (!output.status.success() || !output.stdout.is_empty())
                {
                    crate::logging::warn(&format!(
                        "Plugin {} observer output cannot gate or inject context; output ignored",
                        hook.event
                    ));
                }
            }
        })
        .await;
    }
}

struct Observers {
    sender: mpsc::SyncSender<Box<Job>>,
    #[cfg(test)]
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl Observers {
    fn new() -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Box<Job>>(QUEUED_EVENTS);
        let receiver = Arc::new(Mutex::new(receiver));
        #[cfg(test)]
        let mut workers = Vec::new();
        for index in 0..WORKERS {
            let receiver = Arc::clone(&receiver);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let worker = std::thread::Builder::new()
                .name(format!("plugin-observer-{index}"))
                .spawn(move || {
                    loop {
                        let next = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        match next {
                            Ok(job) => runtime.block_on(job.run()),
                            Err(_) => break,
                        }
                    }
                })?;
            #[cfg(test)]
            workers.push(worker);
            #[cfg(not(test))]
            drop(worker);
        }
        Ok(Self {
            sender,
            #[cfg(test)]
            workers,
        })
    }

    fn submit(&self, event: &HookEvent, hooks: Vec<CommandHook>) -> bool {
        let Some(job) = Job::new(event, hooks) else {
            return false;
        };
        match self.sender.try_send(job) {
            Ok(()) => true,
            Err(error) => {
                crate::logging::warn(&format!("Plugin observer dropped: {error}"));
                false
            }
        }
    }
}

pub(super) fn observe(event: &HookEvent, hooks: Vec<CommandHook>) {
    // Claude's PostToolUse excludes failed tools; native observers still receive them.
    if hooks.is_empty()
        || event.event == "post_tool"
            && event
                .fields
                .iter()
                .any(|(key, value)| *key == "STATUS" && value != "ok")
    {
        return;
    }
    static OBSERVERS: OnceLock<Option<Observers>> = OnceLock::new();
    if let Some(observers) = OBSERVERS.get_or_init(|| match Observers::new() {
        Ok(observers) => Some(observers),
        Err(error) => {
            crate::logging::warn(&format!("Plugin observer workers unavailable: {error}"));
            None
        }
    }) {
        observers.submit(event, hooks);
    }
}

#[cfg(all(test, unix))]
#[path = "observer_tests.rs"]
mod tests;
