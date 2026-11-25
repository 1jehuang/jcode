use super::*;

fn test_inputs_registry(remote: bool) -> Vec<KnownHotkey> {
    let model_switch = ModelSwitchKeys {
        next: key(KeyCode::Tab, KeyModifiers::CONTROL),
        prev: Some(key(
            KeyCode::Tab,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )),
    };
    let effort = EffortSwitchKeys {
        increase: key(KeyCode::Right, KeyModifiers::ALT),
        decrease: key(KeyCode::Left, KeyModifiers::ALT),
    };
    let speed = SpeedSwitchKeys {
        increase: Some(key(KeyCode::Up, KeyModifiers::ALT)),
        decrease: Some(key(KeyCode::Down, KeyModifiers::ALT)),
    };
    let scroll = ScrollKeys {
        up: key(
            KeyCode::Char('k'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        down: key(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        up_fallback: None,
        down_fallback: None,
        page_up: alt('u'),
        page_down: alt('d'),
        prompt_up: ctrl('k'),
        prompt_down: ctrl('j'),
        bookmark: ctrl('g'),
        to_bottom: vec![key(KeyCode::End, KeyModifiers::CONTROL)],
    };
    let centered = CenteredToggleKeys {
        toggle: Some(alt('c')),
    };
    let toggles = crate::tui::keybind::load_toggle_keys();
    let workspace = WorkspaceNavigationKeys {
        left: vec![alt('h')],
        down: vec![alt('j')],
        up: vec![alt('k')],
        right: vec![alt('l')],
    };
    let dictation = OptionalBinding::default();
    // Bind the optional chords in the fixture so coverage tests can verify
    // they flow through build_registry when configured.
    let new_terminal = OptionalBinding {
        binding: Some(key(KeyCode::Enter, KeyModifiers::ALT)),
        label: Some("Alt+Enter".to_string()),
    };
    let open_resume = OptionalBinding {
        binding: Some(alt('r')),
        label: Some("Alt+R".to_string()),
    };
    let fallback_switch = OptionalBinding {
        binding: Some(ctrl('y')),
        label: Some("Ctrl+Y".to_string()),
    };
    let voice_input = OptionalBinding {
        binding: Some(key(KeyCode::Char(' '), KeyModifiers::CONTROL)),
        label: Some("Ctrl+Space".to_string()),
    };
    build_registry(&RegistryInputs {
        model_switch: &model_switch,
        effort: &effort,
        speed: &speed,
        scroll: &scroll,
        centered: &centered,
        toggles: &toggles,
        workspace: &workspace,
        dictation: &dictation,
        new_terminal: &new_terminal,
        open_resume: &open_resume,
        voice_input: &voice_input,
        fallback_switch: &fallback_switch,
        remote,
        queue_mode: false,
    })
}

/// Issue #1500: the hint must match what `send_action` actually does for
/// each `queue_mode` value.
#[test]
fn alternate_enter_description_follows_queue_mode() {
    assert!(alternate_enter_description(false).contains("queue this message"));
    assert!(alternate_enter_description(true).contains("bypassing queue mode"));
    let registry = test_inputs_registry(false);
    let info =
        lookup(&registry, false, KeyCode::Enter, KeyModifiers::CONTROL).expect("ctrl+enter known");
    assert!(info.description.contains("queue this message"));
}

#[test]
fn lookup_finds_configured_ctrl_p_auto_poke() {
    let registry = test_inputs_registry(false);
    let info =
        lookup(&registry, true, KeyCode::Char('p'), KeyModifiers::CONTROL).expect("ctrl+p known");
    assert_eq!(info.action, "auto_poke_toggle");
}

#[test]
fn lookup_prefers_kill_line_over_prompt_jump_with_draft() {
    let registry = test_inputs_registry(false);
    let with_draft =
        lookup(&registry, false, KeyCode::Char('k'), KeyModifiers::CONTROL).expect("ctrl+k known");
    assert_eq!(with_draft.action, "kill_to_end");

    let empty =
        lookup(&registry, true, KeyCode::Char('k'), KeyModifiers::CONTROL).expect("ctrl+k known");
    assert_eq!(empty.action, "prompt_jump_up");
}

#[test]
fn lookup_reports_copy_viewport_for_ctrl_a_on_empty_input() {
    let registry = test_inputs_registry(false);
    let empty =
        lookup(&registry, true, KeyCode::Char('a'), KeyModifiers::CONTROL).expect("ctrl+a known");
    assert_eq!(empty.action, "copy_viewport");

    let with_draft =
        lookup(&registry, false, KeyCode::Char('a'), KeyModifiers::CONTROL).expect("ctrl+a known");
    assert_eq!(with_draft.action, "input_home");
}

#[test]
fn workspace_bindings_only_register_in_remote_mode() {
    let local = test_inputs_registry(false);
    assert!(!local.iter().any(|k| k.action.starts_with("workspace_")));

    let remote = test_inputs_registry(true);
    assert!(remote.iter().any(|k| k.action == "workspace_left"));
}

#[test]
fn nearest_suggests_same_key_one_modifier_away() {
    let registry = test_inputs_registry(false);
    let near = nearest_hotkey(
        &registry,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )
    .expect("suggestion for ctrl+shift+p");
    assert_eq!(near.action, "auto_poke_toggle");
}

#[test]
fn nearest_suggests_same_letter_under_other_modifier() {
    let registry = test_inputs_registry(false);
    // Ctrl+M is unbound; Alt+M toggles the side panel.
    let near = nearest_hotkey(&registry, KeyCode::Char('m'), KeyModifiers::CONTROL)
        .expect("suggestion for ctrl+m");
    assert_eq!(near.action, "side_panel_toggle");
}

#[test]
fn nearest_returns_none_for_unrelated_chords() {
    let registry = test_inputs_registry(false);
    assert!(nearest_hotkey(&registry, KeyCode::Char(';'), KeyModifiers::CONTROL).is_none());
}

#[test]
fn unknown_chord_message_mentions_chord_and_suggestion() {
    let registry = test_inputs_registry(false);
    let msg = unknown_chord_message(
        &registry,
        KeyCode::Char('p'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(msg.contains("Ctrl+Shift+P"), "{msg}");
    assert!(msg.contains("Ctrl+P"), "{msg}");
    assert!(msg.contains("auto-poke"), "{msg}");

    let fallback = unknown_chord_message(&registry, KeyCode::Char(';'), KeyModifiers::CONTROL);
    assert!(fallback.contains("isn't bound"), "{fallback}");
    assert!(fallback.contains("/help"), "{fallback}");
}

#[test]
fn familiarity_thresholds() {
    let now = 10_000_000u64;
    let fresh = UsageStat {
        uses: 0,
        last_used_unix: 0,
    };
    assert!(is_unfamiliar(&fresh, now));

    let learning = UsageStat {
        uses: FAMILIAR_USES - 1,
        last_used_unix: now,
    };
    assert!(is_unfamiliar(&learning, now));

    let familiar = UsageStat {
        uses: FAMILIAR_USES,
        last_used_unix: now,
    };
    assert!(!is_unfamiliar(&familiar, now));

    let stale = UsageStat {
        uses: 50,
        last_used_unix: now - STALE_SECS,
    };
    assert!(is_unfamiliar(&stale, now));
}

/// Drift guard: every configurable keybinding in the canonical registry
/// must be represented in the hotkey-feedback registry (as one of the
/// action ids the builder can emit). If this fails, someone added a new
/// keybinding without teaching the feedback system about it, which would
/// make `note_unrecognized_hotkey` falsely claim the chord "isn't bound".
#[test]
fn registry_covers_every_canonical_keybinding_default() {
    use crate::tui::keybind::KEYBINDING_DEFAULTS;

    // Canonical ids -> the action id(s) the feedback registry uses.
    // A None mapping documents an intentional exclusion.
    let mapping: &[(&str, Option<&[&str]>)] = &[
        ("scroll_up", Some(&["scroll_up"])),
        ("scroll_down", Some(&["scroll_down"])),
        ("scroll_page_up", Some(&["scroll_page_up"])),
        ("scroll_page_down", Some(&["scroll_page_down"])),
        ("model_switch_next", Some(&["model_switch_next"])),
        ("model_switch_prev", Some(&["model_switch_prev"])),
        ("fallback_switch", Some(&["fallback_switch"])),
        ("effort_increase", Some(&["effort_increase"])),
        ("effort_decrease", Some(&["effort_decrease"])),
        ("speed_increase", Some(&["speed_increase"])),
        ("speed_decrease", Some(&["speed_decrease"])),
        ("centered_toggle", Some(&["centered_toggle"])),
        ("auto_poke_toggle", Some(&["auto_poke_toggle"])),
        ("scroll_prompt_up", Some(&["prompt_jump_up"])),
        ("scroll_prompt_down", Some(&["prompt_jump_down"])),
        ("scroll_bookmark", Some(&["scroll_bookmark"])),
        ("scroll_to_bottom", Some(&["scroll_to_bottom"])),
        ("scroll_up_fallback", Some(&["scroll_up"])),
        ("scroll_down_fallback", Some(&["scroll_down"])),
        ("workspace_left", Some(&["workspace_left"])),
        ("workspace_down", Some(&["workspace_down"])),
        ("workspace_up", Some(&["workspace_up"])),
        ("workspace_right", Some(&["workspace_right"])),
        ("new_terminal", Some(&["new_terminal"])),
        ("open_resume", Some(&["open_resume"])),
        ("voice_input", Some(&["voice_input"])),
    ];

    let registry = test_inputs_registry(true);
    let registry_actions: std::collections::HashSet<&str> =
        registry.iter().map(|k| k.action).collect();

    for default in KEYBINDING_DEFAULTS {
        let entry = mapping.iter().find(|(id, _)| *id == default.id);
        let Some((_, actions)) = entry else {
            panic!(
                "keybinding default `{}` is not mapped in the hotkey-feedback \
                     registry test. Add it to build_registry (so users get \
                     press-feedback and never a false \"isn't bound\" notice) and \
                     record the mapping here.",
                default.id
            );
        };
        if let Some(actions) = actions {
            for action in *actions {
                assert!(
                    registry_actions.contains(action),
                    "mapped action `{action}` for keybinding default `{}` is \
                         missing from build_registry output",
                    default.id
                );
            }
        }
    }
}

#[test]
fn hotkeys_listing_groups_chords_and_splits_by_usage() {
    let registry = test_inputs_registry(true);
    let mut usage = HotkeyUsageState::default();
    usage.actions.insert(
        "prompt_jump_up".to_string(),
        UsageStat {
            uses: 7,
            last_used_unix: 10_000_000,
        },
    );
    let listing = render_hotkeys_listing(&registry, &usage, 10_000_000);

    // Used action lands in the muscle-memory section with its count.
    assert!(listing.contains("jump to the previous prompt"), "{listing}");
    assert!(listing.contains("(7 uses)"), "{listing}");
    // Unused actions land in the try-these section.
    assert!(listing.contains("Not yet used"), "{listing}");
    assert!(listing.contains("toggle the scroll bookmark"), "{listing}");
    // Stale familiar actions get flagged.
    let mut stale_usage = HotkeyUsageState::default();
    stale_usage.actions.insert(
        "prompt_jump_up".to_string(),
        UsageStat {
            uses: 7,
            last_used_unix: 0,
        },
    );
    let stale = render_hotkeys_listing(&registry, &stale_usage, 10_000_000);
    assert!(stale.contains("not recently"), "{stale}");
}

/// Drift guard for the pane/mode toggles that live outside
/// KEYBINDING_DEFAULTS (ToggleKeys fields). Uses the real loader so a new
/// ToggleKeys field without registry coverage fails here.
#[test]
fn registry_covers_every_toggle_binding() {
    let registry = test_inputs_registry(false);
    let toggles = crate::tui::keybind::load_toggle_keys();
    let toggle_bindings: &[(&str, Option<&KeyBinding>)] = &[
        ("auto_poke_toggle", toggles.auto_poke.binding()),
        ("side_panel_toggle", toggles.side_panel.binding()),
        ("copy_selection_toggle", toggles.copy_selection.binding()),
        ("diagram_pane_toggle", toggles.diagram_pane.binding()),
        (
            "diagram_pane_visibility_toggle",
            toggles.diagram_pane_visibility.binding(),
        ),
        (
            "typing_scroll_lock_toggle",
            toggles.typing_scroll_lock.binding(),
        ),
        ("diff_mode_cycle", toggles.diff_mode_cycle.binding()),
        ("commit_log", toggles.commit_log.binding()),
        ("info_widget_toggle", toggles.info_widget.binding()),
        ("swarm_panel_focus", toggles.swarm_panel_focus.binding()),
    ];
    for (name, binding) in toggle_bindings {
        let Some(binding) = binding else { continue };
        assert!(
            lookup(&registry, true, binding.code, binding.modifiers).is_some(),
            "toggle `{name}` chord {:?} is not resolvable in the feedback registry",
            binding
        );
    }
}
