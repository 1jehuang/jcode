//! Concurrent execution of adjacent concurrency-safe tool calls.
//!
//! A model response often contains several independent reads (`read`,
//! `ls`, `jcode_docs`, `webfetch`). Running them one after another
//! wastes wall time. This module lets the turn loops start a whole run of
//! consecutive safe calls at once, while the loops themselves stay sequential:
//! results are still recorded in the model's order, interrupts and background
//! detaches still apply per call, and any unsafe call (anything that may write)
//! still runs alone, after everything before it has finished.
//!
//! A call may overlap another only when both are concurrency-safe. The first
//! unsafe call is a barrier. This scheduler starts fixed runs after ordinary
//! response collection and consumes their results strictly in call order.

use super::*;
use jcode_tool_types::ToolOutput;
use std::future::Future;
use tokio::task::JoinHandle;

/// Upper bound on calls started at once from one run, matching `batch`.
pub(super) const MAX_PARALLEL_TOOL_CALLS: usize = 10;

/// Own a spawned call until completion or an explicit background transfer.
/// Taking a task out of the prefetch map must not detach it on cancellation.
pub(super) struct PrefetchedTool {
    handle: JoinHandle<Result<ToolOutput>>,
    abort_guard: AbortOnDrop,
    started_at: Instant,
    completed_at: Arc<std::sync::OnceLock<Instant>>,
}

struct AbortOnDrop(Option<tokio::task::AbortHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

impl PrefetchedTool {
    /// Preserve the streaming loop's existing detach-on-drop semantics for
    /// sequential tools, including Bash processes with their own lifecycle.
    pub fn spawn_sequential(
        future: impl Future<Output = Result<ToolOutput>> + Send + 'static,
    ) -> Self {
        let mut task = Self::spawn(future);
        task.abort_guard.0 = None;
        task
    }

    pub fn spawn(future: impl Future<Output = Result<ToolOutput>> + Send + 'static) -> Self {
        let started_at = Instant::now();
        let completed_at = Arc::new(std::sync::OnceLock::new());
        let completion = completed_at.clone();
        let handle = tokio::spawn(async move {
            // Also timestamp unwinding, so a panic is not charged ordered wait time.
            struct Completion(Arc<std::sync::OnceLock<Instant>>);
            impl Drop for Completion {
                fn drop(&mut self) {
                    self.0.get_or_init(Instant::now);
                }
            }
            let _completion = Completion(completion);
            future.await
        });
        Self {
            abort_guard: AbortOnDrop(Some(handle.abort_handle())),
            handle,
            started_at,
            completed_at,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.completed_at
            .get()
            .copied()
            .unwrap_or_else(Instant::now)
            .duration_since(self.started_at)
    }

    pub fn abort(&self) {
        self.handle.abort();
    }

    pub fn into_background(mut self) -> JoinHandle<Result<ToolOutput>> {
        self.abort_guard.0 = None;
        self.handle
    }
}

impl Future for PrefetchedTool {
    type Output = std::result::Result<Result<ToolOutput>, tokio::task::JoinError>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.handle).poll(cx)
    }
}

/// Prefetched tasks for the current response, keyed by tool call index.
#[derive(Default)]
pub(super) struct ToolPrefetch {
    enabled: bool,
    tasks: HashMap<usize, PrefetchedTool>,
    /// Usage at the start of this response's tool phase, before any result is
    /// appended. Provider-observed usage does not advance when results arrive.
    admission_tokens: usize,
    admission_message_count: usize,
    /// Index of the first call not yet examined for prefetch.
    scanned_until: usize,
}

impl ToolPrefetch {
    pub fn contains(&self, index: usize) -> bool {
        self.tasks.contains_key(&index)
    }

    /// Remove and return the prefetched task for `index`, if one was started.
    pub fn take(&mut self, index: usize) -> Option<PrefetchedTool> {
        self.tasks.remove(&index)
    }

