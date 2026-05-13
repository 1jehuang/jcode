use crate::agent::Agent;
use crate::compaction::CompactionEvent;
use crate::logging;
use crate::memory::PendingMemory;
use jcode_message_types::{Message, ToolDefinition};
use std::sync::Arc;

/// Turn phases that can be customized by different strategy implementations.
/// Default implementations match standard single-agent behavior.
/// The trait covers **preparation phases** (1–9) before the API call.
/// Stream handling and tool execution remain inline in run_turn().
#[allow(unused_variables)]
pub trait TurnStrategy: Send + Sync {
    /// 1. Repair missing tool outputs. Returns repair count.
    fn repair(&self, agent: &mut Agent) -> usize { agent.repair_missing_tool_outputs() }

    /// 2. Prepare messages for provider call.
    fn prepare_messages(&self, agent: &mut Agent) -> (Vec<Message>, Option<CompactionEvent>) {
        agent.messages_for_provider()
    }

    /// 3. Handle compaction event (reset caches, print notification).
    fn handle_compaction(&self, agent: &mut Agent, event: &CompactionEvent, print: bool) {
        agent.cache_tracker.reset();
        agent.locked_tools = None;
        if print {
            let ts = event.pre_tokens.map(|t| format!(" ({} tokens)", t)).unwrap_or_default();
            println!("📦 Context compacted ({}){}", event.trigger, ts);
        }
    }

    /// 4. Build tool definitions for this turn.
    fn tool_defs(&self, agent: &mut Agent) -> impl std::future::Future<Output = Vec<ToolDefinition>> + Send {
        async { agent.tool_definitions().await }
    }

    /// 5. Build non-blocking memory prompt (spawn background check).
    fn build_memory(&self, agent: &mut Agent, msgs: Arc<[Message]>) -> Option<crate::memory::PendingMemory> {
        agent.build_memory_prompt_nonblocking_shared(msgs, None)
    }

    /// 6. Build split system prompt.
    fn build_prompt(&self, agent: &Agent) -> crate::prompt::SplitSystemPrompt {
        agent.build_system_prompt_split(None)
    }

    /// 7. Record client cache request.
    fn record_cache(&self, agent: &mut Agent, msgs: &[Message]) {
        agent.record_client_cache_request(msgs);
    }

    /// 8. Run micro-compaction on messages.
    fn microcompact(&self, msgs: &mut Vec<Message>, print: bool) {
        use jcode_compaction_core::micro_compact::MicroCompactor;
        static MC: std::sync::OnceLock<MicroCompactor> = std::sync::OnceLock::new();
        let mc = MC.get_or_init(MicroCompactor::new);
        if let jcode_compaction_core::micro_compact::MicroCompactResult::Cleared { tools_cleared, tokens_saved, .. } = mc.run(msgs, None) {
            logging::info(&format!("MicroCompact cleared {} tool results (~{} tokens)", tools_cleared, tokens_saved));
            if print { println!("\n[MicroCompact] Cleared {} old tool results (~{} tokens saved)\n", tools_cleared, tokens_saved); }
        }
    }

    /// 9. Inject memory as user message suffix.
    fn inject_memory(&self, msgs: &mut Vec<Message>, memory: &PendingMemory) {
        let cnt = memory.count.max(1);
        let age = memory.computed_at.elapsed().as_millis() as u64;
        crate::memory::record_injected_prompt(&memory.prompt, cnt, age);
        logging::info(&format!("Memory injected ({})", memory.prompt.len()));
        msgs.push(Message::user(&format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt)));
    }
}

/// Standard turn strategy — default behavior matching current Agent.
pub struct StandardTurnStrategy;
impl Default for StandardTurnStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl StandardTurnStrategy { pub const fn new() -> Self { Self } }
impl TurnStrategy for StandardTurnStrategy {}
