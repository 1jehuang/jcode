use chrono::{DateTime, Utc};

use super::{AmbientState, Priority, ScheduleTarget, ScheduledItem, take_pending_directives};

// ---------------------------------------------------------------------------
// Ambient System Prompt Builder
// ---------------------------------------------------------------------------

/// Health stats for the memory graph, used in the ambient system prompt.
#[derive(Debug, Clone, Default)]
pub struct MemoryGraphHealth {
    pub total: usize,
    pub active: usize,
    pub inactive: usize,
    pub low_confidence: usize,
    pub contradictions: usize,
    pub missing_embeddings: usize,
    pub duplicate_candidates: usize,
    pub last_consolidation: Option<DateTime<Utc>>,
}

/// Summary of a recent session for the ambient prompt.
#[derive(Debug, Clone)]
pub struct RecentSessionInfo {
    pub id: String,
    pub status: String,
    pub topic: Option<String>,
    pub duration_secs: i64,
    pub extraction_status: String,
}

/// Resource budget info for the ambient prompt.
#[derive(Debug, Clone, Default)]
pub struct ResourceBudget {
    pub provider: String,
    pub tokens_remaining_desc: String,
    pub window_resets_desc: String,
    pub user_usage_rate_desc: String,
    pub cycle_budget_desc: String,
}

/// Gather memory graph health stats from the MemoryManager.
pub fn gather_memory_graph_health(
    memory_manager: &crate::memory::MemoryManager,
) -> MemoryGraphHealth {
    let mut health = MemoryGraphHealth::default();

    // Accumulate stats from project + global graphs
    for graph in [
        memory_manager.load_project_graph(),
        memory_manager.load_global_graph(),
    ]
    .into_iter()
    .flatten()
    {
        let active_count = graph.memories.values().filter(|m| m.active).count();
        let inactive_count = graph.memories.values().filter(|m| !m.active).count();
        health.total += graph.memories.len();
        health.active += active_count;
        health.inactive += inactive_count;

        // Low confidence: effective confidence < 0.1
        health.low_confidence += graph
            .memories
            .values()
            .filter(|m| m.active && m.effective_confidence() < 0.1)
            .count();

        // Missing embeddings
        health.missing_embeddings += graph
            .memories
            .values()
            .filter(|m| m.active && m.embedding.is_none())
            .count();

        // Count contradiction edges
        for edges in graph.edges.values() {
            for edge in edges {
                if matches!(edge.kind, crate::memory_graph::EdgeKind::Contradicts) {
                    health.contradictions += 1;
                }
            }
        }

        // Use last_cluster_update as a proxy for last consolidation
        if let Some(ts) = graph.metadata.last_cluster_update {
            match health.last_consolidation {
                Some(existing) if ts > existing => health.last_consolidation = Some(ts),
                None => health.last_consolidation = Some(ts),
                _ => {}
            }
        }
    }

    // Contradicts edges are bidirectional, so divide by 2
    health.contradictions /= 2;

    // Duplicate candidates would require embedding similarity scan;
    // placeholder for now — ambient agent will discover them during its cycle.
    health.duplicate_candidates = 0;

    health
}

/// Gather feedback memories relevant to ambient mode.
///
/// Pulls from two sources:
/// 1. Recent ambient transcripts (summaries of past cycles)
/// 2. Memory graph entries tagged "ambient" or "system"
///
/// Returns formatted strings for inclusion in the ambient system prompt.
pub fn gather_feedback_memories(memory_manager: &crate::memory::MemoryManager) -> Vec<String> {
    let mut feedback = Vec::new();

    // --- Source 1: Recent ambient transcripts ---
    let transcripts_dir = match crate::storage::jcode_dir() {
        Ok(d) => d.join("ambient").join("transcripts"),
        Err(_) => return feedback,
    };

    if transcripts_dir.exists()
        && let Ok(dir) = std::fs::read_dir(&transcripts_dir)
    {
        let mut files: Vec<_> = dir.flatten().collect();
        // Sort by filename descending (most recent first)
        files.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
        // Only look at the last 5 transcripts
        files.truncate(5);

        for entry in files {
            if let Ok(content) = std::fs::read_to_string(entry.path())
                && let Ok(transcript) =
                    serde_json::from_str::<crate::safety::AmbientTranscript>(&content)
            {
                let status = format!("{:?}", transcript.status);
                let summary = transcript.summary.as_deref().unwrap_or("no summary");
                let age = format_duration_rough(Utc::now() - transcript.started_at);
                feedback.push(format!(
                    "Past cycle ({} ago, {}): {} memories modified, {} compactions — {}",
                    age,
                    status.to_lowercase(),
                    transcript.memories_modified,
                    transcript.compactions,
                    summary,
                ));
            }
        }
    }

    // --- Source 2: Memory graph entries tagged "ambient" or "system" ---
    for graph in [
        memory_manager.load_project_graph(),
        memory_manager.load_global_graph(),
    ]
    .into_iter()
    .flatten()
    {
        for memory in graph.memories.values() {
            if !memory.active {
                continue;
            }
            let has_ambient_tag = memory.tags.iter().any(|t| t == "ambient" || t == "system");
            if has_ambient_tag {
                feedback.push(format!("Memory [{}]: {}", memory.id, memory.content));
            }
        }
    }

    feedback
}

