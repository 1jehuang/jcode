use super::{Tool, ToolContext, ToolOutput};
use crate::message::{ContentBlock, ToolCall};
use crate::session::Session;
use crate::storage;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::process::Command;

#[derive(Debug, Deserialize)]
struct AgentGrepInput {
    mode: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    terms: Option<Vec<String>>,
    #[serde(default)]
    regex: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
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
    label: String,
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

pub struct AgentGrepTool {
    binary_override: Option<PathBuf>,
}

impl AgentGrepTool {
    pub fn new() -> Self {
        Self {
            binary_override: None,
        }
    }

    fn with_binary_override(path: PathBuf) -> Self {
        Self {
            binary_override: Some(path),
        }
    }
}

#[async_trait]
impl Tool for AgentGrepTool {
    fn name(&self) -> &str {
        "agentgrep"
    }

    fn description(&self) -> &str {
        "Search a codebase using agentgrep. Supports exact grep, ranked file discovery, file outlines, and relation-aware trace search. Best for replacing the agent's first burst of grep/read calls with more grouped, structure-aware results."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["mode"],
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["grep", "find", "outline", "trace", "smart"],
                    "description": "Search mode: grep for exact content search, find for ranked file discovery, outline for known-file structure, trace for relation-aware investigation (smart is an alias)"
                },
                "query": {
                    "type": "string",
                    "description": "Query string for grep and find modes"
                },
                "file": {
                    "type": "string",
                    "description": "File path for outline mode"
                },
                "terms": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Structured smart DSL terms for smart mode, e.g. [\"subject:auth_status\", \"relation:rendered\"]"
                },
                "regex": {
                    "type": "boolean",
                    "description": "For grep mode: treat query as a regular expression"
                },
                "path": {
                    "type": "string",
                    "description": "Optional root path to search instead of the current working directory"
                },
                "glob": {
                    "type": "string",
                    "description": "Restrict candidate files by glob"
                },
                "type": {
                    "type": "string",
                    "description": "Restrict to a known file type"
                },
                "hidden": {
                    "type": "boolean",
                    "description": "Include hidden files"
                },
                "no_ignore": {
                    "type": "boolean",
                    "description": "Ignore .gitignore and related ignore files"
                },
                "max_files": {
                    "type": "integer",
                    "description": "For find/smart: maximum files to return"
                },
                "max_regions": {
                    "type": "integer",
                    "description": "For smart: maximum regions to return"
                },
                "full_region": {
                    "type": "string",
                    "enum": ["auto", "always", "never"],
                    "description": "For smart: region expansion mode"
                },
                "debug_plan": {
                    "type": "boolean",
                    "description": "For smart: print planner details"
                },
                "debug_score": {
                    "type": "boolean",
                    "description": "For find/smart: print score details"
                },
                "paths_only": {
                    "type": "boolean",
                    "description": "Print only matching file paths"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AgentGrepInput = serde_json::from_value(input)?;
        let binary = match resolve_agentgrep_binary(self.binary_override.as_deref()) {
            Some(path) => path,
            None => {
                return Ok(ToolOutput::new(
                    "agentgrep is not available. Install it or set JCODE_AGENTGREP_BIN to the agentgrep binary path.\n\nSearched PATH plus:\n- /home/jeremy/agentgrep/target/debug/agentgrep\n- /home/jeremy/agentgrep/target/release/agentgrep",
                )
                .with_title("agentgrep unavailable"));
            }
        };

        let context_path = maybe_write_context_json(&params, &ctx)?;
        let args = build_agentgrep_args(&params, &ctx, context_path.as_deref())?;
        let mut command = Command::new(&binary);
        command.args(&args);
        if let Some(ref dir) = ctx.working_dir {
            command.current_dir(dir);
        }

        let output = command.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            let detail = if stderr.is_empty() {
                stdout.clone()
            } else if stdout.is_empty() {
                stderr.clone()
            } else {
                format!("{}\n\n{}", stdout, stderr)
            };
            return Err(anyhow::anyhow!(
                "agentgrep {} failed with exit code {:?}: {}",
                params.mode,
                output.status.code(),
                detail.trim()
            ));
        }

        let mut rendered = if stdout.is_empty() {
            "agentgrep completed successfully (no output)".to_string()
        } else {
            stdout
        };
        if !stderr.is_empty() {
            rendered.push_str("\n\n[stderr]\n");
            rendered.push_str(&stderr);
        }

        if let Some(path) = context_path {
            let _ = std::fs::remove_file(path);
        }

        Ok(ToolOutput::new(rendered).with_title(format!("agentgrep {}", params.mode)))
    }
}

