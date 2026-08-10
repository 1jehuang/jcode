//! State-space nodes for the UI.
//!
//! `build_scene` is a pure function of `Model`, so the app's visual states
//! form an enumerable graph. Each named node here is a deterministic `Model`
//! that can be rendered offscreen (`--capture <node> <out.png>`) for visual
//! verification without a window, compositor, or screenshots.

use crate::Model;

type NodeBuilder = fn() -> Model;

/// All named state-space nodes. Keep deterministic: no clocks, no randomness.
pub const NODES: &[(&str, NodeBuilder)] = &[
    ("connecting", connecting),
    ("starting_new_session", starting_new_session),
    ("attach_in_flight", attach_in_flight),
    ("reconnecting", reconnecting),
    ("attached_empty", attached_empty),
    ("boot_opening", boot_opening),
    ("boot_donut", boot_donut),
    ("boot_chrome", boot_chrome),
    ("donut_dragged", donut_dragged),
    ("donut_off", donut_off),
    ("mid_input", mid_input),
    ("mid_input_caret_inside", mid_input_caret_inside),
    ("caret_hidden", caret_hidden),
    ("unfocused", unfocused),
    ("selection", selection),
    ("multiline", multiline),
    ("wrapped_long_line", wrapped_long_line),
    ("unbreakable_paste", unbreakable_paste),
    ("overlong_paste", overlong_paste),
    ("multiline_selection", multiline_selection),
    ("selection_all", selection_all),
    ("streaming", streaming),
    ("reasoning", reasoning),
    ("reasoning_streaming", reasoning_streaming),
    ("reasoning_paragraphs", reasoning_paragraphs),
    ("tool_progress", tool_progress),
    ("background_progress", background_progress),
    ("background_progress_many", background_progress_many),
    ("todo_card", todo_card),
    ("edit_card", edit_card),
    ("edit_cards_many", edit_cards_many),
    ("edit_card_large", edit_card_large),
    ("working", working),
    ("message_sent", message_sent),
    ("queued_message", queued_message),
    ("turn_done", turn_done),
    ("transcript_selection", transcript_selection),
    ("scrolled_back", scrolled_back),
    ("markdown", markdown),
    ("markdown_typography", markdown_typography),
    ("markdown_structure", markdown_structure),
    ("latex", latex),
    ("code_block", code_block),
    ("session_strip", session_strip),
    ("session_strip_second_group", session_strip_second_group),
    ("mem_readout", mem_readout),
    ("overview", overview),
    ("overview_opening", overview_opening),
    ("overview_other_session", overview_other_session),
    ("overview_preview", overview_preview),
    ("overview_thumbnails", overview_thumbnails),
    ("overview_single_session", overview_single_session),
    ("overview_many_sessions", overview_many_sessions),
    ("resume_picker", resume_picker),
    ("resume_picker_preview", resume_picker_preview),
    ("resume_picker_search", resume_picker_search),
    ("resume_picker_group", resume_picker_group),
    ("help_overlay", help_overlay),
    ("settings_panel", settings_panel),
    ("settings_panel_hover", settings_panel_hover),
    ("model_picker", model_picker),
    ("notice", notice),
    ("error", error),
    ("offline", offline),
    ("long_paragraph", long_paragraph),
    // Heavy nodes. Every node above is a small, pretty screen, which is what a
    // capture wants and exactly the wrong thing to profile: a sweep over them
    // would have reported the whole app as fast while a real session lagged.
    // These sit at the slow end of the space on purpose, so `--profile-states`
    // measures the frames that actually hurt.
    ("heavy_long_session", heavy_long_session),
    ("heavy_code_wall", heavy_code_wall),
    ("heavy_wide_table", heavy_wide_table),
    ("heavy_math", heavy_math),
];

pub fn by_name(name: &str) -> Option<Model> {
    NODES
        .iter()
        .find(|(node, _)| *node == name)
        .map(|(_, build)| build())
}

pub fn names() -> Vec<&'static str> {
    NODES.iter().map(|(name, _)| *name).collect()
}

/// Captures must be deterministic, so nodes pin the build identity instead of
/// reading the real version, update channels, and auth store.
fn fixed_meta() -> crate::meta::Meta {
    crate::meta::Meta {
        version: "v0.0.0-demo (0000000)".into(),
        update: crate::meta::UpdateState::Current,
        account: Some("demo@jcode.dev (anthropic)".into()),
    }
}

fn connecting() -> Model {
    Model {
        // Pinned light: nodes must be a pure function of the model, and
        // `from_env` now reads the real system preference, which would make
        // every capture depend on the machine it ran on.
        theme: crate::theme::Theme::print_light(),
        // Pinned for the same reason: a capture must not re-resolve on the
        // machine's live preference behind the pinned palette.
        theme_preference: crate::theme::ThemeMode::Light,
        meta: fixed_meta(),
        status: "connecting to ~/.jcode/jcode-api.sock...".into(),
        session_id: None,
        transcript: crate::transcript::Transcript::default(),
        editor: crate::editor::Editor::default(),
        resume: crate::resume::Picker::default(),
        help_open: false,
        caret: fixed_caret(),
        // Nodes render the focused case: an unfocused window hides the caret,
        // which would make most caret nodes indistinguishable.
        focused: true,
        busy: false,
        activity: crate::activity::Activity::default(),
        scroll: 0.0,
        selection: None,
        notice: None,
        failure: None,
        // No pasted images in a capture: an attachment count is a fact about
        // what the user just did, so a node pins it like anything else.
        attachments: 0,
        attachment_previews: Vec::new(),
        attachment_preview: None,
        donut: Some(fixed_donut()),
        spin: fixed_spin(),
        // Captures pin the hint, so the ghost line is a tested state rather
        // than whatever the clock happened to pick.
        hint: 0,
        // Detached: nothing has told us the model yet, so the caption is absent.
        model: None,
        model_picker: crate::model_picker::Picker::default(),
        strips: crate::strip::Strips::default(),
        workspace: crate::workspace::Workspace::default(),
        // Captures are still frames, so nothing is mid-reveal: a default
        // stream draws every glyph.
        stream: crate::stream::Stream::default(),
        // Closed: the overview is a held gesture, so every ordinary node
        // renders with it away.
        overview: crate::overview::Overview::default(),
        // Captures pin their previews, so a node never depends on what
        // happens to be on disk.
        peeks: crate::overview::Peeks::default(),
        // Captures are still frames, so the scroll is settled rather than
        // mid-glide.
        smooth: crate::scroll::Smooth::default(),
        // Detached: no session, so no directory to name.
        working_dir: None,
        file_tree: crate::file_tree::FileTree::default(),
        // Pinned off: a live RAM figure would make every capture depend on
        // the machine and moment it ran on.
        mem: None,
        // No bars on screen by default, so nothing animates: a node that wants
        // one sets it (see `background_progress`).
        progress_clock: None,
        // Settled: a node renders the window after the boot reveal, so every
        // existing capture is unchanged by it. The reveal has its own nodes.
        boot: crate::boot::Boot::default(),
        // Pinned, not loaded: a capture must not depend on the developer's own
        // saved preferences. The panel is shut, so every existing node is
        // pixel-identical; `settings_panel` is the node that opens it.
        settings: crate::settings::Settings {
            theme: crate::theme::ThemeMode::Light,
            reasoning: crate::reasoning::ReasoningMode::Current,
            motion: true,
            copy_on_select: false,
        },
        panel: crate::settings::Panel::default(),
    }
}

