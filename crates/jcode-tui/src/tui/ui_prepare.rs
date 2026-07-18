use super::*;
use crate::tui::ui::{self, WrappedLineMap};

/// Auxiliary render data for an assistant message that is otherwise recomputed
/// by re-parsing markdown on every body rebuild. Building the body misses its
/// cache whenever `display_messages_version` changes (e.g. an in-place edit to
/// the last assistant/tool message or a streaming finalize), and each miss used
/// to re-render the same markdown two or three additional times just to derive
/// the raw-line/logical-line map used for text selection. Memoizing it keeps the
/// common edit-in-place and finalize paths cheap on long transcripts.
#[derive(Clone)]
struct AssistantAuxData {
    /// Number of leading `cached` lines that correspond to rendered markdown
    /// content (excludes appended tool-call summary lines).
    content_line_count: usize,
    /// Plain logical lines used to build the wrapped->raw line map.
    logical_plain_lines: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AssistantAuxKey {
    message_hash: u64,
    content_len: usize,
    content_width: u16,
    centered: bool,
    diff_mode: crate::config::DiffDisplayMode,
    cached_len: usize,
}

const ASSISTANT_AUX_CACHE_LIMIT: usize = 2048;

#[derive(Default)]
struct AssistantAuxCacheState {
    entries: std::collections::HashMap<AssistantAuxKey, std::sync::Arc<AssistantAuxData>>,
    order: std::collections::VecDeque<AssistantAuxKey>,
}

fn assistant_aux_cache() -> &'static std::sync::Mutex<AssistantAuxCacheState> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<AssistantAuxCacheState>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(AssistantAuxCacheState::default()))
}

/// Compute (and cache) the auxiliary raw-line-map data for an assistant message.
/// `cached` is the already-rendered (and possibly cached) display lines for the
/// message; `align` and `centered` describe the current display mode.
fn assistant_aux_data(
    msg: &DisplayMessage,
    cached: &[Line<'static>],
    content_width: u16,
    centered: bool,
    diff_mode: crate::config::DiffDisplayMode,
    align: ratatui::layout::Alignment,
) -> std::sync::Arc<AssistantAuxData> {
    let build = || {
        let content_lines =
            markdown::render_markdown_with_width(&msg.content, Some(content_width as usize));
        let content_line_count = content_lines.len().min(cached.len());
        let logical_plain_lines: Vec<String> =
            if content_prefers_display_as_logical_lines(&msg.content) {
                cached
                    .iter()
                    .take(content_line_count)
                    .map(ui::line_plain_text)
                    .collect()
            } else {
                markdown::render_markdown(&msg.content)
                    .into_iter()
                    .map(|line| ui::line_plain_text(&align_if_unset(line, align)))
                    .collect()
            };
        AssistantAuxData {
            content_line_count,
            logical_plain_lines,
        }
    };

    if cfg!(test) {
        return std::sync::Arc::new(build());
    }

    let key = AssistantAuxKey {
        message_hash: msg.stable_cache_hash(),
        content_len: msg.content.len(),
        content_width,
        centered,
        diff_mode,
        cached_len: cached.len(),
    };

    {
        let cache = match assistant_aux_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(data) = cache.entries.get(&key) {
            return data.clone();
        }
    }

    let data = std::sync::Arc::new(build());
    let mut cache = match assistant_aux_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cache.entries.insert(key.clone(), data.clone()).is_none() {
        cache.order.push_back(key);
        while cache.order.len() > ASSISTANT_AUX_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    }
    data
}

fn content_prefers_display_as_logical_lines(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
    })
}

fn semantic_swarm_line_text(plain: &str) -> (String, usize) {
    let trimmed = plain.trim_start_matches(' ');
    if let Some(rest) = trimmed.strip_prefix("│ ") {
        let prefix_width = unicode_width::UnicodeWidthStr::width(plain)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(rest));
        (rest.to_string(), prefix_width)
    } else {
        (plain.to_string(), 0)
    }
}

fn map_display_lines_to_logical_lines(
    display_lines: &[Line<'static>],
    logical_plain_lines: &[String],
    raw_base: usize,
) -> Option<Vec<WrappedLineMap>> {
    let mut maps = Vec::with_capacity(display_lines.len());
    let mut logical_idx = 0usize;
    let mut logical_col = 0usize;

    for line in display_lines {
        while logical_idx < logical_plain_lines.len() {
            let logical_width =
                unicode_width::UnicodeWidthStr::width(logical_plain_lines[logical_idx].as_str());
            if logical_col < logical_width || logical_width == 0 {
                break;
            }
            logical_idx += 1;
            logical_col = 0;
        }

        let logical_text = logical_plain_lines.get(logical_idx)?;
        let logical_width = unicode_width::UnicodeWidthStr::width(logical_text.as_str());
        let display_width = line.width();
        let remaining = logical_width.saturating_sub(logical_col);
        if display_width > remaining {
            return None;
        }

        maps.push(WrappedLineMap {
            raw_line: raw_base + logical_idx,
            start_col: logical_col,
            end_col: logical_col + display_width,
        });
        logical_col += display_width;
    }

    Some(maps)
}

fn user_prompt_number_style(color: Color) -> Style {
    Style::default().fg(color).bg(user_bg())
}

fn user_prompt_accent_style() -> Style {
    Style::default().fg(user_color()).bg(user_bg())
}

fn user_prompt_text_style() -> Style {
    Style::default().fg(user_text()).bg(user_bg())
}

fn default_message_alignment(role: &str, centered: bool) -> ratatui::layout::Alignment {
    if centered
        && !matches!(
            role,
            "tool" | "system" | "swarm" | "background_task" | "overnight" | "todos"
        )
    {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    }
}

fn is_error_copy_content(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("Error:") || trimmed.starts_with("error:") || trimmed.starts_with("Failed:")
}

/// Build the image regions for an image/mermaid placeholder in `wrapped_lines`,
/// where each placeholder "owns" the run of blank lines that follow it.
///
/// Done in a single reverse pass that precomputes, for every line, the length
/// of the blank run starting at that line. The previous implementation scanned
/// forward through the trailing blanks for every placeholder, which is O(L^2)
/// when a message has many placeholders each followed by long blank runs.
pub(super) fn compute_image_regions(
    wrapped_lines: &[ratatui::text::Line<'static>],
) -> Vec<ImageRegion> {
    fn is_blank_line(line: &ratatui::text::Line<'static>) -> bool {
        line.spans.is_empty() || (line.spans.len() == 1 && line.spans[0].content.is_empty())
    }

    let len = wrapped_lines.len();
    // blank_run[i] = number of consecutive blank lines starting at index i.
    let mut blank_run = vec![0usize; len + 1];
    for idx in (0..len).rev() {
        blank_run[idx] = if is_blank_line(&wrapped_lines[idx]) {
            blank_run[idx + 1] + 1
        } else {
            0
        };
    }

    let mut image_regions = Vec::new();
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if let Some(hash) = super::super::mermaid::parse_image_placeholder(line) {
            // The placeholder line plus the blank run immediately after it.
            let height = (1 + blank_run[idx + 1]).min(u16::MAX as usize) as u16;
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
                // Mermaid crop regions don't know their rendered width here;
                // 0 = treat the rows as fully occupied for layout purposes.
                width: 0,
                render: jcode_tui_messages::ImageRegionRender::Crop,
            });
        } else if let Some((hash, rows, cols)) =
            super::super::mermaid::parse_inline_image_placeholder(line)
        {
            // Inline raster image anchored in the transcript body. The marker
            // encodes its exact geometry; clamp to the blank run that actually
            // follows so a wrapped/truncated placeholder can never claim
            // non-blank lines below it.
            let available = (1 + blank_run[idx + 1]).min(u16::MAX as usize) as u16;
            let height = rows.max(1).min(available);
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
                width: cols,
                render: jcode_tui_messages::ImageRegionRender::Fit,
            });
        }
    }
    image_regions
}

fn error_copy_target(content: &str, rendered_line_count: usize) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::Error, content, rendered_line_count)
}

fn tool_output_copy_target(content: &str, rendered_line_count: usize) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::ToolOutput, content, rendered_line_count)
}

fn copy_target_for_kind(
    kind: CopyTargetKind,
    content: &str,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    Some(RawCopyTarget {
        kind,
        content: content.to_string(),
        start_raw_line: 0,
        end_raw_line: rendered_line_count.max(1),
        badge_raw_line: 0,
    })
}

fn offset_copy_target(target: RawCopyTarget, line_offset: usize) -> RawCopyTarget {
    RawCopyTarget {
        kind: target.kind,
        content: target.content,
        start_raw_line: target.start_raw_line + line_offset,
        end_raw_line: target.end_raw_line + line_offset,
        badge_raw_line: target.badge_raw_line + line_offset,
    }
}

fn assistant_message_copy_targets(
    content: &str,
    rendered_lines: &[Line<'static>],
) -> Vec<RawCopyTarget> {
    if is_error_copy_content(content) {
        return error_copy_target(content, rendered_lines.len())
            .into_iter()
            .collect();
    }

    crate::tui::markdown::extract_copy_targets_from_rendered_lines(rendered_lines)
}

fn tool_message_copy_target(
    msg: &DisplayMessage,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    if is_error_copy_content(&msg.content) {
        return error_copy_target(&msg.content, rendered_line_count);
    }
    if tools_ui::tool_output_looks_failed(&msg.content) {
        return tool_output_copy_target(&msg.content, rendered_line_count);
    }
    None
}

/// Build the inline "what changed" delta lines for a `todo` tool message, if it
/// is a successful write. `messages` is the full transcript and `abs_idx` is the
/// absolute index of the todo message, so the previous todo list can be found.
fn todo_change_lines(
    messages: &[DisplayMessage],
    abs_idx: usize,
    msg: &DisplayMessage,
    width: u16,
) -> Vec<Line<'static>> {
    if tools_ui::tool_output_looks_failed(&msg.content) {
        return Vec::new();
    }
    let Some(tc) = msg.tool_data.as_ref() else {
        return Vec::new();
    };
    if tools_ui::canonical_tool_name(&tc.name) != "todo" {
        return Vec::new();
    }
    let Some(next) = super::todo_changes::todos_from_tool_input(tc) else {
        return Vec::new();
    };
    let prev = super::todo_changes::previous_todos(messages, abs_idx);
    super::todo_changes::render_todo_change_lines(prev.as_deref(), &next, width)
}

