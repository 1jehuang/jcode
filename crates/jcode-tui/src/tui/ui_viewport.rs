use super::*;
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

#[cfg(target_os = "macos")]
pub(crate) const COPY_BADGE_ALT_LABEL: &str = "⌥";
#[cfg(not(target_os = "macos"))]
pub(crate) const COPY_BADGE_ALT_LABEL: &str = "Alt";

pub(crate) fn copy_badge_alt_label() -> String {
    let config = crate::config::config();
    copy_badge_alt_label_from_config(&config.display.copy_badge_alt_label)
}

/// Build one Ratatui cell symbol that starts at the bottom-right of a reserved
/// math region, moves to its top-left, invokes Handterm's native LaTeX APC, and
/// restores the cursor where Ratatui expects it after printing one cell.
fn handterm_native_latex_cell_symbol(source: &str, rows: u16, cols: u16) -> Option<String> {
    let apc = crate::tui::markdown::encode_handterm_latex_apc(source)?;
    let mut symbol = String::with_capacity(apc.len() + 32);
    symbol.push_str("\x1b[s");
    if rows > 1 {
        write!(symbol, "\x1b[{}A", rows - 1).ok()?;
    }
    if cols > 1 {
        write!(symbol, "\x1b[{}D", cols - 1).ok()?;
    }
    symbol.push_str(&apc);
    // Crossterm's backend records this as one printed cell. Restore the actual
    // terminal cursor and advance once so its state matches that accounting.
    symbol.push_str("\x1b[u\x1b[1C");
    Some(symbol)
}

fn copy_badge_alt_label_from_config(configured: &str) -> String {
    let configured = configured.trim();
    if configured.is_empty() {
        COPY_BADGE_ALT_LABEL.to_string()
    } else {
        configured.to_string()
    }
}

pub(crate) fn copy_badge_alt_badge() -> String {
    format!("[{}]", copy_badge_alt_label())
}

fn copy_badge_shortcut_width(key_label: &str) -> usize {
    // Includes the single separator space rendered between the row content and
    // the first badge.
    UnicodeWidthStr::width(format!(" {} [⇧] [{key_label}]", copy_badge_alt_badge()).as_str())
}

/// Display width reserved for the inline expand-edit badge (`[Alt] [⇧] [E] …`).
/// The badge text differs between the collapsed (` expand`) and just-activated
/// (` ✓ Expanded`) states, so callers pass the suffix that will actually render.
pub(crate) fn expand_badge_reserved_width(badge_text: &str) -> usize {
    UnicodeWidthStr::width(format!(" {} [⇧] [E]{badge_text}", copy_badge_alt_badge()).as_str())
}

fn lower_bound(values: &[usize], target: usize) -> usize {
    values.partition_point(|&v| v < target)
}

/// Appended-row jump size (in rows) above which the tail-follow viewport slides
/// to the bottom over several frames instead of snapping. Small appends
/// (paced streaming) snap as before; only block insertions animate.
const TAIL_CATCHUP_MIN_JUMP: usize = 4;

/// Maximum rows the tail-follow viewport advances per rendered frame while
/// catching up. At animation cadence (~30-60fps) even a full-screen insertion
/// settles within a few hundred milliseconds.
const TAIL_CATCHUP_MAX_STEP: usize = 3;