/// The gap after the gesture and before the daemon assigns the new id used to
/// exist only as `session_id == None`, conflated with first boot. Keep it in the
/// enumerable space because this is the latency-sensitive frame a user sees.
fn starting_new_session() -> Model {
    Model {
        status: "starting a new session...".into(),
        session_id: None,
        working_dir: None,
        ..attached_empty()
    }
}

/// Existing-session navigation optimistically retargets the id while the attach
/// RPC is outstanding. That is observably different from both detached and
/// attached-empty, even though it deliberately renders an empty transcript.
fn attach_in_flight() -> Model {
    Model {
        status: "attaching: session-demo-2".into(),
        session_id: Some("session-demo-2".into()),
        transcript: crate::transcript::Transcript::default(),
        ..attached_empty()
    }
}

/// A dropped connection preserves the current conversation while reporting
/// recovery. It must not regress into the visually unrelated first-boot node.
fn reconnecting() -> Model {
    let mut model = attached_empty();
    model.status = "connection lost; retrying in 500ms".into();
    model.failure = Some("connection lost; retrying in 500ms".into());
    model
}

/// Three frames of the boot reveal: the black opening, the donut half grown,
/// and the chrome fading in over it. Pinned times, so each phase is a
/// deterministic capture rather than something only visible at launch.
fn boot_opening() -> Model {
    Model {
        boot: crate::boot::Boot::pinned(0.02),
        ..attached_empty()
    }
}

fn boot_donut() -> Model {
    Model {
        boot: crate::boot::Boot::pinned(0.16),
        ..attached_empty()
    }
}

fn boot_chrome() -> Model {
    Model {
        boot: crate::boot::Boot::pinned(0.36),
        ..attached_empty()
    }
}

/// The donut is animated in the app, so nodes pin its clock: the field is
/// rendered once, at a fixed time, which keeps captures byte-reproducible while
/// still exercising the halftone path.
fn fixed_donut() -> crate::donut::Donut {
    let mut donut = crate::donut::Donut::new(crate::DONUT_GRID);
    donut.render(DONUT_TIME, 0.0);
    donut
}

/// Attached sessions report a model, so captures pin one rather than reading
/// whatever the local config happens to select.
fn fixed_model() -> crate::ModelId {
    crate::ModelId {
        provider: Some("anthropic".into()),
        model: Some("claude-sonnet-4-5".into()),
    }
}

fn fixed_spin() -> crate::donut::Spin {
    crate::donut::Spin {
        time: DONUT_TIME,
        ..Default::default()
    }
}

/// A flattering pose for captures: the hole is clearly visible.
const DONUT_TIME: f32 = 0.8;

/// Captures must be a pure function of the model, so nodes pin the caret
/// instead of letting it blink on wall-clock time.
fn fixed_caret() -> crate::caret::Caret {
    crate::caret::Caret::pinned(true)
}

fn attached_empty() -> Model {
    Model {
        // Pinned light: nodes must be a pure function of the model, and
        // `from_env` now reads the real system preference, which would make
        // every capture depend on the machine it ran on.
        theme: crate::theme::Theme::print_light(),
        // Pinned for the same reason: a capture must not re-resolve on the
        // machine's live preference behind the pinned palette.
        theme_preference: crate::theme::ThemeMode::Light,
        meta: fixed_meta(),
        status: "attached: session_demo_0000".into(),
        session_id: Some("session_demo_0000".into()),
        transcript: crate::transcript::Transcript::default(),
        editor: crate::editor::Editor::default(),
        resume: crate::resume::Picker::default(),
        help_open: false,
        caret: fixed_caret(),
        // Nodes render the focused case: an unfocused window hides the caret,
        // which would make most caret nodes indistinguishable.
        focused: true,
        busy: false,
        activity: crate::activity::Activity::default(),
        scroll: 0.0,
        selection: None,
        notice: None,
        failure: None,
        // No pasted images in a capture: an attachment count is a fact about
        // what the user just did, so a node pins it like anything else.
        attachments: 0,
        attachment_previews: Vec::new(),
        attachment_preview: None,
        donut: Some(fixed_donut()),
        spin: fixed_spin(),
        // Captures pin the hint, so the ghost line is a tested state rather
        // than whatever the clock happened to pick.
        hint: 0,
        model: Some(fixed_model()),
        model_picker: crate::model_picker::Picker::default(),
        strips: crate::strip::Strips::default(),
        workspace: crate::workspace::Workspace::default(),
        // Captures are still frames, so nothing is mid-reveal: a default
        // stream draws every glyph.
        stream: crate::stream::Stream::default(),
        overview: crate::overview::Overview::default(),
        // Captures pin their previews, so a node never depends on what
        // happens to be on disk.
        peeks: crate::overview::Peeks::default(),
        // Captures are still frames, so the scroll is settled rather than
        // mid-glide.
        smooth: crate::scroll::Smooth::default(),
        // Fixed path, so captures do not depend on where the repo is checked
        // out or on whose `$HOME` the capture ran under.
        working_dir: Some("/home/j/jcode".into()),
        file_tree: crate::file_tree::FileTree::default(),
        // Pinned off: a live RAM figure would make every capture depend on
        // the machine and moment it ran on.
        mem: None,
        // No bars on screen by default, so nothing animates: a node that wants
        // one sets it (see `background_progress`).
        progress_clock: None,
        // Settled: a node renders the window after the boot reveal, so every
        // existing capture is unchanged by it. The reveal has its own nodes.
        boot: crate::boot::Boot::default(),
        // Pinned, not loaded: a capture must not depend on the developer's own
        // saved preferences. The panel is shut, so every existing node is
        // pixel-identical; `settings_panel` is the node that opens it.
        settings: crate::settings::Settings {
            theme: crate::theme::ThemeMode::Light,
            reasoning: crate::reasoning::ReasoningMode::Current,
            motion: true,
            copy_on_select: false,
        },
        panel: crate::settings::Panel::default(),
    }
}