/// Gather recent sessions since a given timestamp.
pub fn gather_recent_sessions(since: Option<DateTime<Utc>>) -> Vec<RecentSessionInfo> {
    let sessions_dir = match crate::storage::jcode_dir() {
        Ok(d) => d.join("sessions"),
        Err(_) => return Vec::new(),
    };
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cutoff = since.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));

    let mut recent = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(session) = crate::session::Session::load(stem)
            {
                // Skip debug sessions
                if session.is_debug {
                    continue;
                }
                // Only include sessions updated after cutoff
                if session.updated_at < cutoff {
                    continue;
                }
                let duration = (session.updated_at - session.created_at)
                    .num_seconds()
                    .max(0);
                let extraction = if session.messages.is_empty() {
                    "no messages"
                } else {
                    // Heuristic: if session closed normally, assume extracted
                    match &session.status {
                        crate::session::SessionStatus::Closed => "extracted",
                        crate::session::SessionStatus::Crashed { .. } => "missed",
                        crate::session::SessionStatus::Active => "in progress",
                        _ => "unknown",
                    }
                };
                recent.push(RecentSessionInfo {
                    id: session.id.clone(),
                    status: session.status.display().to_string(),
                    topic: session.display_title().map(ToOwned::to_owned),
                    duration_secs: duration,
                    extraction_status: extraction.to_string(),
                });
            }
        }
    }

    // Sort by most recent first (we don't have created_at easily, sort by id which embeds timestamp)
    recent.sort_by(|a, b| b.id.cmp(&a.id));
    recent.truncate(20); // Cap at 20 to keep prompt reasonable
    recent
}