/// Resolve the scroll position while following the live tail. Normally this is
/// just `max_scroll`, but when a large block lands at once (committed message,
/// tool result) the bottom-anchored viewport would teleport everything upward
/// in a single frame - the "big pop". Instead, advance from the previous
/// frame's resolved position by a bounded step so the new content slides into
/// view. Disabled on tiers without decorative animations.
fn resolve_tail_follow_scroll(max_scroll: usize, viewport_height: usize) -> usize {
    if super::take_tail_follow_snap_request() {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    if !crate::perf::tui_policy().enable_decorative_animations {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let prev = super::last_resolved_chat_scroll();
    // Only animate forward catch-up from an established position. Backward
    // motion (content shrank) and first frames snap directly.
    if prev == 0 || max_scroll <= prev {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let jump = max_scroll - prev;
    // Never let the live tail drift more than a viewport behind (e.g. a huge
    // paste); cap the lag so the catch-up is at most one screen.
    let max_lag = viewport_height.max(TAIL_CATCHUP_MIN_JUMP);
    if jump <= TAIL_CATCHUP_MIN_JUMP {
        super::set_tail_catchup_active(false);
        return max_scroll;
    }
    let floor = max_scroll.saturating_sub(max_lag);
    let next = prev.max(floor).saturating_add(TAIL_CATCHUP_MAX_STEP);
    if next >= max_scroll {
        super::set_tail_catchup_active(false);
        max_scroll
    } else {
        super::set_tail_catchup_active(true);
        next
    }
}

use super::selection_highlight::highlight_line_selection;

pub(crate) fn truncate_line_in_place_to_width(line: &mut Line<'static>, max_width: usize) {
    let mut remaining = max_width;
    let mut kept: Vec<Span<'static>> = Vec::new();

    for span in line.spans.drain(..) {
        if remaining == 0 {
            break;
        }

        let span_width = span.content.as_ref().width();
        if span_width <= remaining {
            remaining = remaining.saturating_sub(span_width);
            kept.push(span);
            continue;
        }

        let mut text = String::new();
        let mut used = 0usize;
        for ch in span.content.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if ch_width > 0 && used.saturating_add(ch_width) > remaining {
                break;
            }
            text.push(ch);
            used = used.saturating_add(ch_width);
        }
        if !text.is_empty() {
            kept.push(Span::styled(text, span.style));
        }
        break;
    }

    line.spans = kept;
}

/// Remove trailing plain spaces from a line so an appended badge sits exactly
/// one separator space after the content, regardless of how the source line
/// was rendered (code block headers end with a space, blockquote lines don't).
pub(crate) fn trim_line_trailing_spaces(line: &mut Line<'static>) {
    while let Some(last) = line.spans.last_mut() {
        let trimmed = last.content.trim_end_matches(' ');
        if trimmed.len() == last.content.len() {
            break;
        }
        if trimmed.is_empty() {
            line.spans.pop();
        } else {
            last.content = std::borrow::Cow::Owned(trimmed.to_string());
            break;
        }
    }
}

/// Prepare a line to host an inline badge within `max_content_width` cells.
/// When the content already fits, only trailing spaces are trimmed (the badge
/// separator space is accounted for in the reserved width). When the content
/// must be cut, end it with a dim ellipsis so the truncation is visible
/// instead of silently swallowing words.
pub(crate) fn truncate_line_for_copy_badge(line: &mut Line<'static>, max_content_width: usize) {
    if line.width() <= max_content_width {
        trim_line_trailing_spaces(line);
        return;
    }
    truncate_line_in_place_to_width(line, max_content_width.saturating_sub(1));
    trim_line_trailing_spaces(line);
    line.spans
        .push(Span::styled("…", Style::default().fg(dim_color())));
}

/// Choose the wrapped line that hosts an inline copy badge for a copy target
/// spanning `block_start..block_end`.
///
/// Prefers the target's natural badge line (the `┌─ lang` header for code
/// blocks), but when that line is too wide to fit the badge without cutting
/// content (e.g. a full-width blockquote line), falls back to the first
/// visible line of the block with enough free width. Returns the natural line
/// when nothing fits; the caller then truncates with a visible ellipsis.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_copy_badge_line(
    preferred: usize,
    block_start: usize,
    block_end: usize,
    scroll: usize,
    visible_end: usize,
    visible_lines: &[Line<'_>],
    content_width: usize,
    reserved: usize,
) -> usize {
    let fits = |abs_line: usize| -> bool {
        if abs_line < scroll || abs_line >= visible_end {
            return false;
        }
        visible_lines
            .get(abs_line - scroll)
            .is_some_and(|line| line.width().saturating_add(reserved) <= content_width)
    };
    if fits(preferred) {
        return preferred;
    }
    (block_start.max(scroll)..block_end.min(visible_end))
        .find(|&abs_line| abs_line != preferred && fits(abs_line))
        .unwrap_or(preferred)
}

pub(crate) fn copy_badge_reserved_width(
    key: char,
    copy_badge_ui: &crate::tui::app::CopyBadgeUiState,
    now: std::time::Instant,
) -> usize {
    let mut reserved = copy_badge_shortcut_width("A");
    if copy_badge_ui.feedback_for_key(key, now).is_some() {
        // Feedback text plus the spacer between it and the shortcut badges.
        // The separator before the feedback is already counted in
        // `copy_badge_shortcut_width`.
        reserved = reserved.saturating_add(UnicodeWidthStr::width("✓ Copied! "));
    }
    reserved
}

pub(super) fn compute_visible_margins(
    lines: &[Line],
    visible_user_indices: &[usize],
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let mut visible_user_cursor = 0usize;

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        while visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] < row
        {
            visible_user_cursor += 1;
        }
        let is_user_line = visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] == row;

        if row < lines.len() {
            let mut used = lines[row].width().min(area.width as usize) as u16;
            if is_user_line && area.width > 0 {
                used = used.saturating_add(1).min(area.width);
            }

            // Compute the true free space on each side from the line's *rendered*
            // alignment. This matters even in left-aligned mode: the header lines
            // (`server:`/`client:`/model/version, auth status, mcp list, changelog
            // box, etc.) are always centered regardless of mode, so a centered line
            // of width `used` leaves only ~half the slack on the right. Reporting
            // the full `area.width - used` here would let a right-side info widget
            // overlap the centered header text.
            let total_margin = area.width.saturating_sub(used);
            let default_alignment = if centered {
                Alignment::Center
            } else {
                Alignment::Left
            };
            let effective_alignment = lines[row].alignment.unwrap_or(default_alignment);
            let (left_margin, right_margin) = match effective_alignment {
                Alignment::Left => (0, total_margin),
                Alignment::Center => {
                    let left = total_margin / 2;
                    let right = total_margin.saturating_sub(left);
                    (left, right)
                }
                Alignment::Right => (total_margin, 0),
            };

            if centered {
                left_widths.push(left_margin);
                right_widths.push(right_margin);
            } else {
                // Left-aligned mode never places left-side widgets (content is
                // flush-left), so the left gap is reported as 0; the right gap
                // still respects per-line alignment so widgets clear the header.
                left_widths.push(0);
                right_widths.push(right_margin);
            }
        } else if centered {
            let half = area.width / 2;
            left_widths.push(half);
            right_widths.push(area.width.saturating_sub(half));
        } else {
            left_widths.push(0);
            right_widths.push(area.width);
        }
    }

    info_widget::Margins {
        right_widths,
        left_widths,
        centered,
        ..Default::default()
    }
}

pub(crate) fn reserve_copy_badge_margins(
    margins: &mut info_widget::Margins,
    scroll: usize,
    visible_end: usize,
    badge_assignments: &[(usize, char)],
    copy_badge_ui: &crate::tui::app::CopyBadgeUiState,
    now: std::time::Instant,
) {
    for &(badge_line, key) in badge_assignments {
        if badge_line < scroll || badge_line >= visible_end {
            continue;
        }

        let rel_idx = badge_line - scroll;
        if rel_idx >= margins.right_widths.len() {
            continue;
        }

        let reserved = copy_badge_reserved_width(key, copy_badge_ui, now) as u16;
        margins.right_widths[rel_idx] = margins.right_widths[rel_idx].saturating_sub(reserved);
    }
}