/// The hero donut after a drag: same tilt, rotated yaw. Proves the drag path
/// changes only the spin, so the pose stays flattering however hard it is spun.
fn donut_dragged() -> Model {
    let mut donut = crate::donut::Donut::new(crate::DONUT_GRID);
    let offset = 1.2;
    donut.render(DONUT_TIME, offset);
    Model {
        donut: Some(donut),
        spin: crate::donut::Spin {
            offset,
            ..fixed_spin()
        },
        ..attached_empty()
    }
}

/// The donut turned off (`JCODE_DESKTOP2_DONUT=0`): the empty screen must still
/// read as a finished frame with nothing missing.
fn donut_off() -> Model {
    Model {
        donut: None,
        ..attached_empty()
    }
}

/// Build a transcript from (user, assistant) turns. Fixtures speak in turns
/// rather than in a formatted blob, so a capture exercises the real role
/// structure the renderer draws.
fn conversation(turns: Vec<(String, String)>) -> crate::transcript::Transcript {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    for (user, assistant) in turns {
        transcript.push(Message::user(user));
        transcript.push(Message::assistant(assistant));
    }
    transcript
}

fn editor_with(text: &str, cursor: Option<usize>) -> crate::editor::Editor {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str(text);
    if let Some(cursor) = cursor {
        editor.set_cursor_public(cursor);
    }
    editor
}

fn mid_input() -> Model {
    Model {
        editor: editor_with("explain the harness API handshake", None),
        ..attached_empty()
    }
}

/// Caret parked mid-text: proves the input box is a real buffer with a cursor
/// rather than an append-only string.
fn mid_input_caret_inside() -> Model {
    Model {
        editor: editor_with("explain the harness API handshake", Some(7)),
        ..attached_empty()
    }
}

/// The off phase of the blink, so the caret's absence is also a tested state.
fn caret_hidden() -> Model {
    Model {
        editor: editor_with("blink off phase", None),
        caret: crate::caret::Caret::pinned(false),
        ..attached_empty()
    }
}

/// The window without keyboard focus: the field border goes quiet and no
/// caret is drawn, so the frame cannot claim keystrokes it will not receive.
fn unfocused() -> Model {
    Model {
        editor: editor_with("window lost focus", None),
        focused: false,
        ..attached_empty()
    }
}

/// A mouse or shift-arrow selection: proves the band renders and that text on
/// top of it stays readable.
fn selection() -> Model {
    let mut editor = editor_with("select this middle part", None);
    editor.place_cursor(7);
    editor.extend_to(11);
    Model {
        editor,
        ..attached_empty()
    }
}

fn selection_all() -> Model {
    let mut editor = editor_with("everything is selected", None);
    editor.select_all();
    Model {
        editor,
        ..attached_empty()
    }
}

/// A multi-line message: the composer grows and the caret sits on the last
/// line, not the first.
fn multiline() -> Model {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str("first line\nsecond line\nthird line");
    Model {
        editor,
        ..attached_empty()
    }
}

/// One very long logical line: must wrap inside the well rather than running
/// past its right edge.
fn wrapped_long_line() -> Model {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str(
        "this is a single very long line with no newlines at all that has to wrap \
         inside the composer well instead of spilling past its right edge",
    );
    Model {
        editor,
        ..attached_empty()
    }
}

/// A pasted URL longer than the well: one "word" with no break opportunity,
/// which used to run straight off the right edge of the composer.
fn unbreakable_paste() -> Model {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str(
        "https://example.com/some/extremely/long/path/segment/that/never/offers/a/break/opportunity?query=parameter&another=value",
    );
    Model {
        editor,
        ..attached_empty()
    }
}

/// A paste taller than the well: the composer caps at
/// [`crate::layout::COMPOSER_MAX_LINES`], so the layout is scrolled under the
/// field and the rows outside it must be clipped away rather than painted over
/// the transcript and the footnote.
fn overlong_paste() -> Model {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str(&"the quick brown fox jumps over the lazy dog ".repeat(20));
    Model {
        editor,
        ..attached_empty()
    }
}

/// A selection spanning a line break.
fn multiline_selection() -> Model {
    let mut editor = crate::editor::Editor::default();
    editor.insert_str("alpha beta\ngamma delta");
    editor.place_cursor(6);
    editor.extend_to(16);
    Model {
        editor,
        ..attached_empty()
    }
}

fn scrolled_back() -> Model {
    Model {
        transcript: conversation(
            (1..=20)
                .map(|n| {
                    (
                        format!("question {n}"),
                        format!("answer {n}. transcript line {n}"),
                    )
                })
                .collect(),
        ),
        scroll: 200.0,
        // Scrolled back is exactly when the bar is up, so the capture shows it.
        smooth: crate::scroll::Smooth::lit(),
        ..attached_empty()
    }
}

/// Several live sessions across two working directories: the case the strip
/// exists for. Fixed ids so the bars are a pinned, testable arrangement.
fn demo_strip(focused: &str) -> crate::strip::Strips {
    crate::strip::Strips::build(
        vec![
            // Weights differ by an order of magnitude, because that is what
            // the overview's blobs are for: a capture where every session is
            // the same size would prove nothing about the sizing.
            crate::strip::Panel {
                session_id: "session_clover_1785130341680_5a8db08".into(),
                title: None,
                working_dir: Some("/home/j/jcode".into()),
                busy: false,
                weight: 480_000.0,
            },
            crate::strip::Panel {
                session_id: "session_mushroom_1785129393446_e7007f8".into(),
                title: None,
                working_dir: Some("/home/j/jcode".into()),
                busy: true,
                weight: 90_000.0,
            },
            crate::strip::Panel {
                session_id: "session_pebble_1785130002233_1c93aa4".into(),
                title: None,
                working_dir: Some("/home/j/jcode".into()),
                busy: false,
                weight: 6_000.0,
            },
            crate::strip::Panel {
                session_id: "session_harbor_1785128881021_9f0b21d".into(),
                title: None,
                working_dir: Some("/home/j/site".into()),
                busy: false,
                weight: 210_000.0,
            },
            crate::strip::Panel {
                session_id: "session_ember_1785131110907_44de7c2".into(),
                title: None,
                working_dir: Some("/home/j/site".into()),
                busy: false,
                weight: 1_200.0,
            },
        ],
        Some(focused),
    )
}

fn session_strip() -> Model {
    Model {
        transcript: crate::transcript::Transcript::from(
            &[
                crate::transcript::Message::user("what is in this repo"),
                crate::transcript::Message::assistant("A coding agent, written in Rust."),
            ][..],
        ),
        session_id: Some("session_mushroom_1785129393446_e7007f8".into()),
        strips: demo_strip("session_mushroom_1785129393446_e7007f8"),
        ..attached_empty()
    }
}

