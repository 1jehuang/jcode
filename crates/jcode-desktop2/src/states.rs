//! State-space nodes for the UI.
//!
//! `build_scene` is a pure function of `Model`, so the app's visual states
//! form an enumerable graph. Each named node here is a deterministic `Model`
//! that can be rendered offscreen (`--capture <node> <out.png>`) for visual
//! verification without a window, compositor, or screenshots.

use crate::Model;

/// All named state-space nodes. Keep deterministic: no clocks, no randomness.
pub const NODES: &[(&str, fn() -> Model)] = &[
    ("connecting", connecting),
    ("attached_empty", attached_empty),
    ("mid_input", mid_input),
    ("streaming", streaming),
    ("turn_done", turn_done),
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
        status: "connecting to ~/.jcode/jcode-api.sock...".into(),
        session_id: None,
        transcript: String::new(),
        input: String::new(),
        busy: false,
    }
}

fn attached_empty() -> Model {
    Model {
        status: "attached: session_demo_0000".into(),
        session_id: Some("session_demo_0000".into()),
        transcript: String::new(),
        input: String::new(),
        busy: false,
    }
}

fn mid_input() -> Model {
    Model {
        input: "explain the harness API handshake".into(),
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