fn build_agentgrep_args(
    params: &AgentGrepInput,
    ctx: &ToolContext,
    context_json_path: Option<&Path>,
) -> Result<Vec<OsString>> {
    let mut args = Vec::new();
    let mode = params.mode.as_str();
    match mode {
        "grep" | "find" | "outline" => args.push(OsString::from(mode)),
        "trace" | "smart" => args.push(OsString::from("trace")),
        _ => {
            return Err(anyhow::anyhow!(
                "Unsupported agentgrep mode: {}. Use grep, find, outline, or trace.",
                params.mode
            ));
        }
    }

    match mode {
        "grep" => {
            let query = params
                .query
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("agentgrep grep requires 'query'"))?;
            if params.regex.unwrap_or(false) {
                args.push(OsString::from("--regex"));
            }
            push_common_flags(&mut args, params, ctx);
            args.push(OsString::from(query));
        }
        "find" => {
            let query = params
                .query
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("agentgrep find requires 'query'"))?;
            if params.debug_score.unwrap_or(false) {
                args.push(OsString::from("--debug-score"));
            }
            if let Some(max_files) = params.max_files {
                args.push(OsString::from("--max-files"));
                args.push(OsString::from(max_files.to_string()));
            }
            push_common_flags(&mut args, params, ctx);
            for part in query.split_whitespace() {
                args.push(OsString::from(part));
            }
        }
        "outline" => {
            let file = params
                .file
                .as_deref()
                .or(params.query.as_deref())
                .or_else(|| {
                    params
                        .terms
                        .as_ref()
                        .and_then(|terms| terms.first().map(String::as_str))
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "agentgrep outline requires 'file' (or legacy 'query' / first term)"
                    )
                })?;
            if let Some(path) = params.path.as_deref() {
                args.push(OsString::from("--path"));
                args.push(resolve_path_arg(ctx, path).into_os_string());
            }
            args.push(OsString::from(file));
        }
        "trace" | "smart" => {
            let terms = params
                .terms
                .as_ref()
                .filter(|terms| !terms.is_empty())
                .ok_or_else(|| anyhow::anyhow!("agentgrep trace requires non-empty 'terms'"))?;
            if let Some(max_files) = params.max_files {
                args.push(OsString::from("--max-files"));
                args.push(OsString::from(max_files.to_string()));
            }
            if let Some(max_regions) = params.max_regions {
                args.push(OsString::from("--max-regions"));
                args.push(OsString::from(max_regions.to_string()));
            }
            if let Some(full_region) = params.full_region.as_deref() {
                args.push(OsString::from("--full-region"));
                args.push(OsString::from(full_region));
            }
            if params.debug_plan.unwrap_or(false) {
                args.push(OsString::from("--debug-plan"));
            }
            if params.debug_score.unwrap_or(false) {
                args.push(OsString::from("--debug-score"));
            }
            push_common_flags(&mut args, params, ctx);
            if let Some(context_path) = context_json_path {
                args.push(OsString::from("--context-json"));
                args.push(context_path.as_os_str().to_os_string());
            }
            for term in terms {
                args.push(OsString::from(term));
            }
        }
        _ => unreachable!(),
    }

    Ok(args)
}

fn maybe_write_context_json(params: &AgentGrepInput, ctx: &ToolContext) -> Result<Option<PathBuf>> {
    if !matches!(params.mode.as_str(), "trace" | "smart" | "outline") {
        return Ok(None);
    }

    let context = build_harness_context(params, ctx);
    let Some(context) = context else {
        return Ok(None);
    };

    let mut path = storage::runtime_dir();
    path.push(format!(
        "jcode-agentgrep-context-{}-{}.json",
        ctx.session_id, ctx.tool_call_id
    ));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, serde_json::to_vec(&context)?)?;
    Ok(Some(path))
}

fn build_harness_context(
    params: &AgentGrepInput,
    ctx: &ToolContext,
) -> Option<AgentGrepHarnessContext> {
    let session = Session::load(&ctx.session_id).ok()?;
    let observations = collect_tool_exposures(&session);
    let search_root = params
        .path
        .as_deref()
        .map(|path| resolve_path_arg(ctx, path))
        .or_else(|| ctx.working_dir.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let total_messages = session.messages.len().max(1);
    let compaction_cutoff = session
        .compaction
        .as_ref()
        .map(|state| state.covers_up_to_turn.min(total_messages));
    let mut file_mtime_cache = HashMap::new();

    let mut context = AgentGrepHarnessContext {
        version: 1,
        ..Default::default()
    };
    let mut focus = HashSet::new();

    for observation in observations {
        let exposure = ExposureDescriptor {
            timestamp: observation.timestamp,
            message_index: observation.message_index,
            total_messages,
            compaction_cutoff,
        };
        match observation.tool.name.as_str() {
            "read" => collect_read_exposure(
                &observation.tool,
                &search_root,
                ctx,
                &mut context,
                &mut focus,
                exposure,
                &mut file_mtime_cache,
            ),
            "agentgrep" => collect_agentgrep_exposure(
                &observation.tool,
                &observation.content,
                &search_root,
                ctx,
                &mut context,
                &mut focus,
                exposure,
                &mut file_mtime_cache,
            ),
            "bash" => collect_bash_exposure(
                &observation.tool,
                &observation.content,
                &search_root,
                ctx,
                &mut context,
                &mut focus,
                exposure,
                &mut file_mtime_cache,
            ),
            _ => {}
        }
    }

    let mut focus_files = focus.into_iter().collect::<Vec<_>>();
    focus_files.sort();
    context.focus_files = focus_files;
    if context.known_regions.is_empty()
        && context.known_files.is_empty()
        && context.known_symbols.is_empty()
        && context.focus_files.is_empty()
    {
        None
    } else {
        Some(context)
    }
}

fn collect_tool_exposures(session: &Session) -> Vec<ToolExposureObservation> {
    let mut observations = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for (message_index, msg) in session.messages.iter().enumerate() {
        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    tool_map.insert(
                        id.clone(),
                        ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            intent: None,
                        },
                    );
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let tool = tool_map
                        .get(tool_use_id)
                        .cloned()
                        .unwrap_or_else(|| ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: Value::Null,
                            intent: None,
                        });
                    observations.push(ToolExposureObservation {
                        tool,
                        content: content.clone(),
                        timestamp: msg.timestamp,
                        message_index,
                    });
                }
                _ => {}
            }
        }
    }

    observations
}

