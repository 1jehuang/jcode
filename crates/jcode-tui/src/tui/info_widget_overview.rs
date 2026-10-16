use super::info_widget::{
    InfoWidgetData, OVERVIEW_COMMITS_MAX_ROWS, OVERVIEW_SWARM_MAX_ROWS, UsageProvider,
    changes_section_height, commits_has_data, is_traceworthy_memory_event, model_info_height,
};
use ratatui::layout::Rect;

pub(crate) const MAX_TODO_LINES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InfoPageKind {
    CompactOnly,
    TodosExpanded,
    MemoryExpanded,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InfoPage {
    pub kind: InfoPageKind,
    pub height: u16,
}

pub(crate) struct PageLayout {
    pub pages: Vec<InfoPage>,
    pub max_page_height: u16,
    /// Page dots, drawn on the widget's bottom border (they take no row).
    pub show_dots: bool,
}

/// Which tail sections of the compact Overview page a placed rect can actually
/// show. The compact page renders sections in a fixed order (Runtime → Todos
/// → Memory → Background → Usage → KV cache → Compaction → Changes → Commits
/// → Swarm) and `render_overview_framed` truncates whatever the inner height
/// cannot fit. A section that lands entirely in the truncated tail is *not*
/// shown by that Overview, so its standalone fallback must stay available
/// (greptile: a 10-row pocket whose rows Runtime/Todos/Background/Usage filled
/// suppressed the Commits and SwarmStatus fallbacks while the placed Overview
/// truncated both sections away, hiding worker status everywhere).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SectionVisibility {
    /// The Runtime (model info) section fits fully in the rect.
    pub model_info: bool,
    /// The Todos section fits fully in the rect.
    pub todos: bool,
    /// The Background section fits fully in the rect.
    pub background: bool,
    /// The Usage section fits fully in the rect.
    pub usage: bool,
    /// The KV cache section fits fully in the rect.
    pub kv_cache: bool,
    /// The Compaction section fits fully in the rect.
    pub compaction: bool,
    /// The Changes (dirty files) section fits fully in the rect.
    pub changes: bool,
    /// The Commits section fits fully in the rect.
    pub commits: bool,
    /// The Swarm section fits fully in the rect.
    pub swarm: bool,
}

/// Which sections a placed Overview rect (outer rect, borders included) can
/// show. Sections that would only be partially visible count as not shown:
/// the render truncates at inner height, and a half-drawn section header is
/// worse than the standalone fallback that shows the content in full.
/// The rect "shows" a section only when every page it can rotate through
/// shows it: the strip and fallback suppression span the whole widget, not
/// one page, so a section truncated on any page must keep its fallback.
pub(crate) fn section_visibility(data: &InfoWidgetData, rect: Rect) -> SectionVisibility {
    // Mirror render_overview_framed: memory is the dedicated widget's content,
    // so the Overview strips it before rendering (and the MemoryExpanded
    // page therefore never exists here).
    let mut overview = data.clone();
    overview.memory_info = None;
    let inner_height = rect.height.saturating_sub(2);
    let model_h = model_info_height(&overview);
    let todos_h = compact_todos_height(&overview);
    let background_h = compact_background_height(&overview);
    let usage_h = compact_usage_height(&overview);
    let kv_h = compact_kv_cache_height(&overview);
    let compaction_h = compact_compaction_height(&overview);
    let changes_h = changes_section_height(&overview);
    let commits_h = compact_commits_height(&overview);
    let swarm_h = compact_swarm_height(&overview);
    let fits = |front: u16, section: u16| section > 0 && front + section <= inner_height;
    // Sections render front-to-back in a fixed order; a section is shown only
    // when it and everything before it fits, exactly like the truncating render.
    // Returns (total front height, per-section fits flags).
    let front_sections = |expanded_todos: bool| -> (u16, [bool; 6]) {
        let todos = if expanded_todos {
            expanded_todos_height(&overview)
        } else {
            todos_h
        };
        let sections = [
            (model_h, model_h > 0),
            (todos, todos > 0),
            (background_h, background_h > 0),
            (usage_h, usage_h > 0),
            (kv_h, kv_h > 0),
            (compaction_h, compaction_h > 0),
        ];
        let mut front = 0u16;
        let mut flags = [false; 6];
        for (idx, (height, present)) in sections.iter().enumerate() {
            flags[idx] = *present
                && front
                    .checked_add(*height)
                    .is_some_and(|end| end <= inner_height);
            front = front.saturating_add(*height);
        }
        (front, flags)
    };
    let per_page = |expanded_todos: bool| {
        let (front, [model_info, todos, background, usage, kv_cache, compaction]) =
            front_sections(expanded_todos);
        SectionVisibility {
            model_info,
            todos,
            background,
            usage,
            kv_cache,
            compaction,
            changes: fits(front, changes_h),
            commits: fits(front + changes_h, commits_h),
            swarm: fits(front + changes_h + commits_h, swarm_h),
        }
    };
    let compact = per_page(false);
    let layout = compute_page_layout(&overview, 0, inner_height);
    if layout.pages.is_empty() {
        // The slot is shorter than even the compact page: the renderer falls
        // back to the truncated compact page (see render_overview_framed),
        // so only that page's tail visibility matters.
        return compact;
    }
    let mut combined = compact;
    if layout
        .pages
        .iter()
        .any(|page| page.kind == InfoPageKind::TodosExpanded)
    {
        let expanded = per_page(true);
        combined.model_info &= expanded.model_info;
        combined.todos &= expanded.todos;
        combined.background &= expanded.background;
        combined.usage &= expanded.usage;
        combined.kv_cache &= expanded.kv_cache;
        combined.compaction &= expanded.compaction;
        combined.changes &= expanded.changes;
        combined.commits &= expanded.commits;
        combined.swarm &= expanded.swarm;
    }
    combined
}

