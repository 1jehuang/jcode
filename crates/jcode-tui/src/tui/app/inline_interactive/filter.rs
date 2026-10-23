use super::*;

impl App {
    pub(in crate::tui::app) fn picker_fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
        jcode_fuzzy::fuzzy_score_tokens(pattern, text)
    }

    pub(in crate::tui::app) fn apply_inline_interactive_filter(
        picker: &mut InlineInteractiveState,
    ) {
        if picker.filter.is_empty() {
            picker.filtered = (0..picker.entries.len()).collect();
        } else {
            let query = picker.filter.trim();
            // Prepare the query once per keystroke instead of re-parsing and
            // re-lowercasing it for every entry.
            let prepared = jcode_fuzzy::PreparedTokenQuery::new(&picker.filter);
            let mut scored: Vec<(usize, bool, i32)> = picker
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let filter_text = picker.filter_text(m);
                    prepared.score(&filter_text).map(|s| {
                        let usage_bonus = m.usage_score.min(i32::MAX as u32) as i32;
                        let bonus = usage_bonus + if m.recommended { 5 } else { 0 };
                        (
                            i,
                            m.name.eq_ignore_ascii_case(query),
                            s.saturating_add(bonus),
                        )
                    })
                })
                .collect();
            scored.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(b.2.cmp(&a.2))
                    .then(
                        picker.entries[a.0]
                            .recommendation_rank
                            .cmp(&picker.entries[b.0].recommendation_rank),
                    )
                    .then(picker.entries[a.0].name.cmp(&picker.entries[b.0].name))
            });
            picker.filtered = scored.into_iter().map(|(i, _, _)| i).collect();
        }
        if picker.filtered.is_empty() {
            picker.selected = 0;
        } else {
            picker.selected = picker.selected.min(picker.filtered.len() - 1);
        }
    }
}