/// The chrome row's RAM caption beside the working directory: `ui`/`srv`
/// figures pinned so the capture is a tested arrangement rather than whatever
/// the machine was using.
fn mem_readout() -> Model {
    Model {
        mem: Some(crate::mem::Readout {
            client_bytes: 105 * 1024 * 1024,
            server_bytes: Some(428 * 1024 * 1024),
        }),
        ..session_strip()
    }
}

/// Focus in the second group: proves up/down really moves the highlight to
/// another directory rather than only recolouring within one.
fn session_strip_second_group() -> Model {
    Model {
        session_id: Some("session_harbor_1785128881021_9f0b21d".into()),
        strips: demo_strip("session_harbor_1785128881021_9f0b21d"),
        ..attached_empty()
    }
}

/// The overview at rest, from a session in the middle of a busy checkout.
/// The node the whole feature is judged on: five sessions of very different
/// sizes across two projects, so the blobs have to be legibly different and
/// the two clusters have to read as two places.
fn overview() -> Model {
    Model {
        overview: crate::overview::Overview::pinned(
            true,
            1.0,
            Some("session_mushroom_1785129393446_e7007f8"),
        ),
        ..session_strip()
    }
}

/// Mid-zoom. Captured because the transition is the feature: a field that
/// looks right only when settled would still feel like a panel appearing.
fn overview_opening() -> Model {
    Model {
        overview: crate::overview::Overview::pinned(
            true,
            0.45,
            Some("session_mushroom_1785129393446_e7007f8"),
        ),
        ..session_strip()
    }
}

/// Highlight moved off the session we are attached to: the state every switch
/// passes through, and the one that proves "where I am" and "where I am going"
/// are drawn differently.
fn overview_other_session() -> Model {
    Model {
        overview: crate::overview::Overview::pinned(
            true,
            1.0,
            Some("session_harbor_1785128881021_9f0b21d"),
        ),
        ..session_strip()
    }
}

/// One session. The field must still look deliberate rather than like a bug,
/// which is the case a layout that only ever fits a crowd tends to get wrong.
fn overview_single_session() -> Model {
    let strip = crate::strip::Strips::build(
        vec![crate::strip::Panel {
            session_id: "session_willow_1785130555000_7d3e9f1".into(),
            title: None,
            working_dir: Some("/home/j/jcode".into()),
            busy: false,
            weight: 40_000.0,
        }],
        Some("session_willow_1785130555000_7d3e9f1"),
    );
    Model {
        session_id: Some("session_willow_1785130555000_7d3e9f1".into()),
        strips: strip,
        overview: crate::overview::Overview::pinned(
            true,
            1.0,
            Some("session_willow_1785130555000_7d3e9f1"),
        ),
        ..attached_empty()
    }
}

/// A crowded field: four projects, eighteen sessions. The stress case for
/// packing, for fitting the page, and for whether the labels survive at all.
fn overview_many_sessions() -> Model {
    /// Short names in the daemon's own style, so the captured labels are the
    /// length the real ones will be.
    const NAMES: &[&str] = &[
        "clover", "mushroom", "pebble", "harbor", "ember", "willow", "quartz", "lantern", "meadow",
    ];
    let id = |n: usize| {
        format!(
            "session_{}_17851290000{n:02}_a1b2c3d4",
            NAMES[n % NAMES.len()]
        )
    };
    let entries: Vec<crate::strip::Panel> = (0..18)
        .map(|n| crate::strip::Panel {
            session_id: id(n),
            title: None,
            working_dir: Some(format!("/home/j/proj{}", n % 4)),
            busy: n % 5 == 0,
            // A spread of sizes rather than a ramp, so the field is not a
            // suspiciously tidy gradient.
            weight: ((n * 7919) % 400) as f64 * 900.0 + 500.0,
        })
        .collect();
    let strip = crate::strip::Strips::build(entries, Some(&id(3)));
    Model {
        session_id: Some(id(3)),
        strips: strip,
        // Highlight parked away from the attached session: the crowded field
        // is exactly where "where I am" and "where I am going" have to stay
        // distinguishable.
        overview: crate::overview::Overview::pinned(true, 1.0, Some(&id(7))),
        ..attached_empty()
    }
}

/// Every card carrying its own conversation: the field as an actual view of
/// several sessions at once rather than a set of labelled boxes.
///
/// The node the multi-session view is judged on. Five sessions of very
/// different sizes, each with a distinct tail, so the thing to check is whether
/// a card is *identifiable by its content* at thumbnail size and whether the
/// name underneath survives having text above it.
fn overview_thumbnails() -> Model {
    let mut peeks = crate::overview::Peeks::default();
    for (session, exchange) in [
        (
            "session_clover_1785130341680_5a8db08",
            [
                "rewrite the transcript layout to cache per message",
                "Done: layout is memoised on content and width, so scrolling reuses it.",
            ],
        ),
        (
            "session_mushroom_1785129393446_e7007f8",
            [
                "why is the halftone screen in logical units?",
                "So dot density is identical on 1x and HiDPI, like a CSS-pixel lattice.",
            ],
        ),
        (
            "session_pebble_1785130002233_1c93aa4",
            ["bump the changelog", "Bumped to 0.9.4 and dated it."],
        ),
        (
            "session_harbor_1785128881021_9f0b21d",
            [
                "the landing page jumps on load",
                "The hero image had no intrinsic size; added width/height so nothing reflows.",
            ],
        ),
        (
            "session_ember_1785131110907_44de7c2",
            ["deploy", "Deployed; the preview URL is live."],
        ),
    ] {
        let mut tail = crate::transcript::Transcript::default();
        tail.push(crate::transcript::Message::user(exchange[0]));
        tail.push(crate::transcript::Message::assistant(exchange[1]));
        peeks.insert(session, tail);
    }
    Model {
        peeks,
        overview: crate::overview::Overview::pinned(
            true,
            1.0,
            Some("session_mushroom_1785129393446_e7007f8"),
        ),
        ..session_strip()
    }
}

/// Hovering another session, with its conversation fetched: the state the
/// preview exists for. Captured because it is the only one that shows the
/// three layers at once (your own transcript, the hovered session's tail over
/// it, and the field over both), which is where they can be seen to fight.
fn overview_preview() -> Model {
    let mut peeks = crate::overview::Peeks::default();
    let mut tail = crate::transcript::Transcript::default();
    tail.push(crate::transcript::Message::user(
        "why is the halftone screen in logical units?",
    ));
    tail.push(crate::transcript::Message::assistant(
        "So the dot density is identical on 1x and HiDPI, exactly like the \
         website's CSS-pixel lattice.",
    ));
    tail.push(crate::transcript::Message::user("and the gamma?"));
    tail.push(crate::transcript::Message::assistant(
        "Applied to luminance before sizing a dot, so the midtones do not \
         crush.",
    ));
    peeks.insert("session_harbor_1785128881021_9f0b21d", tail);
    Model {
        // A conversation of our own underneath, so the capture shows the
        // preview against real content rather than against blank paper.
        transcript: conversation(vec![(
            "what is in this repo".into(),
            "A coding agent, written in Rust.".into(),
        )]),
        peeks,
        overview: crate::overview::Overview::pinned(
            true,
            1.0,
            Some("session_harbor_1785128881021_9f0b21d"),
        ),
        ..session_strip()
    }
}