    /// Abort every task that has not been taken. Used when the loop exits
    /// early (urgent interrupt, server reload) so no orphaned read keeps
    /// running after its result has been replaced with a skip notice.
    pub fn abort_remaining(&mut self) {
        self.tasks.clear();
    }

    /// Stop all owned work before draining in model order. Completed outputs
    /// survive abort(), including a completion racing with cancellation.
    pub fn cancel_remaining(&self) {
        for task in self.tasks.values() {
            task.abort();
        }
    }
}

impl Drop for ToolPrefetch {
    fn drop(&mut self) {
        self.abort_remaining();
    }
}

/// Whether parallel execution is enabled for this process.
pub(super) fn parallel_tools_enabled() -> bool {
    crate::config::config().tools.parallel
}

/// Which registry handle a prefetched call runs on. Each turn loop already
/// has its own convention for sequential calls, and prefetched calls must
/// behave exactly like the sequential call they replace.
#[derive(Clone, Copy)]
pub(super) enum PrefetchRegistry {
    /// `Registry::clone()`, as the streaming loop uses for its spawned calls.
    Clone,
    /// The same registry and compaction manager, as the blocking loop's
    /// direct `registry.execute` uses.
    Shared,
}

/// Length of the run of consecutive concurrency-safe calls starting at
/// `start`, capped at [`MAX_PARALLEL_TOOL_CALLS`]. Pure so it can be tested
/// without a registry.
pub(super) fn safe_run_len(safety: &[bool], start: usize) -> usize {
    safety
        .iter()
        .skip(start)
        .take(MAX_PARALLEL_TOOL_CALLS)
        .take_while(|safe| **safe)
        .count()
}

