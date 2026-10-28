use super::*;

impl App {
    pub(in crate::tui::app) fn debug_picker_state_json(
        &self,
        visible_limit: Option<usize>,
    ) -> String {
        let Some(picker) = self.inline_interactive_state.as_ref() else {
            return serde_json::json!({
                "open": false,
                "input": self.input,
                "cursor_pos": self.cursor_pos,
            })
            .to_string();
        };

        let filtered_count = picker.filtered.len();
        let total_count = picker.entries.len();
        let list_height = visible_limit.unwrap_or_else(|| filtered_count.min(40));
        let selected = picker.selected.min(filtered_count.saturating_sub(1));
        let start = if filtered_count == 0 || list_height == 0 {
            0
        } else {
            let half = list_height / 2;
            if selected <= half {
                0
            } else if selected + list_height - half > filtered_count {
                filtered_count.saturating_sub(list_height)
            } else {
                selected - half
            }
        };
        let end = if list_height == 0 {
            start
        } else {
            (start + list_height).min(filtered_count)
        };

        let rows: Vec<serde_json::Value> = picker
            .filtered
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
            .filter_map(|(visible_index, entry_index)| {
                let entry = picker.entries.get(*entry_index)?;
                let route = entry.active_option();
                let filter_text = picker.filter_text(entry);
                Some(serde_json::json!({
                    "visible_index": visible_index,
                    "entry_index": entry_index,
                    "selected": visible_index == selected,
                    "name": entry.name,
                    "provider": route.map(|r| r.provider.as_str()).unwrap_or(""),
                    "api_method": route.map(|r| r.api_method.as_str()).unwrap_or(""),
                    "available": route.map(|r| r.available).unwrap_or(false),
                    "detail": route.map(|r| r.detail.as_str()).unwrap_or(""),
                    "filter_text": filter_text,
                    "fuzzy_score": if picker.filter.is_empty() {
                        None
                    } else {
                        Self::picker_fuzzy_score(&picker.filter, &filter_text)
                    },
                    "recommended": entry.recommended,
                    "current": entry.is_current,
                    "default": entry.is_default,
                    "old": entry.old,
                    "created_date": entry.created_date,
                }))
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "open": true,
            "kind": format!("{:?}", picker.kind),
            "preview": picker.preview,
            "input": self.input,
            "cursor_pos": self.cursor_pos,
            "filter": picker.filter,
            "selected": picker.selected,
            "column": picker.column,
            "total_count": total_count,
            "filtered_count": filtered_count,
            "visible_start": start,
            "visible_end": end,
            "visible_count": rows.len(),
            "visible_limit": visible_limit,
            "rows": rows,
        }))
        .unwrap_or_else(|_| "{}".to_string())
    }
}