/// A plausible session store, for the resume nodes: several projects, sessions
/// of very different sizes, and one whose directory is unknown, so a capture
/// shows the grouping doing real work rather than a tidy list.
fn stored_sessions() -> Vec<crate::resume::Record> {
    let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_785_000_000);
    let mut records = Vec::new();
    for (index, (id, dir, bytes)) in [
        (
            "session_mushroom_1785129393446_e7007f8",
            Some("/home/j/jcode"),
            2_400_000u64,
        ),
        (
            "session_clover_1785130341680_5a8db08",
            Some("/home/j/jcode"),
            180_000,
        ),
        (
            "session_pebble_1785130002233_1c93aa4",
            Some("/home/j/jcode"),
            12_000,
        ),
        (
            "session_harbor_1785128881021_9f0b21d",
            Some("/home/j/site"),
            640_000,
        ),
        (
            "session_lantern_1785121180559_44be21a",
            Some("/home/j/site"),
            3_200,
        ),
        (
            "session_drift_1785008810210_77aa03b",
            Some("/home/j/notes"),
            96_000,
        ),
        ("session_ghost_1784900000000_00ff11a", None, 4_100),
    ]
    .into_iter()
    .enumerate()
    {
        records.push(crate::resume::Record {
            session_id: id.into(),
            working_dir: dir.map(str::to_string),
            title: None,
            bytes,
            // Newest first, which is the order the scan returns and therefore
            // the order the projects stack in.
            modified: base - std::time::Duration::from_secs(index as u64 * 3_600),
        });
    }
    records
}

/// The picker as it opens: the newest project at the top, its first session
/// highlighted, and the conversation still legible behind the card. The node
/// the whole feature is judged on.
fn resume_picker() -> Model {
    Model {
        resume: crate::resume::Picker::pinned(stored_sessions(), 1, ""),
        ..session_strip()
    }
}

/// A session highlighted with its tail fetched: the state that makes the panel
/// a picker rather than a list, since this is where recognition happens.
fn resume_picker_preview() -> Model {
    let mut model = resume_picker();
    let mut tail = crate::transcript::Transcript::default();
    tail.push(crate::transcript::Message::user(
        "why is the halftone screen in logical units?",
    ));
    tail.push(crate::transcript::Message::assistant(
        "So the dot density is identical on 1x and HiDPI, exactly like the \
         website's CSS-pixel lattice.",
    ));
    tail.push(crate::transcript::Message::user("and the gamma?"));
    tail.push(crate::transcript::Message::assistant(
        "Applied to luminance before sizing a dot, so the midtones do not crush.",
    ));
    let mut peeks = crate::overview::Peeks::default();
    peeks.insert("session_mushroom_1785129393446_e7007f8", tail);
    model.peeks = peeks;
    model
}

/// Narrowed by a query: the state that proves search reaches across projects
/// and that a search ignores collapse.
fn resume_picker_search() -> Model {
    Model {
        resume: crate::resume::Picker::pinned(stored_sessions(), 1, "site"),
        ..session_strip()
    }
}

/// The highlight on a project heading: no session is selected, so the preview
/// column has to say so rather than looking like a failed fetch.
fn resume_picker_group() -> Model {
    Model {
        resume: crate::resume::Picker::pinned(stored_sessions(), 0, ""),
        ..session_strip()
    }
}

fn help_overlay() -> Model {
    Model {
        help_open: true,
        ..attached_empty()
    }
}

/// The settings panel, open on an empty session: the state a user lands in
/// the moment they click the gear.
fn settings_panel() -> Model {
    let mut panel = crate::settings::Panel::default();
    panel.open();
    Model {
        panel,
        ..attached_empty()
    }
}

/// The same panel with a row highlighted, so the hover band is a capture
/// rather than something only visible with a mouse in hand.
fn settings_panel_hover() -> Model {
    let mut panel = crate::settings::Panel::default();
    panel.open();
    panel.set_hover(Some(1));
    Model {
        panel,
        settings: crate::settings::Settings {
            theme: crate::theme::ThemeMode::Light,
            reasoning: crate::reasoning::ReasoningMode::Full,
            motion: false,
            copy_on_select: false,
        },
        ..attached_empty()
    }
}

/// The SDK-backed model menu, including a selected route and pointer hover.
fn model_picker() -> Model {
    let mut picker = crate::model_picker::Picker::default();
    picker.open_loading();
    picker.set_models(
        vec![
            "openai-oauth:gpt-5.6".into(),
            "claude-api:claude-opus-4-8".into(),
            "claude-oauth:claude-fable-5".into(),
        ],
        Some("openai-oauth:gpt-5.6".into()),
    );
    picker.set_hover(Some(1));
    picker.advance(1.0);
    Model {
        model_picker: picker,
        transcript: conversation(vec![
            (
                "Can you make the model chooser feel native to the conversation?".into(),
                "Yes. I’ll open it inside the transcript and let the surrounding messages make room for it.".into(),
            ),
            (
                "Keep it calm and keyboard-first.".into(),
                "The picker will open with Ctrl+M, move with the arrow keys, and close without disturbing your draft.".into(),
            ),
        ]),
        ..attached_empty()
    }
}

fn notice() -> Model {
    Model {
        editor: editor_with("undo me", None),
        notice: Some("nothing to undo".into()),
        ..attached_empty()
    }
}

/// A finished turn that thought before it answered. The point of the node is
/// the contrast: the thought is muted, indented behind a rule, and set smaller,
/// so the answer below it is unmistakably the reply.
fn reasoning() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("why is the reveal a fraction, not a count?"));
    transcript.push(Message::reasoning(
        "The cursor counts markdown *source* characters, but the renderer \
         draws laid-out glyphs. Every `**` and backtick makes those two \
         numbers differ, so a count would run ahead of the visible edge.",
    ));
    transcript.push(Message::assistant(
        "Because the reveal cursor and the drawn glyphs are counted in \
         different units, and only a fraction is well defined across both.",
    ));
    Model {
        transcript,
        ..attached_empty()
    }
}