pub(super) fn draw_messages(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    prepared: Arc<PreparedChatFrame>,
    show_native_scrollbar: bool,
) -> info_widget::Margins {
    let (render_area, scrollbar_area) =
        super::split_native_scrollbar_area(area, show_native_scrollbar);
    let left_inset = super::left_aligned_content_inset(render_area.width, app.centered_mode());
    let text_render_area = Rect {
        x: render_area.x.saturating_add(left_inset),
        y: render_area.y,
        width: render_area.width.saturating_sub(left_inset),
        height: render_area.height,
    };
    let wrapped_user_indices = &prepared.wrapped_user_indices;
    let wrapped_user_prompt_starts = &prepared.wrapped_user_prompt_starts;
    let wrapped_user_prompt_ends = &prepared.wrapped_user_prompt_ends;
    let user_prompt_texts = &prepared.user_prompt_texts;

    let total_lines = prepared.total_wrapped_lines();
    let viewport_height = render_area.height as usize;
    let max_scroll = compute_max_scroll_with_prompt_preview(
        total_lines,
        wrapped_user_prompt_starts,
        user_prompt_texts,
        text_render_area,
    );

    super::set_last_max_scroll(max_scroll);
    update_user_prompt_positions(wrapped_user_prompt_starts);

    // When older compacted history is being loaded in, the app hands us the
    // reader's distance-from-bottom instead of an absolute offset. Distance from
    // the bottom is invariant under a top-side prepend, so resolving it against
    // the *current* total keeps the same content under the reader and the load
    // is seamless (no jump to the new absolute top).
    let anchored_scroll = app
        .pending_history_anchor_lines_from_bottom()
        .map(|lines_from_bottom| {
            total_lines
                .saturating_sub(lines_from_bottom)
                .min(max_scroll)
        });
    let user_scroll = app.scroll_offset().min(max_scroll);
    let scroll = if let Some(anchored) = anchored_scroll {
        super::set_tail_catchup_active(false);
        anchored
    } else if app.auto_scroll_paused() {
        super::set_tail_catchup_active(false);
        user_scroll.min(max_scroll)
    } else {
        resolve_tail_follow_scroll(max_scroll, viewport_height)
    };

    // Publish the resolved geometry so scroll handlers and the anchor-reconcile
    // tick can adopt the exact on-screen position after a prepend.
    super::set_last_total_wrapped_lines(total_lines);
    super::set_last_resolved_chat_scroll(scroll);

    let prompt_preview_lines = if crate::config::config().display.prompt_preview && scroll > 0 {
        compute_prompt_preview_line_count(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            scroll,
            text_render_area.width,
        )
    } else {
        0u16
    };

    let content_area = Rect {
        x: text_render_area.x,
        y: render_area.y.saturating_add(prompt_preview_lines),
        width: text_render_area.width,
        height: render_area.height.saturating_sub(prompt_preview_lines),
    };
    let visible_height = content_area.height as usize;
    let copy_badge_ui = app.copy_badge_ui();
    let copy_badge_now = std::time::Instant::now();
    let expand_feedback_active = copy_badge_ui.expand_feedback_is_active(copy_badge_now);

    let active_file_context = if app.diff_mode().is_file() {
        active_file_diff_context(prepared.as_ref(), scroll, visible_height)
    } else {
        None
    };
    let active_inline_edit_context = if app.diff_mode().is_inline() || expand_feedback_active {
        active_file_diff_context(prepared.as_ref(), scroll, visible_height)
    } else {
        None
    };

    let visible_end = (scroll + visible_height).min(total_lines);
    let visible_user_start = lower_bound(wrapped_user_indices, scroll);
    let visible_user_end = lower_bound(wrapped_user_indices, visible_end);
    let visible_user_indices: Vec<usize> = wrapped_user_indices
        [visible_user_start..visible_user_end]
        .iter()
        .map(|idx| idx.saturating_sub(scroll))
        .collect();

    let mut visible_lines = prepared.materialize_line_slice(scroll, visible_end);
    let stability_hash = super::viewport_stability_hash(
        &visible_lines,
        &visible_user_indices,
        content_area.width,
        prompt_preview_lines,
    );
    let visible_streaming_hash =
        if prepared.visible_intersects_section(PreparedSectionKind::Streaming, scroll, visible_end)
        {
            super::hash_text_for_cache(app.streaming_text())
        } else {
            0
        };
    let visible_batch_progress_hash = if prepared.visible_intersects_section(
        PreparedSectionKind::BatchProgress,
        scroll,
        visible_end,
    ) {
        super::prepare::active_batch_progress_hash(app)
    } else {
        0
    };
    let content_margins = compute_visible_margins(
        &visible_lines,
        &visible_user_indices,
        content_area,
        app.centered_mode(),
    );
    let mut margins = info_widget::Margins {
        right_widths: vec![0; prompt_preview_lines as usize],
        left_widths: vec![0; prompt_preview_lines as usize],
        centered: content_margins.centered,
        // Bind row `r` of the margin to transcript line `scroll_top + r` so a
        // content-anchored info widget rides the transcript while the user scrolls
        // instead of churning against a fixed screen row. The prompt-preview band at
        // the top is synthetic (not part of the scrolled transcript), so offset by it
        // to keep the content rows aligned. While pinned at the bottom (auto-follow),
        // widgets stay screen-anchored as before.
        scroll_top: scroll.saturating_sub(prompt_preview_lines as usize),
        content_anchored: app.auto_scroll_paused(),
        ..Default::default()
    };
    margins
        .right_widths
        .extend(content_margins.right_widths.clone());
    margins
        .left_widths
        .extend(content_margins.left_widths.clone());
    while margins.right_widths.len() < viewport_height {
        margins.right_widths.push(0);
    }
    while margins.left_widths.len() < viewport_height {
        margins.left_widths.push(0);
    }

    // Image placeholders are blank lines, so the margin scan above sees their
    // rows as fully free and would happily dock an info widget on top of the
    // picture. Carve every visible image region (and its label line) out of
    // the free-width profile. A region with unknown width (mermaid crop = 0)
    // occupies the full row.
    {
        let img_start = prepared
            .image_regions
            .partition_point(|region| region.end_line <= scroll);
        let img_end = prepared
            .image_regions
            .partition_point(|region| region.abs_line_idx < visible_end);
        for region in &prepared.image_regions[img_start..img_end] {
            let occupied = if region.width == 0 {
                content_area.width
            } else {
                region.width.min(content_area.width)
            };
            let leftover = content_area.width.saturating_sub(occupied);
            // Non-centered mode draws the image flush left, leaving all the
            // slack on the right; centered mode splits it across both sides.
            let free_right = if margins.centered {
                leftover / 2
            } else {
                leftover
            };
            // Include the label line directly above the region so a widget
            // can't sit flush against the image top either.
            let row_first = region.abs_line_idx.saturating_sub(1).max(scroll);
            let row_last = region.end_line.min(visible_end);
            for abs_line in row_first..row_last {
                let row = prompt_preview_lines as usize + (abs_line - scroll);
                if let Some(width) = margins.right_widths.get_mut(row) {
                    *width = (*width).min(free_right);
                }
                if margins.centered
                    && let Some(width) = margins.left_widths.get_mut(row)
                {
                    // Centered images leave half the slack on each side.
                    *width = (*width).min(free_right);
                }
            }
        }
    }

    record_copy_viewport_frame_snapshot(
        prepared.clone(),
        scroll,
        visible_end,
        content_area,
        &content_margins.left_widths,
    );

    let mut visible_copy_targets: Vec<VisibleCopyTarget> = Vec::new();
    let mut badge_assignments: Vec<(usize, char)> = Vec::new();
    let first_visible_copy_target = prepared
        .copy_targets
        .partition_point(|target| target.end_line <= scroll);
    for (slot, target) in prepared.copy_targets[first_visible_copy_target..]
        .iter()
        .take_while(|target| target.start_line < visible_end)
        .take(COPY_BADGE_KEYS.len())
        .enumerate()
    {
        let key = COPY_BADGE_KEYS[slot];
        visible_copy_targets.push(VisibleCopyTarget {
            key,
            kind_label: target.kind.label(),
            copied_notice: target.kind.copied_notice(),
            content: target.content.clone(),
        });
        // Prefer a line in the block with enough free width so the badge
        // doesn't cut off content (full-width blockquote lines especially).
        let badge_line = pick_copy_badge_line(
            target.badge_line,
            target.start_line,
            target.end_line,
            scroll,
            visible_end,
            &visible_lines,
            content_area.width as usize,
            copy_badge_reserved_width(key, &copy_badge_ui, copy_badge_now),
        );
        badge_assignments.push((badge_line, key));
    }
    reserve_copy_badge_margins(
        &mut margins,
        scroll,
        visible_end,
        &badge_assignments,
        &copy_badge_ui,
        copy_badge_now,
    );
    set_visible_copy_targets(visible_copy_targets);
    super::note_viewport_metrics(super::ViewportMetrics {
        scroll,
        visible_end,
        visible_lines: visible_lines.len(),
        total_wrapped_lines: total_lines,
        prompt_preview_lines,
        visible_user_prompts: visible_user_indices.len(),
        visible_copy_targets: badge_assignments.len(),
        content_width: content_area.width,
        stability_hash,
        visible_streaming_hash,
        visible_batch_progress_hash,
    });

    let now_ms = app.now_millis();
    let policy = crate::perf::tui_policy();
    let prompt_anim_enabled = crate::config::config().display.prompt_entry_animation
        && policy.enable_decorative_animations
        && policy.tier.prompt_entry_animation_enabled();
    if prompt_anim_enabled {
        update_prompt_entry_animation(wrapped_user_prompt_starts, scroll, visible_end, now_ms);
    } else {
        record_prompt_viewport(scroll, visible_end);
    }

    let active_prompt_anim = if prompt_anim_enabled {
        active_prompt_entry_animation(now_ms)
    } else {
        None
    };

    if visible_lines.len() < visible_height {
        visible_lines.extend(std::iter::repeat_n(
            Line::from(""),
            visible_height - visible_lines.len(),
        ));
    }

    clear_area(frame, area);

    if let Some(anim) = active_prompt_anim {
        let t = (now_ms.saturating_sub(anim.start_ms) as f32 / PROMPT_ENTRY_ANIMATION_MS as f32)
            .clamp(0.0, 1.0);

        let prompt_idx = lower_bound(wrapped_user_prompt_starts, anim.line_idx);
        if prompt_idx < wrapped_user_prompt_starts.len()
            && wrapped_user_prompt_starts[prompt_idx] == anim.line_idx
        {
            let prompt_end = wrapped_user_prompt_ends
                .get(prompt_idx)
                .copied()
                .unwrap_or(anim.line_idx + 1);

            for abs_idx in anim.line_idx.max(scroll)..prompt_end.min(visible_end) {
                let rel_idx = abs_idx - scroll;
                if let Some(line) = visible_lines.get_mut(rel_idx) {
                    let line_width = line.width().max(1) as f32;
                    let mut consumed = 0usize;
                    for span in &mut line.spans {
                        if !span.content.is_empty() {
                            let base_fg = match span.style.fg {
                                Some(c) => c,
                                None => user_text(),
                            };
                            let base_bg = span.style.bg.unwrap_or(user_bg());
                            let span_width = span.content.as_ref().width();
                            let span_center = if span_width == 0 {
                                consumed as f32 / line_width
                            } else {
                                (consumed as f32 + span_width as f32 * 0.5) / line_width
                            }
                            .clamp(0.0, 1.0);

                            let pulsed_fg = prompt_entry_color(base_fg, t);
                            let shimmer_fg = prompt_entry_shimmer_color(pulsed_fg, span_center, t);
                            let spotlight_bg = prompt_entry_bg_color(base_bg, t);

                            span.style = span.style.fg(shimmer_fg).bg(spotlight_bg);
                            consumed += span_width;
                        }
                    }
                }
            }
        }
    }

    if let Some(active) = &active_file_context {
        let highlight_style = Style::default().fg(file_link_color()).bold();
        let accent_style = Style::default().fg(file_link_color());

        for abs_idx in active.start_line.max(scroll)..active.end_line.min(visible_end) {
            let rel_idx = abs_idx.saturating_sub(scroll);
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                if abs_idx == active.start_line {
                    line.spans.insert(
                        0,
                        Span::styled(format!("→ edit#{} ", active.edit_index), highlight_style),
                    );
                } else {
                    line.spans.insert(0, Span::styled("  │ ", accent_style));
                }
            }
        }
    }

    let expand_edit_badge_visible = active_inline_edit_context.as_ref().is_some_and(|active| {
        active.expandable
            && (!app.diff_mode().is_full_inline() || expand_feedback_active)
            && active.start_line >= scroll
            && active.start_line < visible_end
    });
    let visible_expand_badge_line = expand_edit_badge_visible
        .then(|| {
            active_inline_edit_context
                .as_ref()
                .map(|active| active.start_line)
        })
        .flatten();
    super::set_visible_expand_edit_badge(expand_edit_badge_visible, visible_expand_badge_line);

    let expand_badge_line = if expand_feedback_active {
        copy_badge_ui.expand_feedback_line.or_else(|| {
            active_inline_edit_context
                .as_ref()
                .map(|active| active.start_line)
        })
    } else {
        active_inline_edit_context
            .as_ref()
            .filter(|active| active.expandable && !app.diff_mode().is_full_inline())
            .map(|active| active.start_line)
    };

    if let Some(mut badge_line) = expand_badge_line {
        if expand_feedback_active && visible_end > scroll {
            badge_line = badge_line.clamp(scroll, visible_end.saturating_sub(1));
        }
        if badge_line >= scroll && badge_line < visible_end {
            let rel_idx = badge_line - scroll;
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                let badge_text = if expand_feedback_active {
                    " ✓ Expanded"
                } else {
                    " expand"
                };
                let reserved = expand_badge_reserved_width(badge_text);
                let max_content_width = (content_area.width as usize).saturating_sub(reserved);
                truncate_line_for_copy_badge(line, max_content_width);

                // Reserve the badge's width in the info-widget margin profile so a
                // floating widget (e.g. the KV cache panel) docks far enough left to
                // clear the appended `[Alt] [⇧] [E] expand` block instead of being
                // squeezed into a too-narrow slot that wraps/collides with it. The
                // margin row for `visible_lines[rel_idx]` is offset by the synthetic
                // prompt-preview band, matching the image-region carve above.
                let margin_row = prompt_preview_lines as usize + rel_idx;
                if let Some(width) = margins.right_widths.get_mut(margin_row) {
                    *width = (*width).saturating_sub(reserved as u16);
                }

                let alt_style = if copy_badge_ui.alt_is_active(copy_badge_now) {
                    Style::default().fg(queued_color()).bold()
                } else {
                    Style::default().fg(dim_color())
                };
                let shift_style = if copy_badge_ui.shift_is_active(copy_badge_now) {
                    Style::default().fg(queued_color()).bold()
                } else {
                    Style::default().fg(dim_color())
                };
                let key_style = if copy_badge_ui.key_is_active('e', copy_badge_now) {
                    Style::default().fg(accent_color()).bold()
                } else {
                    Style::default().fg(dim_color())
                };

                line.spans.push(Span::raw(" "));
                line.spans
                    .push(Span::styled(copy_badge_alt_badge(), alt_style));
                line.spans.push(Span::raw(" "));
                line.spans.push(Span::styled("[⇧]", shift_style));
                line.spans.push(Span::raw(" "));
                line.spans.push(Span::styled("[E]", key_style));
                let badge_text_style = if expand_feedback_active {
                    Style::default().fg(ai_color()).bold()
                } else {
                    Style::default().fg(dim_color())
                };
                line.spans.push(Span::styled(badge_text, badge_text_style));
            }
        }
    }

    for (badge_line, key) in badge_assignments {
        if badge_line < scroll || badge_line >= visible_end {
            continue;
        }
        let rel_idx = badge_line - scroll;
        if let Some(line) = visible_lines.get_mut(rel_idx) {
            let reserved = copy_badge_reserved_width(key, &copy_badge_ui, copy_badge_now);
            let max_content_width = (content_area.width as usize).saturating_sub(reserved);
            // Trims trailing spaces so the badge sits exactly one separator
            // space after the content; ends with a dim ellipsis when the
            // content genuinely has to be cut.
            truncate_line_for_copy_badge(line, max_content_width);
            line.spans.push(Span::raw(" "));

            let alt_style = if copy_badge_ui.alt_is_active(copy_badge_now) {
                Style::default().fg(queued_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };
            let shift_style = if copy_badge_ui.shift_is_active(copy_badge_now) {
                Style::default().fg(queued_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };
            let key_style = if copy_badge_ui.key_is_active(key, copy_badge_now) {
                Style::default().fg(accent_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };

            if let Some(success) = copy_badge_ui.feedback_for_key(key, copy_badge_now) {
                let feedback_style = if success {
                    Style::default().fg(ai_color()).bold()
                } else {
                    Style::default().fg(Color::Red).bold()
                };
                let feedback_text = if success {
                    "✓ Copied!"
                } else {
                    "✗ Copy failed"
                };
                line.spans.push(Span::styled(feedback_text, feedback_style));
                line.spans.push(Span::raw(" "));
            }

            line.spans
                .push(Span::styled(copy_badge_alt_badge(), alt_style));
            line.spans.push(Span::raw(" "));
            line.spans.push(Span::styled("[⇧]", shift_style));
            line.spans.push(Span::raw(" "));
            line.spans.push(Span::styled(
                format!("[{}]", key.to_ascii_uppercase()),
                key_style,
            ));
        }
    }

    if let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::Chat
            && range.end.pane == crate::tui::CopySelectionPane::Chat
    }) {
        let (start, end) = if (range.start.abs_line, range.start.column)
            <= (range.end.abs_line, range.end.column)
        {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

        for abs_idx in start.abs_line.max(scroll)..=end.abs_line.min(visible_end.saturating_sub(1))
        {
            let rel_idx = abs_idx.saturating_sub(scroll);
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                let copy_start = prepared.wrapped_copy_offset(abs_idx).unwrap_or(0);
                let start_col = if abs_idx == start.abs_line {
                    start.column.max(copy_start)
                } else {
                    copy_start
                };
                let end_col = if abs_idx == end.abs_line {
                    end.column.max(copy_start)
                } else {
                    copy_viewport_line_text(abs_idx)
                        .map(|text| UnicodeWidthStr::width(text.as_str()))
                        .unwrap_or_else(|| line.width())
                };
                *line = highlight_line_selection(line, start_col, end_col);
            }
        }
    }

    // Never draw image-placeholder marker text to the terminal. The marker
    // row only exists to carry `(hash, rows, cols)` into the prepare step,
    // which has already turned it into `prepared.image_regions`. Historically
    // it was drawn styled black-on-black and relied on staying invisible, but
    // terminal features can defeat that (kitty's translucent background /
    // contrast compositing, selection highlighting), leaking raw
    // "IIMG:<hash>:..." junk into the transcript whenever the image itself
    // is not painted over it (cold cache after a reload, prewarm in flight,
    // pinned mode). Video export is the one consumer that intentionally
    // scans printable markers out of the buffer, so it keeps them.
    if !crate::tui::mermaid::is_video_export_mode() {
        let marker_start = prepared
            .image_regions
            .partition_point(|region| region.abs_line_idx < scroll);
        for region in &prepared.image_regions[marker_start..] {
            if region.abs_line_idx >= visible_end {
                break;
            }
            let rel_idx = region.abs_line_idx - scroll;
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                *line = Line::default();
            }
        }
    }

    frame.render_widget(Paragraph::new(visible_lines), content_area);

    let centered = app.centered_mode();
    let diagram_mode = app.diagram_mode();
    let pinned_diagrams = diagram_mode == crate::config::DiagramDisplayMode::Pinned;
    {
        let visible_image_start = prepared
            .image_regions
            .partition_point(|region| region.end_line <= scroll);
        let visible_image_end = prepared
            .image_regions
            .partition_point(|region| region.abs_line_idx < visible_end);

        for region in &prepared.image_regions[visible_image_start..visible_image_end] {
            let abs_idx = region.abs_line_idx;
            let hash = region.hash;
            let total_height = region.height;
            let image_end = region.end_line;
            let is_fit = region.render == jcode_tui_messages::ImageRegionRender::Fit;

            if let Some(native_latex) = crate::tui::markdown::handterm_native_latex_for_hash(hash) {
                // Native math writes real terminal cells rather than an overlay,
                // so draw it only when the complete reserved region is visible.
                // The ordinary paragraph pass has already cleared every cell in
                // the region; placing the APC in the bottom-right cell guarantees
                // it executes after those clears in Ratatui's row-major diff.
                if abs_idx >= scroll
                    && image_end <= visible_end
                    && region.height == native_latex.rows
                    && region.width == native_latex.cols
                    && native_latex.cols <= content_area.width
                {
                    let top = content_area.y + (abs_idx - scroll) as u16;
                    let x = content_area.x + native_latex.cols.saturating_sub(1);
                    let y = top + native_latex.rows.saturating_sub(1);
                    if let Some(symbol) = handterm_native_latex_cell_symbol(
                        &native_latex.source,
                        native_latex.rows,
                        native_latex.cols,
                    ) && let Some(cell) = frame.buffer_mut().cell_mut((x, y))
                    {
                        cell.set_symbol(&symbol)
                            .set_fg(ratatui::style::Color::Rgb(100, 160, 255));
                    }
                }
                continue;
            }
            // Pinned mode only redirects mermaid diagrams (Crop) to the side
            // pane; inline raster images (Fit) always render in the flow.
            if pinned_diagrams && !is_fit {
                continue;
            }

            // Inline raster images are prepared lazily and off-thread: only
            // the ones actually on screen get decoded/scaled, and a cold image
            // schedules background prep instead of stalling this frame.
            let fit_ready = if is_fit && image_end > scroll && abs_idx < visible_end {
                super::inline_image_ui::ensure_drawable(hash, content_area.width, total_height)
            } else {
                true
            };

            if image_end > scroll && abs_idx < visible_end {
                if is_fit && !fit_ready {
                    // Background prep in flight; leave the blank placeholder
                    // rows this frame. A repaint is nudged on completion.
                    continue;
                }
                let marker_visible = abs_idx >= scroll && abs_idx < visible_end;

                if marker_visible {
                    let screen_y = (abs_idx - scroll) as u16;
                    let available_height = content_area.height.saturating_sub(screen_y);
                    let render_height = total_height.min(available_height);

                    if render_height > 0 {
                        let image_area = Rect {
                            x: content_area.x,
                            y: content_area.y + screen_y,
                            width: content_area.width,
                            height: render_height,
                        };
                        let rows = if is_fit {
                            // Stable fit: scale once to the placeholder box and
                            // reuse the transmitted pixels for every frame.
                            // Kitty re-addresses terminal-retained pixels;
                            // other protocols crop a pre-scaled source so the
                            // visible slice never changes the image's scale.
                            if crate::tui::mermaid::render_image_widget_fit_stable(
                                hash,
                                image_area,
                                frame.buffer_mut(),
                                content_area.width,
                                total_height,
                                0,
                                centered,
                                true,
                            ) {
                                image_area.height
                            } else {
                                crate::tui::mermaid::render_image_widget_fit(
                                    hash,
                                    image_area,
                                    frame.buffer_mut(),
                                    centered,
                                    true,
                                )
                            }
                        } else {
                            crate::tui::mermaid::render_image_widget(
                                hash,
                                image_area,
                                frame.buffer_mut(),
                                centered,
                                false,
                            )
                        };
                        if rows == 0 && !is_fit {
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(
                                    "↗ mermaid diagram unavailable",
                                    Style::default().fg(dim_color()),
                                ))),
                                image_area,
                            );
                        }
                    }
                } else {
                    let visible_start = scroll.max(abs_idx);
                    let visible_end_img = visible_end.min(image_end);
                    let screen_y = (visible_start - scroll) as u16;
                    let render_height = (visible_end_img - visible_start) as u16;

                    if render_height > 0 {
                        let image_area = Rect {
                            x: content_area.x,
                            y: content_area.y + screen_y,
                            width: content_area.width,
                            height: render_height,
                        };
                        if is_fit {
                            // Top scrolled off: keep the same scaled pixels and
                            // skip the hidden rows instead of rescaling into the
                            // smaller visible portion on every protocol.
                            let skip_rows = (visible_start - abs_idx) as u16;
                            if !crate::tui::mermaid::render_image_widget_fit_stable(
                                hash,
                                image_area,
                                frame.buffer_mut(),
                                content_area.width,
                                total_height,
                                skip_rows,
                                centered,
                                true,
                            ) {
                                crate::tui::mermaid::render_image_widget_fit(
                                    hash,
                                    image_area,
                                    frame.buffer_mut(),
                                    centered,
                                    true,
                                );
                            }
                        } else {
                            crate::tui::mermaid::render_image_widget(
                                hash,
                                image_area,
                                frame.buffer_mut(),
                                centered,
                                true,
                            );
                        }
                    }
                }
            }
        }

        // Look-ahead prefetch: warm inline raster images within a margin band
        // above/below the viewport so they are already decoded+scaled by the
        // time they scroll into view, killing the first-scroll "blank then
        // pop" hitch. Cheap and non-blocking: prefetch dedups against in-flight
        // and already-warm state, and only the background worker does real
        // work. Margin scales with viewport height so faster scrolls (which
        // cover more rows per frame) get a deeper warm band.
        if content_area.height > 0 {
            const PREFETCH_VIEWPORTS: usize = 2;
            let margin_lines = (content_area.height as usize)
                .saturating_mul(PREFETCH_VIEWPORTS)
                .max(1);
            let prefetch_start = scroll.saturating_sub(margin_lines);
            let prefetch_end = visible_end.saturating_add(margin_lines);
            let band_start = prepared
                .image_regions
                .partition_point(|region| region.end_line <= prefetch_start);
            let band_end = prepared
                .image_regions
                .partition_point(|region| region.abs_line_idx < prefetch_end);
            for region in &prepared.image_regions[band_start..band_end] {
                // Only inline raster images use the prewarm pipeline; mermaid
                // crops build their own state at draw time. In pinned mode the
                // raster images still render in the flow, so always prefetch.
                if region.render != jcode_tui_messages::ImageRegionRender::Fit {
                    continue;
                }
                // Skip the ones already on screen; the draw pass above warmed
                // them via ensure_drawable.
                let on_screen = region.end_line > scroll && region.abs_line_idx < visible_end;
                if on_screen {
                    continue;
                }
                super::inline_image_ui::prefetch(region.hash, content_area.width, region.height);
            }
        }
    }

    let right_x = render_area.x + render_area.width.saturating_sub(1);
    for &line_idx in &wrapped_user_indices[visible_user_start..visible_user_end] {
        if line_idx >= scroll && line_idx < scroll + visible_height {
            let screen_y = content_area.y + (line_idx - scroll) as u16;
            let bar_area = Rect {
                x: right_x,
                y: screen_y,
                width: 1,
                height: 1,
            };
            let bar = Paragraph::new(Span::styled("│", Style::default().fg(user_color())));
            frame.render_widget(bar, bar_area);
        }
    }

    if !show_native_scrollbar && scroll > 0 {
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: render_area.x + render_area.width.saturating_sub(indicator.len() as u16 + 2),
            y: render_area.y,
            width: indicator.len() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                indicator,
                Style::default().fg(dim_color()),
            )])),
            indicator_area,
        );
    }

    if crate::config::config().display.prompt_preview && scroll > 0 {
        let last_offscreen_prompt_idx =
            lower_bound(wrapped_user_prompt_starts, scroll).checked_sub(1);

        if let Some(prompt_order) = last_offscreen_prompt_idx
            && let Some(prompt_text) = user_prompt_texts.get(prompt_order)
        {
            let prompt_text = prompt_text.trim();
            if !prompt_text.is_empty() {
                let prompt_num = prompt_order + 1 + app.compacted_hidden_user_prompts();
                let num_str = format!("{}", prompt_num);
                let prefix_len = num_str.len() + 2;
                let content_width =
                    render_area.width.saturating_sub(prefix_len as u16 + 2) as usize;
                let dim_style = Style::default().dim();
                let align = if app.centered_mode() {
                    ratatui::layout::Alignment::Center
                } else {
                    ratatui::layout::Alignment::Left
                };

                let text_flat = prompt_text.replace('\n', " ");
                let text_chars: Vec<char> = text_flat.chars().collect();
                let is_long = text_chars.len() > content_width;

                let preview_lines: Vec<Line<'static>> = if !is_long {
                    vec![
                        Line::from(vec![
                            Span::styled(num_str.clone(), dim_style.fg(dim_color()).bg(user_bg())),
                            Span::styled("› ", dim_style.fg(user_color()).bg(user_bg())),
                            Span::styled(text_flat, dim_style.fg(user_text()).bg(user_bg())),
                        ])
                        .alignment(align),
                    ]
                } else {
                    let half = content_width.max(4);
                    let head: String = text_chars[..half.min(text_chars.len())].iter().collect();
                    let tail_start = text_chars.len().saturating_sub(half);
                    let tail: String = text_chars[tail_start..].iter().collect();

                    let first = Line::from(vec![
                        Span::styled(num_str.clone(), dim_style.fg(dim_color()).bg(user_bg())),
                        Span::styled("› ", dim_style.fg(user_color()).bg(user_bg())),
                        Span::styled(
                            format!("{} ...", head.trim_end()),
                            dim_style.fg(user_text()).bg(user_bg()),
                        ),
                    ])
                    .alignment(align);

                    let padding: String = " ".repeat(prefix_len);
                    let second = Line::from(vec![
                        Span::styled(padding, dim_style.bg(user_bg())),
                        Span::styled(
                            format!("... {}", tail.trim_start()),
                            dim_style.fg(user_text()).bg(user_bg()),
                        ),
                    ])
                    .alignment(align);

                    vec![first, second]
                };

                let line_count = preview_lines.len() as u16;
                let preview_area = Rect {
                    x: content_area.x,
                    y: render_area.y,
                    width: content_area.width.saturating_sub(1),
                    height: line_count,
                };
                clear_area(frame, preview_area);
                frame.render_widget(Paragraph::new(preview_lines), preview_area);
            }
        }
    }

    if !show_native_scrollbar && app.auto_scroll_paused() && scroll < max_scroll {
        let indicator = format!("↓{}", max_scroll - scroll);
        let indicator_area = Rect {
            x: render_area.x + render_area.width.saturating_sub(indicator.len() as u16 + 2),
            y: render_area.y + render_area.height.saturating_sub(1),
            width: indicator.len() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                indicator,
                Style::default().fg(queued_color()),
            )])),
            indicator_area,
        );
    }

    if let Some(scrollbar_area) = scrollbar_area {
        super::render_native_scrollbar(
            frame,
            scrollbar_area,
            scroll,
            total_lines,
            visible_height,
            false,
        );
    }

    // Derive the look-ahead "reliable" width profile that gates where *new* info
    // widgets may dock, so a freshly placed widget won't be covered by a wide line
    // one scroll line later. We use a small windowed minimum over the assembled
    // per-row free widths (the rows already on screen above/below each candidate
    // row), which keeps it cheap and needs no off-screen line materialization.
    // Pinned widgets still size to the instantaneous widths for full coverage.
    margins.right_reliable = windowed_min(&margins.right_widths, INFO_WIDGET_LOOKAHEAD_ROWS);
    if margins.centered {
        margins.left_reliable = windowed_min(&margins.left_widths, INFO_WIDGET_LOOKAHEAD_ROWS);
    }

    margins
}