fn collect_read_exposure(
    tool: &ToolCall,
    search_root: &Path,
    ctx: &ToolContext,
    context: &mut AgentGrepHarnessContext,
    focus: &mut HashSet<String>,
    exposure: ExposureDescriptor,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    let Some(file_path) = tool.input.get("file_path").and_then(|value| value.as_str()) else {
        return;
    };
    let Some(path) = normalize_context_path(file_path, search_root, ctx) else {
        return;
    };
    let (start_line, end_line) = normalize_read_range_from_tool_input(&tool.input);
    focus.insert(path.clone());
    let region = tune_known_region(
        AgentGrepKnownRegion {
            path: path.clone(),
            start_line,
            end_line,
            body_confidence: 0.85,
            current_version_confidence: 0.88,
            prune_confidence: 0.78,
            source_strength: "full_region",
            reasons: vec!["read_tool_exposure", "session_local_history"],
        },
        exposure,
        search_root,
        ctx,
        file_mtime_cache,
    );
    push_known_region(context, region);
    let file = tune_known_file(
        AgentGrepKnownFile {
            path,
            structure_confidence: 0.55,
            body_confidence: 0.45,
            current_version_confidence: 0.88,
            prune_confidence: 0.4,
            source_strength: "snippet",
            reasons: vec!["read_tool_exposure"],
        },
        exposure,
        search_root,
        ctx,
        file_mtime_cache,
    );
    push_known_file(context, file);
}

fn collect_agentgrep_exposure(
    tool: &ToolCall,
    content: &str,
    search_root: &Path,
    ctx: &ToolContext,
    context: &mut AgentGrepHarnessContext,
    focus: &mut HashSet<String>,
    exposure: ExposureDescriptor,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    let Some(mode) = tool.input.get("mode").and_then(|value| value.as_str()) else {
        return;
    };
    match mode {
        "outline" => {
            let file = tool
                .input
                .get("file")
                .and_then(|value| value.as_str())
                .or_else(|| tool.input.get("query").and_then(|value| value.as_str()));
            let Some(file) = file else {
                return;
            };
            let Some(path) = normalize_context_path(file, search_root, ctx) else {
                return;
            };
            focus.insert(path.clone());
            let known = tune_known_file(
                AgentGrepKnownFile {
                    path: path.clone(),
                    structure_confidence: 0.95,
                    body_confidence: 0.15,
                    current_version_confidence: 0.82,
                    prune_confidence: 0.86,
                    source_strength: "outline_only",
                    reasons: vec!["agentgrep_outline_result"],
                },
                exposure,
                search_root,
                ctx,
                file_mtime_cache,
            );
            push_known_file(context, known);
            collect_outline_symbols(
                content,
                &path,
                context,
                exposure,
                search_root,
                ctx,
                file_mtime_cache,
            );
        }
        "trace" | "smart" => {
            if let Some(path_hint) = tool.input.get("path").and_then(|value| value.as_str())
                && let Some(path) = normalize_context_path(path_hint, search_root, ctx)
            {
                focus.insert(path);
            }
            collect_trace_exposure(
                content,
                search_root,
                ctx,
                context,
                focus,
                exposure,
                file_mtime_cache,
            );
        }
        _ => {}
    }
}

fn collect_bash_exposure(
    tool: &ToolCall,
    content: &str,
    search_root: &Path,
    ctx: &ToolContext,
    context: &mut AgentGrepHarnessContext,
    focus: &mut HashSet<String>,
    exposure: ExposureDescriptor,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    let Some(command) = tool.input.get("command").and_then(|value| value.as_str()) else {
        return;
    };

    if let Some(path) = parse_sed_file_range(command).and_then(|(path, start_line, end_line)| {
        normalize_context_path(&path, search_root, ctx)
            .map(|normalized| (normalized, start_line, end_line))
    }) {
        let (path, start_line, end_line) = path;
        focus.insert(path.clone());
        let region = tune_known_region(
            AgentGrepKnownRegion {
                path,
                start_line,
                end_line,
                body_confidence: 0.78,
                current_version_confidence: 0.7,
                prune_confidence: 0.7,
                source_strength: "snippet",
                reasons: vec!["bash_sed_exposure"],
            },
            exposure,
            search_root,
            ctx,
            file_mtime_cache,
        );
        push_known_region(context, region);
    }

    for candidate in parse_cat_files(command)
        .into_iter()
        .chain(parse_git_show_files(command).into_iter())
        .chain(parse_git_diff_files(command).into_iter())
    {
        let Some(path) = normalize_context_path(&candidate, search_root, ctx) else {
            continue;
        };
        focus.insert(path.clone());
        let known = tune_known_file(
            AgentGrepKnownFile {
                path,
                structure_confidence: 0.5,
                body_confidence: 0.72,
                current_version_confidence: 0.72,
                prune_confidence: 0.55,
                source_strength: "full_file",
                reasons: vec!["bash_file_exposure"],
            },
            exposure,
            search_root,
            ctx,
            file_mtime_cache,
        );
        push_known_file(context, known);
    }

    collect_shell_output_path_exposure(
        content,
        search_root,
        ctx,
        context,
        focus,
        exposure,
        file_mtime_cache,
    );
}

fn normalize_context_path(path: &str, search_root: &Path, ctx: &ToolContext) -> Option<String> {
    let path = path.trim().trim_matches('"').trim_matches('\'');
    let path = path.strip_prefix("./").unwrap_or(path);
    let resolved = ctx.resolve_path(Path::new(path));
    if let Ok(relative) = resolved.strip_prefix(search_root) {
        return Some(relative.display().to_string());
    }
    if Path::new(path).is_relative() {
        return Some(path.to_string());
    }
    None
}

fn normalize_read_range_from_tool_input(input: &Value) -> (usize, usize) {
    if let Some(start_line) = input.get("start_line").and_then(|value| value.as_u64()) {
        let start_line = start_line as usize;
        let end_line = input
            .get("end_line")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(
                start_line
                    .saturating_add(
                        input
                            .get("limit")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(200) as usize,
                    )
                    .saturating_sub(1),
            );
        return (start_line.max(1), end_line.max(start_line.max(1)));
    }
    let offset = input
        .get("offset")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize;
    let limit = input
        .get("limit")
        .and_then(|value| value.as_u64())
        .unwrap_or(200) as usize;
    let start_line = offset + 1;
    let end_line = start_line + limit.saturating_sub(1);
    (start_line, end_line)
}