/// A long thought that spans paragraphs and is interleaved with a tool call:
/// the case where the left rule fragments today. Each reasoning message draws
/// its own rule, so the thought reads as several separate asides instead of
/// one continuous think.
fn reasoning_paragraphs() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("why is the reveal a fraction, not a count?"));
    transcript.push(Message::reasoning(
        "The cursor counts markdown *source* characters, but the renderer \
         draws laid-out glyphs. Every `**` and backtick makes those two \
         numbers differ.\n\nSo a count would run ahead of the visible edge \
         whenever the reply contains markup, which is most replies.\n\nA \
         fraction is the only unit both sides agree on.",
    ));
    transcript.push(Message::reasoning(
        "Second thought after a tool call: the fraction also survives \
         re-layout when the window resizes, which a glyph count would not.",
    ));
    transcript.push(Message::assistant(
        "Because the reveal cursor and the drawn glyphs are counted in \
         different units, and only a fraction is well defined across both.",
    ));
    Model {
        transcript,
        ..attached_empty()
    }
}

/// The same turn mid-flight: reasoning is arriving and being swept in by the
/// same reveal as the answer, with the activity line still running.
fn reasoning_streaming() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("why is the reveal a fraction, not a count?"));
    transcript.push(Message::reasoning(
        "The cursor counts markdown source characters, but the renderer draws \
         laid-out glyphs, so the two disagree by every marker in the reply and",
    ));
    Model {
        transcript,
        busy: true,
        stream: crate::stream::Stream::pinned(0.7),
        activity: crate::activity::Activity::pinned(
            3,
            std::time::Duration::from_secs(5),
            Some("thinking"),
        ),
        ..attached_empty()
    }
}

/// A turn in flight showing its work: the call running right now is one card
/// at the tail of the transcript, so progress is visible where the user is
/// already reading, not only in the composer's activity line. Earlier calls
/// do not accumulate: the card is a slot the current call occupies.
fn tool_progress() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("tighten the scrollbar's fade timing"));
    transcript.set_live_tool("call_1", "read the scroll smoothing module");
    transcript.set_live_tool("call_2", "find every use of the fade alpha");
    transcript.set_live_tool("call_3", "run the desktop2 scroll tests");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            4,
            std::time::Duration::from_secs(23),
            Some("run the desktop2 scroll tests"),
        ),
        ..attached_empty()
    }
}

/// Waiting on a background task, with its bar on the page. This is the state a
/// spinner cannot express: the agent is blocked on work that *does* know how
/// far along it is, and a window that only says "still working" throws that
/// away.
fn background_progress() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("run the whole workspace test suite"));
    transcript.set_live_tool("call_1", "wait for the test sweep");
    transcript.set_progress(
        "224715dw29",
        "bash",
        "62% · Running jcode-desktop2 tests",
        Some(62.0),
    );
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            2,
            std::time::Duration::from_secs(94),
            Some("wait for the test sweep"),
        ),
        // Pinned to the render clock's own instant, so the indeterminate bar in
        // `background_progress_many` draws at phase zero rather than wherever
        // the wall clock happens to be.
        progress_clock: None,
        ..attached_empty()
    }
}

/// Several tasks at once, one of them unable to report a percentage. Bars do
/// not collapse into one line: a turn waiting on three things has to show which
/// of them is the one that is stuck.
fn background_progress_many() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("build, test, and deploy the preview"));
    transcript.set_progress(
        "build-1",
        "bash",
        "88% · Compiling jcode-app-core",
        Some(88.0),
    );
    transcript.set_progress("test-1", "bash", "12/96 crates", Some(12.5));
    transcript.set_progress("swarm-1", "swarm", "working · waiting on 3 workers", None);
    transcript.set_live_tool("call_4", "wait for the plan to resolve");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            6,
            std::time::Duration::from_secs(212),
            Some("wait for the plan to resolve"),
        ),
        ..attached_empty()
    }
}

/// The plan card mid-task: completed, active, and pending items across two
/// groups, so every native state (check, active dot, empty dot, connector,
/// header, progress bar) is visible in one capture.
fn todo_card() -> Model {
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("refit the harness reconnect path"));
    let card = crate::todos::parse(Some(
        r#"{"todos":[
            {"content":"Trace the reconnect events end to end","status":"completed","group":"Investigate"},
            {"content":"Reproduce the dropped-frame race","status":"completed","group":"Investigate"},
            {"content":"Rework the backoff so a flap cannot stampede","status":"in_progress","group":"Fix"},
            {"content":"Surface the retry state in the status line","status":"pending","group":"Fix"},
            {"content":"Add a soak test against the flaky socket","status":"pending","group":"Verify"}
        ]}"#,
    ))
    .expect("static todo json parses");
    transcript.set_todo(&card);
    transcript.set_live_tool("call_1", "rework the reconnect backoff");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            3,
            std::time::Duration::from_secs(61),
            Some("rework the reconnect backoff"),
        ),
        ..attached_empty()
    }
}

/// A finished edit, kept in the transcript. This is the state the live tool
/// card cannot express: the call is over, but what it *did* to the user's files
/// has to stay readable, so the intent, the file, and the changed lines stand as
/// their own card between the turns.
fn edit_card() -> Model {
    use crate::edits::EditCard;
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("make the fade decay instead of snapping"));
    transcript.push_edit(&EditCard {
        intent: Some("decay the scrollbar fade instead of clearing it".into()),
        files: vec!["crates/jcode-desktop2/src/scroll.rs".into()],
        diff: "118- self.fade = 0.0;\n118+ self.fade = (self.fade - dt / FADE_SECONDS).max(0.0);\n"
            .into(),
        added: 1,
        removed: 1,
    });
    transcript.push(Message::assistant(
        "The bar now eases out over `FADE_SECONDS` rather than disappearing on \
         the frame the wheel stops.",
    ));
    Model {
        transcript,
        ..attached_empty()
    }
}

/// Several edits in one turn, with the next call still running. The cards
/// accumulate (each is a change that happened) while the live tool card stays
/// pinned to the tail: the two must not fight over the bottom of the page.
fn edit_cards_many() -> Model {
    use crate::edits::EditCard;
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("rename `alpha` to `fade` everywhere"));
    for (file, line) in [
        ("crates/jcode-desktop2/src/scroll.rs", 118usize),
        ("crates/jcode-desktop2/src/scene.rs", 402),
        ("crates/jcode-desktop2/src/layout.rs", 77),
    ] {
        transcript.push_edit(&EditCard {
            intent: Some(format!(
                "rename the field in {}",
                file.rsplit('/').next().unwrap()
            )),
            files: vec![file.into()],
            diff: format!("{line}- let alpha = self.alpha;\n{line}+ let fade = self.fade;\n"),
            added: 1,
            removed: 1,
        });
    }
    transcript.set_live_tool("call_9", "run the desktop2 tests");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            4,
            std::time::Duration::from_secs(31),
            Some("run the desktop2 tests"),
        ),
        ..attached_empty()
    }
}

