use super::*;
use unicode_width::UnicodeWidthStr;

/// Exercise the cache dispatcher, not just a hand-selected incremental offset:
/// appending/editing a DM must rewind to the start of its shared row.
fn assert_dm_rebuild_matches_full(
    before: Vec<DisplayMessage>,
    after: Vec<DisplayMessage>,
    width: u16,
) -> Arc<PreparedMessages> {
    let state = TestState {
        display_messages: before,
        ..Default::default()
    };
    let base = Arc::new(prepare::prepare_body(&state, width, false));
    let previous_count = state.display_messages.len();
    let state = TestState {
        display_messages: after,
        messages_version: 1,
        ..Default::default()
    };
    let (incremental, path) = prepare::build_body_from_base(
        &state,
        width,
        base,
        previous_count,
        0,
        state.display_messages.len(),
    );
    assert_ne!(path, "full", "test must exercise cache reuse");
    let full = prepare::prepare_body(&state, width, false);
    assert_eq!(incremental.wrapped_lines, full.wrapped_lines);
    assert_eq!(incremental.wrapped_plain_lines, full.wrapped_plain_lines);
    assert_eq!(incremental.raw_plain_lines, full.raw_plain_lines);
    assert_eq!(incremental.wrapped_copy_offsets, full.wrapped_copy_offsets);
    assert_eq!(incremental.user_prompt_texts, full.user_prompt_texts);
    assert_eq!(incremental.wrapped_user_indices, full.wrapped_user_indices);
    assert_eq!(
        incremental.wrapped_user_prompt_starts,
        full.wrapped_user_prompt_starts
    );
    assert_eq!(
        incremental.wrapped_user_prompt_ends,
        full.wrapped_user_prompt_ends
    );
    let boundaries = |body: &PreparedMessages| {
        body.message_boundaries
            .iter()
            .map(|b| (b.msg_hash, b.wrapped_len, b.raw_len, b.user_prompt_len))
            .collect::<Vec<_>>()
    };
    assert_eq!(boundaries(&incremental), boundaries(&full));
    let raw_map = |body: &PreparedMessages| {
        body.wrapped_line_map
            .iter()
            .map(|m| m.raw_line)
            .collect::<Vec<_>>()
    };
    assert_eq!(raw_map(&incremental), raw_map(&full));
    incremental
}

fn dm(sender: &str, body: &str) -> DisplayMessage {
    DisplayMessage::swarm(format!("DM from {sender}"), body)
}

fn token_rows(body: &PreparedMessages) -> Vec<&str> {
    body.wrapped_plain_lines
        .iter()
        .filter(|line| line.contains(" tok"))
        .map(String::as_str)
        .collect()
}

#[test]
fn compact_dm_await_started_is_hidden_but_finished_result_stays_visible() {
    let _guard = viewport_snapshot_test_lock();
    let before = vec![DisplayMessage::user("Wait for workers")];
    let mut after = before.clone();
    after.push(DisplayMessage::background_task(
        "🐝 **Swarm await started** · watching `fox`\n\nWaiting in the background.",
    ));
    let hidden = assert_dm_rebuild_matches_full(before, after.clone(), 100);
    assert_eq!(hidden.message_boundaries.len(), 2);
    assert_eq!(
        hidden.message_boundaries[0].wrapped_len,
        hidden.message_boundaries[1].wrapped_len
    );
    assert!(
        !hidden
            .wrapped_plain_lines
            .join("\n")
            .contains("Swarm await started")
    );
    let before = after.clone();
    after.push(DisplayMessage::background_task(
        "🐝 **Swarm await finished**\n\nAll members done.\n\nMember statuses:\n  ✓ fox (completed)",
    ));
    let finished = assert_dm_rebuild_matches_full(before, after, 100);
    let text = finished.wrapped_plain_lines.join("\n");
    assert!(text.contains("Swarm await"), "{text}");
    assert!(text.contains("completed"), "{text}");
}

