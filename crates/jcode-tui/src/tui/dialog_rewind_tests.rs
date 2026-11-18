use super::*;
use crate::protocol::RewindTargetKind;

fn checkpoint(id: &str, prompt: &str, files: usize) -> RewindCheckpoint {
    RewindCheckpoint {
        target_kind: RewindTargetKind::UserPrompt,
        id: id.to_string(),
        message_index: 0,
        prompt: prompt.to_string(),
        timestamp: None,
        can_restore_files: files > 0,
        changed_file_count: files,
    }
}

fn key(dialog: &mut Dialog, code: KeyCode) -> DialogKey {
    dialog.handle_key(code, KeyModifiers::empty())
}

#[test]
fn rewind_dialog_selects_stable_checkpoint_and_explicit_modes() {
    for (index, mode) in [
        RewindMode::ConversationAndFiles,
        RewindMode::ConversationOnly,
        RewindMode::FilesOnly,
    ]
    .into_iter()
    .enumerate()
    {
        let mut dialog = Dialog::rewind_loading();
        dialog.set_rewind_checkpoints(vec![
            checkpoint("older", "first", 0),
            checkpoint("latest", "second\nfull prompt", 2),
        ]);
        assert_eq!(
            dialog.item_summaries(),
            vec!["first · You", "second · You", "(current)"]
        );
        key(&mut dialog, KeyCode::Up);
        assert_eq!(key(&mut dialog, KeyCode::Enter), DialogKey::Handled);
        assert_eq!(
            dialog.item_summaries(),
            vec![
                "Restore conversation and files",
                "Restore conversation only",
                "Restore files only",
                "Never mind"
            ]
        );
        assert_eq!(
            key(&mut dialog, KeyCode::Char(char::from(b'1' + index as u8))),
            DialogKey::Rewind {
                checkpoint_id: "latest".to_string(),
                mode
            }
        );
        assert!(dialog.pending_rewind);
        assert_eq!(
            key(&mut dialog, KeyCode::Enter),
            DialogKey::Handled,
            "pending restore cannot be submitted twice"
        );
        assert_eq!(
            key(&mut dialog, KeyCode::Esc),
            DialogKey::Handled,
            "pending restore cannot abandon its result"
        );
    }
}

#[test]
fn rewind_dialog_omits_files_modes_without_tracked_changes() {
    for (can_restore, count) in [(false, 0), (true, 0), (false, 2)] {
        let mut row = checkpoint("target", "original", count);
        row.can_restore_files = can_restore;
        let mut dialog = Dialog::rewind_loading();
        dialog.set_rewind_checkpoints(vec![row]);
        key(&mut dialog, KeyCode::Up);
        key(&mut dialog, KeyCode::Enter);
        assert_eq!(
            dialog.item_summaries(),
            vec!["Restore conversation only", "Never mind"]
        );
        assert_eq!(
            key(&mut dialog, KeyCode::Enter),
            DialogKey::Rewind {
                checkpoint_id: "target".to_string(),
                mode: RewindMode::ConversationOnly
            }
        );
    }
}

#[test]
fn rewind_dialog_back_cancel_and_full_prompt_search() {
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(vec![checkpoint(
        "target",
        "first line\nneedle in second",
        1,
    )]);
    key(&mut dialog, KeyCode::Char('n'));
    assert_eq!(dialog.stack.last().unwrap().visible().len(), 1);
    key(&mut dialog, KeyCode::Enter);
    assert!(matches!(
        dialog.stack.last().unwrap().route,
        Route::RewindModes(_)
    ));
    assert_eq!(key(&mut dialog, KeyCode::Esc), DialogKey::Handled);
    assert_eq!(
        key(&mut dialog, KeyCode::Esc),
        DialogKey::Handled,
        "clear search before close"
    );
    assert_eq!(key(&mut dialog, KeyCode::Esc), DialogKey::Close);
}

#[test]
fn rewind_dialog_renders_empty_and_small_mode_views() {
    use ratatui::backend::TestBackend;
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(Vec::new());
    let mut terminal = ratatui::Terminal::new(TestBackend::new(36, 10)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("Search messages…"));
    assert!(!rendered.contains("Search skills…"));
    assert_eq!(key(&mut dialog, KeyCode::Enter), DialogKey::Close);
    dialog.set_rewind_checkpoints(vec![checkpoint("target", "日本語 checkpoint", 3)]);
    key(&mut dialog, KeyCode::Up);
    key(&mut dialog, KeyCode::Enter);
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
}

#[test]
fn rewind_current_is_default_and_enter_closes_without_restoring() {
    for checkpoints in [Vec::new(), vec![checkpoint("only", "real prompt", 1)]] {
        let mut dialog = Dialog::rewind_loading();
        dialog.set_rewind_checkpoints(checkpoints);
        let view = dialog.stack.last().unwrap();
        assert_eq!(view.items[view.selected].label, "(current)");
        assert_eq!(key(&mut dialog, KeyCode::Enter), DialogKey::Close);
        assert!(!dialog.pending_rewind);
    }
}

