//! Composition of the live session page with cached neighboring previews.
//!
//! The focused model is rendered through the existing full scene builder and
//! remains the only interactive page. Other columns use the same builder with a
//! read-only model made from their cached `Peek` transcript. All pages are then
//! appended into one Vello scene at the camera positions supplied by
//! [`crate::workspace`].

use crate::{Model, paint, scene, strip, workspace};
use vello::Scene;
use vello::kurbo::{Affine, Rect};

/// Build the actual window scene. The overview deliberately keeps the legacy
/// full-window path: it is already a view of every session, and nesting that
/// spatial navigator inside one workspace column would make both modes worse.
pub fn build_workspace_scene(
    output: &mut Scene,
    painter: &mut paint::Painter,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    let entries = model.strip.entries();
    if entries.len() <= 1 || model.overview.is_visible() {
        scene::build_scene(output, painter, model, size, scale);
        return;
    }

    let column_width = workspace::column_width(size.0, entries.len());
    let focused_index = entries
        .iter()
        .position(|entry| Some(entry.session_id.as_str()) == model.session_id.as_deref())
        .or_else(|| {
            let focused = model.strip.focused_session()?;
            entries.iter().position(|entry| entry.session_id == focused)
        })
        .unwrap_or(0);
    let columns = model.workspace.layout(
        entries.len(),
        focused_index,
        f64::from(size.0),
        f64::from(column_width),
        workspace::GAP * scale,
    );

    // The gutters belong to the workspace, not to any session. A quiet wash
    // makes the page boundaries legible without adding permanent chrome.
    output.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        model.theme.wash,
        None,
        &Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)),
    );

    // Neighbors first, then the live page. They normally do not overlap, but
    // this ordering keeps subpixel edge antialiasing from washing over focus.
    for column in columns
        .iter()
        .copied()
        .filter(|column| !column.focused && column.is_visible(f64::from(size.0)))
    {
        let mut child = Scene::new();
        let retained = retained_session_model(model, &entries[column.index]);
        scene::build_scene(
            &mut child,
            painter,
            &retained,
            (column_width, size.1),
            scale,
        );
        output.append(&child, Some(Affine::translate((column.x, 0.0))));
    }

    if let Some(column) = columns.iter().find(|column| column.focused) {
        let mut child = Scene::new();
        scene::build_scene(&mut child, painter, model, (column_width, size.1), scale);
        output.append(&child, Some(Affine::translate((column.x, 0.0))));
    }
}