/// Build the dynamic system prompt for an ambient cycle.
///
/// Populates the template from AMBIENT_MODE.md with real data from the
/// current state, queue, memory graph, sessions, and resource budget.
pub fn build_ambient_system_prompt(
    state: &AmbientState,
    queue: &[ScheduledItem],
    graph_health: &MemoryGraphHealth,
    recent_sessions: &[RecentSessionInfo],
    feedback_memories: &[String],
    budget: &ResourceBudget,
    active_user_sessions: usize,
) -> String {
    let mut prompt = String::with_capacity(4096);

    prompt.push_str(
        "You are the ambient agent for jcode. You operate autonomously without \
         user prompting. Your job is to maintain and improve the user's \
         development environment.\n\n",
    );

    // --- Current State ---
    prompt.push_str("## Current State\n");
    if let Some(last_run) = state.last_run {
        let ago = Utc::now() - last_run;
        let ago_str = format_duration_rough(ago);
        prompt.push_str(&format!(
            "- Last ambient cycle: {} ({} ago)\n",
            last_run.format("%Y-%m-%d %H:%M UTC"),
            ago_str,
        ));
    } else {
        prompt.push_str("- Last ambient cycle: never (first run)\n");
    }
    if active_user_sessions > 0 {
        prompt.push_str(&format!(
            "- Active user sessions: {}\n",
            active_user_sessions
        ));
    } else {
        prompt.push_str("- Active user sessions: none\n");
    }
    prompt.push_str(&format!(
        "- Total cycles completed: {}\n",
        state.total_cycles
    ));
    prompt.push('\n');

    // --- Scheduled Queue ---
    prompt.push_str("## Scheduled Queue\n");
    if queue.is_empty() {
        prompt.push_str("Empty -- do general ambient work.\n");
    } else {
        for item in queue {
            let age = Utc::now() - item.created_at;
            let priority = match item.priority {
                Priority::Low => "low",
                Priority::Normal => "normal",
                Priority::High => "HIGH",
            };
            prompt.push_str(&format!(
                "- [{}] {} (scheduled {} ago, priority: {})\n",
                item.id,
                item.context,
                format_duration_rough(age),
                priority,
            ));
            match &item.target {
                ScheduleTarget::Ambient => {}
                ScheduleTarget::Session { session_id } => {
                    prompt.push_str(&format!("  Target session: {}\n", session_id));
                }
                ScheduleTarget::Spawn { parent_session_id } => {
                    prompt.push_str(&format!("  Spawn from session: {}\n", parent_session_id));
                }
            }
            if let Some(ref dir) = item.working_dir {
                prompt.push_str(&format!("  Working dir: {}\n", dir));
            }
            if let Some(ref desc) = item.task_description {
                prompt.push_str(&format!("  Details: {}\n", desc));
            }
            if !item.relevant_files.is_empty() {
                prompt.push_str(&format!("  Files: {}\n", item.relevant_files.join(", ")));
            }
            if let Some(ref branch) = item.git_branch {
                prompt.push_str(&format!("  Branch: {}\n", branch));
            }
            if let Some(ref ctx) = item.additional_context {
                for line in ctx.lines() {
                    prompt.push_str(&format!("  {}\n", line));
                }
            }
        }
    }
    prompt.push('\n');

    // --- Recent Sessions ---
    prompt.push_str("## Recent Sessions (since last cycle)\n");
    if recent_sessions.is_empty() {
        prompt.push_str("No sessions since last cycle.\n");
    } else {
        for s in recent_sessions {
            let topic = s.topic.as_deref().unwrap_or("(no title)");
            let dur = format_duration_rough(chrono::Duration::seconds(s.duration_secs));
            prompt.push_str(&format!(
                "- {} | {} | {} | {} | extraction: {}\n",
                s.id, s.status, dur, topic, s.extraction_status,
            ));
        }
    }
    prompt.push('\n');

    // --- Memory Graph Health ---
    prompt.push_str("## Memory Graph Health\n");
    prompt.push_str(&format!(
        "- Total memories: {} ({} active, {} inactive)\n",
        graph_health.total, graph_health.active, graph_health.inactive,
    ));
    prompt.push_str(&format!(
        "- Memories with confidence < 0.1: {}\n",
        graph_health.low_confidence,
    ));
    prompt.push_str(&format!(
        "- Unresolved contradictions: {}\n",
        graph_health.contradictions,
    ));
    prompt.push_str(&format!(
        "- Memories without embeddings: {}\n",
        graph_health.missing_embeddings,
    ));
    if graph_health.duplicate_candidates > 0 {
        prompt.push_str(&format!(
            "- Duplicate candidates (similarity > 0.95): {}\n",
            graph_health.duplicate_candidates,
        ));
    } else {
        prompt.push_str("- Duplicate candidates: run embedding scan to detect\n");
    }
    if let Some(ts) = graph_health.last_consolidation {
        let ago = format_duration_rough(Utc::now() - ts);
        prompt.push_str(&format!("- Last consolidation: {} ago\n", ago));
    } else {
        prompt.push_str("- Last consolidation: never\n");
    }
    prompt.push('\n');

    // --- User Feedback History ---
    prompt.push_str("## User Feedback History\n");
    if feedback_memories.is_empty() {
        prompt.push_str("No feedback memories found about ambient mode yet.\n");
    } else {
        for mem in feedback_memories {
            prompt.push_str(&format!("- {}\n", mem));
        }
    }
    prompt.push('\n');

    // --- Resource Budget ---
    prompt.push_str("## Resource Budget\n");
    prompt.push_str(&format!("- Provider: {}\n", budget.provider));
    prompt.push_str(&format!(
        "- Tokens remaining in window: {}\n",
        budget.tokens_remaining_desc,
    ));
    prompt.push_str(&format!("- Window resets: {}\n", budget.window_resets_desc));
    prompt.push_str(&format!(
        "- User usage rate: {}\n",
        budget.user_usage_rate_desc,
    ));
    prompt.push_str(&format!(
        "- Budget for this cycle: {}\n",
        budget.cycle_budget_desc,
    ));
    prompt.push('\n');

    // --- User Directives (from email/Telegram replies) ---
    let pending_directives = take_pending_directives();
    if !pending_directives.is_empty() {
        prompt.push_str("## User Directives (from replies)\n");
        prompt.push_str(
            "The user replied to ambient notifications with these instructions. \
             Address them as your **top priority** this cycle.\n\n",
        );
        for dir in &pending_directives {
            let ago = format_duration_rough(Utc::now() - dir.received_at);
            prompt.push_str(&format!(
                "- [reply to cycle {}] ({} ago): {}\n",
                dir.in_reply_to_cycle, ago, dir.text,
            ));
        }
        prompt.push('\n');
    }

    // --- Instructions ---
    prompt.push_str(
        "## Instructions\n\n\
         Start by using the todos tool to plan what you'll do this cycle.\n\n\
         Priority order:\n\
         1. Execute any scheduled queue items first.\n\
         2. Garden the memory graph -- consolidate duplicates, resolve \
            contradictions, prune dead memories, verify stale facts, \
            extract from missed sessions.\n\
         3. Scout for proactive work (only if enabled and past cold start) -- \
            look at recent sessions and git history to identify useful work \
            the user would appreciate.\n\n\
         For gardening: focus on highest-value maintenance first. Duplicates \
         and contradictions before pruning. Verify stale facts only if you \
         have budget left.\n\n\
         For proactive work: be conservative. A bad surprise is worse than \
         no surprise. Check the user feedback memories -- if they've rejected \
         similar work before, don't do it. Code changes must go on a worktree \
         branch with a PR via request_permission.\n\n\
         Every request_permission call must be reviewer-ready. Include:\n\
         - description: concise summary of what you are about to do\n\
         - rationale: why approval is needed right now\n\
         - context.summary: what you are working on in this cycle\n\
         - context.why_permission_needed: explicit justification for permission\n\
         - context.planned_steps, context.files, context.commands (if known)\n\
         - context.risks and context.rollback_plan (if relevant)\n\n\
         Good sources for scouting proactive work:\n\
         - Todoist (via MCP) — check for relevant tasks and deadlines\n\
         - Canvas (via MCP) — check for upcoming assignments or deadlines\n\
         - Git history — recent commits, open branches, stale PRs\n\
         - Session history — patterns in what the user works on\n\n\
         When done, you MUST call end_ambient_cycle with a summary of \
         everything you did, including compaction count. Always schedule \
         your next wake time with context for what you plan to do next.\n\n\
         ## Messaging Check-ins\n\n\
         You have a `send_message` tool. Use it to keep the user informed \
         about what you're doing. Send a brief message when you start a cycle \
         and when you finish significant work. Keep messages short and useful — \
         the user should be able to glance at their messages and know what's happening \
         without opening jcode. You can optionally target a specific channel \
         (e.g. telegram, discord) or omit channel to send to all.\n",
    );

    prompt
}