/// Look-ahead window (in rows) used to compute the "reliable" margin profile that
/// gates where new info widgets may dock. Small by design: it only needs to cover
/// the distance content travels in the few frames between a widget being placed and
/// a nearby long line scrolling into its rows.
const INFO_WIDGET_LOOKAHEAD_ROWS: usize = 2;

/// Per-index minimum over `[i-window, i+window]`. Returns an empty vec for empty
/// input (callers treat empty reliable profiles as "no look-ahead").
fn windowed_min(widths: &[u16], window: usize) -> Vec<u16> {
    if widths.is_empty() {
        return Vec::new();
    }
    let n = widths.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let lo = i.saturating_sub(window);
        let hi = (i + window).min(n - 1);
        out.push(widths[lo..=hi].iter().copied().min().unwrap_or(0));
    }
    out
}

fn compute_prompt_preview_line_count(
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    scroll: usize,
    area_width: u16,
) -> u16 {
    let last_offscreen = lower_bound(wrapped_user_prompt_starts, scroll).checked_sub(1);
    let Some(prompt_order) = last_offscreen else {
        return 0;
    };
    let Some(prompt_text) = user_prompt_texts.get(prompt_order) else {
        return 0;
    };
    let prompt_text = prompt_text.trim();
    if prompt_text.is_empty() {
        return 0;
    }
    let num_str = format!("{}", prompt_order + 1);
    let prefix_len = num_str.len() + 2;
    let content_width = area_width.saturating_sub(prefix_len as u16 + 2) as usize;
    let text_flat = prompt_text.replace('\n', " ");
    let display_width = UnicodeWidthStr::width(text_flat.as_str());
    if display_width > content_width { 2 } else { 1 }
}

