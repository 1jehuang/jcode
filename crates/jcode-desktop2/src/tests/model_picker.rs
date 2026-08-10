//! The model caption button, SDK catalog handoff, and menu selection behavior.

use crate::keymap::Action;
use crate::{App, ModelId, harness};
use std::sync::mpsc::{Receiver, Sender, channel};

fn app() -> (
    App,
    Sender<harness::HarnessUpdate>,
    Receiver<harness::Command>,
) {
    let mut app = App::default();
    app.model.session_id = Some("session_model_picker".into());
    app.model.model = Some(ModelId {
        provider: Some("openai".into()),
        model: Some("gpt-5.6".into()),
    });
    let (update_tx, update_rx) = channel();
    let (command_tx, command_rx) = channel();
    app.harness = Some((update_rx, harness::CommandSender::for_test(command_tx)));
    (app, update_tx, command_rx)
}

fn click(app: &mut App, point: (f64, f64)) {
    app.pointer = point;
    app.on_pointer_pressed();
}

fn centre(rect: vello::kurbo::Rect) -> (f64, f64) {
    (rect.x0 + rect.width() / 2.0, rect.y0 + rect.height() / 2.0)
}

fn open_model_picker(app: &mut App) {
    assert!(app.apply(Action::ToggleModelPicker, None));
}

#[test]
fn ctrl_m_requests_sdk_models_and_opens_the_inline_picker() {
    let (mut app, _, commands) = app();
    open_model_picker(&mut app);
    assert!(app.model.model_picker.is_open());
    assert!(app.model.model_picker.is_loading());
    assert!(matches!(
        commands.try_recv(),
        Ok(harness::Command::ListModels)
    ));
}

#[test]
fn model_picker_action_requests_sdk_models_and_toggles_the_menu() {
    let (mut app, _, commands) = app();
    assert!(app.apply(Action::ToggleModelPicker, None));
    assert!(app.model.model_picker.is_open());
    assert!(matches!(
        commands.try_recv(),
        Ok(harness::Command::ListModels)
    ));

    assert!(app.apply(Action::ToggleModelPicker, None));
    assert!(!app.model.model_picker.is_open());
}

#[test]
fn ctrl_m_can_open_before_the_current_model_caption_arrives() {
    let (mut app, _, commands) = app();
    app.model.model = None;
    assert!(app.apply(Action::ToggleModelPicker, None));
    assert!(app.model.model_picker.is_open());
    assert!(matches!(
        commands.try_recv(),
        Ok(harness::Command::ListModels)
    ));
}

#[test]
fn sdk_results_populate_the_open_menu_and_a_row_uses_set_model() {
    let (mut app, updates, commands) = app();
    open_model_picker(&mut app);
    let _ = commands.try_recv();
    updates
        .send(harness::HarnessUpdate::Models {
            models: vec![
                "openai-oauth:gpt-5.6".into(),
                "claude-api:claude-opus-4-8".into(),
            ],
            current: Some("openai-oauth:gpt-5.6".into()),
        })
        .unwrap();
    app.drain_harness_updates();
    assert_eq!(app.model.model_picker.models().len(), 2);
    assert_eq!(
        app.model.model_picker.current(),
        Some("openai-oauth:gpt-5.6")
    );

    // Provider → connection → model.
    let openai = app.frame.model_menu_row(2, 0);
    click(&mut app, centre(openai));
    let oauth = app.frame.model_menu_row(1, 0);
    click(&mut app, centre(oauth));
    let model = app.frame.model_menu_row(1, 0);
    click(&mut app, centre(model));
    let selected = std::iter::from_fn(|| commands.try_recv().ok()).find_map(|command| {
        match command {
            harness::Command::SetModel(model) => Some(model),
            // Draining the catalog update may request a fresh session preview.
            // It is independent of the model menu and shares the ordered worker
            // channel by design.
            _ => None,
        }
    });
    assert_eq!(selected.as_deref(), Some("openai-oauth:gpt-5.6"));
    assert!(!app.model.model_picker.is_open());

    updates
        .send(harness::HarnessUpdate::ModelSelected(
            "claude-api:claude-opus-4-8".into(),
        ))
        .unwrap();
    app.drain_harness_updates();
    assert_eq!(
        app.model.model_picker.current(),
        Some("claude-api:claude-opus-4-8")
    );
}

#[test]
fn dismissing_the_menu_does_not_move_the_composer_caret() {
    let (mut app, _, commands) = app();
    app.model.editor.insert_str("keep the caret here");
    let cursor = app.model.editor.cursor();
    open_model_picker(&mut app);
    let _ = commands.try_recv();
    let composer = (app.frame.left + 4.0, app.frame.composer_top + 4.0);
    click(&mut app, composer);
    assert!(!app.model.model_picker.is_open());
    assert_eq!(app.model.editor.cursor(), cursor);
}

#[test]
fn catalog_updates_do_not_reopen_a_menu_dismissed_while_loading() {
    let (mut app, updates, commands) = app();
    open_model_picker(&mut app);
    let _ = commands.try_recv();
    open_model_picker(&mut app);
    updates
        .send(harness::HarnessUpdate::Models {
            models: vec!["gpt-5.6".into()],
            current: Some("gpt-5.6".into()),
        })
        .unwrap();
    app.drain_harness_updates();
    assert!(!app.model.model_picker.is_open());
    assert_eq!(app.model.model_picker.models(), ["gpt-5.6"]);
}

#[test]
fn capture_state_exercises_the_rendered_catalog() {
    let model = crate::states::by_name("model_picker").expect("model picker state");
    assert!(model.model_picker.is_open());
    assert_eq!(model.model_picker.models().len(), 3);
    assert_eq!(model.model_picker.hover(), Some(1));
}

#[test]
#[ignore = "requires a GPU"]
fn open_catalog_reaches_the_pixels_above_the_composer() {
    let model = crate::states::by_name("model_picker").expect("model picker state");
    let Some(rendered) = super::visual::Rendered::new(&model) else {
        eprintln!("skipping: no GPU");
        return;
    };
    let menu = rendered.frame.model_menu(model.model_picker.visual_rows());
    assert!(
        rendered.darkest_in(menu.x0, menu.y0, menu.x1, menu.y1) < 0.9,
        "the open model catalog did not reach the rendered frame"
    );
}
