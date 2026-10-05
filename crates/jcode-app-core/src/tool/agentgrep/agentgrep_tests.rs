//! Shared fixtures and harness construction for the `AgentGrep` tool tests.
//!
//! The tests themselves live in `agentgrep/tests/*.rs`, one module per concern
//! (argument building, rendering, execution, exposure and tuning, input
//! defaults, scope guards, foreground budget). They are split out because this
//! file passed the 1200-line test-size ratchet at 1262 LOC. Each child repeats
//! the `use` block below, because `use super::*` does not re-export a parent's
//! private imports; only the helpers here are shared.

use super::args::scope_root_for;

use super::*;

use chrono::Duration;

use std::fs;

/// Render a relative path in both spellings the tool's output can use, so an
/// assertion on *observed* output does not hardcode POSIX separators.
///
/// `render_grep_output` (in the external `agentgrep` crate) pushes `file.path`
/// through verbatim, and that path comes from `rg`, so it carries `\` on
/// Windows and `/` elsewhere, behind a leading `.\` on Windows. Asserting the
/// literal `"src/app.rs"` therefore only ever holds on Unix.
///
/// `Path::to_string_lossy` is *not* a separator normalizer: it preserves
/// whatever separator the input string already contained, so it cannot be used
/// to build the Windows spelling. The two spellings are therefore spelled out
/// explicitly here, which also keeps this helper honest when read next to the
/// tests that depend on it.
fn rendered_rel_spellings(relative: &str) -> [String; 2] {
    let posix = relative.replace('\\', "/");
    [posix.clone(), posix.replace('/', "\\")]
}

/// True when `output` mentions `relative` in either spelling. Used for both
/// positive and negative assertions: accepting either separator cannot mask a
/// real regression, because the file being excluded has a different basename
/// whichever way it is spelled.
fn output_mentions(output: &str, relative: &str) -> bool {
    rendered_rel_spellings(relative)
        .iter()
        .any(|spelling| output.contains(spelling.as_str()))
}

fn test_ctx(root: &Path) -> ToolContext {
    ToolContext {
        session_id: "test".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(root.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

fn test_exposure(message_index: usize, total_messages: usize) -> ExposureDescriptor {
    ExposureDescriptor {
        timestamp: Some(Utc::now()),
        message_index,
        total_messages,
        compaction_cutoff: None,
    }
}

fn grep_input(query: &str, max_regions: Option<usize>) -> AgentGrepInput {
    AgentGrepInput {
        mode: "grep".to_string(),
        query: Some(query.to_string()),
        file: None,
        terms: None,
        regex: Some(false),
        path: None,
        glob: None,
        file_type: None,
        hidden: None,
        no_ignore: None,
        max_files: None,
        max_regions,
        full_region: None,
        debug_plan: None,
        debug_score: None,
        paths_only: None,
    }
}

// Every child module below repeats this file's `use` block: a glob import
// reaches the parent's own items but not its private imports, so a child that
// needs `HashMap` has to import it. Grouped by concern rather than by file
// size, so the split says what each module is about.
#[path = "tests/args_building.rs"]
mod args_building;

#[path = "tests/rendering.rs"]
mod rendering;

#[path = "tests/execute.rs"]
mod execute;

#[path = "tests/exposure_and_tuning.rs"]
mod exposure_and_tuning;

#[path = "tests/input_and_defaults.rs"]
mod input_and_defaults;

#[path = "tests/scope_guards.rs"]
mod scope_guards;

#[path = "tests/foreground_budget.rs"]
mod foreground_budget;