fn compute_max_scroll_with_prompt_preview(
    total_lines: usize,
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    area: Rect,
) -> usize {
    let mut max_scroll = total_lines.saturating_sub(area.height as usize);
    if max_scroll == 0 || !crate::config::config().display.prompt_preview {
        return max_scroll;
    }

    for _ in 0..4 {
        let prompt_preview_lines = compute_prompt_preview_line_count(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            max_scroll,
            area.width,
        );
        let content_height = area.height.saturating_sub(prompt_preview_lines) as usize;
        let adjusted = total_lines.saturating_sub(content_height);
        if adjusted == max_scroll {
            break;
        }
        max_scroll = adjusted;
    }

    max_scroll
}

#[cfg(test)]
mod tests {
    #[test]
    fn handterm_native_latex_cell_symbol_moves_invokes_and_restores_cursor() {
        assert_eq!(
            super::handterm_native_latex_cell_symbol(r"\frac{a}{b}", 3, 2).as_deref(),
            Some("\x1b[s\x1b[2A\x1b[1D\x1b_L;\\frac{a}{b}\x1b\\\x1b[u\x1b[1C")
        );
        assert_eq!(
            super::handterm_native_latex_cell_symbol("x", 1, 1).as_deref(),
            Some("\x1b[s\x1b_L;x\x1b\\\x1b[u\x1b[1C")
        );
        assert!(super::handterm_native_latex_cell_symbol("x\x1by", 1, 1).is_none());
    }