pub fn format_scheduled_session_message(item: &ScheduledItem) -> String {
    let mut lines = vec![
        "[Scheduled task]".to_string(),
        "A scheduled task for this session is now due.".to_string(),
        String::new(),
        format!(
            "Task: {}",
            item.task_description.as_deref().unwrap_or(&item.context)
        ),
    ];

    if let Some(ref dir) = item.working_dir {
        lines.push(format!("Working directory: {}", dir));
    }
    if !item.relevant_files.is_empty() {
        lines.push(format!(
            "Relevant files: {}",
            item.relevant_files.join(", ")
        ));
    }
    if let Some(ref branch) = item.git_branch {
        lines.push(format!("Branch: {}", branch));
    }
    if let Some(ref ctx) = item.additional_context {
        lines.push(String::new());
        lines.push(ctx.clone());
    }

    lines.join("\n")
}

/// Format a chrono::Duration into a rough human-readable string.
pub(crate) fn format_duration_rough(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else {
        let days = secs / 86400;
        format!("{}d", days)
    }
}

/// Format a number of minutes into a human-friendly string.
/// E.g. 5 → "5m", 90 → "1h 30m", 370 → "6h 10m", 1500 → "1d 1h"
pub fn format_minutes_human(mins: u32) -> String {
    if mins < 60 {
        format!("{}m", mins)
    } else if mins < 1440 {
        let h = mins / 60;
        let m = mins % 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else {
        let d = mins / 1440;
        let h = (mins % 1440) / 60;
        if h > 0 {
            format!("{}d {}h", d, h)
        } else {
            format!("{}d", d)
        }
    }
}