pub(crate) fn compute_page_layout(
    data: &InfoWidgetData,
    _inner_width: usize,
    inner_height: u16,
) -> PageLayout {
    let compact_height = compact_overview_height(data);
    if compact_height == 0 {
        return PageLayout {
            pages: Vec::new(),
            max_page_height: 0,
            show_dots: false,
        };
    }

    let mut candidates: Vec<InfoPage> = Vec::new();
    let todos_compact = compact_todos_height(data);

    let todos_expanded = expanded_todos_height(data);
    if todos_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::TodosExpanded,
            height: compact_height - todos_compact + todos_expanded,
        });
    }

    let memory_compact = compact_memory_height(data);
    let memory_expanded = expanded_memory_height(data);
    if memory_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::MemoryExpanded,
            height: compact_height - memory_compact + memory_expanded,
        });
    }

    let mut pages: Vec<InfoPage> = candidates
        .into_iter()
        .filter(|page| page.height <= inner_height)
        .collect();

    if pages.is_empty() {
        if compact_height <= inner_height {
            pages.push(InfoPage {
                kind: InfoPageKind::CompactOnly,
                height: compact_height,
            });
        } else {
            return PageLayout {
                pages,
                max_page_height: 0,
                show_dots: false,
            };
        }
    }

    let show_dots = pages.len() > 1;
    let max_page_height = pages.iter().map(|page| page.height).max().unwrap_or(0);

    PageLayout {
        pages,
        max_page_height,
        show_dots,
    }
}

fn compact_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() { 0 } else { 2 }
}

fn compact_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info
        && info.should_render()
    {
        return 1;
    }
    0
}

fn compact_background_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.background_info
        && info.has_content()
    {
        let total = if info.rows.is_empty() {
            info.running_tasks.len()
        } else {
            info.rows.len()
        };
        let task_lines = total.min(3) as u16;
        let overflow_line = u16::from(total > 3);
        return 1 + task_lines + overflow_line;
    }
    0
}

fn compact_usage_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.usage_info
        && info.available
    {
        // Must mirror render_usage_compact exactly, otherwise the compact
        // overview page either clips its last lines or reserves blank rows.
        if matches!(info.provider, UsageProvider::CostBased) {
            // Single "$cost · tokens" line.
            return 1;
        }
        // Subscription-style providers render an optional provider label plus
        // whichever primary, secondary, and Spark windows are actually present.
        let label = info.provider.label();
        let label_line = u16::from(!label.is_empty());
        let primary_line = u16::from(info.primary_limit_label.is_some());
        let secondary_line = u16::from(info.secondary_limit_label.is_some());
        let spark_line = u16::from(info.spark.is_some());
        return label_line + primary_line + secondary_line + spark_line;
    }
    0
}

fn compact_kv_cache_height(data: &InfoWidgetData) -> u16 {
    if data.cache_hit_info.is_some() { 1 } else { 0 }
}