impl Agent {
    /// Drain cancelled prefetch in model order, retaining every real completion.
    /// Only calls that never started or actually cancelled receive skip results.
    pub(super) async fn finish_interrupted_prefetch(
        &mut self,
        prefetch: &mut ToolPrefetch,
        tool_calls: &[ToolCall],
        start: usize,
        skip_message: &str,
        event_tx: &tokio::sync::mpsc::UnboundedSender<ServerEvent>,
    ) -> usize {
        prefetch.cancel_remaining();
        let mut skipped = 0;
        for (index, tc) in tool_calls.iter().enumerate().skip(start) {
            let (result, duration) = if let Some(mut task) = prefetch.take(index) {
                let joined = (&mut task).await;
                let duration = Some(task.elapsed().as_millis() as u64);
                let result = match joined {
                    Ok(result) => Some(result),
                    Err(error) if error.is_cancelled() => None,
                    Err(error) => Some(Err(anyhow::anyhow!("Tool task panicked: {}", error))),
                };
                (result, duration)
            } else {
                (None, None)
            };
            if result.is_some() {
                self.unlock_tools_if_needed(&tc.name);
            }
            let (blocks, output, error) = match result {
                Some(Ok(output)) => {
                    let output = self.admit_prefetched_output(prefetch, tc, output).await;
                    let output = cap_tool_output_for_history(&tc.name, output);
                    let images = tool_output_side_pane_images(&tc.id, &tc.name, &tc.input, &output);
                    if !images.is_empty()
                        && event_tx
                            .send(ServerEvent::SidePaneImages {
                                session_id: self.session.id.clone(),
                                images,
                            })
                            .is_err()
                    {
                        logging::debug(
                            "Interrupted tool images could not reach the closed event receiver",
                        );
                    }
                    let text = output.output.clone();
                    (
                        tool_output_to_content_blocks(tc.id.clone(), output),
                        text,
                        None,
                    )
                }
                result => {
                    let text = if let Some(Err(error)) = result {
                        format!("Error: {}", error)
                    } else {
                        skipped += 1;
                        skip_message.to_string()
                    };
                    (
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: text.clone(),
                            is_error: Some(true),
                        }],
                        text.clone(),
                        Some(text),
                    )
                }
            };
            if event_tx
                .send(ServerEvent::ToolDone {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    output,
                    error,
                })
                .is_err()
            {
                logging::debug("Interrupted tool result persisted after the event receiver closed");
            }
            self.add_tool_result_with_duration(blocks, duration).await;
        }
        skipped
    }

    pub(super) async fn tool_prefetch(&mut self) -> ToolPrefetch {
        if !parallel_tools_enabled() {
            return ToolPrefetch::default();
        }
        let compaction = self.registry.compaction();
        let manager = compaction.read().await;
        ToolPrefetch {
            enabled: true,
            tasks: HashMap::new(),
            admission_tokens: manager.effective_token_count_with(self.session.provider_messages()),
            admission_message_count: self.session.messages.len(),
            scanned_until: 0,
        }
    }

    pub(super) async fn admit_prefetched_output(
        &mut self,
        prefetch: &ToolPrefetch,
        tc: &ToolCall,
        output: ToolOutput,
    ) -> ToolOutput {
        let appended_chars: usize = self
            .session
            .messages
            .iter()
            .skip(prefetch.admission_message_count)
            .map(|message| crate::compaction::content_char_count(&message.content))
            .sum();
        let current_tokens = {
            let compaction = self.registry.compaction();
            let manager = compaction.read().await;
            // Observed usage is a fixed provider snapshot, so a larger observed
            // count can mask growth in the history estimate. Advance that
            // baseline by appended content, then take max rather than adding
            // to the live estimate (which already includes those messages).
            manager
                .effective_token_count_with(self.session.provider_messages())
                .max(
                    prefetch
                        .admission_tokens
                        .saturating_add(appended_chars / crate::compaction::CHARS_PER_TOKEN),
                )
        };
        self.registry
            .guard_context_overflow_with_usage(
                &tc.name,
                output,
                crate::tool::accepts_large_output(&tc.input),
                current_tokens,
            )
            .await
    }

    /// Whether the call at `index` should be considered for early start.
    ///
    /// Calls that the loop will not execute locally (invalid calls, calls the
    /// SDK already answered, provider-handled non-native calls) are excluded
    /// so they keep their existing handling, and so they also break a run:
    /// they are cheap, so the loop reaches the next run immediately anyway.
    async fn prefetch_candidate(
        &self,
        tc: &ToolCall,
        sdk_tool_results: &HashMap<String, (String, bool)>,
    ) -> bool {
        if tc.validation_error().is_some() || self.validate_tool_allowed(&tc.name).is_err() {
            return false;
        }
        if sdk_tool_results.contains_key(&tc.id) {
            return false;
        }
        self.registry
            .is_concurrency_safe(&self.session.id, &tc.name, &tc.input)
            .await
    }

    /// When the loop is about to run call `index`, start it together with the
    /// following consecutive concurrency-safe calls if it is safe itself.
    ///
    /// Only runs of two or more calls are prefetched: a lone safe call gains
    /// nothing and keeps the exact sequential behavior. Calls already
    /// examined are never re-examined, so each call is started at most once.
    pub(super) async fn maybe_prefetch_safe_run(
        &self,
        prefetch: &mut ToolPrefetch,
        tool_calls: &[ToolCall],
        index: usize,
        message_id: &str,
        sdk_tool_results: &HashMap<String, (String, bool)>,
        registry_mode: PrefetchRegistry,
    ) {
        if index < prefetch.scanned_until || !prefetch.enabled {
            return;
        }
        let mut safety = Vec::new();
        for tc in tool_calls.iter().skip(index).take(MAX_PARALLEL_TOOL_CALLS) {
            let safe = self.prefetch_candidate(tc, sdk_tool_results).await;
            safety.push(safe);
            if !safe {
                break;
            }
        }
        let run = safe_run_len(&safety, 0);
        prefetch.scanned_until = index + run.max(1);
        if run < 2 {
            return;
        }

        logging::info(&format!(
            "Running {} concurrency-safe tool calls in parallel: {}",
            run,
            tool_calls[index..index + run]
                .iter()
                .map(|tc| tc.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for (offset, tc) in tool_calls[index..index + run].iter().enumerate() {
            let ctx = ToolContext {
                session_id: self.session.id.clone(),
                message_id: message_id.to_string(),
                tool_call_id: tc.id.clone(),
                working_dir: self.working_dir().map(PathBuf::from),
                stdin_request_tx: self.stdin_request_tx.clone(),
                graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                execution_mode: ToolExecutionMode::AgentTurn,
            };
            let registry = match registry_mode {
                PrefetchRegistry::Clone => self.registry.clone(),
                PrefetchRegistry::Shared => self.registry.shared_handle(),
            };
            let name = tc.name.clone();
            let input = tc.input.clone();
            let task = PrefetchedTool::spawn(async move {
                registry
                    .execute_deferred_context_guard(&name, input, ctx)
                    .await
            });
            prefetch.tasks.insert(index + offset, task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn assert_task_lifetime(sequential: bool, background: bool, abort: bool) {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (ended_tx, ended_rx) = tokio::sync::oneshot::channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let task_gate = gate.clone();
        let future = async move {
            struct Ended(Option<tokio::sync::oneshot::Sender<bool>>, bool);
            impl Drop for Ended {
                fn drop(&mut self) {
                    if let Some(sender) = self.0.take() {
                        sender
                            .send(self.1)
                            .expect("lifecycle receiver remains alive");
                    }
                }
            }
            let mut ended = Ended(Some(ended_tx), false);
            started_tx.send(()).expect("start receiver remains alive");
            task_gate.acquire().await.expect("gate open").forget();
            ended.1 = true;
            Ok(ToolOutput::new("finished"))
        };
        let task = if sequential {
            PrefetchedTool::spawn_sequential(future)
        } else {
            PrefetchedTool::spawn(future)
        };
        started_rx.await.expect("task starts");
        if abort {
            task.abort();
        }
        if background {
            drop(task.into_background());
        } else {
            drop(task);
        }
        let survives = (sequential || background) && !abort;
        if survives {
            gate.add_permits(1);
        }
        let completed = tokio::time::timeout(Duration::from_secs(5), ended_rx)
            .await
            .expect("task lifecycle resolves")
            .expect("task drops lifecycle guard");
        assert_eq!(completed, survives);
    }

    #[tokio::test]
    async fn dropping_sequential_wrapper_retains_task_lifetime() {
        assert_task_lifetime(true, false, false).await;
    }

    #[tokio::test]
    async fn dropping_prefetched_wrapper_cancels_task() {
        assert_task_lifetime(false, false, false).await;
    }

    #[tokio::test]
    async fn background_transfer_retains_task_lifetime() {
        assert_task_lifetime(false, true, false).await;
    }

    #[tokio::test]
    async fn explicit_abort_cancels_both_wrapper_modes() {
        assert_task_lifetime(false, false, true).await;
        assert_task_lifetime(true, false, true).await;
    }

    #[test]
    fn safe_run_stops_at_first_unsafe_call() {
        assert_eq!(safe_run_len(&[true, true, false, true], 0), 2);
        assert_eq!(safe_run_len(&[false, true, true], 0), 0);
        assert_eq!(safe_run_len(&[false, true, true], 1), 2);
        assert_eq!(safe_run_len(&[], 0), 0);
        assert_eq!(safe_run_len(&[true], 5), 0);
    }

    #[test]
    fn safe_run_is_capped() {
        let all_safe = vec![true; MAX_PARALLEL_TOOL_CALLS + 5];
        assert_eq!(safe_run_len(&all_safe, 0), MAX_PARALLEL_TOOL_CALLS);
        assert_eq!(safe_run_len(&all_safe, 10), 5);
    }
}
