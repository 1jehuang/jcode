use super::*;

fn original(s: &Session) -> Output {
    final_text_original::render_messages_and_images_with_compacted_history(s, 64)
}
fn cases() -> Vec<(&'static str, Session)> {
    let session = |blocks| {
        let mut s = Session::create_with_id("final-text-probe".into(), None, None);
        s.add_message(Role::Assistant, blocks);
        s
    };
    let reasoning = || ContentBlock::ReasoningTrace {
        text: "r".repeat(256 * 1024),
    };
    vec![
        ("empty", Session::create_with_id("empty".into(), None, None)),
        ("tiny", session(vec![text("tiny")])),
        ("plain-large", session(vec![text(&"x".repeat(256 * 1024))])),
        ("reasoning-only", session(vec![reasoning()])),
        (
            "mixed",
            session(vec![reasoning(), text(&"x".repeat(256 * 1024))]),
        ),
        (
            "images",
            session(vec![image(), text(&"x".repeat(256 * 1024))]),
        ),
        ("no-text", session(vec![image()])),
    ]
}
fn parity(s: &Session) {
    assert_eq!(
        serde_json::to_value(original(s)).unwrap(),
        serde_json::to_value(optimized(s)).unwrap()
    );
}
#[test]
fn final_text_frozen_parity() {
    for (_, s) in cases() {
        parity(&s);
    }
    // Include empty retained buffers following both earlier flush paths, joined
    // label fragments, suppressed labels and user-image prompt ordinals.
    for role in [Role::User, Role::Assistant] {
        for blocks in [
            vec![text("before"), result("a"), text("after"), image()],
            vec![text("before"), native(), image()],
            vec![
                text("[Attached image associated with the preceding tool result: "),
                text("joined.png]"),
                image(),
            ],
            vec![
                image(),
                text("[Attached image associated with the preceding tool result: label.png]"),
            ],
            vec![
                ContentBlock::Reasoning {
                    text: "thought".into(),
                },
                image(),
            ],
            vec![text(""), result("a")],
        ] {
            let mut s = Session::create_with_id("final-edges".into(), None, None);
            s.add_message(role.clone(), blocks);
            s.add_message(Role::User, vec![image(), text("next prompt")]);
            parity(&s);
        }
    }
}

// Run alone with JCODE_REASONING_DISPLAY=full and a dedicated JCODE_HOME.
#[test]
#[ignore = "isolated alternating thread CPU/allocation measurement"]
#[cfg(target_os = "linux")]
fn final_text_cpu_allocation_probe() {
    assert_eq!(
        crate::config::config().display.reasoning_display(),
        ReasoningDisplayMode::Full
    );
    for (name, s) in cases() {
        parity(&s);
        if name == "reasoning-only" {
            assert!(!optimized(&s).0.is_empty());
        }
        for _ in 0..64 {
            drop(original(&s));
            drop(optimized(&s));
        }
        let old_alloc = allocations(original, &s);
        let new_alloc = allocations(optimized, &s);
        assert!(new_alloc.0 <= old_alloc.0 && new_alloc.1 <= old_alloc.1);
        let iterations = if matches!(name, "empty" | "tiny" | "no-text") {
            20_000
        } else {
            500
        };
        let measure = |f: fn(&Session) -> Output| {
            let start = thread_cpu_ns();
            for _ in 0..iterations {
                drop(black_box(f(black_box(&s))));
            }
            thread_cpu_ns() - start
        };
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..12 {
            if round % 2 == 0 {
                old.push(measure(original));
                new.push(measure(optimized));
            } else {
                new.push(measure(optimized));
                old.push(measure(original));
            }
        }
        old.sort_unstable();
        new.sort_unstable();
        let ratio = new[6] as f64 / old[6] as f64;
        println!(
            "{name}: CPU new/old={ratio:.4}, old={}ns new={}ns allocations {old_alloc:?} -> {new_alloc:?}",
            old[6], new[6]
        );
        assert!(ratio <= 1.05, "{name}: CPU regression {ratio}");
    }
}