/// Overview rows used by the Compaction section, mirroring [`render_sections`].
fn compact_compaction_height(data: &InfoWidgetData) -> u16 {
    u16::from(data.compaction_info.is_some())
}

/// Overview rows used by the Commits section, mirroring [`render_sections`].
/// Must mirror `render_commits_compact` exactly: header + shown commits +
/// overflow row.
fn compact_commits_height(data: &InfoWidgetData) -> u16 {
    let Some(info) = data.git_info.as_ref() else {
        return 0;
    };
    if !commits_has_data(info) || info.recent_commits.is_empty() {
        return 0;
    }
    let rows = OVERVIEW_COMMITS_MAX_ROWS.min(info.recent_commits.len());
    let overflow = u16::from(info.recent_commits.len() > rows);
    1 + rows as u16 + overflow
}

/// Overview rows used by the Swarm section, mirroring [`render_sections`].
/// Must mirror `render_swarm_compact` exactly: header + shown agents +
/// overflow row + plan meter.
fn compact_swarm_height(data: &InfoWidgetData) -> u16 {
    let Some(info) = data.swarm_info.as_ref() else {
        return 0;
    };
    let members = info.managed_members.len();
    if members == 0 {
        return 0;
    }
    let rows = members.min(OVERVIEW_SWARM_MAX_ROWS);
    let overflow = u16::from(members > rows);
    let plan = u16::from(
        info.plan_progress
            .map(|(_, _, total)| total > 0)
            .unwrap_or(false),
    );
    1 + rows as u16 + overflow + plan
}

/// Overview height. Status-line facts (model identity, context %, branch and
/// counts) are not sections here, only the detail behind them.
fn compact_overview_height(data: &InfoWidgetData) -> u16 {
    model_info_height(data)
        + compact_todos_height(data)
        + compact_memory_height(data)
        + compact_background_height(data)
        + compact_usage_height(data)
        + compact_kv_cache_height(data)
        + compact_compaction_height(data)
        + changes_section_height(data)
        + compact_commits_height(data)
        + compact_swarm_height(data)
}

fn expanded_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() {
        return 0;
    }

    let available_lines = MAX_TODO_LINES.saturating_sub(1);
    let todo_lines = data.todos.len().min(available_lines);
    let mut height = 1 + u16::try_from(todo_lines).unwrap_or(u16::MAX);
    if data.todos.len() > available_lines {
        height += 1;
    }
    height
}

fn expanded_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info
        && info.should_render()
    {
        let mut height = 1u16;
        if info.should_show_activity() {
            height += 1 + 4;
            if let Some(activity) = &info.activity
                && activity
                    .recent_events
                    .iter()
                    .any(is_traceworthy_memory_event)
            {
                height += 1;
            }
        }
        return height;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{InfoPageKind, compute_page_layout};
    use crate::todo::TodoItem;
    use crate::tui::info_widget::{InfoWidgetData, MemoryInfo};
    use std::collections::HashMap;

    #[test]
    fn compute_page_layout_falls_back_to_compact_page() {
        let data = InfoWidgetData {
            session_name: Some("sauropod".to_string()),
            queue_mode: Some(true),
            ..Default::default()
        };

        let layout = compute_page_layout(&data, 40, 8);

        assert_eq!(layout.pages.len(), 1);
        assert_eq!(layout.pages[0].kind, InfoPageKind::CompactOnly);
        assert!(!layout.show_dots);
    }

    #[test]
    fn compute_page_layout_keeps_multiple_expanded_pages_when_height_allows() {
        let data = InfoWidgetData {
            todos: vec![TodoItem {
                group: None,
                content: "ship refactor".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                id: "todo-1".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            }],
            memory_info: Some(MemoryInfo {
                total_count: 3,
                project_count: 2,
                global_count: 1,
                by_category: HashMap::from([("fact".to_string(), 3usize)]),
                sidecar_model: Some("openai · gpt-5.3-codex-spark".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let layout = compute_page_layout(&data, 40, 8);

        assert!(layout.pages.len() >= 2);
        assert!(layout.show_dots);
        assert!(
            layout
                .pages
                .iter()
                .any(|page| page.kind == InfoPageKind::TodosExpanded)
        );
        assert!(
            layout
                .pages
                .iter()
                .any(|page| page.kind == InfoPageKind::MemoryExpanded)
        );
    }
}