fn collect_outline_symbols(
    content: &str,
    path: &str,
    context: &mut AgentGrepHarnessContext,
    exposure: ExposureDescriptor,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    for (kind, label, _start_line, _end_line) in parse_structure_items(content) {
        let symbol = tune_known_symbol(
            AgentGrepKnownSymbol {
                path: path.to_string(),
                symbol: label,
                kind: Some(kind),
                structure_confidence: 0.92,
                body_confidence: 0.1,
                current_version_confidence: 0.82,
                prune_confidence: 0.8,
                source_strength: "outline_only",
                reasons: vec!["agentgrep_outline_structure"],
            },
            exposure,
            search_root,
            ctx,
            file_mtime_cache,
        );
        push_known_symbol(context, symbol);
    }
}

fn collect_trace_exposure(
    content: &str,
    search_root: &Path,
    ctx: &ToolContext,
    context: &mut AgentGrepHarnessContext,
    focus: &mut HashSet<String>,
    exposure: ExposureDescriptor,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    let mut current_file: Option<String> = None;
    let mut section: Option<&str> = None;
    let mut pending_region: Option<PendingTraceRegion> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(path) = parse_ranked_file_header(trimmed) {
            current_file = Some(path.clone());
            focus.insert(path.clone());
            let known = tune_known_file(
                AgentGrepKnownFile {
                    path,
                    structure_confidence: 0.72,
                    body_confidence: 0.2,
                    current_version_confidence: 0.78,
                    prune_confidence: 0.62,
                    source_strength: "trace_summary",
                    reasons: vec!["agentgrep_trace_file"],
                },
                exposure,
                search_root,
                ctx,
                file_mtime_cache,
            );
            push_known_file(context, known);
            section = None;
            pending_region = None;
            continue;
        }
        if let Some(best_file) = trimmed.strip_prefix("best answer likely in ") {
            if let Some(path) = normalize_context_path(best_file.trim(), search_root, ctx) {
                focus.insert(path);
            }
            continue;
        }
        match trimmed {
            "structure:" => {
                section = Some("structure");
                pending_region = None;
                continue;
            }
            "regions:" => {
                section = Some("regions");
                pending_region = None;
                continue;
            }
            _ => {}
        }

        let Some(file_path) = current_file.clone() else {
            continue;
        };

        if section == Some("structure") {
            if let Some((kind, label, _start_line, _end_line)) = parse_structure_item_line(trimmed)
            {
                let symbol = tune_known_symbol(
                    AgentGrepKnownSymbol {
                        path: file_path,
                        symbol: label,
                        kind: Some(kind),
                        structure_confidence: 0.82,
                        body_confidence: 0.12,
                        current_version_confidence: 0.78,
                        prune_confidence: 0.66,
                        source_strength: "trace_structure",
                        reasons: vec!["agentgrep_trace_structure"],
                    },
                    exposure,
                    search_root,
                    ctx,
                    file_mtime_cache,
                );
                push_known_symbol(context, symbol);
            }
            continue;
        }

        if section == Some("regions") {
            if let Some((label, start_line, end_line)) = parse_region_header_line(trimmed) {
                pending_region = Some(PendingTraceRegion {
                    path: file_path.clone(),
                    label: label.clone(),
                    kind: None,
                    start_line,
                    end_line,
                });
                let symbol = tune_known_symbol(
                    AgentGrepKnownSymbol {
                        path: file_path,
                        symbol: label,
                        kind: None,
                        structure_confidence: 0.86,
                        body_confidence: 0.28,
                        current_version_confidence: 0.8,
                        prune_confidence: 0.68,
                        source_strength: "trace_region",
                        reasons: vec!["agentgrep_trace_region_header"],
                    },
                    exposure,
                    search_root,
                    ctx,
                    file_mtime_cache,
                );
                push_known_symbol(context, symbol);
                continue;
            }
            if let Some(kind) = trimmed.strip_prefix("kind: ") {
                if let Some(region) = pending_region.as_mut() {
                    region.kind = Some(leak_str(kind.trim().to_string()));
                }
                continue;
            }
            if trimmed == "full region:" || trimmed == "snippet:" {
                if let Some(region) = pending_region.take() {
                    let profile = if trimmed == "full region:" {
                        RegionConfidenceProfile {
                            body_confidence: 0.9,
                            current_version_confidence: 0.72,
                            prune_confidence: 0.82,
                            source_strength: "full_region",
                        }
                    } else {
                        RegionConfidenceProfile {
                            body_confidence: 0.48,
                            current_version_confidence: 0.72,
                            prune_confidence: 0.52,
                            source_strength: "snippet",
                        }
                    };
                    let region = tune_known_region(
                        AgentGrepKnownRegion {
                            path: region.path,
                            start_line: region.start_line,
                            end_line: region.end_line,
                            body_confidence: profile.body_confidence,
                            current_version_confidence: profile.current_version_confidence,
                            prune_confidence: profile.prune_confidence,
                            source_strength: profile.source_strength,
                            reasons: vec!["agentgrep_trace_region_body"],
                        },
                        exposure,
                        search_root,
                        ctx,
                        file_mtime_cache,
                    );
                    push_known_region(context, region);
                }
            }
        }
    }
}

