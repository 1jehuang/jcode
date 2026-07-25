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
    ("attached_empty", attached_empty),
    ("donut_dragged", donut_dragged),
    ("donut_off", donut_off),
    ("mid_input", mid_input),
    ("mid_input_caret_inside", mid_input_caret_inside),
    ("caret_hidden", caret_hidden),
    ("unfocused", unfocused),
    ("selection", selection),
    ("multiline", multiline),
    ("wrapped_long_line", wrapped_long_line),
    ("multiline_selection", multiline_selection),
    ("selection_all", selection_all),
    ("streaming", streaming),
    ("turn_done", turn_done),
    ("scrolled_back", scrolled_back),
    ("notice", notice),
    ("error", error),
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
        theme: crate::theme::Theme::from_env(),
        meta: fixed_meta(),
        status: "connecting to ~/.jcode/jcode-api.sock...".into(),
        session_id: None,
        transcript: String::new(),
        editor: crate::editor::Editor::default(),
        caret: fixed_caret(),
        // Nodes render the focused case: an unfocused window hides the caret,
        // which would make most caret nodes indistinguishable.
        focused: true,
        busy: false,
        scroll: 0,
        notice: None,
        donut: Some(fixed_donut()),
        spin: fixed_spin(),
        // Captures pin the hint, so the ghost line is a tested state rather
        // than whatever the clock happened to pick.
        hint: 0,
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
        theme: crate::theme::Theme::from_env(),
        meta: fixed_meta(),
        status: "attached: session_demo_0000".into(),
        session_id: Some("session_demo_0000".into()),
        transcript: String::new(),
        editor: crate::editor::Editor::default(),
        caret: fixed_caret(),
        // Nodes render the focused case: an unfocused window hides the caret,
        // which would make most caret nodes indistinguishable.
        focused: true,
        busy: false,
        scroll: 0,
        notice: None,
        donut: Some(fixed_donut()),
        spin: fixed_spin(),
        // Captures pin the hint, so the ghost line is a tested state rather
        // than whatever the clock happened to pick.
        hint: 0,
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
        transcript: (1..=60)
            .map(|n| format!("transcript line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        scroll: 12,
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

fn streaming() -> Model {
    Model {
        transcript: "\n> explain the harness API handshake\n\n\
            The client opens the socket and sends a `hello` frame carrying \
            its supported version range. The server replies with `hello_ok` \
            and the negotiated version, after which"
            .into(),
        busy: true,
        ..attached_empty()
    }
}

fn turn_done() -> Model {
    Model {
        transcript: "\n> explain the harness API handshake\n\n\
            The client opens the socket and sends a `hello` frame carrying \
            its supported version range. The server replies with `hello_ok` \
            and the negotiated version, after which normal requests flow.\n"
            .into(),
        busy: false,
        ..attached_empty()
    }
}

fn error() -> Model {
    Model {
        status: "disconnected: daemon connection closed".into(),
        ..turn_done()
    }
}
