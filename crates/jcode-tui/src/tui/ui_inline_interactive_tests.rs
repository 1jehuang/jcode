use super::*;

#[test]
fn account_priority_titles_and_normalized_colors_are_width_safe() {
    let mut picker = sample_account_picker(false);
    let entry = &mut picker.entries[0];
    entry.recommendation_rank = 0;
    entry.name = "界界 very.long.account@example.com · Pro".into();
    let (title, prefix) = account_picker_entry_title(entry, false);
    assert!(!title.starts_with("P1"));
    assert_eq!(prefix, 0);
    entry.recommendation_rank = u32::MAX as usize;
    assert_eq!(account_picker_entry_title(entry, false).0, title);
    for available in [0, 1, 4, 8, 12, 16, 24, 40, 80, 160] {
        assert!(picker_render_width(&picker, available) <= available);
        assert!(display_width(&pad_left_display(&title, available)) <= available);
    }
    let base = Color::Rgb(200, 200, 220);
    let bright = account_priority_color(base, 40, [40, 50, 90].into_iter());
    let middle = account_priority_color(base, 50, [40, 50, 90].into_iter());
    let dim = account_priority_color(base, 90, [40, 50, 90].into_iter());
    assert_eq!(bright, base);
    assert_eq!(middle, Color::Rgb(160, 160, 176));
    assert_eq!(dim, Color::Rgb(120, 120, 132));
    let indexed = Color::Indexed(239);
    assert_eq!(
        account_priority_color(indexed, 0, [0, 1].into_iter()),
        indexed
    );
    assert_ne!(
        account_priority_color(indexed, 1, [0, 1].into_iter()),
        indexed
    );
    assert_ne!(bright, middle);
    assert_ne!(middle, dim);
    assert_eq!(
        bright,
        account_priority_color(base, 0, [0, 1, 2].into_iter())
    );
    assert_eq!(
        bright,
        account_priority_color(base, 90, [90, 90].into_iter())
    );
    assert!(model_picker_top_hint(&picker).unwrap().contains("Ctrl+R"));
}

#[test]
fn format_elapsed_uses_whole_seconds_below_one_minute() {
    assert_eq!(format_elapsed(0.0), "0s");
    assert_eq!(format_elapsed(1.2), "1s");
    assert_eq!(format_elapsed(59.9), "59s");
    assert_eq!(format_elapsed(61.2), "1m 1s");
}

#[test]
fn fallback_route_details_are_warning_limited() {
    assert!(route_detail_is_limited(
        "https://mkp-api.fptcloud.com; fallback: static provider model list"
    ));
    assert_eq!(picker_row_marker(true, false, true), "⚠");
    assert_eq!(picker_row_marker(false, false, true), "⚠");
}

#[test]
fn selected_fallback_model_shows_warning_notice() {
    let mut picker = sample_picker();
    picker.entries[0].options[0].detail =
        "https://mkp-api.fptcloud.com; fallback: static provider model list".to_string();

    let (notice, warning) = selected_route_notice_text(&picker, picker.entries[0].active_option())
        .expect("fallback model should show a warning notice");

    assert!(warning);
    assert!(notice.starts_with("⚠ "));
    assert!(notice.contains("fallback: static provider model list"));
}

fn sample_picker() -> crate::tui::InlineInteractiveState {
    crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        filtered: vec![0],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
        entries: vec![crate::tui::PickerEntry {
            name: "gpt-5.4".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "openai".to_string(),
                api_method: "oauth".to_string(),
                available: true,
                detail: String::new(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Model,
            selected_option: 0,
            is_current: true,
            is_default: false,
            is_favorite: false,
            is_memory_model: false,
            recommended: true,
            recommendation_rank: 0,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        }],
    }
}