fn collect_shell_output_path_exposure(
    content: &str,
    search_root: &Path,
    ctx: &ToolContext,
    context: &mut AgentGrepHarnessContext,
    focus: &mut HashSet<String>,
    exposure: ExposureDescriptor,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    for (path, line_number) in parse_path_line_hits(content) {
        let Some(path) = normalize_context_path(&path, search_root, ctx) else {
            continue;
        };
        focus.insert(path.clone());
        let file = tune_known_file(
            AgentGrepKnownFile {
                path: path.clone(),
                structure_confidence: 0.28,
                body_confidence: 0.22,
                current_version_confidence: 0.68,
                prune_confidence: 0.18,
                source_strength: "match_line_only",
                reasons: vec!["bash_output_file_hit"],
            },
            exposure,
            search_root,
            ctx,
            file_mtime_cache,
        );
        push_known_file(context, file);
        let region = tune_known_region(
            AgentGrepKnownRegion {
                path,
                start_line: line_number,
                end_line: line_number,
                body_confidence: 0.26,
                current_version_confidence: 0.68,
                prune_confidence: 0.2,
                source_strength: "match_line_only",
                reasons: vec!["bash_output_line_hit"],
            },
            exposure,
            search_root,
            ctx,
            file_mtime_cache,
        );
        push_known_region(context, region);
    }
}

fn tune_known_file(
    mut known: AgentGrepKnownFile,
    exposure: ExposureDescriptor,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) -> AgentGrepKnownFile {
    apply_exposure_tuning(
        Some(&mut known.structure_confidence),
        &mut known.body_confidence,
        &mut known.current_version_confidence,
        &mut known.prune_confidence,
        &mut known.reasons,
        &known.path,
        exposure,
        search_root,
        ctx,
        file_mtime_cache,
    );
    known
}

fn tune_known_region(
    mut known: AgentGrepKnownRegion,
    exposure: ExposureDescriptor,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) -> AgentGrepKnownRegion {
    apply_exposure_tuning(
        None,
        &mut known.body_confidence,
        &mut known.current_version_confidence,
        &mut known.prune_confidence,
        &mut known.reasons,
        &known.path,
        exposure,
        search_root,
        ctx,
        file_mtime_cache,
    );
    known
}

fn tune_known_symbol(
    mut known: AgentGrepKnownSymbol,
    exposure: ExposureDescriptor,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) -> AgentGrepKnownSymbol {
    apply_exposure_tuning(
        Some(&mut known.structure_confidence),
        &mut known.body_confidence,
        &mut known.current_version_confidence,
        &mut known.prune_confidence,
        &mut known.reasons,
        &known.path,
        exposure,
        search_root,
        ctx,
        file_mtime_cache,
    );
    known
}

fn apply_exposure_tuning(
    structure_confidence: Option<&mut f32>,
    body_confidence: &mut f32,
    current_version_confidence: &mut f32,
    prune_confidence: &mut f32,
    reasons: &mut Vec<&'static str>,
    path: &str,
    exposure: ExposureDescriptor,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) {
    let position_ratio = if exposure.total_messages <= 1 {
        1.0
    } else {
        (exposure.message_index + 1) as f32 / exposure.total_messages as f32
    };
    let memory_multiplier = if exposure
        .compaction_cutoff
        .is_some_and(|cutoff| exposure.message_index < cutoff)
    {
        merge_reasons(reasons, vec!["compacted_history"]);
        0.42
    } else if position_ratio >= 0.85 {
        merge_reasons(reasons, vec!["active_context_tail"]);
        1.0
    } else if position_ratio >= 0.6 {
        merge_reasons(reasons, vec!["recent_context"]);
        0.88
    } else {
        merge_reasons(reasons, vec!["older_context"]);
        0.72
    };

    if let Some(structure_confidence) = structure_confidence {
        *structure_confidence =
            (*structure_confidence * (0.75 + 0.25 * memory_multiplier)).clamp(0.0, 1.0);
    }
    *body_confidence = (*body_confidence * memory_multiplier).clamp(0.0, 1.0);
    *prune_confidence = (*prune_confidence * memory_multiplier).clamp(0.0, 1.0);

    let freshness_multiplier =
        file_freshness_multiplier(path, exposure.timestamp, search_root, ctx, file_mtime_cache);
    if freshness_multiplier < 0.999 {
        merge_reasons(reasons, vec!["file_changed_since_seen"]);
    } else if exposure.timestamp.is_some() {
        merge_reasons(reasons, vec!["file_unchanged_since_seen"]);
    }
    *current_version_confidence =
        (*current_version_confidence * freshness_multiplier).clamp(0.0, 1.0);
}

fn file_freshness_multiplier(
    path: &str,
    exposure_time: Option<DateTime<Utc>>,
    search_root: &Path,
    ctx: &ToolContext,
    file_mtime_cache: &mut HashMap<String, Option<DateTime<Utc>>>,
) -> f32 {
    let Some(exposure_time) = exposure_time else {
        return 0.7;
    };

    let modified_at = file_mtime_cache
        .entry(path.to_string())
        .or_insert_with(|| file_modified_at(path, search_root, ctx))
        .to_owned();
    let Some(modified_at) = modified_at else {
        return 0.72;
    };
    if modified_at <= exposure_time {
        return 1.0;
    }

    let delta = modified_at.signed_duration_since(exposure_time);
    if delta.num_seconds() <= 5 {
        0.92
    } else if delta.num_minutes() <= 10 {
        0.68
    } else if delta.num_hours() <= 6 {
        0.45
    } else {
        0.25
    }
}

fn file_modified_at(path: &str, search_root: &Path, ctx: &ToolContext) -> Option<DateTime<Utc>> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        let resolved = ctx.resolve_path(Path::new(path));
        if resolved.starts_with(search_root) {
            resolved
        } else {
            search_root.join(path)
        }
    };
    let modified = std::fs::metadata(candidate).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified))
}

fn push_known_file(context: &mut AgentGrepHarnessContext, known: AgentGrepKnownFile) {
    if let Some(existing) = context
        .known_files
        .iter_mut()
        .find(|entry| entry.path == known.path)
    {
        existing.structure_confidence = existing
            .structure_confidence
            .max(known.structure_confidence);
        existing.body_confidence = existing.body_confidence.max(known.body_confidence);
        existing.current_version_confidence = existing
            .current_version_confidence
            .max(known.current_version_confidence);
        existing.prune_confidence = existing.prune_confidence.max(known.prune_confidence);
        merge_reasons(&mut existing.reasons, known.reasons);
        return;
    }
    context.known_files.push(known);
}

