use super::{Tool, ToolContext, ToolOutput};
use crate::message::{ContentBlock, ToolCall};
use crate::session::Session;
use crate::storage;
use crate::{logging, util};
use ::agentgrep::cli::{FindArgs, FullRegionMode, GrepArgs, OutlineArgs, SmartArgs};
use ::agentgrep::find::{FindResult, run_find};
use ::agentgrep::outline::run_outline;
use ::agentgrep::search::{GrepResult, run_grep};
use ::agentgrep::smart_dsl::{SmartQuery, parse_smart_query};
use ::agentgrep::smart_engine::{SmartResult, run_smart};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

mod args;
mod context;

#[cfg(test)]
use self::args::trace_or_smart_terms_owned;
use self::args::{
    build_find_args, build_grep_args, build_outline_args, build_smart_args_and_query,
    resolve_search_root, summarize_agentgrep_request,
};
use self::context::maybe_write_context_json;
#[cfg(test)]
use self::context::{
    collect_bash_exposure, collect_trace_exposure, tune_known_file, tune_known_region,
};
use ::agentgrep::render::{
    render_find_output, render_grep_output, render_outline_output, render_smart_output,
};

#[derive(Debug, Deserialize)]
struct AgentGrepInput {
    #[serde(default = "default_agentgrep_mode")]
    mode: String,
    // `pattern` accepted for legacy grep-tool calls aliased to agentgrep.
    #[serde(default, alias = "pattern")]
    query: Option<String>,
    // `file_path` accepted because agents frequently pass it instead of `file`.
    #[serde(default, alias = "file_path")]
    file: Option<String>,
    #[serde(default)]
    terms: Option<Vec<String>>,
    #[serde(default)]
    regex: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    // `include` accepted for legacy grep-tool calls aliased to agentgrep.
    #[serde(default, alias = "include")]
    glob: Option<String>,
    #[serde(rename = "type", default)]
    file_type: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    no_ignore: Option<bool>,
    #[serde(default)]
    max_files: Option<usize>,
    #[serde(default)]
    max_regions: Option<usize>,
    #[serde(default)]
    full_region: Option<String>,
    #[serde(default)]
    debug_plan: Option<bool>,
    #[serde(default)]
    debug_score: Option<bool>,
    #[serde(default)]
    paths_only: Option<bool>,
}

fn default_agentgrep_mode() -> String {
    "grep".to_string()
}