/// A rewrite big enough that the card cannot show all of it, over a file whose
/// language is not one the highlighter knows. Both are the cases where a diff
/// card most easily goes wrong: it either swallows the page or renders as a
/// wall of one colour.
fn edit_card_large() -> Model {
    use crate::edits::EditCard;
    use crate::transcript::{Message, Transcript};
    let mut transcript = Transcript::default();
    transcript.push(Message::user("port the config loader to the new schema"));
    let mut diff = String::new();
    for line in 1..=60usize {
        diff.push_str(&format!("{line}- old_key_{line} = \"value {line}\"\n"));
        diff.push_str(&format!("{line}+ new.key.{line} = \"value {line}\"\n"));
    }
    transcript.push_edit(&EditCard {
        intent: Some("move every key under the new namespace".into()),
        files: vec!["config/defaults.toml".into()],
        diff,
        added: 60,
        removed: 60,
    });
    Model {
        transcript,
        ..attached_empty()
    }
}

fn streaming() -> Model {
    Model {
        transcript: conversation(vec![(
            "explain the harness API handshake".into(),
            "The client opens the socket and sends a `hello` frame carrying \
             its supported version range. The server replies with `hello_ok` \
             and the negotiated version, after which"
                .into(),
        )]),
        busy: true,
        // Pinned so the spinner cell and the elapsed time are the same in
        // every capture; a live clock here would make the node unreviewable.
        activity: crate::activity::Activity::pinned(
            2,
            std::time::Duration::from_secs(8),
            Some("reading crates/jcode-desktop2/src/scene.rs"),
        ),
        ..attached_empty()
    }
}

/// A turn that has produced no text yet: the state the old design showed as a
/// blank screen. The activity line is the whole of the feedback here, so it is
/// worth a node of its own.
fn working() -> Model {
    let mut transcript = crate::transcript::Transcript::default();
    transcript.set_live_tool("", "thinking");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            5,
            std::time::Duration::from_secs(42),
            Some("running the desktop2 test suite"),
        ),
        ..attached_empty()
    }
}

/// The first frame after Enter: the message is on the page and out the socket,
/// but nothing has confirmed it landed. This is the longest-lived state of the
/// send lifecycle on a slow link, and the one where the user is most likely to
/// be staring at their own words, so it gets a node of its own: the tone here
/// is what made a prompt unreadable in dark mode.
fn message_sent() -> Model {
    let mut transcript = crate::transcript::Transcript::default();
    transcript.push(crate::transcript::Message::sent(
        "explain the harness API handshake",
    ));
    transcript.set_live_tool("", "thinking");
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(0, std::time::Duration::ZERO, None),
        ..attached_empty()
    }
}

/// A message typed while the agent was mid-turn: it waits at the tail in the
/// queued tone, under the reply that is still streaming in. This is the state
/// that replaced the daemon's "already processing" error.
fn queued_message() -> Model {
    let mut transcript = conversation(vec![(
        "explain the harness API handshake".into(),
        "The client opens the socket and sends a `hello` frame carrying \
         its supported version range. The server replies with"
            .into(),
    )]);
    transcript.push(crate::transcript::Message::queued(
        "and after that, add a reconnect test",
    ));
    Model {
        transcript,
        busy: true,
        activity: crate::activity::Activity::pinned(
            2,
            std::time::Duration::from_secs(8),
            Some("thinking"),
        ),
        ..attached_empty()
    }
}

fn turn_done() -> Model {
    Model {
        transcript: conversation(vec![(
            "explain the harness API handshake".into(),
            "The client opens the socket and sends a `hello` frame carrying \
             its supported version range. The server replies with `hello_ok` \
             and the negotiated version, after which normal requests flow."
                .into(),
        )]),
        busy: false,
        ..attached_empty()
    }
}

/// A transcript selection spanning both turns: the highlight has to band the
/// tail of the question, all of the gap between, and the head of the reply.
/// Rendered as a node so the bands can be reviewed and pixel-tested without a
/// window, which is the only way to see that they line up with the glyphs.
fn transcript_selection() -> Model {
    let done = turn_done();
    Model {
        selection: Some(crate::select::Selection::new(
            crate::select::Position {
                message: 0,
                block: 0,
                offset: 8,
            },
            crate::select::Position {
                message: 1,
                block: 0,
                offset: 40,
            },
        )),
        ..done
    }
}

/// Markdown a model actually emits: headings, emphasis, inline code, lists,
/// a quote, and a table. Proves the transcript renders structure rather than
/// echoing punctuation.
fn markdown() -> Model {
    Model {
        transcript: conversation(vec![(
            "summarise the transport".into(),
            "## Transport\n\nThe protocol is **line-delimited JSON** over a \
             *Unix socket*, framed by `\\n`.\n\n\
             - `hello` negotiates the version\n\
             - `subscribe` attaches to a session\n\n\
             > Framing is unchanged across transports.\n\n\
             | frame | direction |\n|---|---|\n| hello | client |\n| hello_ok | server |\n"
                .into(),
        )]),
        ..attached_empty()
    }
}

/// Every inline and block treatment at once, so one capture answers "does
/// markdown read well" rather than needing a state per feature.
///
/// This is the state the typography work is judged against: inline code has to
/// be visibly literal, a link visibly a link, a list visibly one list, a
/// heading visibly attached to the text under it, and a rule visibly a rule
/// rather than three dashes.
fn markdown_typography() -> Model {
    Model {
        transcript: conversation(vec![(
            "walk me through the renderer".into(),
            // Written as one block with explicit newlines rather than with Rust
            // line continuations, because a continuation eats the leading
            // whitespace and a nested list item would silently flatten.
            concat!(
                "# Renderer\n\n",
                "Markdown comes from `jcode-render-core`, so the desktop and the TUI ",
                "agree on what a document *is*. See ",
                "[the notes](https://example.com/notes) for the shape of it.\n\n",
                "## Blocks\n\n",
                "A block is laid out once and reused while it is unchanged:\n\n",
                "- a paragraph wraps to the measure\n",
                "- a `CodeBlock { language }` sits on its own wash\n",
                "  - nested items step in\n",
                "  - and stay one list\n",
                "- a `Table` is columnised by the front-end, and ~~never~~ by the core\n\n",
                "Then, in order:\n\n",
                "1. parse into blocks\n",
                "2. flatten each into spans\n",
                "3. hand the spans to **Parley**\n\n",
                "> Geometry is measured, never estimated.\n\n",
                "---\n\n",
                "### Cost\n\n",
                "Laying out $n$ blocks costs $O(n)$, and a delta re-lays only the ",
                "tail, so the total is\n\n",
                "$$\\sum_{i=1}^{n} c_i \\leq n \\cdot \\max_i c_i$$\n\n",
                "which is why streaming stays flat. Use `--stream-bench` to check it.\n",
            )
            .into(),
        )]),
        ..attached_empty()
    }
}

