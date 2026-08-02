use super::*;

#[test]
fn keybinding_edit_reports_as_live() {
    let report = summarize_toml_change(
        "[keybindings]\nscroll_up = \"ctrl+k\"\n",
        "[keybindings]\nscroll_up = \"ctrl+y\"\n",
    )
    .expect("changed key should produce a report");

    assert!(report.contains("`keybindings.scroll_up`"), "{report}");
    assert!(report.contains("\"ctrl+k\" -> \"ctrl+y\""), "{report}");
    assert!(report.contains("live now"), "{report}");
    assert!(report.contains("no restart needed"), "{report}");
}

#[test]
fn gateway_edit_reports_as_needing_restart() {
    let report = summarize_toml_change("[gateway]\nport = 7777\n", "[gateway]\nport = 8888\n")
        .expect("changed key should produce a report");

    assert!(report.contains("needs restart"), "{report}");
    assert!(
        report.contains("Restart required for: gateway.port"),
        "{report}"
    );
}

#[test]
fn comment_and_formatting_only_edits_report_nothing() {
    assert!(
        summarize_toml_change(
            "[display]\ncentered = true\n",
            "# a comment\n[display]\ncentered   = true\n",
        )
        .is_none(),
        "a semantically identical file should not claim a change"
    );
}

#[test]
fn added_and_removed_keys_are_reported() {
    let changes = diff_toml("[display]\ncentered = true\n", "[display]\nemoji = false\n");
    let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, vec!["display.centered", "display.emoji"]);

    let removed = &changes[0];
    assert_eq!(removed.before.as_deref(), Some("true"));
    assert_eq!(removed.after, None);

    let added = &changes[1];
    assert_eq!(added.before, None);
    assert_eq!(added.after.as_deref(), Some("false"));
}

#[test]
fn unparseable_previous_content_still_reports_the_new_values() {
    let report = summarize_toml_change("this is not = = toml", "[display]\ncentered = true\n")
        .expect("repairing a broken config should still report the resulting keys");
    assert!(report.contains("`display.centered`"), "{report}");
}

#[test]
fn mixed_edits_report_restart_only_for_the_restart_keys() {
    let report = summarize_toml_change(
        "[gateway]\nport = 7777\n\n[display]\ncentered = true\n",
        "[gateway]\nport = 8888\n\n[display]\ncentered = false\n",
    )
    .expect("report");

    assert!(
        report.contains("Restart required for: gateway.port"),
        "{report}"
    );
    assert!(
        report.contains("Other changes are already live"),
        "{report}"
    );
    assert_eq!(liveness_for_key("display.centered"), Liveness::Live);
}

#[test]
fn nested_tables_and_arrays_flatten_to_dotted_keys() {
    let changes = diff_toml(
        "[providers.mine]\nmodels = [\"a\"]\n",
        "[providers.mine]\nmodels = [\"a\", \"b\"]\n",
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].key, "providers.mine.models");
    assert_eq!(changes[0].liveness, Liveness::Live);
}

/// The restart-required list is a claim about how the code consumes each
/// section, so it must stay small and deliberate. If a section is added or
/// removed, that is a behavioural change to every config edit report and
/// should be an explicit decision rather than a drive-by edit.
#[test]
fn the_restart_required_list_is_the_reviewed_set() {
    assert_eq!(
        RESTART_REQUIRED_SECTIONS,
        &["gateway", "acp", "launch_hotkeys"],
        "changing which sections need a restart changes what users are told; \
         confirm the consuming code really snapshots the value at startup"
    );
}

/// Sections that are read through `config()` on every use are live by
/// definition. Spot-check the ones users change most often, so a careless
/// addition to the restart list cannot silently start telling people to
/// restart for a setting that already applies.
#[test]
fn commonly_edited_sections_are_live() {
    for key in [
        "keybindings.scroll_up",
        "display.centered",
        "features.thinking",
        "provider.openai_reasoning_effort",
        "agents.swarm_spawn_mode",
        "tools.profile",
        "websearch.engine",
        "notifications.enabled",
    ] {
        assert_eq!(
            liveness_for_key(key),
            Liveness::Live,
            "{key} is re-read through the config cache and should report as live"
        );
    }
}