#[test]
fn rewind_never_mind_and_escape_return_to_the_selected_prompt() {
    for cancel in [KeyCode::Esc, KeyCode::Enter] {
        let mut dialog = Dialog::rewind_loading();
        dialog.set_rewind_checkpoints(vec![
            checkpoint("older", "first", 0),
            checkpoint("newer", "second", 1),
        ]);
        key(&mut dialog, KeyCode::Up);
        key(&mut dialog, KeyCode::Enter);
        if cancel == KeyCode::Enter {
            key(&mut dialog, KeyCode::End);
            assert_eq!(
                dialog.stack.last().unwrap().items.last().unwrap().label,
                "Never mind"
            );
        }
        assert_eq!(key(&mut dialog, cancel), DialogKey::Handled);
        assert!(!dialog.pending_rewind);
        assert_eq!(dialog.stack.len(), 1);
        let view = dialog.stack.last().unwrap();
        assert_eq!(view.items[view.selected].label, "second");
        key(&mut dialog, KeyCode::Down);
        assert_eq!(key(&mut dialog, KeyCode::Enter), DialogKey::Close);
    }
}

#[test]
fn rewind_legacy_message_id_has_a_conversation_only_restore() {
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(vec![checkpoint(
        "message:immutable-user-id",
        "legacy prompt",
        0,
    )]);
    key(&mut dialog, KeyCode::Up);
    key(&mut dialog, KeyCode::Enter);
    assert_eq!(
        dialog.item_summaries(),
        vec!["Restore conversation only", "Never mind"]
    );
    assert_eq!(
        key(&mut dialog, KeyCode::Enter),
        DialogKey::Rewind {
            checkpoint_id: "message:immutable-user-id".to_string(),
            mode: RewindMode::ConversationOnly,
        }
    );
}

#[test]
fn rewind_long_list_navigation_keeps_chronological_identity() {
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(
        (0..40)
            .map(|index| checkpoint(&format!("id-{index}"), &format!("prompt-{index}"), 0))
            .collect(),
    );
    key(&mut dialog, KeyCode::Up);
    assert_eq!(dialog.stack.last().unwrap().selected, 39);
    key(&mut dialog, KeyCode::PageUp);
    assert_eq!(dialog.stack.last().unwrap().selected, 29);
    key(&mut dialog, KeyCode::Home);
    key(&mut dialog, KeyCode::Down);
    key(&mut dialog, KeyCode::Enter);
    assert_eq!(
        key(&mut dialog, KeyCode::Enter),
        DialogKey::Rewind {
            checkpoint_id: "id-1".to_string(),
            mode: RewindMode::ConversationOnly,
        }
    );
}

#[test]
fn rewind_identical_text_keeps_role_identity_and_assistant_confirmation() {
    let mut assistant = checkpoint("assistant:reply", "same text\nassistant needle", 9);
    assistant.target_kind = RewindTargetKind::AssistantMessage;
    assistant.timestamp = Some("2026-10-23".to_string());
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(vec![
        checkpoint("message:prompt", "same text", 1),
        assistant,
    ]);
    assert_eq!(
        dialog.item_summaries(),
        vec![
            "same text · You",
            "same text · Assistant · 2026-10-23",
            "(current)"
        ]
    );
    assert_eq!(dialog.stack.last().unwrap().selected, 2);
    key(&mut dialog, KeyCode::Up);
    key(&mut dialog, KeyCode::Enter);
    let view = dialog.stack.last().unwrap();
    assert_eq!(
        dialog.item_summaries(),
        vec!["Restore conversation only", "Never mind"]
    );
    assert!(
        view.notes
            .iter()
            .any(|note| note.contains("after this assistant message"))
    );
    assert!(
        view.notes.iter().any(
            |note| note == "Keeps this assistant message. Files and your draft stay unchanged."
        )
    );
    assert!(
        !view
            .notes
            .iter()
            .any(|note| note.contains("original prompt"))
    );
    key(&mut dialog, KeyCode::End);
    key(&mut dialog, KeyCode::Enter);
    assert_eq!(dialog.stack.last().unwrap().selected, 1);
    key(&mut dialog, KeyCode::Enter);
    assert_eq!(
        key(&mut dialog, KeyCode::Char('1')),
        DialogKey::Rewind {
            checkpoint_id: "assistant:reply".to_string(),
            mode: RewindMode::ConversationOnly,
        }
    );
}

#[test]
fn rewind_assistant_multiline_search_and_back_keep_selected_identity() {
    let mut assistant = checkpoint("assistant:reply", "reply\nneedle 日本語", 0);
    assistant.target_kind = RewindTargetKind::AssistantMessage;
    let mut dialog = Dialog::rewind_loading();
    dialog.set_rewind_checkpoints(vec![checkpoint("user", "other", 0), assistant]);
    for ch in "needle".chars() {
        key(&mut dialog, KeyCode::Char(ch));
    }
    assert_eq!(dialog.stack.last().unwrap().visible(), vec![1]);
    key(&mut dialog, KeyCode::Enter);
    assert!(
        matches!(&dialog.stack.last().unwrap().route, Route::RewindModes(id) if id == "assistant:reply")
    );
    key(&mut dialog, KeyCode::Esc);
    let view = dialog.stack.last().unwrap();
    assert_eq!(view.filter, "needle");
    assert_eq!(view.items[view.visible()[view.selected]].label, "reply");
    key(&mut dialog, KeyCode::Esc);
    assert_eq!(key(&mut dialog, KeyCode::Esc), DialogKey::Close);
}
