//! Measure the palette topology from real rendered frames.
//!
//! The graph model in `jcode-tui-style::harmony::graph` needs to know which
//! roles actually cover screen area and which ones sit next to each other. Both
//! are properties of the rendered UI, not of the palette, so they are measured
//! here by rendering frames and attributing every cell back to a role.
//!
//! Run with `--ignored --nocapture` to print a fresh `Topology`; the printed
//! counts are what belongs in the checked-in default.

use super::TestState;
use jcode_tui_messages::DisplayMessage;
use jcode_tui_style::palette::role_for_rendered;
use ratatui::style::Color;
use std::collections::BTreeMap;

/// Render a set of representative frames and tally role area plus adjacency.
fn measure() -> (
    BTreeMap<&'static str, u32>,
    BTreeMap<(&'static str, &'static str), u32>,
) {
    // Attribution matches rendered RGB back to role defaults, so the frame
    // must be rendered in truecolor. A hosted CI runner without COLORTERM
    // detects 256-color and quantizes every cell, which pushed most colors
    // out of their role's family radius and left the adjacency graph nearly
    // empty (4 edges instead of the required 5+).
    jcode_tui_style::color::pin_truecolor_for_tests();
    let mut area: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut touches: BTreeMap<(&'static str, &'static str), u32> = BTreeMap::new();

    // A few sizes, so layout-dependent widgets (wrapping, panes) contribute.
    for (width, height) in [(80u16, 24u16), (120, 40), (60, 20)] {
        let message = |role: &str, content: &str| DisplayMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        };
        let app = TestState {
            display_messages: vec![
                message("user", "hello there"),
                message(
                    "assistant",
                    "Here is **bold**, `code`, and a path src/main.rs to click.",
                ),
                message("system", "system notice"),
                DisplayMessage::error("something failed"),
            ],
            input: "next question".to_string(),
            ..Default::default()
        };

        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();

        let area_rect = *buffer.area();
        let role_at = |x: u16, y: u16| -> Option<&'static str> {
            let cell = &buffer[(x, y)];
            if cell.symbol().trim().is_empty() {
                return None;
            }
            if cell.fg == Color::Reset {
                return None;
            }
            role_for_rendered(cell.fg).map(|role| role.key())
        };

        for y in area_rect.top()..area_rect.bottom() {
            for x in area_rect.left()..area_rect.right() {
                let Some(role) = role_at(x, y) else { continue };
                *area.entry(role).or_default() += 1;
                // Adjacency: the cell to the right and the cell below. Counting
                // both directions once each keeps the graph undirected without
                // double counting a pair.
                for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                    if nx >= area_rect.right() || ny >= area_rect.bottom() {
                        continue;
                    }
                    let Some(other) = role_at(nx, ny) else {
                        continue;
                    };
                    if other == role {
                        continue;
                    }
                    let key = if role <= other {
                        (role, other)
                    } else {
                        (other, role)
                    };
                    *touches.entry(key).or_default() += 1;
                }
            }
        }
    }
    (area, touches)
}

/// Print a `Topology` literal for the current UI.
#[test]
#[ignore = "reporting helper: regenerates the checked-in topology"]
fn print_measured_palette_topology() {
    let (area, touches) = measure();
    println!("// nodes: role area in rendered cells");
    for (role, cells) in &area {
        println!("    (Role::{}, {}),", camel(role), cells);
    }
    println!("// edges: measured adjacency");
    let mut edges: Vec<_> = touches.iter().collect();
    edges.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for ((left, right), n) in edges.iter().take(40) {
        println!(
            "    (Role::{}, Role::{}, {}),",
            camel(left),
            camel(right),
            n
        );
    }
}

fn camel(key: &str) -> String {
    key.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The measurement must find real structure, otherwise the graph model would be
/// scoring an empty layout and silently reporting perfect results.
#[test]
fn measured_topology_covers_real_area_and_adjacency() {
    let (area, touches) = measure();
    assert!(
        area.len() >= 5,
        "expected several roles to render, got {:?}",
        area.keys().collect::<Vec<_>>()
    );
    assert!(
        touches.len() >= 5,
        "expected roles to touch each other, got {} edges",
        touches.len()
    );
    let total: u32 = area.values().sum();
    assert!(
        total > 200,
        "expected substantial painted area, got {total}"
    );
}
