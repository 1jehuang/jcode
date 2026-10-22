//! Process-level regression test for the Mermaid Markdown gate inside
//! herdr-webui panes.
//!
//! Motivation (greptile PR-thread finding): with `HERDR_WEBUI=1` the protocol
//! picker stays on Halfblocks, but the Markdown layer's
//! `should_render_mermaid_block()` used to require
//! `native_image_protocol_available()`, which rejects Halfblocks. Completed
//! Mermaid fences therefore rendered as highlighted source with no diagram
//! registered, even though the whole point of the webui gate is that the
//! browser terminal draws Halfblocks text art.
//!
//! This test spawns child test binaries with the exact pane environment the
//! builtin webui backend exports (`HERDR_WEBUI=1`, `TERM_PROGRAM=ghostty`,
//! `KITTY_WINDOW_ID` scrubbed). The picker is never initialized in the child,
//! so no native image protocol is available and the only path to the diagram
//! render is the HERDR_WEBUI text-art exception. The webui child must render
//! a completed Mermaid fence as a registered diagram, not highlighted source;
//! a control child without `HERDR_WEBUI` must keep showing source, proving
//! the gate is still intact for genuinely graphics-less terminals.
//!
//! The tested signal is process-global (env var + global diagram registry),
//! so the child-process pattern (same as tests/webui_protocol_gate.rs in
//! jcode-tui-mermaid) keeps the parent test deterministic.

#![cfg(feature = "mermaid-renderer")]

use std::process::Command;

use jcode_tui_markdown::DiagramDisplayMode;

const CHILD_MODE_VAR: &str = "JCODE_TEST_WEBUI_MARKDOWN_CHILD";
const TEST_NAME: &str = "webui_mermaid_markdown_gate_child";

fn spawn_gate_child(mode: &'static str) -> std::process::Output {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = Command::new(exe);
    command
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(CHILD_MODE_VAR, mode)
        // Pin the pane environment the builtin webui backend exports.
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "ghostty")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("LC_TERMINAL")
        .env_remove("JCODE_MERMAID_PICKER_PROBE");
    match mode {
        "webui" => {
            command.env("HERDR_WEBUI", "1");
        }
        "control" => {
            command.env_remove("HERDR_WEBUI");
        }
        other => panic!("unknown gate child mode: {other}"),
    }
    command.output().expect("spawn webui markdown gate child")
}

fn require_gate_child(mode: &'static str, expected: &str) {
    let output = spawn_gate_child(mode);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{mode} child failed with {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
    let marker = format!("GATE_RESULT={expected}");
    assert!(
        stdout.contains(&marker),
        "{mode} child did not report {marker}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn render_completed_mermaid_fence() -> (String, usize) {
    // Completed (non-streaming) render, matching the greptile repro. Pin the
    // diagram display mode so completed diagrams register in the global
    // active-diagram registry, which the pane reads for the dedicated diagram
    // view; the default config mode (None) skips registration.
    let md = "```mermaid\nflowchart LR\n    A --> B\n```";
    jcode_tui_markdown::set_diagram_mode_override(Some(DiagramDisplayMode::Pinned));
    let lines = jcode_tui_markdown::render_markdown_with_width(md, Some(80));
    jcode_tui_markdown::set_diagram_mode_override(None);
    let text = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();
    let diagrams = jcode_tui_mermaid::get_active_diagrams().len();
    (text, diagrams)
}

#[test]
fn webui_mermaid_markdown_gate_child() {
    // Child mode: render a completed Mermaid fence under the inherited
    // environment and report whether the diagram path ran.
    if let Ok(mode) = std::env::var(CHILD_MODE_VAR) {
        let expect_diagram = match mode.as_str() {
            "webui" => true,
            "control" => false,
            other => panic!("unknown gate child mode: {other}"),
        };
        let (text, diagrams) = render_completed_mermaid_fence();
        // Source rendering shows the fenced source verbatim and registers
        // nothing; the diagram path never echoes the raw source lines.
        let shows_source = text.contains("flowchart LR");
        assert_eq!(
            shows_source, !expect_diagram,
            "{mode} child gate mismatch:\n{text}"
        );
        assert_eq!(
            diagrams > 0,
            expect_diagram,
            "{mode} child expected {} diagram, got {diagrams}\n{text}",
            if expect_diagram { "a registered" } else { "no" }
        );
        println!(
            "GATE_RESULT={}",
            if shows_source { "source" } else { "diagram" }
        );
        return;
    }
    require_gate_child("webui", "diagram");
    require_gate_child("control", "source");
}