    #[test]
    fn tail_follow_small_appends_snap_to_bottom() {
        // Streaming-sized appends (<= min jump) snap directly; no animation.
        crate::tui::ui::set_last_resolved_chat_scroll(100);
        let scroll = super::resolve_tail_follow_scroll(103, 30);
        assert_eq!(scroll, 103);
        assert!(!crate::tui::ui::tail_catchup_active());
    }

    #[test]
    fn tail_follow_large_append_slides_in_bounded_steps() {
        // A 12-row jump advances by at most TAIL_CATCHUP_MAX_STEP per frame
        // and reports an active catch-up until it reaches the bottom.
        crate::tui::ui::set_last_resolved_chat_scroll(100);
        let first = super::resolve_tail_follow_scroll(112, 30);
        assert!(first < 112, "must not snap: {first}");
        assert!(
            first - 100 <= super::TAIL_CATCHUP_MAX_STEP,
            "step bounded: {first}"
        );
        assert!(crate::tui::ui::tail_catchup_active());

        // Subsequent frames converge to the bottom and clear the flag.
        let mut scroll = first;
        let mut guard = 0;
        while scroll < 112 {
            crate::tui::ui::set_last_resolved_chat_scroll(scroll);
            scroll = super::resolve_tail_follow_scroll(112, 30);
            guard += 1;
            assert!(guard < 50, "catch-up must converge");
        }
        assert_eq!(scroll, 112);
        assert!(!crate::tui::ui::tail_catchup_active());
    }