fn push_known_region(context: &mut AgentGrepHarnessContext, known: AgentGrepKnownRegion) {
    if let Some(existing) = context.known_regions.iter_mut().find(|entry| {
        entry.path == known.path
            && entry.start_line == known.start_line
            && entry.end_line == known.end_line
    }) {
        existing.body_confidence = existing.body_confidence.max(known.body_confidence);
        existing.current_version_confidence = existing
            .current_version_confidence
            .max(known.current_version_confidence);
        existing.prune_confidence = existing.prune_confidence.max(known.prune_confidence);
        merge_reasons(&mut existing.reasons, known.reasons);
        return;
    }
    context.known_regions.push(known);
}

fn push_known_symbol(context: &mut AgentGrepHarnessContext, known: AgentGrepKnownSymbol) {
    if let Some(existing) = context.known_symbols.iter_mut().find(|entry| {
        entry.path == known.path && entry.symbol == known.symbol && entry.kind == known.kind
    }) {
        existing.structure_confidence = existing
            .structure_confidence
            .max(known.structure_confidence);
        existing.body_confidence = existing.body_confidence.max(known.body_confidence);
        existing.current_version_confidence = existing
            .current_version_confidence
            .max(known.current_version_confidence);
        existing.prune_confidence = existing.prune_confidence.max(known.prune_confidence);
        merge_reasons(&mut existing.reasons, known.reasons);
        return;
    }
    context.known_symbols.push(known);
}

fn merge_reasons(existing: &mut Vec<&'static str>, new_reasons: Vec<&'static str>) {
    for reason in new_reasons {
        if !existing.contains(&reason) {
            existing.push(reason);
        }
    }
}

fn parse_structure_items(content: &str) -> Vec<(&'static str, String, usize, usize)> {
    content
        .lines()
        .filter_map(|line| parse_structure_item_line(line.trim()))
        .collect()
}

fn parse_structure_item_line(line: &str) -> Option<(&'static str, String, usize, usize)> {
    static STRUCTURE_ITEM_RE: OnceLock<Regex> = OnceLock::new();
    let captures = STRUCTURE_ITEM_RE
        .get_or_init(|| {
            Regex::new(r"^-\s+([A-Za-z0-9_-]+)\s+(.+?)\s+@\s*(\d+)-(\d+)")
                .expect("valid structure item regex")
        })
        .captures(line)?;
    let kind = captures.get(1)?.as_str();
    let label = captures.get(2)?.as_str().trim().to_string();
    let start_line = captures.get(3)?.as_str().parse().ok()?;
    let end_line = captures.get(4)?.as_str().parse().ok()?;
    Some((leak_str(kind.to_string()), label, start_line, end_line))
}

fn parse_ranked_file_header(line: &str) -> Option<String> {
    static FILE_HEADER_RE: OnceLock<Regex> = OnceLock::new();
    FILE_HEADER_RE
        .get_or_init(|| Regex::new(r"^\d+\.\s+(.+)$").expect("valid ranked file regex"))
        .captures(line)
        .and_then(|captures| {
            captures
                .get(1)
                .map(|value| value.as_str().trim().to_string())
        })
}

fn parse_region_header_line(line: &str) -> Option<(String, usize, usize)> {
    static REGION_HEADER_RE: OnceLock<Regex> = OnceLock::new();
    let captures = REGION_HEADER_RE
        .get_or_init(|| Regex::new(r"^-\s+(.+?)\s+@\s*(\d+)-(\d+)").expect("valid region regex"))
        .captures(line)?;
    let label = captures.get(1)?.as_str().trim().to_string();
    let start_line = captures.get(2)?.as_str().parse().ok()?;
    let end_line = captures.get(3)?.as_str().parse().ok()?;
    Some((label, start_line, end_line))
}

fn parse_sed_file_range(command: &str) -> Option<(String, usize, usize)> {
    static SED_RE: OnceLock<Regex> = OnceLock::new();
    let captures = SED_RE
        .get_or_init(|| {
            Regex::new(r#"sed\s+-n\s+['"]?(\d+),(\d+)p['"]?\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#)
                .expect("valid sed range regex")
        })
        .captures(command)?;
    let start_line = captures.get(1)?.as_str().parse().ok()?;
    let end_line = captures.get(2)?.as_str().parse().ok()?;
    let path = captures
        .get(3)
        .or_else(|| captures.get(4))
        .or_else(|| captures.get(5))?
        .as_str()
        .to_string();
    Some((path, start_line, end_line))
}

fn parse_cat_files(command: &str) -> Vec<String> {
    static CAT_RE: OnceLock<Regex> = OnceLock::new();
    CAT_RE
        .get_or_init(|| {
            Regex::new(r#"(?:^|[;&|]\s*)cat\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#)
                .expect("valid cat regex")
        })
        .captures_iter(command)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().to_string())
        })
        .collect()
}

fn parse_git_show_files(command: &str) -> Vec<String> {
    static GIT_SHOW_RE: OnceLock<Regex> = OnceLock::new();
    GIT_SHOW_RE
        .get_or_init(|| {
            Regex::new(r#"git\s+show\s+[^:\s]+:(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#)
                .expect("valid git show regex")
        })
        .captures_iter(command)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().to_string())
        })
        .collect()
}

fn parse_git_diff_files(command: &str) -> Vec<String> {
    static GIT_DIFF_RE: OnceLock<Regex> = OnceLock::new();
    GIT_DIFF_RE
        .get_or_init(|| {
            Regex::new(r#"git\s+diff(?:\s+[^\n]*)?\s+--\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#)
                .expect("valid git diff regex")
        })
        .captures_iter(command)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().to_string())
        })
        .collect()
}