/// Make the immutable scene input for one retained session. This deliberately
/// does not clone the live model wholesale: doing so would copy every cached
/// Peek transcript on every frame and would leak the focused page's draft,
/// selection, overlays, and animation state into its neighbors.
fn retained_session_model(source: &Model, entry: &strip::Entry) -> Model {
    let cached = source.peeks.get(&entry.session_id);
    let loading = cached.is_none();
    let transcript = cached.cloned().unwrap_or_default();
    let mut session_strip = source.strip.clone();
    session_strip.focus_session(&entry.session_id);

    Model {
        theme: source.theme,
        theme_preference: source.theme_preference,
        meta: source.meta.clone(),
        status: String::new(),
        session_id: Some(entry.session_id.clone()),
        transcript,
        editor: crate::editor::Editor::default(),
        caret: crate::caret::Caret::default(),
        busy: false,
        activity: crate::activity::Activity::default(),
        focused: false,
        scroll: 0.0,
        selection: None,
        notice: loading.then(|| "loading session…".to_string()),
        attachments: 0,
        attachment_previews: Vec::new(),
        attachment_preview: None,
        failure: None,
        donut: None,
        spin: crate::donut::Spin::default(),
        hint: source.hint,
        stream: crate::stream::Stream::default(),
        smooth: crate::scroll::Smooth::default(),
        strip: session_strip,
        workspace: workspace::Workspace::default(),
        overview: crate::overview::Overview::default(),
        peeks: crate::overview::Peeks::default(),
        resume: crate::resume::Picker::default(),
        working_dir: entry.working_dir.clone(),
        model: None,
        model_picker: crate::model_picker::Picker::default(),
        boot: crate::boot::Boot::default(),
        progress_clock: None,
        settings: source.settings,
        panel: crate::settings::Panel::default(),
        mem: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Message, Transcript};

    fn transcript(messages: impl IntoIterator<Item = Message>) -> Transcript {
        let mut transcript = Transcript::default();
        for message in messages {
            transcript.push(message);
        }
        transcript
    }

    #[test]
    fn retained_page_uses_the_complete_cached_transcript() {
        let mut source = Model::default();
        let entry = strip::Entry::new("neighbor", Some("/work/neighbor"));
        source.strip = strip::Strip::build(
            vec![strip::Entry::new("live", Some("/work/live")), entry.clone()],
            Some("live"),
        );
        let transcript = transcript([
            Message::user("a long question that must remain a paragraph"),
            Message::assistant("a complete markdown answer\n\nwith a second paragraph"),
        ]);
        source.peeks.insert("neighbor", transcript.clone());

        let retained = retained_session_model(&source, &entry);

        assert_eq!(retained.transcript, transcript);
        assert_eq!(retained.session_id.as_deref(), Some("neighbor"));
        assert_eq!(retained.working_dir.as_deref(), Some("/work/neighbor"));
        assert_eq!(retained.strip.focused_session(), Some("neighbor"));
        assert_eq!(source.strip.focused_session(), Some("live"));
    }

    #[test]
    fn retained_page_is_a_read_only_settled_shell() {
        let mut source = Model::default();
        let entry = strip::Entry::new("neighbor", None);
        source.editor.insert_str("unsent live draft");
        source.focused = true;
        source.busy = true;
        source.stream = crate::stream::Stream::pinned(0.25);
        source.notice = Some("live notice".into());
        source.overview.open(Some("neighbor"));
        source.peeks.insert(
            "neighbor",
            transcript([Message::assistant("cached answer")]),
        );

        let retained = retained_session_model(&source, &entry);

        assert!(retained.editor.is_empty());
        assert!(!retained.focused);
        assert!(!retained.busy);
        assert!(!retained.stream.is_animating());
        assert_eq!(retained.notice, None);
        assert!(!retained.overview.is_visible());
        assert_eq!(source.editor.text(), "unsent live draft");
        assert!(source.stream.is_animating());
        assert!(source.overview.is_visible());
    }

    #[test]
    fn missing_peek_still_gets_the_full_loading_shell() {
        let source = Model::default();
        let retained = retained_session_model(&source, &strip::Entry::new("pending", None));

        assert!(retained.transcript.is_empty());
        assert_eq!(retained.notice.as_deref(), Some("loading session…"));
        assert!(!retained.focused);
    }

    #[test]
    fn retained_page_builds_with_the_actual_session_scene() {
        let mut source = Model::default();
        let entry = strip::Entry::new("neighbor", Some("/work/neighbor"));
        source.session_id = Some("live".into());
        source.strip = strip::Strip::build(
            vec![strip::Entry::new("live", Some("/work/live")), entry.clone()],
            Some("live"),
        );
        source
            .transcript
            .push(Message::assistant("focused session stays live"));
        source.peeks.insert(
            "neighbor",
            transcript([
                Message::user("question with **formatting**"),
                Message::assistant("answer with `code` and\n\nmultiple paragraphs"),
            ]),
        );
        let mut painter = paint::Painter::default();
        let mut output = Scene::new();

        build_workspace_scene(&mut output, &mut painter, &source, (1000, 720), 1.0);
    }

    #[test]
    #[ignore = "requires a GPU-backed capture"]
    fn centered_retained_column_matches_the_full_session_scene_within_raster_tolerance() {
        const VIEW_WIDTH: u32 = 1000;
        const COLUMN_WIDTH: u32 = 760;
        const HEIGHT: u32 = 720;
        const COLUMN_X: u32 = (VIEW_WIDTH - COLUMN_WIDTH) / 2;

        let mut source = Model::default();
        let entry = strip::Entry::new("neighbor", Some("/work/neighbor"));
        source.theme = crate::theme::Theme::print_light();
        source.session_id = Some("live".into());
        source.strip = strip::Strip::build(
            vec![strip::Entry::new("live", Some("/work/live")), entry.clone()],
            Some("live"),
        );
        source.peeks.insert(
            "neighbor",
            transcript([
                Message::user("question with **formatting**"),
                Message::assistant("answer with `code` and\n\nmultiple paragraphs"),
            ]),
        );
        // At phase zero the page being left is centered. With two columns and
        // rightward motion, that is the retained neighbor at x = 120.
        source.workspace.begin(workspace::Direction::Right);

        let mut workspace_scene = Scene::new();
        build_workspace_scene(
            &mut workspace_scene,
            &mut paint::Painter::default(),
            &source,
            (VIEW_WIDTH, HEIGHT),
            1.0,
        );
        let workspace_pixels =
            crate::capture::capture_scene_to_rgba(&workspace_scene, VIEW_WIDTH, HEIGHT)
                .expect("capture workspace scene");

        let retained = retained_session_model(&source, &entry);
        let mut retained_scene = Scene::new();
        scene::build_scene(
            &mut retained_scene,
            &mut paint::Painter::default(),
            &retained,
            (COLUMN_WIDTH, HEIGHT),
            1.0,
        );
        let retained_pixels =
            crate::capture::capture_scene_to_rgba(&retained_scene, COLUMN_WIDTH, HEIGHT)
                .expect("capture retained scene");

        let mut max_channel_delta = 0;
        for y in 0..HEIGHT as usize {
            let workspace_start = (y * VIEW_WIDTH as usize + COLUMN_X as usize) * 4;
            let workspace_end = workspace_start + COLUMN_WIDTH as usize * 4;
            let retained_start = y * COLUMN_WIDTH as usize * 4;
            let retained_end = retained_start + COLUMN_WIDTH as usize * 4;
            for (&workspace, &retained) in workspace_pixels[workspace_start..workspace_end]
                .iter()
                .zip(&retained_pixels[retained_start..retained_end])
            {
                max_channel_delta = max_channel_delta.max(workspace.abs_diff(retained));
            }
        }
        // Vello may round edge coverage a few byte values differently when the
        // same vector scene is rasterized at a translated target origin. The
        // observed delta stays below one sixteenth of a channel and is confined
        // to antialiased edges; a structural or color mismatch is much larger.
        assert!(
            max_channel_delta <= 16,
            "retained scene diverged by {max_channel_delta}/255",
        );
    }
}
