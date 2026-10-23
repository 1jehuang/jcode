#![cfg(all(unix, feature = "mermaid-renderer"))]

use jcode_tui_markdown::{
    IncrementalMarkdownRenderer, line_is_mermaid_pending_placeholder, render_markdown_lazy,
    render_markdown_with_width,
};
use jcode_tui_mermaid::{
    D2Layout, D2Options, configure_d2, parse_inline_image_placeholder, with_image_protocol_override,
};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

fn text(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn real_markdown_paths_defer_d2_refresh_identical_text_and_keep_source_fallbacks() {
    jcode_tui_mermaid::force_test_kitty_picker();
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("d2");
    let calls = directory.path().join("calls");
    std::fs::write(&executable, format!("#!/bin/sh\n/bin/cat >/dev/null\nprintf x >> '{}'\n/bin/sleep 1\nprintf '<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><rect width=\"100\" height=\"50\" fill=\"blue\"/></svg>'\n", calls.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    configure_d2(
        true,
        D2Options {
            binary: executable,
            layout: D2Layout::Tala,
            ..D2Options::default()
        },
    );
    let markdown = "Before\n\n```D2 title=test\na -> b: local\n```\n\nAfter";
    let mut incremental = IncrementalMarkdownRenderer::new(Some(80));
    with_image_protocol_override(Some(true), || {
        let started = Instant::now();
        let pending = incremental.update(markdown);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "D2 blocked the Markdown caller"
        );
        assert!(pending.iter().any(line_is_mermaid_pending_placeholder));
        let deadline = Instant::now() + Duration::from_secs(8);
        let rendered = loop {
            let lines = incremental.update(markdown);
            if lines
                .iter()
                .any(|line| parse_inline_image_placeholder(line).is_some())
            {
                break lines;
            }
            assert!(
                Instant::now() < deadline,
                "D2 never completed: {}",
                text(&lines)
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(!rendered.iter().any(line_is_mermaid_pending_placeholder));
        assert!(text(&rendered).contains("Before") && text(&rendered).contains("After"));
        let lazy = render_markdown_lazy(markdown, Some(80), 0..100);
        assert!(
            lazy.iter()
                .any(|line| parse_inline_image_placeholder(line).is_some())
        );
        assert_eq!(
            std::fs::read_to_string(&calls).unwrap(),
            "x",
            "full/lazy paths did not share the real cache"
        );

        let mixed = format!("```mermaid\nflowchart LR\nA-->B\n```\n\n{markdown}");
        let both = render_markdown_with_width(&mixed, Some(80));
        assert_eq!(
            both.iter()
                .filter(|line| parse_inline_image_placeholder(line).is_some())
                .count(),
            2
        );
        jcode_tui_markdown::set_config_snapshot_hook(|| {
            jcode_tui_markdown::MarkdownConfigSnapshot {
                mermaid_enabled: false,
                ..Default::default()
            }
        });
        for lines in [
            render_markdown_with_width(&mixed, Some(80)),
            render_markdown_lazy(&mixed, Some(80), 0..100),
        ] {
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| parse_inline_image_placeholder(line).is_some())
                    .count(),
                1
            );
            assert!(
                text(&lines).contains("flowchart LR"),
                "disabled Mermaid source was hidden"
            );
        }
        jcode_tui_markdown::set_config_snapshot_hook(Default::default);

        let unsafe_markdown = "```d2\nnode.icon: /private/forbidden.svg\n```";
        for lines in [
            render_markdown_with_width(unsafe_markdown, Some(80)),
            render_markdown_lazy(unsafe_markdown, Some(80), 0..100),
        ] {
            assert!(text(&lines).contains("node.icon"));
            assert!(text(&lines).contains("D2:"));
            assert!(!text(&lines).contains("mermaid error"));
        }
        let open = incremental.update("```d2\nx -> y");
        assert!(text(&open).contains("x -> y"));
        assert!(!open.iter().any(line_is_mermaid_pending_placeholder));
        assert_eq!(
            std::fs::read_to_string(&calls).unwrap(),
            "x",
            "unsafe or unfinished source launched compiler"
        );
    });
    with_image_protocol_override(Some(false), || {
        let source = render_markdown_with_width(markdown, Some(80));
        assert!(text(&source).contains("a -> b: local"));
        assert!(
            !source
                .iter()
                .any(|line| parse_inline_image_placeholder(line).is_some())
        );
    });
}
