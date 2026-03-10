use super::info_widget::{
    context_entries, AuthMethod, InfoWidgetData, UsageProvider, MAX_MEMORY_EVENTS,
};

pub(crate) const MAX_CONTEXT_LINES: usize = 5;
pub(crate) const MAX_TODO_LINES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InfoPageKind {
    CompactOnly,
    TodosExpanded,
    ContextExpanded,
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
    let context_compact = compact_context_height(data);
    let todos_compact = compact_todos_height(data);

    let context_expanded = expanded_context_height(data);
    if context_expanded > 0 {
        candidates.push(InfoPage {
            kind: InfoPageKind::ContextExpanded,
            height: compact_height - context_compact + context_expanded,
        });
    }

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

    let mut show_dots = false;
    if pages.len() > 1 {
        let filtered: Vec<InfoPage> = pages
            .iter()
            .copied()
            .filter(|page| page.height + 1 <= inner_height)
            .collect();
        if filtered.len() > 1 {
            pages = filtered;
            show_dots = true;
        } else if filtered.len() == 1 {
            pages = filtered;
        }
    }

    let max_page_height = pages
        .iter()
        .map(|page| page.height + if show_dots { 1 } else { 0 })
        .max()
        .unwrap_or(0);

    PageLayout {
        pages,
        max_page_height,
        show_dots,
    }
}

fn compact_context_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            return 1;
        }
    }
    0
}

fn compact_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() {
        0
    } else {
        2
    }
}

fn compact_queue_height(data: &InfoWidgetData) -> u16 {
    if data.queue_mode.is_some() {
        1
    } else {
        0
    }
}

fn compact_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 {
            return 1;
        }
    }
    0
}

fn compact_model_height(data: &InfoWidgetData) -> u16 {
    if data.model.is_some() {
        let mut lines = 1u16;
        let has_provider = data
            .provider_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some();
        if has_provider || data.auth_method != AuthMethod::Unknown {
            lines += 1;
        }
        if data.session_count.is_some() || data.session_name.is_some() {
            lines += 1;
        }
        lines
    } else {
        0
    }
}

fn compact_background_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.background_info {
        if info.running_count > 0 || info.memory_agent_active {
            return 1;
        }
    }
    0
}

fn compact_usage_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.usage_info {
        if info.available {
            match info.provider {
                UsageProvider::CostBased | UsageProvider::Copilot => return 2,
                _ => {
                    let label = info.provider.label();
                    let label_line = if label.is_empty() { 0 } else { 1 };
                    let spark_line = if info.spark.is_some() { 1 } else { 0 };
                    return 2 + label_line + spark_line;
                }
            }
        }
    }
    0
}

fn compact_git_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.git_info {
        if info.is_interesting() {
            return 1;
        }
    }
    0
}

fn compact_overview_height(data: &InfoWidgetData) -> u16 {
    compact_model_height(data)
        + compact_context_height(data)
        + compact_todos_height(data)
        + compact_queue_height(data)
        + compact_memory_height(data)
        + compact_background_height(data)
        + compact_usage_height(data)
        + compact_git_height(data)
}

fn expanded_context_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.context_info {
        if info.total_chars > 0 {
            return 3 + context_entries(info).len().min(MAX_CONTEXT_LINES) as u16;
        }
    }
    0
}

fn expanded_todos_height(data: &InfoWidgetData) -> u16 {
    if data.todos.is_empty() {
        return 0;
    }

    let available_lines = MAX_TODO_LINES.saturating_sub(2);
    let todo_lines = data.todos.len().min(available_lines);
    let mut height = 2 + todo_lines as u16;
    if data.todos.len() > available_lines {
        height += 1;
    }
    height
}

fn expanded_memory_height(data: &InfoWidgetData) -> u16 {
    if let Some(info) = &data.memory_info {
        if info.total_count > 0 || info.activity.is_some() {
            let mut height = 2u16;
            if let Some(activity) = &info.activity {
                height += 1;
                height += activity.recent_events.len().min(MAX_MEMORY_EVENTS) as u16;
            }
            if !info.by_category.is_empty() {
                height += 1;
            }
            return height;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{compute_page_layout, InfoPageKind};
    use crate::prompt::ContextInfo;
    use crate::todo::TodoItem;
    use crate::tui::info_widget::InfoWidgetData;

    #[test]
    fn compute_page_layout_falls_back_to_compact_page() {
        let data = InfoWidgetData {
            model: Some("gpt-test".to_string()),
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
            context_info: Some(ContextInfo {
                total_chars: 24_000,
                system_prompt_chars: 12_000,
                ..Default::default()
            }),
            todos: vec![TodoItem {
                content: "ship refactor".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                id: "todo-1".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
            ..Default::default()
        };

        let layout = compute_page_layout(&data, 40, 8);

        assert!(layout.pages.len() >= 2);
        assert!(layout.show_dots);
        assert!(layout
            .pages
            .iter()
            .any(|page| page.kind == InfoPageKind::ContextExpanded));
        assert!(layout
            .pages
            .iter()
            .any(|page| page.kind == InfoPageKind::TodosExpanded));
    }
}