#[expect(
    clippy::too_many_arguments,
    reason = "User prompt rendering updates the prepared-line side tables together"
)]
fn push_user_prompt_lines(
    lines: &mut Vec<Line<'static>>,
    raw_plain_lines: &mut Vec<String>,
    line_raw_overrides: &mut Vec<Option<WrappedLineMap>>,
    line_copy_offsets: &mut Vec<usize>,
    user_line_indices: &mut Vec<usize>,
    prompt_num: usize,
    num_color: Color,
    content: &str,
    align: ratatui::layout::Alignment,
) {
    let prefix_width = unicode_width::UnicodeWidthStr::width(prompt_num.to_string().as_str())
        + unicode_width::UnicodeWidthStr::width("› ");
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    for (line_idx, content_line) in normalized.split('\n').enumerate() {
        let raw_line = raw_plain_lines.len();
        raw_plain_lines.push(content_line.to_string());
        let prompt_width = unicode_width::UnicodeWidthStr::width(content_line);
        let rendered_line_idx = lines.len();
        let is_first_line = line_idx == 0;
        if is_first_line {
            user_line_indices.push(rendered_line_idx);
        }

        let prefix_spans = if is_first_line {
            vec![
                Span::styled(
                    format!("{}", prompt_num),
                    user_prompt_number_style(num_color),
                ),
                Span::styled("› ", user_prompt_accent_style()),
            ]
        } else {
            vec![Span::styled(
                " ".repeat(prefix_width),
                user_prompt_accent_style(),
            )]
        };
        let mut spans = prefix_spans;
        spans.push(Span::styled(
            content_line.to_string(),
            user_prompt_text_style(),
        ));
        lines.push(Line::from(spans).alignment(align));
        line_raw_overrides.push(Some(WrappedLineMap {
            raw_line,
            start_col: 0,
            end_col: prompt_width,
        }));
        line_copy_offsets.push(prefix_width);
    }
}

fn empty_prepared_messages() -> PreparedMessages {
    PreparedMessages {
        wrapped_lines: Vec::new(),
        wrapped_plain_lines: Arc::new(Vec::new()),
        wrapped_copy_offsets: Arc::new(Vec::new()),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }
}

/// Stamp `prepared` with the deferred-mermaid staleness marker: `Some(epoch)`
/// when any wrapped line is the "rendering mermaid diagram..." placeholder
/// (a background render is still in flight), `None` otherwise. `epoch_before`
/// must be the deferred-render epoch read *before* the markdown was rendered,
/// so a render completing mid-build immediately reads as stale.
fn stamp_mermaid_pending(prepared: &mut PreparedMessages, epoch_before: u64) {
    prepared.mermaid_pending_epoch = prepared
        .wrapped_lines
        .iter()
        .any(markdown::line_is_mermaid_pending_placeholder)
        .then_some(epoch_before);
}