#[derive(Debug, Serialize, Default)]
struct AgentGrepHarnessContext {
    version: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    known_regions: Vec<AgentGrepKnownRegion>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    known_files: Vec<AgentGrepKnownFile>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    known_symbols: Vec<AgentGrepKnownSymbol>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    focus_files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AgentGrepKnownRegion {
    path: String,
    start_line: usize,
    end_line: usize,
    body_confidence: f32,
    current_version_confidence: f32,
    prune_confidence: f32,
    source_strength: &'static str,
    reasons: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct AgentGrepKnownFile {
    path: String,
    structure_confidence: f32,
    body_confidence: f32,
    current_version_confidence: f32,
    prune_confidence: f32,
    source_strength: &'static str,
    reasons: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct AgentGrepKnownSymbol {
    path: String,
    symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    structure_confidence: f32,
    body_confidence: f32,
    current_version_confidence: f32,
    prune_confidence: f32,
    source_strength: &'static str,
    reasons: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct RegionConfidenceProfile {
    body_confidence: f32,
    current_version_confidence: f32,
    prune_confidence: f32,
    source_strength: &'static str,
}

#[derive(Debug, Clone)]
struct PendingTraceRegion {
    path: String,
    kind: Option<&'static str>,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone)]
struct ToolExposureObservation {
    tool: ToolCall,
    content: String,
    timestamp: Option<DateTime<Utc>>,
    message_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct ExposureDescriptor {
    timestamp: Option<DateTime<Utc>>,
    message_index: usize,
    total_messages: usize,
    compaction_cutoff: Option<usize>,
}

pub struct AgentGrepTool;

impl AgentGrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AgentGrepTool {
    fn name(&self) -> &str {
        "agentgrep"
    }

    fn description(&self) -> &str {
        "Search code and file names. Defaults to grep mode when mode is omitted."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "mode": {
                    "type": "string",
                    "enum": ["grep", "find", "outline", "trace"],
                    "description": "Mode: grep (default), find (file names), outline (one file), trace (relationship DSL)."
                },
                "query": {
                    "type": "string",
                    "description": "Search query. Required for grep (literal unless regex=true); optional ranking terms for find."
                },
                "file": {
                    "type": "string",
                    "description": "Single file to inspect. Required for outline."
                },
                "terms": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Trace DSL terms, e.g. [\"subject:auth_status\", \"relation:rendered\"]. Not for grep/find; use query."
                },
                "regex": {
                    "type": "boolean",
                    "description": "In grep mode, treat query as a regex. Defaults to false (literal)."
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search, relative to the workspace. Omit to search the whole workspace."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional file glob filter such as **/*.rs. Omit to search everything."
                },
                "type": {
                    "type": "string",
                    "description": "Optional ripgrep file type filter, such as rs, py, js, ts, or md."
                },
                "max_files": {
                    "type": "integer",
                    "description": "Maximum number of files to return for find/trace-style modes."
                },
                "max_regions": {
                    "type": "integer",
                    "description": "Maximum number of matching regions to return."
                },
                "paths_only": {
                    "type": "boolean",
                    "description": "Return only matching paths instead of match excerpts where supported."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AgentGrepInput = serde_json::from_value(input)?;
        // The search shells out to ripgrep and walks/reads files (and for
        // trace/outline modes also loads the session and reads more files),
        // all of which is blocking work with no async yield points. Offload it
        // to the blocking pool so we never stall a tokio worker thread. When it
        // ran inline, a single poll of this future executed the whole search to
        // completion, freezing the TUI's select! render/input loop, which made
        // the first cold-cache search feel like it "takes forever" with no
        // spinner and an unresponsive interrupt. This mirrors how the sibling
        // grep/glob/ls tools offload their work.
        tokio::task::spawn_blocking(move || run_agentgrep_blocking(&params, &ctx))
            .await
            .map_err(|err| anyhow::anyhow!("agentgrep task failed to join: {err}"))?
    }
}

fn run_agentgrep_blocking(params: &AgentGrepInput, ctx: &ToolContext) -> Result<ToolOutput> {
    if ctx.working_dir.is_none() {
        let explicit_path = params.path.as_deref().or(params.file.as_deref());
        if explicit_path.is_none_or(|path| !Path::new(path).is_absolute()) {
            anyhow::bail!(
                "agentgrep requires a session working directory unless an absolute path is provided"
            );
        }
    }
    let context_path = maybe_write_context_json(params, ctx)?;
    let request = summarize_agentgrep_request(params, ctx, context_path.as_deref());
    let started_at = std::time::Instant::now();
    let outcome = execute_linked_agentgrep(params, ctx, context_path.as_deref());
    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    if let Some(path) = context_path {
        let _ = std::fs::remove_file(path);
    }

    match outcome {
        Ok(output) => {
            if elapsed_ms >= 2_000 {
                logging::warn(&format!(
                    "agentgrep slow mode={} elapsed_ms={} request={}",
                    params.mode, elapsed_ms, request
                ));
            }
            Ok(output)
        }
        Err(err) => {
            let detail = err.to_string();
            let detail = util::truncate_str(detail.trim(), 600);
            logging::warn(&format!(
                "agentgrep failure mode={} elapsed_ms={} request={} error={}",
                params.mode, elapsed_ms, request, detail
            ));
            Err(anyhow::anyhow!(
                "agentgrep {} failed after {}ms: {}",
                params.mode,
                elapsed_ms,
                err
            ))
        }
    }
}

fn execute_linked_agentgrep(
    params: &AgentGrepInput,
    ctx: &ToolContext,
    context_json_path: Option<&Path>,
) -> Result<ToolOutput> {
    let exact_file = exact_search_file_path(ctx, params.path.as_deref());
    match params.mode.as_str() {
        "grep" => {
            let args = build_grep_args(params, ctx)?;
            let root = resolve_search_root(ctx, args.path.as_deref())?;
            let result = filter_grep_result_to_exact_file(
                run_grep(&root, &args).map_err(anyhow::Error::msg)?,
                exact_file.as_deref(),
            );
            Ok(
                ToolOutput::new(render_grep_output(&result, &args, params.max_regions))
                    .with_title("agentgrep grep"),
            )
        }
        "find" => {
            let args = build_find_args(params, ctx)?;
            let root = resolve_search_root(ctx, args.path.as_deref())?;
            let result =
                filter_find_result_to_exact_file(run_find(&root, &args), exact_file.as_deref());
            Ok(ToolOutput::new(render_find_output(&result, &args)).with_title("agentgrep find"))
        }
        "outline" => {
            let args = build_outline_args(params, ctx, context_json_path)?;
            let root = resolve_search_root(ctx, args.path.as_deref())?;
            let result = run_outline(&root, &args).map_err(anyhow::Error::msg)?;
            Ok(ToolOutput::new(render_outline_output(&result)).with_title("agentgrep outline"))
        }
        "trace" | "smart" => {
            let (args, query) = build_smart_args_and_query(params, ctx, context_json_path)?;
            let root = resolve_search_root(ctx, args.path.as_deref())?;
            let result = filter_smart_result_to_exact_file(
                run_smart(&root, &query, &args).map_err(anyhow::Error::msg)?,
                exact_file.as_deref(),
            );
            Ok(ToolOutput::new(render_smart_output(&result, &args))
                .with_title(format!("agentgrep {}", params.mode)))
        }
        _ => Err(anyhow::anyhow!(
            "Unsupported agentgrep mode: {}. Use grep, find, outline, or trace.",
            params.mode
        )),
    }
}

fn resolve_path_arg(ctx: &ToolContext, path: &str) -> PathBuf {
    ctx.resolve_path(Path::new(path))
}

fn exact_search_file_path(ctx: &ToolContext, path: Option<&str>) -> Option<String> {
    let path = path?;
    let resolved = resolve_path_arg(ctx, path);
    if !resolved.is_file() {
        return None;
    }
    resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn filter_grep_result_to_exact_file(
    mut result: GrepResult,
    exact_file: Option<&str>,
) -> GrepResult {
    let Some(exact_file) = exact_file else {
        return result;
    };

    result.files.retain(|file| file.path == exact_file);
    result.total_files = result.files.len();
    result.total_matches = result.files.iter().map(|file| file.matches.len()).sum();
    result
}

fn filter_find_result_to_exact_file(
    mut result: FindResult,
    exact_file: Option<&str>,
) -> FindResult {
    let Some(exact_file) = exact_file else {
        return result;
    };

    result.files.retain(|file| file.path == exact_file);
    result
}

fn filter_smart_result_to_exact_file(
    mut result: SmartResult,
    exact_file: Option<&str>,
) -> SmartResult {
    let Some(exact_file) = exact_file else {
        return result;
    };

    result.files.retain(|file| file.path == exact_file);
    result.summary.total_files = result.files.len();
    result.summary.total_regions = result.files.iter().map(|file| file.regions.len()).sum();
    result.summary.best_file = result.files.first().map(|file| file.path.clone());
    result
}

fn normalized_agentgrep_glob(glob: Option<&str>) -> Option<&str> {
    let glob = glob?.trim();
    if glob.is_empty() {
        return None;
    }

    if is_match_all_glob(glob) {
        return None;
    }

    Some(glob)
}

fn normalized_agentgrep_glob_owned(glob: Option<&str>) -> Option<String> {
    normalized_agentgrep_glob(glob).map(ToOwned::to_owned)
}

fn is_match_all_glob(glob: &str) -> bool {
    matches!(glob, "*" | "**" | "**/*" | "./*" | "./**" | "./**/*")
}

#[cfg(test)]
#[path = "agentgrep_tests.rs"]
mod tests;
