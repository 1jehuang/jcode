#![cfg(all(unix, feature = "mermaid-renderer"))]

use jcode_tui_markdown::IncrementalMarkdownRenderer;
use jcode_tui_mermaid::{
    D2Layout, D2Options, configure_d2, force_test_kitty_picker, parse_inline_image_placeholder,
    with_image_protocol_override,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

fn compiler_starts(path: &Path) -> usize {
    std::fs::read(path).map(|bytes| bytes.len()).unwrap_or(0)
}

fn image_hash(lines: &[ratatui::text::Line<'_>]) -> Option<u64> {
    lines
        .iter()
        .find_map(|line| parse_inline_image_placeholder(line).map(|image| image.0))
}

#[test]
fn independent_closed_d2_fences_finish_while_both_renderers_append_prose() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("d2");
    let calls = directory.path().join("calls");
    std::fs::write(&executable, format!(
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf x >> '{}'\n/bin/sleep 0.15\nprintf '<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><rect width=\"100\" height=\"50\" fill=\"blue\"/></svg>'\n",
        calls.display(),
    )).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    configure_d2(
        true,
        D2Options {
            binary: executable,
            layout: D2Layout::Tala,
            ..Default::default()
        },
    );
    force_test_kitty_picker();

    with_image_protocol_override(Some(true), || {
        // Positive control proves the same slow compiler and public consumer
        // can produce an image before testing interference between consumers.
        let mut solo = IncrementalMarkdownRenderer::new(Some(80));
        let mut solo_text = "```d2\ncontrol_source -> control_target\n```\n\n".to_owned();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            solo_text.push_str("positive control prose\n");
            if image_hash(&solo.update(&solo_text)).is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "solo positive control never became ready"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(compiler_starts(&calls), 1, "solo fence was recompiled");
        eprintln!("solo positive control ready after one compiler start");

        let mut renderers = [
            IncrementalMarkdownRenderer::new(Some(80)),
            IncrementalMarkdownRenderer::new(Some(80)),
        ];
        let mut documents = [
            "```d2\nleft_source -> left_target\n```\n\n".to_owned(),
            "```d2\nright_source -> right_target\n```\n\n".to_owned(),
        ];
        let initial_starts = compiler_starts(&calls);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ready = [None, None];
        let mut updates = 0;
        while Instant::now() < deadline {
            for index in 0..renderers.len() {
                // Each source remains its renderer's first, already closed
                // fence. Only surrounding prose changes on every update.
                documents[index].push_str("more surrounding prose\n");
                ready[index] = image_hash(&renderers[index].update(&documents[index]));
            }
            updates += 1;
            if ready.iter().all(Option::is_some) {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let starts = compiler_starts(&calls) - initial_starts;
        eprintln!(
            "concurrent streams: updates={updates}, compiler starts={starts}, ready={ready:?}"
        );
        assert!(
            ready.iter().all(Option::is_some),
            "independent closed fences stayed pending: updates={updates}, starts={starts}, ready={ready:?}"
        );
        assert_ne!(ready[0], ready[1], "independent sources shared an artifact");
        assert_eq!(
            starts, 2,
            "unchanged closed fences launched duplicate compilers"
        );

        for index in 0..renderers.len() {
            documents[index].push_str("even more prose after completion\n");
            assert_eq!(
                image_hash(&renderers[index].update(&documents[index])),
                ready[index]
            );
        }
        assert_eq!(
            compiler_starts(&calls) - initial_starts,
            2,
            "warm prose updates recompiled a closed fence"
        );
    });
}