/// Merge the pending stamp of a reused base with a freshly rendered part.
/// Keeps the earliest epoch so staleness is never masked.
fn merge_mermaid_pending(
    base: Option<u64>,
    fresh_lines_pending: bool,
    epoch_before: u64,
) -> Option<u64> {
    let fresh = fresh_lines_pending.then_some(epoch_before);
    match (base, fresh) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Transcript message index owning the first deferred-mermaid pending
/// placeholder line, for cutting a stale base at a message boundary. `None`
/// when no pending line exists.
fn first_mermaid_pending_message(prepared: &PreparedMessages) -> Option<usize> {
    let line_idx = prepared
        .wrapped_lines
        .iter()
        .position(markdown::line_is_mermaid_pending_placeholder)?;
    Some(
        prepared
            .message_boundaries
            .partition_point(|boundary| boundary.wrapped_len <= line_idx),
    )
}

fn active_batch_progress(app: &dyn TuiState) -> Option<crate::bus::BatchProgress> {
    match app.status() {
        ProcessingStatus::RunningTool(name) if name == "batch" => app.batch_progress(),
        _ => None,
    }
}

pub(super) fn active_batch_progress_hash(app: &dyn TuiState) -> u64 {
    let Some(progress) = active_batch_progress(app) else {
        return 0;
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if progress.completed < progress.total {
        super::activity_indicator_frame_index(app.animation_elapsed(), 12.5).hash(&mut hasher);
    }
    progress.total.hash(&mut hasher);
    progress.completed.hash(&mut hasher);
    progress.last_completed.hash(&mut hasher);
    for subcall in &progress.subcalls {
        subcall.index.hash(&mut hasher);
        subcall.tool_call.id.hash(&mut hasher);
        subcall.tool_call.name.hash(&mut hasher);
        match subcall.state {
            crate::bus::BatchSubcallState::Running => 0u8,
            crate::bus::BatchSubcallState::Succeeded => 1u8,
            crate::bus::BatchSubcallState::Failed => 2u8,
        }
        .hash(&mut hasher);
        if let Ok(input) = serde_json::to_string(&subcall.tool_call.input) {
            input.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn swarm_members_signature(members: &[crate::protocol::SwarmMemberStatus]) -> u64 {
    // Chat only renders a stable one-line identity/status summary. Excluding
    // elapsed time, age, output tails, todos, tool progress, and runtime details
    // prevents high-frequency live swarm updates from invalidating the entire
    // transcript preparation cache.
    let mut ordered: Vec<_> = members.iter().collect();
    ordered.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let mut hasher = DefaultHasher::new();
    for member in ordered {
        member.session_id.hash(&mut hasher);
        member.friendly_name.hash(&mut hasher);
        member.status.hash(&mut hasher);
        member.task_label.hash(&mut hasher);
    }
    hasher.finish()
}

fn spawned_member_for_tool<'a>(
    msg: &DisplayMessage,
    members: &'a [crate::protocol::SwarmMemberStatus],
) -> Option<&'a crate::protocol::SwarmMemberStatus> {
    let tool = msg.tool_data.as_ref()?;
    if tools_ui::canonical_tool_name(&tool.name) != "swarm" {
        return None;
    }

    // Live remote tool output is decorated as `[swarm] Spawned new agent: …`.
    // The completed ToolCall can also lose its parsed input across a reload or
    // event race, so the server-issued session ID is the authoritative signal.
    let session_id = msg
        .content
        .lines()
        .find_map(|line| line.split_once("Spawned new agent: ").map(|(_, id)| id))
        .map(str::trim);
    if let Some(member) = session_id.and_then(|session_id| {
        members
            .iter()
            .find(|member| member.session_id == session_id)
    }) {
        return Some(member);
    }

    // The server may publish the member snapshot before or after the tool result,
    // and older/reformatted tool results may not retain the exact session-id line.
    // The spawn label is copied into `task_label`, so use a unique label match as
    // a safe fallback. Requiring uniqueness prevents an old spawn row from
    // adopting a newer worker when labels are reused.
    if tool.input.get("action").and_then(|value| value.as_str()) != Some("spawn") {
        return None;
    }
    let label = tool.input.get("label").and_then(|value| value.as_str())?;
    let mut matching = members
        .iter()
        .filter(|member| member.task_label.as_deref() == Some(label));
    let member = matching.next()?;
    matching.next().is_none().then_some(member)
}

fn prepare_active_batch_progress(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    let Some(progress) = active_batch_progress(app) else {
        return empty_prepared_messages();
    };

    let centered = app.centered_mode();
    let accent = rgb(255, 193, 94);
    let spinner = super::activity_indicator(app.animation_elapsed(), 12.5);
    let block_width = if centered {
        super::centered_content_block_width(width, 96)
    } else {
        width as usize
    };
    let row_width = block_width.saturating_sub(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    if prefix_blank {
        lines.push(Line::from(""));
    }

    let mut header = vec![
        Span::styled(format!("  {} ", spinner), Style::default().fg(accent)),
        Span::styled("batch", Style::default().fg(tool_color())),
        Span::styled(
            format!(" · {}/{} done", progress.completed, progress.total),
            Style::default().fg(dim_color()),
        ),
    ];
    if let Some(last) = progress
        .last_completed
        .as_ref()
        .filter(|_| progress.completed < progress.total)
    {
        header.push(Span::styled(
            format!(" · last done: {}", last),
            Style::default().fg(dim_color()),
        ));
    }
    lines.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(header),
        width.saturating_sub(1) as usize,
    ));

    let mut hidden_completed = 0usize;
    for subcall in &progress.subcalls {
        let (icon, icon_color) = match subcall.state {
            crate::bus::BatchSubcallState::Running => (spinner, accent),
            crate::bus::BatchSubcallState::Succeeded => {
                hidden_completed += 1;
                continue;
            }
            crate::bus::BatchSubcallState::Failed => ("✗", rgb(220, 100, 100)),
        };

        lines.push(tools_ui::render_batch_subcall_line(
            &subcall.tool_call,
            icon,
            icon_color,
            50,
            Some(row_width),
            None,
        ));
    }

    if hidden_completed > 0 && progress.completed < progress.total {
        lines.push(Line::from(Span::styled(
            format!("    … {} completed", hidden_completed),
            Style::default().fg(dim_color()),
        )));
    }

    if centered {
        super::left_pad_lines_to_block_width(&mut lines, width, block_width);
    }

    wrap_lines_with_map(lines, &[], &[], &[], &[], &[], width, &[], &[], &[])
}

pub(super) fn prepare_messages(
    app: &dyn TuiState,
    width: u16,
    height: u16,
) -> Arc<PreparedChatFrame> {
    // A cached prepared frame intentionally owns only image ids. Recover any
    // staged source evicted by the byte budget or a visibility toggle before an
    // exact frame-cache hit can bypass the normal anchored-image resolver.
    super::inline_image_ui::restage_requested_payloads(app);
    if cfg!(test) {
        return Arc::new(prepare_messages_inner(app, width, height));
    }

    let key = FullPrepCacheKey {
        width,
        height,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
        centered: app.centered_mode(),
        mermaid_aspect_bucket: crate::tui::mermaid::current_preferred_aspect_ratio_bucket(),
        is_processing: app.is_processing(),
        streaming_text_len: app.streaming_text().len(),
        streaming_text_hash: super::hash_text_for_cache(app.streaming_text()),
        batch_progress_hash: active_batch_progress_hash(app),
        // An unpinned transcript must not reuse a previously prepared frame
        // containing anchored images. With no images, both modes are visually
        // identical and `(0, 0)` reuse is safe.
        inline_images_signature: if app.pin_images() {
            app.side_pane_images_signature()
        } else {
            (0, 0)
        },
        inline_images_visible: app.inline_images_visible(),
        expanded_images_version: app.expanded_images_version(),
        swarm_members_signature: swarm_members_signature(&app.swarm_members_for_transcript()),
    };

    super::note_full_prep_request();
    let cache_lookup_start = Instant::now();

    {
        let cache = match full_prep_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => {
                let mut c = poisoned.into_inner();
                c.entries.clear();
                c
            }
        };
        let mut cache = cache;
        if let Some((prepared, kind)) = cache.get_exact_with_kind(&key) {
            // A completed deferred mermaid render does not bump
            // `messages_version`/`streaming_text_hash`, so an exact hit can
            // still bake in a stale "rendering..." placeholder. Fall through
            // to a rebuild, which re-renders the pending tail.
            let stale = prepared
                .mermaid_pending_epoch()
                .is_some_and(|stamp| crate::tui::mermaid::deferred_render_epoch() != stamp);
            if !stale {
                super::note_full_prep_cache_lookup(cache_lookup_start.elapsed());
                super::note_full_prep_cache_hit(kind, prepared.as_ref());
                return prepared;
            }
        }
    }

    super::note_full_prep_cache_lookup(cache_lookup_start.elapsed());
    super::note_full_prep_cache_miss();

    let build_start = Instant::now();
    let prepared = Arc::new(prepare_messages_inner(app, width, height));
    super::note_full_prep_built(prepared.as_ref(), build_start.elapsed());

    {
        if let Ok(mut cache) = full_prep_cache().lock() {
            cache.insert(key, prepared.clone());
        }
    }

    prepared
}

fn prepare_messages_inner(app: &dyn TuiState, width: u16, height: u16) -> PreparedChatFrame {
    let header_start = Instant::now();
    let header_prepared = prepare_header_cached(app, width);
    let header_ms = header_start.elapsed().as_secs_f64() * 1000.0;

    let body_start = Instant::now();
    let body_prepared = prepare_body_cached(app, width);
    let body_ms = body_start.elapsed().as_secs_f64() * 1000.0;

    // Anchored images render inside the body at their producing message; only
    // images without a resolvable anchor target fall back to this trailing
    // inline section so nothing silently disappears.
    let inline_images_prepared = if app.pin_images() {
        let anchored = super::inline_image_ui::resolve_anchored_items_cached(app);
        let items = anchored.unplaced_items(app.display_messages());
        if items.is_empty() {
            Arc::new(empty_prepared_messages())
        } else {
            let prefix_blank = !body_prepared.wrapped_lines.is_empty();
            Arc::new(super::inline_image_ui::build_section(
                &items,
                width,
                height,
                prefix_blank,
                app.inline_images_visible(),
                &super::inline_image_ui::AppExpandLevels(app),
            ))
        }
    } else {
        Arc::new(empty_prepared_messages())
    };

    let batch_start = Instant::now();
    let has_batch_progress = active_batch_progress(app).is_some();
    let batch_prefix_blank = has_batch_progress && !body_prepared.wrapped_lines.is_empty();
    let batch_progress_prepared = if has_batch_progress {
        Arc::new(prepare_active_batch_progress(
            app,
            width,
            batch_prefix_blank,
        ))
    } else {
        Arc::new(empty_prepared_messages())
    };
    let batch_ms = batch_start.elapsed().as_secs_f64() * 1000.0;

    let streaming_start = Instant::now();
    // Reasoning traces in `current` mode are anchored display messages inside
    // the body now; no separate retained/collapsing trace section exists.
    let reasoning_prepared = Arc::new(empty_prepared_messages());
    let has_streaming = app.is_processing() && !app.streaming_text().is_empty();
    let stream_prefix_blank = has_streaming
        && (!body_prepared.wrapped_lines.is_empty()
            || !batch_progress_prepared.wrapped_lines.is_empty()
            || !reasoning_prepared.wrapped_lines.is_empty());
    let streaming_prepared = if has_streaming {
        Arc::new(prepare_streaming_cached(app, width, stream_prefix_blank))
    } else {
        Arc::new(empty_prepared_messages())
    };
    let streaming_ms = streaming_start.elapsed().as_secs_f64() * 1000.0;

    let is_initial_empty = app.onboarding_preview_mode()
        || (app.display_messages().is_empty()
            && !app.is_processing()
            && app.streaming_text().is_empty());

    if is_initial_empty {
        let compose_start = Instant::now();
        let suggestions = app.suggestion_prompts();
        let is_centered = app.centered_mode();
        let suggestion_align = if is_centered {
            ratatui::layout::Alignment::Center
        } else {
            ratatui::layout::Alignment::Left
        };
        let mut wrapped_lines = header_prepared.wrapped_lines.clone();

        if !suggestions.is_empty() {
            wrapped_lines.push(Line::from(""));
            for (i, (label, prompt)) in suggestions.iter().enumerate() {
                let is_login = prompt.starts_with('/');
                let pad = if is_centered { "" } else { "  " };
                let spans = if is_login {
                    vec![
                        Span::styled(
                            format!("{}{} ", pad, label),
                            Style::default()
                                .fg(rgb(138, 180, 248))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("(type {})", prompt),
                            Style::default().fg(dim_color()),
                        ),
                    ]
                } else {
                    vec![
                        Span::styled(
                            format!("{}[{}] ", pad, i + 1),
                            Style::default().fg(rgb(138, 180, 248)),
                        ),
                        Span::styled(label.clone(), Style::default().fg(rgb(200, 200, 200))),
                    ]
                };
                wrapped_lines.push(Line::from(spans).alignment(suggestion_align));
            }
            if suggestions.len() > 1 {
                wrapped_lines.push(Line::from(""));
                wrapped_lines.push(
                    Line::from(Span::styled(
                        format!(
                            "{}Press 1-{} or type anything to start",
                            if is_centered { "" } else { "  " },
                            suggestions.len()
                        ),
                        Style::default().fg(dim_color()),
                    ))
                    .alignment(suggestion_align),
                );
            }
        }

        let content_height = wrapped_lines.len();
        let input_reserve = 4;
        let available = (height as usize).saturating_sub(input_reserve);
        let pad_top = available.saturating_sub(content_height) / 2;
        let mut centered = Vec::with_capacity(pad_top + content_height);
        for _ in 0..pad_top {
            centered.push(Line::from(""));
        }
        centered.extend(wrapped_lines);
        let wrapped_lines = centered;
        let wrapped_line_count = wrapped_lines.len();
        let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());
        let prepared = Arc::new(PreparedMessages {
            wrapped_lines,
            wrapped_plain_lines,
            wrapped_copy_offsets: Arc::new(vec![0; wrapped_line_count]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        });
        let frame = PreparedChatFrame::from_single(prepared);
        super::note_full_prep_phase_metrics(super::FullPrepPhaseMetrics {
            header_ms,
            body_ms,
            batch_ms,
            streaming_ms,
            compose_ms: compose_start.elapsed().as_secs_f64() * 1000.0,
        });
        return frame;
    }

    let compose_start = Instant::now();
    let frame = PreparedChatFrame::from_sections(vec![
        (PreparedSectionKind::Header, header_prepared),
        (PreparedSectionKind::Body, body_prepared),
        (PreparedSectionKind::InlineImages, inline_images_prepared),
        (PreparedSectionKind::BatchProgress, batch_progress_prepared),
        (PreparedSectionKind::Reasoning, reasoning_prepared),
        (PreparedSectionKind::Streaming, streaming_prepared),
    ]);
    super::note_full_prep_phase_metrics(super::FullPrepPhaseMetrics {
        header_ms,
        body_ms,
        batch_ms,
        streaming_ms,
        compose_ms: compose_start.elapsed().as_secs_f64() * 1000.0,
    });
    frame
}

/// TTL + cheap-signature cache for the prepared (built + wrapped) header.
///
/// `build_persistent_header`/`build_header_lines` look cheap but are not: per
/// call they probe auth credential files (`AuthStatus::check_fast`), list and
/// deserialize every goal JSON for the goal badge, reload the project skill
/// overlay from disk, and stat release/binary channels for the update badges.
/// On this hardware that costs 30-50ms, and it ran on every full-prep cache
/// miss - i.e. on every streaming tick and every keystroke that changes the
/// input-derived cache key - which showed up directly as input lag
/// (TUI_SLOW_FRAME logs attributed 30-47ms of ~45ms slow frames to
/// `full_prep_header_ms`).
///
/// The header's inputs fall into two groups:
/// - cheap in-memory fields (model, session/server names, connection type,
///   mcp list, ...) - hashed into a signature so changes rebuild immediately;
/// - disk-backed surfaces (auth line, goal badge, skills list, update
///   badges, changelog) - refreshed only when the TTL lapses, since they
///   change rarely and independently of the render loop.
const HEADER_PREP_CACHE_TTL: std::time::Duration = std::time::Duration::from_millis(1000);

struct HeaderPrepCacheState {
    signature: u64,
    built_at: Instant,
    prepared: Arc<PreparedMessages>,
}

fn header_prep_cache() -> &'static std::sync::Mutex<Option<HeaderPrepCacheState>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<HeaderPrepCacheState>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Hash of the header inputs that are cheap to read every frame. Anything
/// expensive (disk probes) is intentionally excluded and covered by the TTL.
fn header_prep_signature(app: &dyn TuiState, width: u16) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut hasher);
    app.provider_model().hash(&mut hasher);
    app.provider_name().hash(&mut hasher);
    app.session_display_name().hash(&mut hasher);
    app.server_display_name().hash(&mut hasher);
    app.server_display_version().hash(&mut hasher);
    app.server_display_icon().hash(&mut hasher);
    app.connection_type().hash(&mut hasher);
    app.upstream_provider().hash(&mut hasher);
    app.is_replay().hash(&mut hasher);
    app.is_remote_mode().hash(&mut hasher);
    app.is_canary().hash(&mut hasher);
    app.server_update_available().hash(&mut hasher);
    app.mcp_servers().hash(&mut hasher);
    app.connected_clients().hash(&mut hasher);
    app.server_sessions().len().hash(&mut hasher);
    app.working_dir().hash(&mut hasher);
    // The goal badge renders the focused side-panel page title when a goal
    // page is focused; keying on it keeps focus changes instant.
    if let Some(page) = app.side_panel().focused_page() {
        page.id.hash(&mut hasher);
        page.title.hash(&mut hasher);
    }
    hasher.finish()
}