#[test]
fn model_suggestions_align_provider_and_method_columns() {
    let mut picker = sample_picker();
    picker.entries[0].is_default = true;
    let mut other = picker.entries[0].clone();
    other.name = "模型 (minimal)".into();
    other.is_current = false;
    other.is_default = false;
    other.recommended = false;
    other.created_date = Some("Sep 2026".into());
    other.options[0].provider = "Anthropic".into();
    other.options[0].api_method = "openai-api-key".into();
    picker.entries.push(other);
    picker.filtered.push(1);
    for preview in [false, true] {
        picker.preview = preview;
        let lines = model_suggestion_lines(&picker, 3);
        let texts: Vec<String> = lines
            .iter()
            .take(2)
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let column =
            |row: usize, label: &str| display_width(&texts[row][..texts[row].find(label).unwrap()]);
        assert_eq!(column(0, "openai"), column(1, "Anthropic"));
        assert_eq!(column(0, "oauth"), column(1, "api key"));
        assert!(texts[0].contains("default current"));
        assert!(texts[1].contains("Sep 2026"));
    }
}

#[test]
fn model_suggestions_measure_only_visible_filtered_rows() {
    let mut picker = sample_picker();
    let mut hidden = picker.entries[0].clone();
    hidden.name = "x".repeat(200);
    hidden.options[0].provider = "y".repeat(200);
    picker.entries.push(hidden);
    let baseline = model_suggestion_lines(&picker, 1);
    picker.filtered.push(1);
    let lines = model_suggestion_lines(&picker, 1);
    // The only difference is the remaining-results indicator.
    assert_eq!(
        lines[0].spans[..lines[0].spans.len() - 1],
        baseline[0].spans
    );
    picker.filtered = vec![1, 0];
    picker.selected = 1;
    let scrolled = model_suggestion_lines(&picker, 1);
    assert_eq!(
        scrolled[0].spans[..scrolled[0].spans.len() - 1],
        baseline[0].spans
    );
    assert!(model_suggestion_lines(&picker, 0).is_empty());
}

fn sample_account_picker(mixed_providers: bool) -> crate::tui::InlineInteractiveState {
    let mut models = vec![crate::tui::PickerEntry {
        name: "work".to_string(),
        options: vec![crate::tui::PickerOption {
            provider: "Claude".to_string(),
            api_method: "active".to_string(),
            available: true,
            detail: String::new(),
            estimated_reference_cost_micros: None,
        }],
        action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
            provider_id: "claude".to_string(),
            label: "work".to_string(),
        }),
        selected_option: 0,
        is_current: true,
        is_default: false,
        is_favorite: false,
        is_memory_model: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }];

    if mixed_providers {
        models.push(crate::tui::PickerEntry {
            name: "personal".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "OpenAI".to_string(),
                api_method: "saved".to_string(),
                available: true,
                detail: String::new(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                provider_id: "openai".to_string(),
                label: "personal".to_string(),
            }),
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            is_memory_model: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        });
    }

    crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Account,
        filtered: (0..models.len()).collect(),
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
        entries: models,
    }
}

fn sample_agent_target_picker() -> crate::tui::InlineInteractiveState {
    crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        filtered: vec![0],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
        entries: vec![crate::tui::PickerEntry {
            name: "Swarm / subagent".to_string(),
            options: vec![crate::tui::PickerOption {
                provider: "gpt-5 default".to_string(),
                api_method: "agents.swarm_model".to_string(),
                available: true,
                detail: "/agents swarm".to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm),
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            is_memory_model: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        }],
    }
}

#[test]
fn picker_row_marker_uses_explicit_unavailable_marker() {
    assert_eq!(picker_row_marker(true, true, false), "×");
    assert_eq!(picker_row_marker(false, true, false), "×");
    // Limited routes keep their warning marker even when selected, so the
    // fallback/limited signal never disappears while navigating.
    assert_eq!(picker_row_marker(true, false, true), "⚠");
    assert_eq!(picker_row_marker(false, false, true), "⚠");
    assert_eq!(picker_row_marker(true, false, false), "▸");
    assert_eq!(picker_row_marker(false, false, false), " ");
}

