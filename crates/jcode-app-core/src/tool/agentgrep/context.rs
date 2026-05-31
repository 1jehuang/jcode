use super::*;

pub(super) fn maybe_write_context_json(
    params: &AgentGrepInput,
    ctx: &ToolContext,
) -> Result<Option<PathBuf>> {
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

#[expect(
    clippy::too_many_arguments,
    reason = "agentgrep exposure collection needs tool payload, content, search root, context, focus set, exposure metadata, and mtime cache"
)]
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

#[expect(
    clippy::too_many_arguments,
    reason = "bash exposure collection needs tool payload, output content, search root, context, focus set, exposure metadata, and mtime cache"
)]
pub(super) fn collect_bash_exposure(
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

pub(super) fn collect_trace_exposure(
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
            if (trimmed == "full region:" || trimmed == "snippet:")
                && let Some(region) = pending_region.take()
            {
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

pub(super) fn tune_known_file(
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

pub(super) fn tune_known_region(
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

#[expect(
    clippy::too_many_arguments,
    reason = "exposure tuning uses several confidence outputs plus exposure metadata and file freshness cache"
)]
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
    static STRUCTURE_ITEM_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let captures = STRUCTURE_ITEM_RE
        .get_or_init(|| Regex::new(r"^-\s+([A-Za-z0-9_-]+)\s+(.+?)\s+@\s*(\d+)-(\d+)").ok())
        .as_ref()?
        .captures(line)?;
    let kind = captures.get(1)?.as_str();
    let label = captures.get(2)?.as_str().trim().to_string();
    let start_line = captures.get(3)?.as_str().parse().ok()?;
    let end_line = captures.get(4)?.as_str().parse().ok()?;
    Some((leak_str(kind.to_string()), label, start_line, end_line))
}

fn parse_ranked_file_header(line: &str) -> Option<String> {
    static FILE_HEADER_RE: OnceLock<Option<Regex>> = OnceLock::new();
    FILE_HEADER_RE
        .get_or_init(|| Regex::new(r"^\d+\.\s+(.+)$").ok())
        .as_ref()?
        .captures(line)
        .and_then(|captures| {
            captures
                .get(1)
                .map(|value| value.as_str().trim().to_string())
        })
}

fn parse_region_header_line(line: &str) -> Option<(String, usize, usize)> {
    static REGION_HEADER_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let captures = REGION_HEADER_RE
        .get_or_init(|| Regex::new(r"^-\s+(.+?)\s+@\s*(\d+)-(\d+)").ok())
        .as_ref()?
        .captures(line)?;
    let label = captures.get(1)?.as_str().trim().to_string();
    let start_line = captures.get(2)?.as_str().parse().ok()?;
    let end_line = captures.get(3)?.as_str().parse().ok()?;
    Some((label, start_line, end_line))
}

fn parse_sed_file_range(command: &str) -> Option<(String, usize, usize)> {
    static SED_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let captures = SED_RE
        .get_or_init(|| {
            Regex::new(r#"sed\s+-n\s+['"]?(\d+),(\d+)p['"]?\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#)
                .ok()
        })
        .as_ref()?
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
    static CAT_RE: OnceLock<Option<Regex>> = OnceLock::new();
    CAT_RE
        .get_or_init(|| {
            Regex::new(r#"(?:^|[;&|]\s*)cat\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#).ok()
        })
        .as_ref()
        .map(|regex| {
            regex
                .captures_iter(command)
                .filter_map(|captures| {
                    captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .or_else(|| captures.get(3))
                        .map(|value| value.as_str().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_git_show_files(command: &str) -> Vec<String> {
    static GIT_SHOW_RE: OnceLock<Option<Regex>> = OnceLock::new();
    GIT_SHOW_RE
        .get_or_init(|| {
            Regex::new(r#"git\s+show\s+[^:\s]+:(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#).ok()
        })
        .as_ref()
        .map(|regex| {
            regex
                .captures_iter(command)
                .filter_map(|captures| {
                    captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .or_else(|| captures.get(3))
                        .map(|value| value.as_str().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_git_diff_files(command: &str) -> Vec<String> {
    static GIT_DIFF_RE: OnceLock<Option<Regex>> = OnceLock::new();
    GIT_DIFF_RE
        .get_or_init(|| {
            Regex::new(r#"git\s+diff(?:\s+[^\n]*)?\s+--\s+(?:"([^"]+)"|'([^']+)'|([^\s|;]+))"#).ok()
        })
        .as_ref()
        .map(|regex| {
            regex
                .captures_iter(command)
                .filter_map(|captures| {
                    captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .or_else(|| captures.get(3))
                        .map(|value| value.as_str().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_path_line_hits(content: &str) -> Vec<(String, usize)> {
    static PATH_LINE_RE: OnceLock<Option<Regex>> = OnceLock::new();
    PATH_LINE_RE
        .get_or_init(|| Regex::new(r"(?m)^([^:\n]+):(\d+):").ok())
        .as_ref()
        .map(|regex| {
            regex
                .captures_iter(content)
                .filter_map(|captures| {
                    let path = captures.get(1)?.as_str().trim().to_string();
                    let line_number = captures.get(2)?.as_str().parse().ok()?;
                    Some((path, line_number))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