/// The structural end of markdown: a wide aligned table, a task list, and a
/// list with a fenced block and a quote written *inside* its items.
///
/// These are the cases that read as broken rather than merely plain when the
/// front-end ignores them: a table that runs off the measure loses its right
/// columns, `[x]` renders as source next to a rendered bullet, and a fenced
/// block indented back to the margin breaks its list open.
fn markdown_structure() -> Model {
    Model {
        transcript: conversation(vec![(
            "what changed in the wire format".into(),
            concat!(
                "| field | meaning | bytes |\n",
                "|:--|:-:|--:|\n",
                "| `kind` | which frame this is, and how to read the rest of it | 1 |\n",
                "| `session` | the session the frame belongs to | 16 |\n",
                "| `payload` | length-prefixed body, encoded as line-delimited JSON | 4096 |\n\n",
                "Migration:\n\n",
                "- [x] carry the alignments through the model\n",
                "- [x] budget the columns against the measure\n",
                "- [ ] version the header\n\n",
                "1. read the header\n\n",
                "   ```rust\n",
                "   let kind = Kind::from_u8(bytes[0])?;\n",
                "   ```\n\n",
                "   then dispatch on it.\n\n",
                "2. read the payload\n\n",
                "   > A short frame is a protocol error, never a partial read.\n",
                "   >\n",
                "   > > and a long one is a bug in the sender.\n",
            )
            .into(),
        )]),
        ..attached_empty()
    }
}

/// Inline and display math. The transcript must render these as math, not
/// print the LaTeX source at the user.
fn latex() -> Model {
    Model {
        transcript: conversation(vec![(
            "what is the cost".into(),
            "The march is $O(n^2)$ per frame, with $n$ the grid side.\n\n\
             $$\\frac{a + b}{c}$$\n\n\
             The total work is a sum over rays:\n\n\
             $$\\sum_{i=1}^{n} \\sqrt{x_i^2 + y_i^2} \\leq \\alpha \\cdot \\pi n$$\n\n\
             with the rotation applied as\n\n\
             $$\\begin{pmatrix} \\cos\\theta & -\\sin\\theta \\\\ \\sin\\theta & \\cos\\theta \\end{pmatrix}$$\n\n\
             So halving $n$ quarters the work."
                .into(),
        )]),
        ..attached_empty()
    }
}

/// A fenced code block: it must read as a quoted artefact on its own wash,
/// not as more prose.
fn code_block() -> Model {
    Model {
        transcript: conversation(vec![(
            "show me the handler".into(),
            "Here is the entry point:\n\n```rust\nfn main() -> Result<()> {\n    \
             App::default().run()\n}\n```\n\nIt returns on the first error."
                .into(),
        )]),
        ..attached_empty()
    }
}

fn error() -> Model {
    Model {
        status: "disconnected: daemon connection closed".into(),
        ..turn_done()
    }
}

/// The failure this whole path exists for: the machine is offline, so the turn
/// the user asked for could not run. The report has to be *in the
/// conversation*, because the status line is suppressed for an attached
/// session and a failure nobody can see reads as an app that ignored them.
fn offline() -> Model {
    let mut transcript = conversation(vec![(
        "explain the harness API handshake".into(),
        String::new(),
    )]);
    transcript.push_notice(
        "no network connection: error sending request for \
         url (https://api.anthropic.com/v1/messages): dns error",
    );
    Model {
        transcript,
        busy: false,
        status: "no network connection".into(),
        failure: Some("no network connection".into()),
        ..attached_empty()
    }
}

/// One very long unwrapped paragraph: the transcript must stay inside its own
/// region instead of running down over the composer.
fn long_paragraph() -> Model {
    Model {
        transcript: conversation(vec![(
            "explain everything".into(),
            "the client opens the socket and sends a hello frame carrying its supported version range. "
                .repeat(24),
        )]),
        ..attached_empty()
    }
}

/// A realistic long session: the shape that made the window feel laggy, and
/// the shape no other node covers. Sixty turns is an afternoon of work, not a
/// pathological input.
fn heavy_long_session() -> Model {
    let turns = (0..60)
        .map(|n| {
            (
                format!("question {n} about the transport layer"),
                format!(
                    "answer {n}. {}",
                    "the client opens the socket and sends a hello frame carrying its \
                     supported version range. "
                        .repeat(3)
                ),
            )
        })
        .collect();
    Model {
        transcript: conversation(turns),
        ..attached_empty()
    }
}

/// A reply that is mostly code. Code blocks carry their own wash, inset, and
/// padding, so they cost more per line than prose and are worth measuring
/// separately.
fn heavy_code_wall() -> Model {
    let code = (0..120)
        .map(|n| format!("    let value_{n} = compute(input[{n}], &config, depth + {n});"))
        .collect::<Vec<_>>()
        .join("\n");
    Model {
        transcript: conversation(vec![(
            "show me the whole function".into(),
            format!("Here it is:\n\n```rust\nfn main() {{\n{code}\n}}\n```\n"),
        )]),
        ..attached_empty()
    }
}

/// A wide table. Column widths are measured per cell by the desktop's own
/// table adapter, so this exercises a path prose never touches.
fn heavy_wide_table() -> Model {
    let header = "| frame | direction | payload | notes | since |";
    let rule = "|---|---|---|---|---|";
    let rows = (0..40)
        .map(|n| format!("| frame_{n} | client | {{\"id\": {n}}} | row {n} notes | v0.{n} |"))
        .collect::<Vec<_>>()
        .join("\n");
    Model {
        transcript: conversation(vec![(
            "list every frame".into(),
            format!("{header}\n{rule}\n{rows}\n"),
        )]),
        ..attached_empty()
    }
}

/// Math-heavy output. LaTeX goes through render-core's math translation before
/// it is ever laid out, so a reply full of it is a different cost profile
/// again.
fn heavy_math() -> Model {
    let body = (0..30)
        .map(|n| format!("The bound $x_{{{n}}}^2 + y_{{{n}}}^2 \\leq z_{{{n}}}$ holds.\n\n$$\\frac{{a_{{{n}}}}}{{b_{{{n}}}}} = \\sum_{{i=0}}^{{{n}}} c_i$$"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Model {
        transcript: conversation(vec![("derive the bounds".into(), body)]),
        ..attached_empty()
    }
}