#[test]
fn route_detail_display_text_prefixes_unavailable_reason() {
    assert_eq!(
        route_detail_display_text("credentials expired", true).as_deref(),
        Some("unavailable · credentials expired")
    );
    assert_eq!(
        route_detail_display_text("no matching configured provider route", true).as_deref(),
        Some("unavailable · no matching configured provider route")
    );
    assert_eq!(
        route_detail_display_text("", true).as_deref(),
        Some("unavailable")
    );
    assert_eq!(route_detail_display_text("", false), None);
    assert_eq!(
        route_detail_display_text("catalog still loading", false).as_deref(),
        Some("catalog still loading")
    );
}

#[test]
fn selected_model_route_notice_explains_unavailable_and_limited_routes() {
    let mut picker = sample_picker();
    picker.entries[0].options[0].available = false;
    picker.entries[0].options[0].detail = "legacy Bedrock model".to_string();
    let notice = selected_route_notice_text(&picker, picker.entries[0].active_option());
    assert_eq!(
        notice
            .as_ref()
            .map(|(text, warning)| (text.as_str(), *warning)),
        Some(("× unavailable · legacy Bedrock model", true))
    );

    picker.entries[0].options[0].available = true;
    picker.entries[0].options[0].detail = "ConverseStream · no tools".to_string();
    let notice = selected_route_notice_text(&picker, picker.entries[0].active_option());
    assert_eq!(
        notice
            .as_ref()
            .map(|(text, warning)| (text.as_str(), *warning)),
        Some(("⚠ ConverseStream · no tools", true))
    );
}

#[test]
fn picker_render_width_uses_intrinsic_content_width() {
    let picker = sample_picker();
    let width = picker_render_width(&picker, 120);
    assert!(
        width < 120,
        "model picker should fit content, not fill the window"
    );
    // Content-fit: marker + the widths of the model/provider/via columns.
    // Each column is at least as wide as its header label, then grows to
    // fit the widest row (the sample entry: "gpt-5.4 ★" / "openai" / OAuth).
    let model = display_width(picker_entry_display_name(&picker.entries[0]).as_str())
        .max(display_width(picker.primary_label()));
    let provider = display_width("openai").max(display_width(picker.secondary_label(false))) + 1;
    let via = display_width(api_method_display("oauth").as_str())
        .max(display_width(picker.tertiary_label()))
        + 1;
    assert_eq!(
        width,
        3 + model + provider + via,
        "model picker width should equal its content-fit column widths"
    );
}

#[test]
fn picker_render_area_centers_in_centered_mode() {
    let picker = sample_picker();
    let width = picker_render_width(&picker, 80) as u16;
    let area = Rect::new(5, 3, 80, 2);
    let horizontal_offset = area.width.saturating_sub(width) / 2;
    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y,
        width,
        height: area.height,
    };

    assert!(
        render_area.x > area.x,
        "content-fit picker should center when possible"
    );
    assert_eq!(render_area.width, width);
}

#[test]
fn model_picker_method_display_uses_user_friendly_labels() {
    assert_eq!(api_method_display("openai-oauth@work"), "oauth · work");
    assert_eq!(
        api_method_display("claude-oauth@personal"),
        "oauth · personal"
    );
    assert_eq!(api_method_display("openai-oauth"), "oauth");
    assert_eq!(api_method_display("openai-api-key"), "api key");
    assert_eq!(api_method_display("openai-compatible:comtegra"), "api key");
}

#[test]
fn oauth_row_displays_email_instead_of_account_label() {
    let mut picker = sample_picker();
    let route = &mut picker.entries[0].options[0];
    route.api_method = "openai-oauth@work".into();
    route.detail = "account work · full.person@example.com · expired".into();
    assert_eq!(
        route_method_display(route),
        "oauth · full.person@example.com"
    );
    route.detail = "account work · quota reached, contact support@example.com".into();
    assert_eq!(route_method_display(route), "oauth · work");
    route.detail = "[global] account work · full.person@example.com".into();
    assert!(route_method_display(route).contains("full.person@example.com"));
    route.api_method = "openai-api-key".into();
    assert_eq!(route_method_display(route), "api key");
}