fn parse_path_line_hits(content: &str) -> Vec<(String, usize)> {
    static PATH_LINE_RE: OnceLock<Regex> = OnceLock::new();
    PATH_LINE_RE
        .get_or_init(|| Regex::new(r"(?m)^([^:\n]+):(\d+):").expect("valid path line regex"))
        .captures_iter(content)
        .filter_map(|captures| {
            let path = captures.get(1)?.as_str().trim().to_string();
            let line_number = captures.get(2)?.as_str().parse().ok()?;
            Some((path, line_number))
        })
        .collect()
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn push_common_flags(args: &mut Vec<OsString>, params: &AgentGrepInput, ctx: &ToolContext) {
    if params.paths_only.unwrap_or(false) {
        args.push(OsString::from("--paths-only"));
    }
    if params.hidden.unwrap_or(false) {
        args.push(OsString::from("--hidden"));
    }
    if params.no_ignore.unwrap_or(false) {
        args.push(OsString::from("--no-ignore"));
    }
    if let Some(file_type) = params.file_type.as_deref() {
        args.push(OsString::from("--type"));
        args.push(OsString::from(file_type));
    }
    if let Some(glob) = params.glob.as_deref() {
        args.push(OsString::from("--glob"));
        args.push(OsString::from(glob));
    }
    if let Some(path) = params.path.as_deref() {
        args.push(OsString::from("--path"));
        args.push(resolve_path_arg(ctx, path).into_os_string());
    }
}

fn resolve_path_arg(ctx: &ToolContext, path: &str) -> PathBuf {
    ctx.resolve_path(Path::new(path))
}

fn resolve_agentgrep_binary(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        if path.exists() {
            return Some(path.to_path_buf());
        }
        return None;
    }

    if let Some(path) = std::env::var_os("JCODE_AGENTGREP_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(path) = find_in_path(binary_name()) {
        return Some(path);
    }

    default_agentgrep_candidates()
        .into_iter()
        .find(|path| path.exists())
}

fn binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "agentgrep.exe"
    }
    #[cfg(not(windows))]
    {
        "agentgrep"
    }
}

