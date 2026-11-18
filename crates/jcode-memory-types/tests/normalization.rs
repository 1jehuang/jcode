use jcode_memory_types::{normalize_memory_search_text, normalize_search_text};

// Original implementation retained as an independent semantic oracle and paired
// benchmark baseline. Do not simplify this to call production normalization.
fn original_text(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let mut normalized = String::with_capacity(lowered.len());
    let mut last_was_space = true;
    for ch in lowered.chars() {
        let mapped = if ch.is_whitespace() || matches!(ch, '-' | '_' | '/' | '\\' | '.' | ':') {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }
    normalized.trim_end().to_string()
}

fn original_memory(content: &str, tags: &[String]) -> String {
    let content = original_text(content);
    let tags: Vec<String> = tags
        .iter()
        .map(|tag| original_text(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.is_empty() {
        return content;
    }
    if content.is_empty() {
        return tags.join(" ");
    }
    format!("{} {}", content, tags.join(" "))
}

#[test]
fn normalization_matches_original_for_all_unicode_scalars() {
    for ch in (0..=0x10ffff).filter_map(char::from_u32) {
        let text = format!(" /A{ch}B_ ");
        assert_eq!(
            normalize_search_text(&text),
            original_text(&text),
            "U+{:04X}",
            ch as u32
        );
    }
}

#[test]
fn memory_normalization_preserves_component_boundaries_and_unicode_context() {
    let cases = [
        "",
        " --/ ",
        " A\u{000b}B ",
        "ΟΣ",
        "ΟΣ Α",
        "İSTANBUL",
        "\u{2003}Hello\u{00a0}World",
        "Foo_Bar/Baz:Qux.",
        "a\0B",
    ];
    for content in cases {
        for a in cases {
            for b in cases {
                let tags = vec![a.to_owned(), b.to_owned()];
                assert_eq!(
                    normalize_memory_search_text(content, &tags),
                    original_memory(content, &tags)
                );
            }
        }
    }
}

#[test]
#[ignore = "paired CPU microbenchmark, run with --profile selfdev --ignored --nocapture"]
fn paired_memory_normalization_microbenchmark() {
    use std::{hint::black_box, time::Instant};
    let contents = [
        "Project preferences: preserve existing working-tree changes and run focused regression tests before committing.",
        "Use hosted justrust check/test for Rust projects. Never silently disable diagnostics or persistence.",
        "Unicode ΟΣ and İSTANBUL / paths_with-tags: keep normalization semantics unchanged.",
    ];
    let entries: Vec<_> = (0..512)
        .map(|i| {
            (
                format!("{} Entry {i}", contents[i % contents.len()]),
                vec![
                    "Project_Config".to_owned(),
                    "Rust/Testing".to_owned(),
                    "  ".to_owned(),
                ],
            )
        })
        .collect();
    for (content, tags) in &entries {
        assert_eq!(
            original_memory(content, tags),
            normalize_memory_search_text(content, tags)
        );
    }
    let run = |normalize: fn(&str, &[String]) -> String| {
        let start = Instant::now();
        for _ in 0..200 {
            for (content, tags) in &entries {
                black_box(normalize(black_box(content), black_box(tags)));
            }
        }
        start.elapsed()
    };
    let mut old = Vec::new();
    let mut new = Vec::new();
    for round in 0..9 {
        let (a, b) = if round % 2 == 0 {
            (run(original_memory), run(normalize_memory_search_text))
        } else {
            let b = run(normalize_memory_search_text);
            (run(original_memory), b)
        };
        eprintln!("round {round}: original={a:?} optimized={b:?}");
        old.push(a);
        new.push(b);
    }
    old.sort();
    new.sort();
    eprintln!(
        "paired normalization median: original={:?} optimized={:?} speedup={:.3}x",
        old[4],
        new[4],
        old[4].as_secs_f64() / new[4].as_secs_f64()
    );
}