#[test]
fn oauth_email_is_visible_in_unselected_suggestion_rows_and_width_scan() {
    let mut picker = sample_picker();
    picker.entries.push(picker.entries[0].clone());
    picker.filtered.push(1);
    picker.selected = 1;
    picker.entries[0].options[0].api_method = "claude-oauth@personal".into();
    picker.entries[0].options[0].detail = "account personal · full.person@example.com".into();
    let lines = model_suggestion_lines(&picker, 6);
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("full.person@example.com"))
    );
    let wide = picker_render_width(&picker, 160);
    assert!(wide >= display_width("oauth · full.person@example.com") + 16);
    for width in [24, 40, 60, 80, 160] {
        assert!(picker_render_width(&picker, width) <= width);
    }
    let shortened = truncate_display("oauth · full.person@example.com", 24);
    assert!(shortened.ends_with('…'));
    assert!(display_width(&shortened) <= 24);
}

#[test]
fn picker_entry_display_name_labels_recently_added_models_as_new() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.is_current = false;
    entry.options[0].detail = "recently added · https://llm.comtegra.cloud/v1".to_string();

    assert!(picker_entry_display_name(entry).contains(" new"));
}

#[test]
fn picker_entry_display_name_memory_mark_wins_over_recently_added_new_badge() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.is_current = false;
    entry.is_memory_model = true;
    entry.options[0].detail = "recently added · https://llm.comtegra.cloud/v1".to_string();

    let name = picker_entry_display_name(entry);
    assert!(name.contains(" 🧠"), "memory mark missing in {name:?}");
    assert!(
        !name.contains(" new"),
        "new badge should not hide the memory mark in {name:?}"
    );
}

#[test]
fn picker_entry_display_name_labels_recommended_even_when_current() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.is_current = true;
    entry.recommended = true;

    assert!(picker_entry_display_name(entry).contains("★"));
}

#[test]
fn picker_entry_display_name_labels_default_models_explicitly() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.is_default = true;

    assert!(picker_entry_display_name(entry).contains(" default"));
}

#[test]
fn model_picker_shows_default_shortcut_hint() {
    let picker = sample_picker();

    assert!(picker.shows_default_shortcut_hint());
}

#[test]
fn model_picker_keybind_hint_mentions_default_and_favorites() {
    let picker = sample_picker();
    let hint = model_picker_top_hint(&picker).expect("active model picker should show hint");

    assert!(hint.contains("Ctrl+O set default"));
    assert!(hint.contains("Ctrl+N favorite"));
    // The marking chord must advertise the verb so it reads as a toggle on
    // the highlighted row, not as opening another picker.
    assert!(hint.contains("Alt+M mark memory model"));
}

#[test]
fn swarm_agent_model_picker_permanently_links_to_swarm_prompt_command() {
    let mut picker = sample_picker();
    for entry in &mut picker.entries {
        entry.action = crate::tui::PickerAction::AgentModelChoice {
            target: crate::tui::AgentModelTarget::Swarm,
            clear_override: false,
        };
    }

    let hint = model_picker_top_hint(&picker).expect("swarm picker should show prompt hint");
    assert!(hint.contains("/swarm-prompt"));
    assert!(hint.contains("swarm routing"));
}