    #[test]
    fn explicit_tail_follow_request_skips_content_catch_up_animation() {
        crate::tui::ui::set_last_resolved_chat_scroll(40);
        crate::tui::ui::request_tail_follow_snap();

        let scroll = super::resolve_tail_follow_scroll(80, 30);

        assert_eq!(scroll, 80);
        assert!(!crate::tui::ui::tail_catchup_active());
    }

    #[test]
    fn tail_follow_caps_lag_to_one_viewport() {
        // A huge append (way beyond a screen) starts at most one viewport
        // behind the bottom so the catch-up never replays pages of content.
        crate::tui::ui::set_last_resolved_chat_scroll(100);
        let scroll = super::resolve_tail_follow_scroll(400, 30);
        assert!(scroll >= 400 - 30, "lag capped to viewport: {scroll}");
        assert!(crate::tui::ui::tail_catchup_active());
    }

    #[test]
    fn tail_follow_backward_motion_snaps() {
        // Content shrank (commit collapsed reasoning): snap, don't animate.
        crate::tui::ui::set_last_resolved_chat_scroll(100);
        let scroll = super::resolve_tail_follow_scroll(80, 30);
        assert_eq!(scroll, 80);
        assert!(!crate::tui::ui::tail_catchup_active());
    }

    #[test]
    fn default_copy_badge_alt_label_matches_platform() {
        #[cfg(target_os = "macos")]
        assert_eq!(super::copy_badge_alt_label_from_config(""), "⌥");

        #[cfg(not(target_os = "macos"))]
        assert_eq!(super::copy_badge_alt_label_from_config(""), "Alt");
    }

    #[test]
    fn copy_badge_alt_label_uses_trimmed_config_override() {
        assert_eq!(
            super::copy_badge_alt_label_from_config(" Option "),
            "Option"
        );
        assert_eq!(super::copy_badge_alt_label_from_config("⌥"), "⌥");
    }
}