fn prepare_header_cached(app: &dyn TuiState, width: u16) -> Arc<PreparedMessages> {
    let build = || {
        let mut all_header_lines = header::build_persistent_header(app, width);
        all_header_lines.extend(header::build_header_lines(app, width));
        Arc::new(wrap_lines(all_header_lines, &[], &[], &[], width))
    };

    if cfg!(test) {
        return build();
    }

    let signature = header_prep_signature(app, width);

    if let Ok(cache) = header_prep_cache().lock()
        && let Some(state) = cache.as_ref()
        && state.signature == signature
        && state.built_at.elapsed() < HEADER_PREP_CACHE_TTL
    {
        return state.prepared.clone();
    }

    let prepared = build();
    if let Ok(mut cache) = header_prep_cache().lock() {
        *cache = Some(HeaderPrepCacheState {
            signature,
            built_at: Instant::now(),
            prepared: prepared.clone(),
        });
    }
    prepared
}

fn prepare_body_cached(app: &dyn TuiState, width: u16) -> Arc<PreparedMessages> {
    if cfg!(test) {
        return Arc::new(prepare_body(app, width, false));
    }

    super::note_body_request();

    let key = BodyCacheKey {
        width,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
        centered: app.centered_mode(),
        mermaid_aspect_bucket: crate::tui::mermaid::current_preferred_aspect_ratio_bucket(),
        pin_images: app.pin_images(),
        inline_images_visible: app.inline_images_visible(),
        images_signature: app.side_pane_images_signature(),
        expanded_images_version: app.expanded_images_version(),
        swarm_members_signature: swarm_members_signature(&app.swarm_members_for_transcript()),
    };
    let msg_count = app.display_messages().len();
    let cache_lookup_start = Instant::now();

    let cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => {
            let mut c = poisoned.into_inner();
            c.entries.clear();
            c
        }
    };

    let mut cache = cache;
    if let Some((prepared, kind)) = cache.get_exact_with_kind(&key) {
        // A deferred mermaid render completing does not bump
        // `messages_version`, so an exact hit can still be stale: it bakes in
        // a "rendering..." placeholder whose background render has since
        // finished. Fall through to the rebuild path, which truncates the
        // base at the pending message and re-renders the tail.
        let stale = prepared
            .mermaid_pending_epoch
            .is_some_and(|stamp| crate::tui::mermaid::deferred_render_epoch() != stamp);
        if !stale {
            super::note_body_cache_lookup(cache_lookup_start.elapsed());
            super::note_body_cache_hit(kind, prepared.as_ref());
            return prepared;
        }
    }

    super::note_body_cache_lookup(cache_lookup_start.elapsed());
    super::note_body_cache_miss();

    let incremental_base = cache.take_best_incremental_base(&key);

    drop(cache);

    let build_start = Instant::now();
    let (prepared, build_path) = match incremental_base {
        Some((prev, prev_count, prev_prompt_offset)) => {
            build_body_from_base(app, width, prev, prev_count, prev_prompt_offset, msg_count)
        }
        None => (Arc::new(prepare_body(app, width, false)), "full"),
    };

    super::note_body_built(prepared.as_ref(), build_start.elapsed(), build_path);

    let mut cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.insert(
        key,
        prepared.clone(),
        msg_count,
        app.compacted_hidden_user_prompts(),
    );
    prepared
}

/// Rebuild the body reusing as much of a cached base as possible. Tries, in
/// order: exact reuse (identical messages), suffix reuse (older history was
/// prepended above an otherwise unchanged transcript - the "scroll to top of a
/// compacted session" path), and prefix reuse (append / tail edit). Falls back
/// to re-rendering whatever cannot be reused.
pub(super) fn build_body_from_base(
    app: &dyn TuiState,
    width: u16,
    mut prev: Arc<PreparedMessages>,
    mut prev_count: usize,
    prev_prompt_offset: usize,
    msg_count: usize,
) -> (Arc<PreparedMessages>, &'static str) {
    let messages = app.display_messages();
    // A stale deferred-mermaid base (its background render finished after the
    // base was built) must not be reused verbatim: cut it at the message that
    // owns the first pending placeholder so that message and everything after
    // it re-render and pick up the completed diagram.
    if let Some(stamp) = prev.mermaid_pending_epoch
        && crate::tui::mermaid::deferred_render_epoch() != stamp
    {
        match first_mermaid_pending_message(prev.as_ref()) {
            Some(keep) if !prev.message_boundaries.is_empty() => {
                let prepared = Arc::make_mut(&mut prev);
                truncate_prepared_to_boundary(prepared, keep);
                prepared.mermaid_pending_epoch = None;
                prev_count = prepared.message_boundaries.len();
            }
            Some(_) => {
                // No boundary tracking: cannot cut at the pending message.
                return (Arc::new(prepare_body(app, width, false)), "full");
            }
            None => {
                // Stamp outlived its placeholder (e.g. the pending tail was
                // truncated away); clear it and reuse normally.
                Arc::make_mut(&mut prev).mermaid_pending_epoch = None;
            }
        }
    }
    // The selected base shares this key's width/mode/image signature. Find the
    // longest message prefix whose hashes still match the current transcript.
    let k = matching_prefix_len(prev.as_ref(), messages);
    if k == prev_count && prev_count == msg_count {
        // Exact same messages (the body differs only by something not in the
        // cache key, e.g. a width-independent flag); reuse as-is.
        return (prev, "prefix_exact");
    }

    // Prepend (suffix) reuse: when older compacted history is loaded in, the
    // new transcript is `new head + unchanged old messages`, so the prefix
    // match above collapses (the compacted-history marker at index 0 changes
    // every load) even though almost everything is reusable. Re-rendering the
    // whole transcript per 64-message chunk made "scroll to the start of a
    // long session" O(total transcript) per chunk - the issue #344 hang.
    // Instead, when a message suffix still matches, render only the new head
    // and stitch the prepared suffix below it.
    let base_msgs = prev.message_boundaries.len();
    let s = matching_suffix_len(prev.as_ref(), messages)
        .min(base_msgs.saturating_sub(1))
        .min(msg_count.saturating_sub(1));
    if s > k {
        let drop_msgs = base_msgs - s;
        let head_count = msg_count - s;
        if suffix_reuse_compatible(
            app,
            messages,
            head_count,
            prev.as_ref(),
            drop_msgs,
            prev_prompt_offset,
        ) {
            match prepare_body_prepended(app, width, prev, drop_msgs, head_count) {
                Ok(stitched) => {
                    super::note_body_incremental_reuse(s);
                    return (stitched, "suffix_reuse");
                }
                Err(returned) => prev = returned,
            }
        }
    }

    // Prefix reuse: if the whole base matches it's a pure append
    // (`k == prev_count`); otherwise the tail diverged (in-place edit,
    // finalize, truncation) and we truncate the base to the matching prefix
    // before re-rendering only the changed tail.
    if k != prev_count {
        truncate_prepared_to_boundary(Arc::make_mut(&mut prev), k);
    }
    super::note_body_incremental_reuse(k);
    (
        prepare_body_incremental(app, width, prev, k),
        if k == prev_count {
            "incremental"
        } else {
            "prefix_reuse"
        },
    )
}

/// Immutable per-build render context shared by the full and incremental body
/// builders. Holding this in one place keeps the single per-message renderer
/// (`render_message_into`) free of divergence between the two entry points.
struct BodyRenderCtx<'a> {
    app: &'a dyn TuiState,
    width: u16,
    centered: bool,
    /// Number rendered next to the first user prompt = global prompt count +
    /// number of prompts hidden by compaction.
    prompt_number_offset: usize,
    total_prompts: usize,
    pending_count: usize,
    anchored_images: Arc<super::inline_image_ui::AnchoredInlineImages>,
    inline_images_visible: bool,
    messages: &'a [DisplayMessage],
    swarm_members: Vec<crate::protocol::SwarmMemberStatus>,
}

/// Mutable accumulator for one body build. Both `prepare_body` (full) and
/// `prepare_body_incremental` (tail only) push into one of these via the shared
/// `render_message_into`, guaranteeing byte-identical per-message output and
/// recording a per-message boundary table for prefix reuse.
#[derive(Default)]
struct BodyAcc {
    lines: Vec<Line<'static>>,
    raw_plain_lines: Vec<String>,
    line_raw_overrides: Vec<Option<WrappedLineMap>>,
    line_copy_offsets: Vec<usize>,
    user_line_indices: Vec<usize>,
    user_prompt_texts: Vec<String>,
    edit_tool_line_ranges: Vec<(usize, String, usize, usize, bool)>,
    copy_targets: Vec<RawCopyTarget>,
    /// One entry per rendered message, in order: `(msg_hash, lines_len_after,
    /// raw_len_after, user_prompt_len_after)`. `lines_len_after` counts any
    /// separator blank pushed at the start of that message, so truncating the
    /// wrapped body at a boundary cleanly drops the message together with its
    /// leading blank. `raw_len_after` is the contiguous raw count, used to
    /// truncate the raw array in lockstep.
    segments: Vec<(u64, usize, usize, usize)>,
    /// Next 1-based prompt number to assign. Seeded from the reused base in the
    /// incremental path so numbering continues without rescanning the prefix.
    prompt_num: usize,
    /// 0-based ordinal of the next rendered user prompt excluding synthetic
    /// attached-image label messages; mirrors the session renderer's count.
    anchor_prompt_ordinal: usize,
    /// True when a prior (reused) body already has content, so the first message
    /// rendered here still gets its leading separator blank.
    body_has_content: bool,
}