/// `/agents` must say which settings a choice changes and how to switch:
/// the hint follows the `[session]` / `[global]` tags on the rows.
#[test]
fn agents_pickers_name_their_scope_and_the_toggle() {
    let mut picker = sample_picker();
    for entry in &mut picker.entries {
        entry.action = crate::tui::PickerAction::AgentModelChoice {
            target: crate::tui::AgentModelTarget::Review,
            clear_override: false,
        };
        for option in &mut entry.options {
            option.detail = format!("[session] {}", option.detail);
        }
    }
    let hint = model_picker_top_hint(&picker).expect("session hint");
    assert!(hint.contains("THIS SESSION ONLY"), "{hint}");
    assert!(hint.contains("Ctrl+G global default"), "{hint}");

    for entry in &mut picker.entries {
        for option in &mut entry.options {
            option.detail = option.detail.replacen("[session]", "[global]", 1);
        }
    }
    let hint = model_picker_top_hint(&picker).expect("global hint");
    assert!(hint.contains("GLOBAL DEFAULT"), "{hint}");
    assert!(hint.contains("Ctrl+G this session"), "{hint}");

    // The role list carries the scope in its row names.
    let mut roles = sample_picker();
    for entry in &mut roles.entries {
        entry.name = "Swarm [global]".to_string();
        entry.action = crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm);
    }
    let hint = model_picker_top_hint(&roles).expect("role list hint");
    assert!(hint.contains("GLOBAL DEFAULT"), "{hint}");

    // The plain /model picker keeps its own key hint.
    let hint = model_picker_top_hint(&sample_picker()).expect("model hint");
    assert!(hint.contains("Ctrl+O set default"), "{hint}");
}

#[test]
fn picker_entry_display_name_prettifies_known_model_families() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.recommended = false;
    entry.is_current = false;
    entry.name = "claude-opus-4-8".to_string();
    assert_eq!(picker_entry_display_name(entry), "Claude Opus 4.8");

    entry.name = "gpt-5.5 (high)".to_string();
    entry.effort = Some("high".to_string());
    assert_eq!(picker_entry_display_name(entry), "GPT-5.5 (high)");
}

#[test]
fn picker_entry_display_name_keeps_unknown_and_namespaced_ids_verbatim() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.recommended = false;
    entry.is_current = false;
    for raw in [
        "deepseek-ai/DeepSeek-V3",
        "qwen3-coder-plus",
        "openai/gpt-5.5",
        "gpt-oss-120b",
        "GLM-5.1",
    ] {
        entry.name = raw.to_string();
        assert_eq!(picker_entry_display_name(entry), raw);
    }
}

#[test]
fn picker_entry_display_name_labels_favorites() {
    let mut picker = sample_picker();
    let entry = &mut picker.entries[0];
    entry.is_favorite = true;
    entry.recommended = true;

    assert!(picker_entry_display_name(entry).contains("♥"));
}

#[test]
fn account_picker_width_uses_compact_two_column_layout() {
    let picker = sample_account_picker(true);
    let width = picker_render_width(&picker, 120);
    assert!(width < 60, "account picker should stay compact");
    assert!(
        width >= 18,
        "account picker should still fit title and state"
    );
}

#[test]
fn account_picker_only_shows_provider_badges_when_needed() {
    let mixed = sample_account_picker(true);
    let single = sample_account_picker(false);

    assert!(account_picker_shows_provider_badge(&mixed));
    assert!(!account_picker_shows_provider_badge(&single));

    let (mixed_title, _) = account_picker_entry_title(&mixed.entries[0], true);
    let (single_title, _) = account_picker_entry_title(&single.entries[0], false);
    assert!(mixed_title.starts_with("Claude · "));
    assert_eq!(single_title, "work");
}

#[test]
fn account_picker_fits_full_email_and_clamps_to_narrow_terminals() {
    for mixed in [false, true] {
        let mut picker = sample_account_picker(mixed);
        picker.entries[0].name = "Claude Otter · personal.account@example.com".to_string();
        let (title, _) = account_picker_entry_title(&picker.entries[0], mixed);
        let width = picker_render_width(&picker, 120);
        assert!(width >= display_width(&title) + 3 + 7);
        for available in [16, 24, 40] {
            assert!(picker_render_width(&picker, available) <= available);
        }
    }
}

#[test]
fn agent_target_picker_uses_specific_column_labels() {
    let picker = sample_agent_target_picker();

    assert!(picker.is_agent_target_picker());
    assert_eq!(picker.primary_label(), "TARGET");
    assert_eq!(picker.secondary_label(false), "MODEL");
    assert_eq!(picker.tertiary_label(), "CONFIG");
    assert!(!picker.shows_default_shortcut_hint());
}