fn default_agentgrep_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(format!(
            "/home/jeremy/agentgrep/target/debug/{}",
            binary_name()
        )),
        PathBuf::from(format!(
            "/home/jeremy/agentgrep/target/release/{}",
            binary_name()
        )),
    ]
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::fs;

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

    #[test]
    fn build_args_for_grep_includes_scope_flags() {
        let ctx = test_ctx(Path::new("/tmp/root"));
        let params = AgentGrepInput {
            mode: "grep".to_string(),
            query: Some("auth_status".to_string()),
            file: None,
            terms: None,
            regex: Some(true),
            path: Some("src".to_string()),
            glob: Some("src/**/*.rs".to_string()),
            file_type: Some("rs".to_string()),
            hidden: Some(true),
            no_ignore: Some(true),
            max_files: None,
            max_regions: None,
            full_region: None,
            debug_plan: None,
            debug_score: None,
            paths_only: Some(true),
        };

        let args = build_agentgrep_args(&params, &ctx, None).unwrap();
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "grep",
                "--regex",
                "--paths-only",
                "--hidden",
                "--no-ignore",
                "--type",
                "rs",
                "--glob",
                "src/**/*.rs",
                "--path",
                "/tmp/root/src",
                "auth_status"
            ]
        );
    }

    #[test]
    fn build_args_for_smart_uses_terms() {
        let ctx = test_ctx(Path::new("/workspace"));
        let params = AgentGrepInput {
            mode: "smart".to_string(),
            query: None,
            file: None,
            terms: Some(vec![
                "subject:auth_status".to_string(),
                "relation:rendered".to_string(),
                "path:src/tui".to_string(),
            ]),
            regex: None,
            path: Some("repo".to_string()),
            glob: None,
            file_type: Some("rs".to_string()),
            hidden: None,
            no_ignore: None,
            max_files: Some(3),
            max_regions: Some(4),
            full_region: Some("auto".to_string()),
            debug_plan: Some(true),
            debug_score: Some(true),
            paths_only: None,
        };

        let args = build_agentgrep_args(&params, &ctx, None).unwrap();
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "trace",
                "--max-files",
                "3",
                "--max-regions",
                "4",
                "--full-region",
                "auto",
                "--debug-plan",
                "--debug-score",
                "--type",
                "rs",
                "--path",
                "/workspace/repo",
                "subject:auth_status",
                "relation:rendered",
                "path:src/tui"
            ]
        );
    }

    #[test]
    fn build_args_for_outline_accepts_file_field() {
        let ctx = test_ctx(Path::new("/workspace"));
        let params = AgentGrepInput {
            mode: "outline".to_string(),
            query: None,
            file: Some("src/tool/agentgrep.rs".to_string()),
            terms: None,
            regex: None,
            path: Some("repo".to_string()),
            glob: None,
            file_type: None,
            hidden: None,
            no_ignore: None,
            max_files: None,
            max_regions: None,
            full_region: None,
            debug_plan: None,
            debug_score: None,
            paths_only: None,
        };

        let args = build_agentgrep_args(&params, &ctx, None).unwrap();
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "outline",
                "--path",
                "/workspace/repo",
                "src/tool/agentgrep.rs"
            ]
        );
    }

    #[tokio::test]
    async fn missing_binary_returns_helpful_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = AgentGrepTool::with_binary_override(temp.path().join("missing-agentgrep"));
        let ctx = test_ctx(temp.path());
        let output = tool
            .execute(json!({"mode": "grep", "query": "lsp"}), ctx)
            .await
            .expect("tool output");
        assert!(output.output.contains("agentgrep is not available"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_runs_configured_binary() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake-agentgrep");
        fs::write(&script, "#!/usr/bin/env bash\nprintf 'args:%s\n' \"$*\"\n")
            .expect("write script");
        let mut perms = fs::metadata(&script).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");

        let tool = AgentGrepTool::with_binary_override(script);
        let ctx = test_ctx(temp.path());
        let output = tool
            .execute(
                json!({
                    "mode": "smart",
                    "terms": ["subject:lsp", "relation:implementation"],
                    "path": "repo",
                    "max_files": 2,
                    "max_regions": 3,
                    "debug_plan": true
                }),
                ctx,
            )
            .await
            .expect("agentgrep execution");
        assert!(
            output
                .output
                .contains("args:trace --max-files 2 --max-regions 3 --debug-plan --path")
        );
        assert!(
            output
                .output
                .contains("subject:lsp relation:implementation")
        );
    }

    #[test]
    fn trace_output_collects_symbols_regions_and_focus() {
        let ctx = test_ctx(Path::new("/repo"));
        let mut context = AgentGrepHarnessContext {
            version: 1,
            ..Default::default()
        };
        let mut focus = HashSet::new();
        let mut file_mtime_cache = HashMap::new();
        let content = r#"
query parameters:
  subject: auth_status
  relation: rendered

top results: 1 files, 1 regions
best answer likely in src/tui/app.rs

1. src/tui/app.rs
   role: ui
   structure:
     - function render_status_bar @ 9002-9017 (16 lines)
     - function draw_header @ 9035-9056 (22 lines)
   regions:
     - render_status_bar @ 9002-9017 (16 lines)
       kind: render-site
       full region:
         fn render_status_bar(&self, ui: &mut Ui) {
             let status = auth_status();
         }
       why:
         - exact subject match
"#;

        collect_trace_exposure(
            content,
            Path::new("/repo"),
            &ctx,
            &mut context,
            &mut focus,
            test_exposure(8, 10),
            &mut file_mtime_cache,
        );

        assert!(focus.contains("src/tui/app.rs"));
        assert!(
            context
                .known_files
                .iter()
                .any(|entry| entry.path == "src/tui/app.rs")
        );
        assert!(context.known_symbols.iter().any(|entry| {
            entry.path == "src/tui/app.rs" && entry.symbol == "render_status_bar"
        }));
        assert!(context.known_regions.iter().any(|entry| {
            entry.path == "src/tui/app.rs" && entry.start_line == 9002 && entry.end_line == 9017
        }));
    }

    #[test]
    fn bash_exposure_collects_file_and_line_hits() {
        let ctx = test_ctx(Path::new("/repo"));
        let mut context = AgentGrepHarnessContext {
            version: 1,
            ..Default::default()
        };
        let mut focus = HashSet::new();
        let mut file_mtime_cache = HashMap::new();
        let tool = ToolCall {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: json!({
                "command": "cat src/tool/lsp.rs && rg -n auth_status src/tool/lsp.rs"
            }),
            intent: None,
        };
        let content = "src/tool/lsp.rs:42:let status = auth_status();\n";

        collect_bash_exposure(
            &tool,
            content,
            Path::new("/repo"),
            &ctx,
            &mut context,
            &mut focus,
            test_exposure(9, 10),
            &mut file_mtime_cache,
        );

        assert!(focus.contains("src/tool/lsp.rs"));
        assert!(
            context
                .known_files
                .iter()
                .any(|entry| entry.path == "src/tool/lsp.rs")
        );
        assert!(context.known_regions.iter().any(|entry| {
            entry.path == "src/tool/lsp.rs" && entry.start_line == 42 && entry.end_line == 42
        }));
    }

    #[test]
    fn tuning_penalizes_compacted_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ctx = test_ctx(temp.path());
        let file_path = temp.path().join("src/foo.rs");
        fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
        fs::write(&file_path, "fn foo() {}\n").expect("write file");

        let known = AgentGrepKnownFile {
            path: "src/foo.rs".to_string(),
            structure_confidence: 0.9,
            body_confidence: 0.8,
            current_version_confidence: 0.9,
            prune_confidence: 0.8,
            source_strength: "full_file",
            reasons: vec!["test"],
        };
        let mut cache = HashMap::new();
        let tuned = tune_known_file(
            known,
            ExposureDescriptor {
                timestamp: Some(Utc::now()),
                message_index: 1,
                total_messages: 10,
                compaction_cutoff: Some(8),
            },
            temp.path(),
            &ctx,
            &mut cache,
        );

        assert!(tuned.body_confidence < 0.5);
        assert!(tuned.prune_confidence < 0.5);
        assert!(tuned.reasons.contains(&"compacted_history"));
    }

    #[test]
    fn tuning_detects_file_changed_since_seen() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ctx = test_ctx(temp.path());
        let file_path = temp.path().join("src/bar.rs");
        fs::create_dir_all(file_path.parent().expect("parent")).expect("mkdir");
        fs::write(&file_path, "fn bar() {}\n").expect("write file");

        let mut cache = HashMap::new();
        let tuned = tune_known_region(
            AgentGrepKnownRegion {
                path: "src/bar.rs".to_string(),
                start_line: 1,
                end_line: 1,
                body_confidence: 0.9,
                current_version_confidence: 0.9,
                prune_confidence: 0.8,
                source_strength: "full_region",
                reasons: vec!["test"],
            },
            ExposureDescriptor {
                timestamp: Some(Utc::now() - Duration::hours(1)),
                message_index: 9,
                total_messages: 10,
                compaction_cutoff: None,
            },
            temp.path(),
            &ctx,
            &mut cache,
        );

        assert!(tuned.current_version_confidence < 0.6);
        assert!(tuned.reasons.contains(&"file_changed_since_seen"));
    }
}
