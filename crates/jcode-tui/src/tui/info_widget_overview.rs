use super::info_widget::{
    AuthMethod, InfoWidgetData, changes_section_height, compact_usage_rows,
    is_traceworthy_memory_event, model_info_height,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InfoPageKind {
    CompactOnly,
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

/// Only a genuine pending first snapshot reserves space. An absent snapshot
/// after initialization is not evidence that this account supports quota data.
pub(super) fn startup_usage_pending(data: &InfoWidgetData) -> bool {
    data.startup_usage_pending
        && matches!(data.auth_method, AuthMethod::OpenAIOAuth)
        && !data
            .usage_info
            .as_ref()
            .is_some_and(|info| info.available || info.limit_reached)
}

pub(super) fn render_overview_usage(
    data: &InfoWidgetData,
    width: u16,
) -> Vec<ratatui::text::Line<'static>> {
    if startup_usage_pending(data) {
        return super::info_widget::startup_usage_lines(width);
    }
    data.usage_info
        .as_ref()
        .filter(|info| info.available || info.limit_reached)
        .map(|info| compact_usage_rows(info, width, data.usage_display_used))
        .unwrap_or_default()
}

fn compact_usage_height(data: &InfoWidgetData) -> u16 {
    if startup_usage_pending(data) {
        return 2;
    }
    let Some(info) = data.usage_info.as_ref() else {
        return 0;
    };
    if info.limit_reached {
        return 1;
    }
    if !info.available {
        return 0;
    }
    if matches!(info.provider, super::info_widget::UsageProvider::CostBased) {
        return 1;
    }
    // Mirror render_usage_compact without formatting bars during measurement.
    u16::from(!info.provider.label().is_empty())
        + u16::from(info.primary_limit_label.is_some())
        + u16::from(info.secondary_limit_label.is_some())
        + u16::from(info.spark.is_some())
}

/// Overview height. Status-line facts (model identity, context %, branch and
/// counts) are not sections here, only the detail behind them.
fn compact_overview_height(data: &InfoWidgetData) -> u16 {
    model_info_height(data)
        + compact_memory_height(data)
        + compact_background_height(data)
        + compact_usage_height(data)
        + changes_section_height(data)
        + u16::from(data.cloud_info.is_some())
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
    fn compute_page_layout_only_offers_non_plan_pages() {
        let data = InfoWidgetData {
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

        assert_eq!(layout.pages.len(), 1);
        assert!(!layout.show_dots);
        assert!(
            layout
                .pages
                .iter()
                .any(|page| page.kind == InfoPageKind::MemoryExpanded)
        );
    }
}