impl BodyAcc {
    /// Push a fully-rendered display line whose raw selection text is the line
    /// itself (no logical-line mapping). Seeds a contiguous raw entry and an
    /// explicit full-width override so the body's `raw_plain_lines` stays
    /// message-ordered. Keeping raws contiguous is what lets a tail-edit rebuild
    /// truncate both the wrapped arrays *and* the raw array at a message
    /// boundary, avoiding an unbounded raw leak across repeated tail edits (e.g.
    /// a streaming tool result that updates many times).
    fn push_auto(&mut self, line: Line<'static>) {
        let raw_text = ui::line_plain_text(&line);
        let raw_width = unicode_width::UnicodeWidthStr::width(raw_text.as_str());
        let raw_line = self.raw_plain_lines.len();
        self.raw_plain_lines.push(raw_text);
        self.lines.push(line);
        self.line_raw_overrides.push(Some(WrappedLineMap {
            raw_line,
            start_col: 0,
            end_col: raw_width,
        }));
        self.line_copy_offsets.push(0);
    }

    /// Push a blank separator line. Routed through `push_auto` so it seeds a
    /// (empty) raw and keeps the raw array contiguous.
    fn push_blank(&mut self) {
        self.push_auto(Line::from(""));
    }
}

/// Render a single transcript message into `acc`. This is the one canonical
/// per-message renderer; the full and incremental body builders both call it so
/// their output cannot drift (which previously caused subtle text-selection and
/// spacing differences on the incremental path).
fn render_message_into(
    ctx: &BodyRenderCtx<'_>,
    acc: &mut BodyAcc,
    msg: &DisplayMessage,
    msg_global_idx: usize,
) {
    let width = ctx.width;
    let centered = ctx.centered;
    let app = ctx.app;
    let role = msg.effective_role();
    let align = default_message_alignment(role, centered);

    if (acc.body_has_content || !acc.lines.is_empty())
        && role != "tool"
        && role != "meta"
        && role != "swarm"
    {
        acc.push_blank();
    }

    match role {
        "user" => {
            acc.prompt_num += 1;
            acc.user_prompt_texts.push(msg.content.clone());
            let distance = ctx.total_prompts + ctx.pending_count + 1 - acc.prompt_num;
            let num_color = rainbow_prompt_color(distance);
            let displayed_prompt_num = acc.prompt_num + ctx.prompt_number_offset;
            push_user_prompt_lines(
                &mut acc.lines,
                &mut acc.raw_plain_lines,
                &mut acc.line_raw_overrides,
                &mut acc.line_copy_offsets,
                &mut acc.user_line_indices,
                displayed_prompt_num,
                num_color,
                &msg.content,
                align,
            );
            if !crate::session::is_attached_image_label_text(&msg.content) {
                let ordinal = acc.anchor_prompt_ordinal;
                acc.anchor_prompt_ordinal += 1;
                if let Some(items) = ctx.anchored_images.by_prompt.get(&ordinal) {
                    for line in super::inline_image_ui::anchored_image_lines(
                        items,
                        width,
                        ctx.inline_images_visible,
                        &super::inline_image_ui::AppExpandLevels(app),
                    ) {
                        acc.push_auto(line);
                    }
                }
            }
        }
        "assistant" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                render_assistant_message,
            );
            let message_copy_targets = assistant_message_copy_targets(&msg.content, &cached);
            for target in message_copy_targets {
                acc.copy_targets
                    .push(offset_copy_target(target, acc.lines.len()));
            }
            let aux = assistant_aux_data(
                msg,
                &cached,
                content_width,
                centered,
                app.diff_mode(),
                align,
            );
            let content_line_count = aux.content_line_count;
            let logical_plain_lines = &aux.logical_plain_lines;
            let raw_base = acc.raw_plain_lines.len();
            let content_maps = map_display_lines_to_logical_lines(
                &cached[..content_line_count],
                logical_plain_lines,
                raw_base,
            );
            if let Some(maps) = content_maps {
                // Content lines map back into the logical markdown lines for
                // text selection; seed those raws contiguously. Trailing
                // tool-summary lines (idx >= content_line_count) and any line
                // the mapping could not cover fall back to self-raws via
                // `push_auto`, keeping `raw_plain_lines` message-contiguous.
                acc.raw_plain_lines
                    .extend(logical_plain_lines.iter().cloned());
                for (idx, line) in cached.into_iter().enumerate() {
                    let line = align_if_unset(line, align);
                    if let Some(map) = maps.get(idx).copied() {
                        acc.lines.push(line);
                        acc.line_raw_overrides.push(Some(map));
                        acc.line_copy_offsets.push(0);
                    } else {
                        acc.push_auto(line);
                    }
                }
            } else {
                // Logical mapping failed; every display line uses its own plain
                // text as the raw, matching the previous wrap-time fallback but
                // recorded contiguously and in message order.
                for line in cached {
                    acc.push_auto(align_if_unset(line, align));
                }
            }
        }
        "meta" => {
            let raw_line = acc.raw_plain_lines.len();
            acc.raw_plain_lines.push(msg.content.clone());
            let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
            let prefix_width = if centered {
                0
            } else {
                unicode_width::UnicodeWidthStr::width("  ")
            };
            acc.lines.push(
                Line::from(vec![
                    Span::raw(if centered { "" } else { "  " }),
                    Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                ])
                .alignment(align),
            );
            acc.line_raw_overrides.push(Some(WrappedLineMap {
                raw_line,
                start_col: 0,
                end_col: raw_width,
            }));
            acc.line_copy_offsets.push(prefix_width);
        }
        "tool" => {
            let tool_start_line = acc.lines.len();
            let cached = get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
            if let Some(target) = tool_message_copy_target(msg, cached.len()) {
                acc.copy_targets
                    .push(offset_copy_target(target, tool_start_line));
            }
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
            if let Some(member) = spawned_member_for_tool(msg, &ctx.swarm_members) {
                for line in crate::tui::info_widget::swarm_gallery::render_swarm_chat_card_lines(
                    std::slice::from_ref(member),
                    width.saturating_sub(1) as usize,
                ) {
                    acc.push_auto(line.alignment(ratatui::layout::Alignment::Left));
                }
            }
            for line in todo_change_lines(ctx.messages, msg_global_idx, msg, width) {
                acc.push_auto(align_if_unset(line, align));
            }
            if let Some(ref tc) = msg.tool_data {
                let is_edit_tool = tools_ui::is_edit_tool_name(&tc.name);
                if is_edit_tool {
                    let file_path = tc
                        .input
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .or_else(|| {
                            tc.input
                                .get("patch_text")
                                .and_then(|v| v.as_str())
                                .and_then(|patch_text| {
                                    match tools_ui::canonical_tool_name(&tc.name) {
                                        "apply_patch" => {
                                            tools_ui::extract_apply_patch_primary_file(patch_text)
                                        }
                                        "patch" => {
                                            tools_ui::extract_unified_patch_primary_file(patch_text)
                                        }
                                        _ => None,
                                    }
                                })
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    let expandable =
                        messages::edit_tool_inline_diff_is_expandable(tc, &msg.content, width);
                    acc.edit_tool_line_ranges.push((
                        msg_global_idx,
                        file_path,
                        tool_start_line,
                        acc.lines.len(),
                        expandable,
                    ));
                }
                if let Some(items) = ctx.anchored_images.by_tool.get(&tc.id) {
                    for line in super::inline_image_ui::anchored_image_lines(
                        items,
                        width,
                        ctx.inline_images_visible,
                        &super::inline_image_ui::AppExpandLevels(app),
                    ) {
                        acc.push_auto(line);
                    }
                }
            }
        }
        "system" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                render_system_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "reasoning" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                render_reasoning_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "background_task" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                render_background_task_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "swarm" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, app.diff_mode(), render_swarm_message);
            for line in cached {
                let line = align_if_unset(line, align);
                let plain = ui::line_plain_text(&line);
                let (semantic, prefix_width) = semantic_swarm_line_text(plain.as_str());
                let raw_line = acc.raw_plain_lines.len();
                let raw_width = unicode_width::UnicodeWidthStr::width(semantic.as_str());
                acc.raw_plain_lines.push(semantic);
                acc.lines.push(line);
                acc.line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                acc.line_copy_offsets.push(prefix_width);
            }
        }
        "memory" => {
            let border_style = Style::default().fg(rgb(130, 140, 180));
            let text_style = Style::default().fg(dim_color());
            let entries = super::memory_ui::parse_memory_display_entries(&msg.content);

            let count = entries.len();
            let tiles = group_into_tiles(entries);

            let header_text = if let Some(title) = &msg.title {
                title.clone()
            } else if count == 1 {
                "🧠 1 memory".to_string()
            } else {
                format!("🧠 {} memories", count)
            };
            let header = Line::from(Span::styled(header_text, border_style)).alignment(align);

            let total_width = if centered {
                (width.saturating_sub(4) as usize).min(120)
            } else {
                width.saturating_sub(2) as usize
            };
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "usage" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, app.diff_mode(), render_usage_message);
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "overnight" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                super::messages::render_overnight_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "todos" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                app.diff_mode(),
                super::messages::render_todos_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "error" => {
            let error_start_line = acc.lines.len();
            if let Some(target) = error_copy_target(&msg.content, 1) {
                acc.copy_targets
                    .push(offset_copy_target(target, error_start_line));
            }
            let raw_line = acc.raw_plain_lines.len();
            acc.raw_plain_lines.push(msg.content.clone());
            let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
            let prefix_width =
                unicode_width::UnicodeWidthStr::width(if centered { "✗ " } else { "  ✗ " });
            acc.lines.push(
                Line::from(vec![
                    Span::styled(
                        if centered { "✗ " } else { "  ✗ " },
                        Style::default().fg(Color::Red),
                    ),
                    Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
                ])
                .alignment(align),
            );
            acc.line_raw_overrides.push(Some(WrappedLineMap {
                raw_line,
                start_col: 0,
                end_col: raw_width,
            }));
            acc.line_copy_offsets.push(prefix_width);
        }
        _ => {}
    }

    acc.segments.push((
        msg.stable_cache_hash(),
        acc.lines.len(),
        acc.raw_plain_lines.len(),
        acc.user_prompt_texts.len(),
    ));
}