#[test]
fn compact_dm_single_and_group_have_only_one_total_and_badge() {
    let _guard = viewport_snapshot_test_lock();
    for messages in [
        vec![dm("fox", "a".repeat(44).as_str())],
        vec![
            dm("fox", "a".repeat(44).as_str()),
            dm("sheep", "b".repeat(48).as_str()),
        ],
    ] {
        let total = messages
            .iter()
            .map(|message| crate::util::estimate_tokens(message.content.trim()))
            .sum();
        let icons = messages
            .iter()
            .map(|message| {
                crate::id::session_icon(
                    message
                        .title
                        .as_deref()
                        .unwrap()
                        .trim_start_matches("DM from "),
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let label = if messages.len() == 1 { "DM" } else { "DMs" };
        let state = TestState {
            display_messages: messages,
            ..Default::default()
        };
        let body = prepare::prepare_body(&state, 100, false);
        let rows = token_rows(&body);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].trim(),
            format!(
                "{label} · {icons} · {} ⊞",
                crate::util::format_approx_token_count(total)
            )
        );
    }
}

#[test]
fn compact_dm_group_chrome_stays_neutral_for_large_total() {
    let _guard = viewport_snapshot_test_lock();
    let messages = vec![dm("fox", &"a".repeat(52_000)), dm("sheep", "short")];
    let line = messages::render_agent_dm_run(&messages, 100);
    let neutral = Some(dim_color());
    let mut token_spans = 0;
    for span in &line.spans {
        if span.content.contains(" tok") {
            token_spans += 1;
            assert_ne!(span.style.fg, neutral, "large total must carry severity");
        } else if !span.content.trim().is_empty() {
            assert_eq!(span.style.fg, neutral, "chrome must stay neutral: {span:?}");
        }
    }
    assert_eq!(token_spans, 1, "only the aggregate total carries severity");
}

#[test]
fn compact_dm_prepend_merges_with_existing_first_dm() {
    let _guard = viewport_snapshot_test_lock();
    let before = vec![
        dm("sheep", "existing private message"),
        dm("owl", "another existing private message"),
        DisplayMessage::assistant("Visible tail"),
    ];
    let mut after = vec![dm("fox", "older private message")];
    after.extend(before.clone());
    let body = assert_dm_rebuild_matches_full(before, after, 100);
    let rows = token_rows(&body);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].matches(" tok").count(), 1);
    assert_eq!(rows[0].matches('⊞').count(), 1);
    assert!(rows[0].contains("DMs ·"));
    assert!(rows[0].contains(crate::id::session_icon("fox")));
    assert!(rows[0].contains(crate::id::session_icon("sheep")));
    assert!(rows[0].contains(crate::id::session_icon("owl")));
    assert!(!body.wrapped_plain_lines.join("\n").contains("private"));
}

#[test]
fn compact_dm_consecutive_append_matches_full_prepare() {
    let _guard = viewport_snapshot_test_lock();
    let mut messages = vec![
        DisplayMessage::user("Keep the prompt"),
        dm("fox", "private alpha"),
    ];
    for sender in ["cow", "owl"] {
        let before = messages.clone();
        messages.push(dm(sender, "private beta"));
        let body = assert_dm_rebuild_matches_full(before, messages.clone(), 100);
        let rows = token_rows(&body);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].matches(" tok").count(), 1);
        assert_eq!(rows[0].matches('⊞').count(), 1);
        assert!(rows[0].contains("DMs ·"));
        assert!(rows[0].contains(crate::id::session_icon(sender)));
        assert!(!body.wrapped_plain_lines.join("\n").contains("private"));
        assert_eq!(body.message_boundaries.len(), messages.len());
        let first_dm = &body.message_boundaries[1];
        for boundary in &body.message_boundaries[2..] {
            assert_eq!(boundary.wrapped_len, first_dm.wrapped_len);
            assert_eq!(boundary.raw_len, first_dm.raw_len);
        }
    }
}

