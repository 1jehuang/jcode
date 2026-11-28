//! Shared stateless tool construction.
use super::*;

impl Registry {
    /// Base tools that are stateless and can be shared across sessions.
    /// Created once and cached in a OnceLock, then cloned (cheap Arc bumps) per session.
    pub(super) fn base_tools(
        skills: &Arc<RwLock<SkillRegistry>>,
    ) -> HashMap<String, Arc<dyn Tool>> {
        use std::sync::OnceLock;
        static BASE: OnceLock<HashMap<String, Arc<dyn Tool>>> = OnceLock::new();
        let base = BASE.get_or_init(|| {
            let init_start = std::time::Instant::now();
            let mut timings = Vec::new();
            let mut m = HashMap::new();
            Self::insert_tool_timed(&mut m, &mut timings, "read", read::ReadTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "write", write::WriteTool::new);
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "agentgrep",
                agentgrep::AgentGrepTool::new,
            );
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "side_panel",
                side_panel::SidePanelTool::new,
            );
            Self::insert_tool_timed(&mut m, &mut timings, "panel", panel::PanelTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "edit", edit::EditTool::new);
            // `multiedit` merged into `edit`, and `patch` into `apply_patch`.
            // Both old names still resolve through `resolve_tool_name`.
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "apply_patch",
                apply_patch::ApplyPatchTool::new,
            );
            Self::insert_tool_timed(&mut m, &mut timings, "lsp", lsp::LspTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "bash", bash::BashTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "browser", browser::BrowserTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "open", open::OpenTool::new);
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                load_tools::LOAD_TOOLS_NAME,
                load_tools::LoadToolsTool::new,
            );
            #[cfg(target_os = "macos")]
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "macos_computer_use",
                computer::ComputerTool::new,
            );
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "webfetch",
                webfetch::WebFetchTool::new,
            );
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "websearch",
                websearch::WebSearchTool::new,
            );
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "maintainer_feedback",
                feedback::MaintainerFeedbackTool::new,
            );
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "jcode_docs",
                jcode_docs::JcodeDocsTool::new,
            );
            Self::insert_tool_timed(&mut m, &mut timings, "todo", todo::TodoTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "bg", bg::BgTool::new);
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "session_search",
                session_search::SessionSearchTool::new,
            );
            Self::insert_tool_timed(&mut m, &mut timings, "memory", memory::MemoryTool::new);
            // Initiative is temporarily unavailable. Keep its implementation and
            // saved data intact so it can be restored without a migration.
            Self::insert_tool_timed(&mut m, &mut timings, "gmail", gmail::GmailTool::new);
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "calendar",
                calendar::CalendarTool::new,
            );
            Self::insert_tool_timed(&mut m, &mut timings, "schedule", ambient::ScheduleTool::new);
            Self::insert_tool_timed(&mut m, &mut timings, "selfdev", selfdev::SelfDevTool::new);
            Self::insert_tool_timed(
                &mut m,
                &mut timings,
                "desktop_selfdev",
                desktop_selfdev::DesktopSelfDevTool::new,
            );
            let nonzero: Vec<String> = timings
                .iter()
                .filter(|(_, ms)| *ms > 0)
                .map(|(name, ms)| format!("{name}={ms}ms"))
                .collect();
            crate::logging::info(&format!(
                "[TIMING] registry_base_tools_init: total={}ms, nonzero=[{}]",
                init_start.elapsed().as_millis(),
                nonzero.join(", ")
            ));
            m
        });
        // Clone the Arc entries (cheap refcount bumps, not deep copies)
        let mut tools = base.clone();
        // SkillTool needs the skills registry reference (shared across sessions)
        Self::insert_tool(
            &mut tools,
            "skill_manage",
            skill::SkillTool::new(skills.clone()),
        );
        // The swarm tool captures the user-editable swarm prompt in its
        // description. Construct it once per session rather than sharing the
        // process-wide instance. Existing sessions keep their stable tool
        // definition (and provider KV cache), while newly created agents see
        // prompt edits immediately.
        Self::insert_tool(&mut tools, "swarm", communicate::CommunicateTool::new());
        tools
    }
}