pub(super) fn prepare_body_incremental(
    app: &dyn TuiState,
    width: u16,
    mut prev: Arc<PreparedMessages>,
    prev_msg_count: usize,
) -> Arc<PreparedMessages> {
    let messages = app.display_messages();
    let new_messages = &messages[prev_msg_count..];
    if new_messages.is_empty() {
        return prev;
    }

    // Read before rendering the tail: a background diagram render completing
    // mid-build must leave the stamp already-stale.
    let mermaid_epoch_before = crate::tui::mermaid::deferred_render_epoch();
    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);

    // Images anchored to transcript messages render inline right after the
    // message that produced them. An incremental base is only reused when the
    // image set is unchanged (cache key includes the image signature), so any
    // anchored image matching a *new* message must be injected here; its anchor
    // target did not exist when the base was built.
    let anchored_images = super::inline_image_ui::resolve_anchored_items_cached(app);

    // The number of user prompts already rendered equals the number of cached
    // user prompt texts. Re-counting `messages[..prev_msg_count]` here on every
    // incremental append rescans the whole prior transcript, making a session
    // that grows one message at a time O(n^2). `prev.user_prompt_texts` is
    // extended in lockstep with each rendered user message, so its length is the
    // exact prior prompt count.
    let prev_prompt_count = prev.user_prompt_texts.len();
    // 0-based ordinal of the next rendered user prompt, excluding synthetic
    // attached-image label messages, mirroring the session renderer's count.
    let anchor_prompt_ordinal = if anchored_images.by_prompt.is_empty() {
        0
    } else {
        prev.user_prompt_texts
            .iter()
            .filter(|text| !crate::session::is_attached_image_label_text(text))
            .count()
    };

    let ctx = BodyRenderCtx {
        app,
        width,
        centered,
        prompt_number_offset: app.compacted_hidden_user_prompts(),
        total_prompts: app.display_user_message_count(),
        pending_count: input_ui::pending_prompt_count(app),
        anchored_images,
        inline_images_visible: app.inline_images_visible(),
        messages,
        swarm_members: app.swarm_members_for_transcript(),
    };

    let mut acc = BodyAcc {
        prompt_num: prev_prompt_count,
        anchor_prompt_ordinal,
        body_has_content: !prev.wrapped_lines.is_empty(),
        ..BodyAcc::default()
    };

    for (new_msg_offset, msg) in new_messages.iter().enumerate() {
        render_message_into(&ctx, &mut acc, msg, prev_msg_count + new_msg_offset);
    }

    let new_wrapped = wrap_lines_with_map(
        acc.lines,
        &acc.raw_plain_lines,
        &acc.line_raw_overrides,
        &acc.line_copy_offsets,
        &acc.user_line_indices,
        &acc.user_prompt_texts,
        width,
        &acc.edit_tool_line_ranges,
        &acc.copy_targets,
        &acc.segments,
    );

    let prepared = Arc::make_mut(&mut prev);
    let new_tail_pending = new_wrapped
        .wrapped_lines
        .iter()
        .any(markdown::line_is_mermaid_pending_placeholder);
    prepared.mermaid_pending_epoch = merge_mermaid_pending(
        prepared.mermaid_pending_epoch,
        new_tail_pending,
        mermaid_epoch_before,
    );
    let prev_len = prepared.wrapped_lines.len();
    let prev_raw_len = prepared.raw_plain_lines.len();
    let prev_prompt_len = prepared.user_prompt_texts.len();
    let edit_index_base = prepared.edit_tool_ranges.len();

    prepared.wrapped_lines.extend(new_wrapped.wrapped_lines);
    Arc::make_mut(&mut prepared.wrapped_plain_lines)
        .extend(new_wrapped.wrapped_plain_lines.iter().cloned());
    Arc::make_mut(&mut prepared.wrapped_copy_offsets)
        .extend(new_wrapped.wrapped_copy_offsets.iter().copied());
    Arc::make_mut(&mut prepared.raw_plain_lines)
        .extend(new_wrapped.raw_plain_lines.iter().cloned());

    {
        let wrapped_line_map = Arc::make_mut(&mut prepared.wrapped_line_map);
        for map in new_wrapped.wrapped_line_map.iter().copied() {
            wrapped_line_map.push(WrappedLineMap {
                raw_line: map.raw_line + prev_raw_len,
                ..map
            });
        }
    }

    prepared.wrapped_user_indices.extend(
        new_wrapped
            .wrapped_user_indices
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared.wrapped_user_prompt_starts.extend(
        new_wrapped
            .wrapped_user_prompt_starts
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared.wrapped_user_prompt_ends.extend(
        new_wrapped
            .wrapped_user_prompt_ends
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared
        .user_prompt_texts
        .extend(new_wrapped.user_prompt_texts);
    prepared
        .image_regions
        .extend(
            new_wrapped
                .image_regions
                .into_iter()
                .map(|region| ImageRegion {
                    abs_line_idx: region.abs_line_idx + prev_len,
                    end_line: region.end_line + prev_len,
                    ..region
                }),
        );
    prepared
        .edit_tool_ranges
        .extend(
            new_wrapped
                .edit_tool_ranges
                .into_iter()
                .map(|r| EditToolRange {
                    edit_index: edit_index_base + r.edit_index,
                    msg_index: r.msg_index,
                    file_path: r.file_path,
                    start_line: r.start_line + prev_len,
                    end_line: r.end_line + prev_len,
                    expandable: r.expandable,
                }),
        );
    prepared.copy_targets.extend(
        new_wrapped
            .copy_targets
            .into_iter()
            .map(|target| CopyTarget {
                start_line: target.start_line + prev_len,
                end_line: target.end_line + prev_len,
                badge_line: target.badge_line + prev_len,
                ..target
            }),
    );
    prepared
        .message_boundaries
        .extend(
            new_wrapped
                .message_boundaries
                .into_iter()
                .map(|b| MessageBoundary {
                    msg_hash: b.msg_hash,
                    wrapped_len: b.wrapped_len + prev_len,
                    raw_len: b.raw_len + prev_raw_len,
                    user_prompt_len: b.user_prompt_len + prev_prompt_len,
                }),
        );

    prev
}

/// Truncate a prepared body so it contains exactly the first `keep_msgs`
/// messages, in place on the (uniquely owned) `prepared`. Used by prefix reuse:
/// when an incoming rebuild shares a hash prefix of length `keep_msgs` with a
/// cached body, the body is truncated here and the changed tail is re-appended
/// via `prepare_body_incremental`. The cut always lands on a message boundary,
/// so every wrapped-indexed entry belongs entirely to either the kept prefix or
/// the dropped tail; filtering by `start < wrapped_len` is exact.
///
/// Requires message-contiguous raws (guaranteed by the shared body renderer),
/// which lets the raw array be truncated too rather than leaked.
pub(super) fn truncate_prepared_to_boundary(prepared: &mut PreparedMessages, keep_msgs: usize) {
    if keep_msgs >= prepared.message_boundaries.len() {
        return;
    }
    let (wrapped_len, raw_len, user_prompt_len) = if keep_msgs == 0 {
        (0, 0, 0)
    } else {
        let b = prepared.message_boundaries[keep_msgs - 1];
        (b.wrapped_len, b.raw_len, b.user_prompt_len)
    };

    prepared.wrapped_lines.truncate(wrapped_len);
    Arc::make_mut(&mut prepared.wrapped_plain_lines).truncate(wrapped_len);
    Arc::make_mut(&mut prepared.wrapped_copy_offsets).truncate(wrapped_len);
    Arc::make_mut(&mut prepared.wrapped_line_map).truncate(wrapped_len);
    Arc::make_mut(&mut prepared.raw_plain_lines).truncate(raw_len);
    prepared.user_prompt_texts.truncate(user_prompt_len);
    prepared.message_boundaries.truncate(keep_msgs);

    prepared.wrapped_user_indices.retain(|&i| i < wrapped_len);
    // The prompt start/end arrays are parallel; drop any prompt that begins at
    // or after the cut. A prompt cannot straddle the cut because the cut is a
    // message boundary and a prompt belongs to exactly one message.
    {
        let starts = &mut prepared.wrapped_user_prompt_starts;
        let ends = &mut prepared.wrapped_user_prompt_ends;
        let keep = starts.iter().take_while(|&&s| s < wrapped_len).count();
        starts.truncate(keep);
        ends.truncate(keep);
    }
    prepared
        .image_regions
        .retain(|r| r.abs_line_idx < wrapped_len);
    prepared
        .edit_tool_ranges
        .retain(|r| r.start_line < wrapped_len);
    prepared.copy_targets.retain(|t| t.start_line < wrapped_len);

    // The pending-mermaid placeholder may have lived in the dropped tail;
    // recompute so a stale-positive stamp cannot force rebuilds forever.
    if prepared.mermaid_pending_epoch.is_some()
        && !prepared
            .wrapped_lines
            .iter()
            .any(markdown::line_is_mermaid_pending_placeholder)
    {
        prepared.mermaid_pending_epoch = None;
    }
}

/// Longest message prefix length `k` such that `base.message_boundaries[..k]`
/// hashes match the first `k` messages of `messages`. Returns `0` when nothing
/// matches (caller then does a full rebuild). Bounded by the shorter of the two
/// lengths.
pub(super) fn matching_prefix_len(base: &PreparedMessages, messages: &[DisplayMessage]) -> usize {
    let limit = base.message_boundaries.len().min(messages.len());
    let mut k = 0;
    while k < limit && base.message_boundaries[k].msg_hash == messages[k].stable_cache_hash() {
        k += 1;
    }
    k
}

/// Longest message suffix length `s` such that the last `s` boundaries of
/// `base` hash-match the last `s` entries of `messages`. Used to detect a
/// prepend (older compacted history loaded above an unchanged tail).
pub(super) fn matching_suffix_len(base: &PreparedMessages, messages: &[DisplayMessage]) -> usize {
    let base_n = base.message_boundaries.len();
    let msg_n = messages.len();
    let limit = base_n.min(msg_n);
    let mut s = 0;
    while s < limit
        && base.message_boundaries[base_n - 1 - s].msg_hash
            == messages[msg_n - 1 - s].stable_cache_hash()
    {
        s += 1;
    }
    s
}

/// Whether a cached body whose message suffix matches the current transcript
/// can be reused verbatim below a freshly rendered head. The reused lines bake
/// in per-prompt state, so this verifies that state is invariant under the
/// prepend:
///
/// - Displayed prompt numbers: the base rendered `local_num + base_offset`; a
///   full rebuild renders `new_local_num + current_offset`. They agree exactly
///   when `dropped_prompts + base_offset == head_prompts + current_offset`
///   (loading compacted history reveals prompts, decreasing the hidden-prompt
///   offset by the same amount, so this holds in the real flow).
/// - Prompt-anchored inline images embed window-relative prompt ordinals that
///   all shift under a prepend, so any `by_prompt` anchors force a rebuild.
///
/// Rainbow prompt-number colors and todo "what changed" deltas may be slightly
/// stale in the reused suffix (they can depend on messages outside it). Both
/// are cosmetic, match the existing staleness of the append/prefix-reuse path,
/// and self-correct on any full rebuild (e.g. a width change).
pub(super) fn suffix_reuse_compatible(
    app: &dyn TuiState,
    messages: &[DisplayMessage],
    head_count: usize,
    base: &PreparedMessages,
    drop_msgs: usize,
    base_prompt_offset: usize,
) -> bool {
    if drop_msgs == 0 || drop_msgs >= base.message_boundaries.len() {
        return false;
    }
    let anchored = super::inline_image_ui::resolve_anchored_items_cached(app);
    if !anchored.by_prompt.is_empty() {
        return false;
    }
    let dropped_prompts = base.message_boundaries[drop_msgs - 1].user_prompt_len;
    let head_prompts = messages[..head_count.min(messages.len())]
        .iter()
        .filter(|msg| msg.effective_role() == "user")
        .count();
    dropped_prompts + base_prompt_offset == head_prompts + app.compacted_hidden_user_prompts()
}

/// Rebuild a body as `freshly rendered head + reused suffix of a cached base`,
/// in place on `prev`. This is the prepend analogue of
/// [`prepare_body_incremental`]: when older compacted history is loaded above
/// an unchanged tail, only the new head (marker + revealed messages) is
/// rendered and wrapped; the prepared suffix is kept and its line/raw indices
/// are shifted. Returns `Err(prev)` untouched when the stitch would not be
/// sound so the caller can fall back to the prefix/full paths.
pub(super) fn prepare_body_prepended(
    app: &dyn TuiState,
    width: u16,
    mut prev: Arc<PreparedMessages>,
    drop_msgs: usize,
    head_count: usize,
) -> Result<Arc<PreparedMessages>, Arc<PreparedMessages>> {
    let messages = app.display_messages();
    if drop_msgs == 0
        || drop_msgs >= prev.message_boundaries.len()
        || head_count == 0
        || head_count > messages.len()
    {
        return Err(prev);
    }

    let (cut_wrapped, cut_raw, cut_prompt) = {
        let b = prev.message_boundaries[drop_msgs - 1];
        (b.wrapped_len, b.raw_len, b.user_prompt_len)
    };
    // The cut must land inside every parallel array, and the suffix's wrapped
    // lines must map to raws at or after the raw cut (message-contiguous raws
    // are guaranteed by the shared body renderer, but a synthetic/legacy base
    // could violate it; bail instead of stitching garbage).
    if cut_wrapped > prev.wrapped_lines.len()
        || cut_wrapped > prev.wrapped_plain_lines.len()
        || cut_wrapped > prev.wrapped_copy_offsets.len()
        || cut_wrapped > prev.wrapped_line_map.len()
        || cut_raw > prev.raw_plain_lines.len()
        || cut_prompt > prev.user_prompt_texts.len()
    {
        return Err(prev);
    }
    if prev.wrapped_line_map[cut_wrapped..]
        .iter()
        .any(|map| map.raw_line < cut_raw)
    {
        return Err(prev);
    }

    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);

    let ctx = BodyRenderCtx {
        app,
        width,
        centered,
        prompt_number_offset: app.compacted_hidden_user_prompts(),
        total_prompts: app.display_user_message_count(),
        pending_count: input_ui::pending_prompt_count(app),
        anchored_images: super::inline_image_ui::resolve_anchored_items_cached(app),
        inline_images_visible: app.inline_images_visible(),
        messages,
        swarm_members: app.swarm_members_for_transcript(),
    };

    // The head sits at the very top of the transcript, so it starts with the
    // same blank-separator state as a full rebuild (no content above it).
    let mut acc = BodyAcc::default();
    for (idx, msg) in messages[..head_count].iter().enumerate() {
        render_message_into(&ctx, &mut acc, msg, idx);
    }

    let head = wrap_lines_with_map(
        acc.lines,
        &acc.raw_plain_lines,
        &acc.line_raw_overrides,
        &acc.line_copy_offsets,
        &acc.user_line_indices,
        &acc.user_prompt_texts,
        width,
        &acc.edit_tool_line_ranges,
        &acc.copy_targets,
        &acc.segments,
    );

    // Blank-separator continuity at the seam: the suffix's first message baked
    // a leading blank iff the dropped prefix rendered content; a full rebuild
    // gives it one iff the head renders content. Both sides are non-empty in
    // the real compacted flow; bail on the degenerate mismatch.
    if (cut_wrapped > 0) == head.wrapped_lines.is_empty() {
        return Err(prev);
    }

    let head_pending_epoch = head.mermaid_pending_epoch;
    let head_wrapped_len = head.wrapped_lines.len();
    let head_raw_len = head.raw_plain_lines.len();
    let head_prompt_len = head.user_prompt_texts.len();
    let msg_index_delta = head_count as isize - drop_msgs as isize;
    let shift_wrapped = |idx: usize| idx - cut_wrapped + head_wrapped_len;

    let prepared = Arc::make_mut(&mut prev);

    prepared
        .wrapped_lines
        .splice(0..cut_wrapped, head.wrapped_lines);
    Arc::make_mut(&mut prepared.wrapped_plain_lines).splice(
        0..cut_wrapped,
        Arc::try_unwrap(head.wrapped_plain_lines).unwrap_or_else(|arc| (*arc).clone()),
    );
    Arc::make_mut(&mut prepared.wrapped_copy_offsets).splice(
        0..cut_wrapped,
        Arc::try_unwrap(head.wrapped_copy_offsets).unwrap_or_else(|arc| (*arc).clone()),
    );
    Arc::make_mut(&mut prepared.raw_plain_lines).splice(
        0..cut_raw,
        Arc::try_unwrap(head.raw_plain_lines).unwrap_or_else(|arc| (*arc).clone()),
    );

    {
        let map = Arc::make_mut(&mut prepared.wrapped_line_map);
        for entry in map[cut_wrapped..].iter_mut() {
            entry.raw_line = entry.raw_line - cut_raw + head_raw_len;
        }
        map.splice(
            0..cut_wrapped,
            Arc::try_unwrap(head.wrapped_line_map).unwrap_or_else(|arc| (*arc).clone()),
        );
    }

    {
        let keep_from = prepared
            .wrapped_user_indices
            .partition_point(|&idx| idx < cut_wrapped);
        let mut indices = head.wrapped_user_indices;
        indices.extend(
            prepared.wrapped_user_indices[keep_from..]
                .iter()
                .map(|&idx| shift_wrapped(idx)),
        );
        prepared.wrapped_user_indices = indices;
    }
    {
        // Starts/ends are parallel; a prompt cannot straddle the cut because the
        // cut is a message boundary.
        let keep_from = prepared
            .wrapped_user_prompt_starts
            .partition_point(|&idx| idx < cut_wrapped);
        let mut starts = head.wrapped_user_prompt_starts;
        starts.extend(
            prepared.wrapped_user_prompt_starts[keep_from..]
                .iter()
                .map(|&idx| shift_wrapped(idx)),
        );
        let mut ends = head.wrapped_user_prompt_ends;
        ends.extend(
            prepared.wrapped_user_prompt_ends[keep_from..]
                .iter()
                .map(|&idx| shift_wrapped(idx)),
        );
        prepared.wrapped_user_prompt_starts = starts;
        prepared.wrapped_user_prompt_ends = ends;
    }

    prepared
        .user_prompt_texts
        .splice(0..cut_prompt, head.user_prompt_texts);

    {
        let mut regions = head.image_regions;
        regions.extend(
            prepared
                .image_regions
                .iter()
                .filter(|region| region.abs_line_idx >= cut_wrapped)
                .map(|region| ImageRegion {
                    abs_line_idx: shift_wrapped(region.abs_line_idx),
                    end_line: shift_wrapped(region.end_line),
                    ..*region
                }),
        );
        prepared.image_regions = regions;
    }

    {
        let mut ranges = head.edit_tool_ranges;
        let head_edit_count = ranges.len();
        ranges.extend(
            prepared
                .edit_tool_ranges
                .iter()
                .filter(|range| range.start_line >= cut_wrapped)
                .enumerate()
                .map(|(seq, range)| EditToolRange {
                    edit_index: head_edit_count + seq,
                    msg_index: (range.msg_index as isize + msg_index_delta).max(0) as usize,
                    file_path: range.file_path.clone(),
                    start_line: shift_wrapped(range.start_line),
                    end_line: shift_wrapped(range.end_line),
                    expandable: range.expandable,
                }),
        );
        prepared.edit_tool_ranges = ranges;
    }

    {
        let mut targets = head.copy_targets;
        targets.extend(
            prepared
                .copy_targets
                .iter()
                .filter(|target| target.start_line >= cut_wrapped)
                .map(|target| CopyTarget {
                    kind: target.kind.clone(),
                    content: target.content.clone(),
                    start_line: shift_wrapped(target.start_line),
                    end_line: shift_wrapped(target.end_line),
                    badge_line: shift_wrapped(target.badge_line),
                }),
        );
        prepared.copy_targets = targets;
    }

    {
        let mut boundaries = head.message_boundaries;
        boundaries.extend(prepared.message_boundaries[drop_msgs..].iter().map(|b| {
            MessageBoundary {
                msg_hash: b.msg_hash,
                wrapped_len: shift_wrapped(b.wrapped_len),
                raw_len: b.raw_len - cut_raw + head_raw_len,
                user_prompt_len: b.user_prompt_len - cut_prompt + head_prompt_len,
            }
        }));
        prepared.message_boundaries = boundaries;
    }

    prepared.mermaid_pending_epoch = match (prepared.mermaid_pending_epoch, head_pending_epoch) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    Ok(prev)
}

fn prepare_streaming_cached(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    let streaming = app.streaming_text();
    if streaming.is_empty() {
        return empty_prepared_messages();
    }

    // Read before rendering: see `stamp_mermaid_pending`.
    let mermaid_epoch_before = crate::tui::mermaid::deferred_render_epoch();
    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let display_width = width.saturating_sub(4) as usize;

    let content_width = if centered {
        display_width.clamp(1, 96)
    } else {
        display_width
    };
    let mut md_lines = app.render_streaming_markdown(content_width);
    if centered {
        markdown::recenter_structured_blocks_for_display(&mut md_lines, display_width);
    }
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if prefix_blank {
        lines.push(Line::from(""));
    }
    for line in md_lines {
        lines.push(align_if_unset(line, align));
    }

    let mut prepared = wrap_lines(lines, &[], &[], &[], width);
    stamp_mermaid_pending(&mut prepared, mermaid_epoch_before);
    prepared
}

pub(super) fn prepare_body(
    app: &dyn TuiState,
    width: u16,
    include_streaming: bool,
) -> PreparedMessages {
    // Read before rendering: a background diagram render completing mid-build
    // must leave the stamp already-stale (see `stamp_mermaid_pending`).
    let mermaid_epoch_before = crate::tui::mermaid::deferred_render_epoch();
    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let display_width = width.saturating_sub(4) as usize;

    let messages = app.display_messages();
    let ctx = BodyRenderCtx {
        app,
        width,
        centered,
        prompt_number_offset: app.compacted_hidden_user_prompts(),
        total_prompts: app.display_user_message_count(),
        pending_count: input_ui::pending_prompt_count(app),
        // Images anchored to transcript messages render inline right after the
        // message that produced them (tool result or user prompt).
        anchored_images: super::inline_image_ui::resolve_anchored_items_cached(app),
        inline_images_visible: app.inline_images_visible(),
        messages,
        swarm_members: app.swarm_members_for_transcript(),
    };

    let mut acc = BodyAcc::default();
    for (msg_idx, msg) in messages.iter().enumerate() {
        render_message_into(&ctx, &mut acc, msg, msg_idx);
    }

    // Streaming output is appended after the message boundaries are recorded so
    // it is never treated as a reusable message segment (its content changes on
    // every token). `include_streaming` is false on the cached body path; this
    // branch only fires for the rare direct streaming preview.
    if include_streaming && app.is_processing() && !app.streaming_text().is_empty() {
        if !acc.lines.is_empty() {
            acc.lines.push(Line::from(""));
            acc.line_raw_overrides.push(None);
            acc.line_copy_offsets.push(0);
        }
        let content_width = if centered {
            display_width.clamp(1, 96)
        } else {
            display_width
        };
        let mut md_lines = app.render_streaming_markdown(content_width);
        if centered {
            markdown::recenter_structured_blocks_for_display(&mut md_lines, display_width);
        }
        let align = default_message_alignment("assistant", centered);
        for line in md_lines {
            acc.lines.push(align_if_unset(line, align));
            acc.line_raw_overrides.push(None);
            acc.line_copy_offsets.push(0);
        }
    }

    let mut prepared = wrap_lines_with_map(
        acc.lines,
        &acc.raw_plain_lines,
        &acc.line_raw_overrides,
        &acc.line_copy_offsets,
        &acc.user_line_indices,
        &acc.user_prompt_texts,
        width,
        &acc.edit_tool_line_ranges,
        &acc.copy_targets,
        &acc.segments,
    );
    stamp_mermaid_pending(&mut prepared, mermaid_epoch_before);
    prepared
}

fn wrap_lines(
    lines: Vec<Line<'static>>,
    line_copy_offsets: &[usize],
    user_line_indices: &[usize],
    user_prompt_texts: &[String],
    width: u16,
) -> PreparedMessages {
    let full_width = width.saturating_sub(1) as usize;
    let user_width = width.saturating_sub(2) as usize;
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_starts: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_ends: Vec<usize> = Vec::new();
    let mut raw_plain_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut wrapped_line_map: Vec<WrappedLineMap> = Vec::new();
    let mut wrapped_copy_offsets: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let raw_text = ui::line_plain_text(&line);
        let raw_width = unicode_width::UnicodeWidthStr::width(raw_text.as_str());
        raw_plain_lines.push(raw_text);
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();
        let mut remaining_copy_offset = line_copy_offsets.get(orig_idx).copied().unwrap_or(0);
        let mut start_col = 0usize;

        for wrapped_line in &new_lines {
            let width = wrapped_line.width();
            let end_col = (start_col + width).min(raw_width);
            wrapped_line_map.push(WrappedLineMap {
                raw_line: orig_idx,
                start_col,
                end_col,
            });
            wrapped_copy_offsets.push(remaining_copy_offset.min(width));
            remaining_copy_offset = remaining_copy_offset.saturating_sub(width);
            start_col = end_col;
        }

        if is_user_line {
            wrapped_user_prompt_starts.push(wrapped_idx);
            wrapped_user_prompt_ends.push(wrapped_idx + count);
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }

    let image_regions = compute_image_regions(&wrapped_lines);

    let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());

    PreparedMessages {
        wrapped_lines,
        wrapped_plain_lines,
        wrapped_copy_offsets: Arc::new(wrapped_copy_offsets),
        raw_plain_lines: Arc::new(raw_plain_lines),
        wrapped_line_map: Arc::new(wrapped_line_map),
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Wrapped-line preparation carries explicit render state to avoid hidden coupling"
)]
fn wrap_lines_with_map(
    lines: Vec<Line<'static>>,
    seeded_raw_plain_lines: &[String],
    line_raw_overrides: &[Option<WrappedLineMap>],
    line_copy_offsets: &[usize],
    user_line_indices: &[usize],
    user_prompt_texts: &[String],
    width: u16,
    edit_ranges: &[(usize, String, usize, usize, bool)],
    copy_ranges: &[RawCopyTarget],
    segments: &[(u64, usize, usize, usize)],
) -> PreparedMessages {
    let full_width = width.saturating_sub(1) as usize;
    let user_width = width.saturating_sub(2) as usize;
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_starts: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_ends: Vec<usize> = Vec::new();
    let mut raw_plain_lines: Vec<String> = seeded_raw_plain_lines.to_vec();
    let mut wrapped_line_map: Vec<WrappedLineMap> = Vec::new();
    let mut wrapped_copy_offsets: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut raw_to_wrapped: Vec<usize> = Vec::with_capacity(lines.len() + 1);

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let (raw_line, start_col, end_col) =
            if let Some(Some(map)) = line_raw_overrides.get(orig_idx) {
                (map.raw_line, map.start_col, map.end_col)
            } else {
                let raw_text = ui::line_plain_text(&line);
                let raw_width = unicode_width::UnicodeWidthStr::width(raw_text.as_str());
                let raw_line = raw_plain_lines.len();
                raw_plain_lines.push(raw_text);
                (raw_line, 0usize, raw_width)
            };
        raw_to_wrapped.push(wrapped_idx);
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();
        let mut remaining_copy_offset = line_copy_offsets.get(orig_idx).copied().unwrap_or(0);
        let mut segment_start = start_col;

        for wrapped_line in &new_lines {
            let width = wrapped_line.width();
            let segment_end = (segment_start + width).min(end_col);
            wrapped_line_map.push(WrappedLineMap {
                raw_line,
                start_col: segment_start,
                end_col: segment_end,
            });
            wrapped_copy_offsets.push(remaining_copy_offset.min(width));
            remaining_copy_offset = remaining_copy_offset.saturating_sub(width);
            segment_start = segment_end;
        }

        if is_user_line {
            wrapped_user_prompt_starts.push(wrapped_idx);
            wrapped_user_prompt_ends.push(wrapped_idx + count);
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }
    raw_to_wrapped.push(wrapped_idx);

    let image_regions = compute_image_regions(&wrapped_lines);

    let mut edit_tool_ranges = Vec::new();
    for (msg_idx, file_path, raw_start, raw_end, expandable) in edit_ranges {
        let start_line = raw_to_wrapped.get(*raw_start).copied().unwrap_or(0);
        let end_line = raw_to_wrapped
            .get(*raw_end)
            .copied()
            .unwrap_or(wrapped_lines.len());
        edit_tool_ranges.push(EditToolRange {
            edit_index: edit_tool_ranges.len(),
            msg_index: *msg_idx,
            file_path: file_path.clone(),
            start_line,
            end_line,
            expandable: *expandable,
        });
    }

    let mut copy_targets = Vec::new();
    for target in copy_ranges {
        let start_line = raw_to_wrapped
            .get(target.start_raw_line)
            .copied()
            .unwrap_or(0);
        let end_line = raw_to_wrapped
            .get(target.end_raw_line)
            .copied()
            .unwrap_or(wrapped_lines.len());
        let badge_line = raw_to_wrapped
            .get(target.badge_raw_line)
            .copied()
            .unwrap_or(start_line);
        copy_targets.push(CopyTarget {
            kind: target.kind.clone(),
            content: target.content.clone(),
            start_line,
            end_line,
            badge_line,
        });
    }

    let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());

    // Translate the per-message unwrapped-line boundaries into wrapped-line
    // boundaries. `raw_to_wrapped[i]` is the first wrapped index produced by
    // unwrapped line `i` (with a final sentinel at `lines.len()`), so a segment
    // that ends just before unwrapped line `L` ends at wrapped line
    // `raw_to_wrapped[L]`. This lets a later rebuild truncate the wrapped body at
    // any message boundary and reuse the unchanged prefix.
    let message_boundaries: Vec<MessageBoundary> = segments
        .iter()
        .map(
            |&(msg_hash, lines_len_after, raw_len_after, user_prompt_len_after)| MessageBoundary {
                msg_hash,
                wrapped_len: raw_to_wrapped
                    .get(lines_len_after)
                    .copied()
                    .unwrap_or(wrapped_lines.len()),
                raw_len: raw_len_after,
                user_prompt_len: user_prompt_len_after,
            },
        )
        .collect();

    PreparedMessages {
        wrapped_lines,
        wrapped_plain_lines,
        wrapped_copy_offsets: Arc::new(wrapped_copy_offsets),
        raw_plain_lines: Arc::new(raw_plain_lines),
        wrapped_line_map: Arc::new(wrapped_line_map),
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges,
        copy_targets,
        message_boundaries,
        mermaid_pending_epoch: None,
    }
}

#[cfg(test)]
#[path = "ui_prepare/tests.rs"]
mod tests;
