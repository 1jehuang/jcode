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
    ("mid_input", mid_input),
    ("mid_input_caret_inside", mid_input_caret_inside),
    ("caret_hidden", caret_hidden),
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

fn connecting() -> Model {
    Model {
        theme: crate::theme::Theme::from_env(),
        status: "connecting to ~/.jcode/jcode-api.sock...".into(),
        session_id: None,
        transcript: String::new(),
        editor: crate::editor::Editor::default(),
        caret: fixed_caret(),
        busy: false,
        scroll: 0,
        notice: None,
    }
}

/// Captures must be a pure function of the model, so nodes pin the caret
/// instead of letting it blink on wall-clock time.
fn fixed_caret() -> crate::caret::Caret {
    crate::caret::Caret::pinned(true)
}

fn attached_empty() -> Model {
    Model {
        theme: crate::theme::Theme::from_env(),
        status: "attached: session_demo_0000".into(),
        session_id: Some("session_demo_0000".into()),
        transcript: String::new(),
        editor: crate::editor::Editor::default(),
        caret: fixed_caret(),
        busy: false,
        scroll: 0,
        notice: None,
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