#[test]
fn compact_dm_body_edit_matches_full_prepare() {
    let _guard = viewport_snapshot_test_lock();
    let before = vec![
        DisplayMessage::user("Preserved"),
        dm("fox", "short"),
        dm("cow", "old"),
    ];
    let mut after = before.clone();
    after[2].content = "edited secret body ".repeat(80);
    let body = assert_dm_rebuild_matches_full(before.clone(), after.clone(), 100);
    let old_state = TestState {
        display_messages: before,
        ..Default::default()
    };
    let old = prepare::prepare_body(&old_state, 100, false);
    assert_ne!(
        token_rows(&body),
        token_rows(&old),
        "token estimate must update after body edit"
    );
    let expected = crate::util::format_approx_token_count(
        after[1..]
            .iter()
            .map(|message| crate::util::estimate_tokens(message.content.trim()))
            .sum(),
    );
    assert!(token_rows(&body)[0].contains(&expected));
    assert!(
        !body
            .wrapped_plain_lines
            .join("\n")
            .contains("edited secret")
    );
}

#[test]
fn compact_dm_assistant_breaks_group_matches_full_prepare() {
    let _guard = viewport_snapshot_test_lock();
    let before = vec![dm("fox", "hidden first"), dm("cow", "hidden second")];
    let mut after = before.clone();
    after.push(DisplayMessage::assistant("Visible assistant separator"));
    after.push(dm("owl", "hidden third"));
    let body = assert_dm_rebuild_matches_full(before, after, 100);
    let rows = token_rows(&body);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].matches(" tok").count(), 1);
    assert!(rows[0].contains("DMs ·"));
    assert!(rows[1].contains("DM ·"));
    assert_eq!(rows[1].matches(" tok").count(), 1);
    let text = body.wrapped_plain_lines.join("\n");
    assert!(text.find(rows[0]).unwrap() < text.find("Visible assistant separator").unwrap());
    assert!(text.find("Visible assistant separator").unwrap() < text.rfind(rows[1]).unwrap());
}

#[test]
fn compact_dm_expanded_message_excluded_from_group_matches_full_prepare() {
    let _guard = viewport_snapshot_test_lock();
    let collapsed = jcode_tui_messages::encode_collapsible_swarm_content(
        "expanded summary",
        "Visible expanded DM body",
    );
    let expanded = jcode_tui_messages::toggle_collapsible_swarm_content(&collapsed).unwrap();
    let before = vec![
        dm("fox", "hidden left"),
        dm("cow", &collapsed),
        dm("owl", "hidden right"),
    ];
    let mut after = before.clone();
    after[1].content = expanded;
    let body = assert_dm_rebuild_matches_full(before, after, 100);
    let text = body.wrapped_plain_lines.join("\n");
    assert!(text.contains("Visible expanded DM body"), "{text}");
    assert!(!text.contains("hidden left"));
    assert!(!text.contains("hidden right"));
    let rows = token_rows(&body);
    assert!(
        rows.len() >= 2,
        "collapsed neighbors must not share a row across expanded DM: {text}"
    );
    assert!(
        rows.iter().all(|row| row.matches(" tok").count() <= 1),
        "{rows:?}"
    );
}

#[test]
fn compact_dm_narrow_unicode_rows_match_full_prepare_without_wrapping() {
    let _guard = viewport_snapshot_test_lock();
    for width in [5, 6, 8, 12, 18, 24, 34] {
        let before = vec![dm("fox", "秘密 🦊 e\u{301}")];
        let mut after = before.clone();
        after.push(dm("cow", "追加の本文 🐄"));
        after.push(dm("owl", "more hidden Unicode 🦉"));
        let body = assert_dm_rebuild_matches_full(before, after, width);
        let rows: Vec<_> = body
            .wrapped_plain_lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(
            rows.len(),
            1,
            "DM chrome must remain one row at width {width}: {rows:?}"
        );
        assert!(
            UnicodeWidthStr::width(rows[0].as_str()) <= width as usize,
            "Unicode cells overflow at width {width}: {rows:?}"
        );
        assert!(!rows[0].contains("秘密"));
        assert!(!rows[0].contains("追加"));
    }
}
