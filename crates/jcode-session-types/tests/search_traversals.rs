//! Exact legacy oracle plus an isolated single-allocator optimized Linux probe.
use jcode_session_types::*;
mod legacy {
    use super::*;
    include!("search_traversal_legacy.inc");
}
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
struct Allocator;
thread_local! { static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNTS.try_with(|c| {
            if let Some((n, b)) = c.get() {
                c.set(Some((n + 1, b + l.size())));
            }
        });
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, s: usize) -> *mut u8 {
        let _ = COUNTS.try_with(|c| {
            if let Some((n, b)) = c.get() {
                c.set(Some((n + 1, b + s)));
            }
        });
        unsafe { System.realloc(p, l, s) }
    }
}
fn compare(text: &str, q: &SessionSearchQueryProfile) {
    let a = legacy::score_session_search_text_match(text, q);
    let b = score_session_search_text_match(text, q);
    match (a, b) {
        (None, None) => (),
        (Some(a), Some(b)) => {
            assert_eq!(a.score.to_bits(), b.score.to_bits());
            assert_eq!(a.snippet, b.snippet);
            assert_eq!(a.matched_terms, b.matched_terms);
            assert_eq!(a.exact_match, b.exact_match);
        }
        _ => panic!("score mismatch {text:?} {q:?}"),
    }
    assert_eq!(
        legacy::normalized_session_search_text_matches(text, q),
        normalized_session_search_text_matches(text, q)
    );
    for bytes in [text.as_bytes(), &[255, b'a', b'A', 0][..]] {
        assert_eq!(
            legacy::session_search_raw_matches_query(bytes, q),
            session_search_raw_matches_query(bytes, q)
        );
    }
}
#[test]
fn exact_legacy_custom_profiles() {
    let terms = [
        vec![],
        vec![""],
        vec!["a", "a", ""],
        vec!["aa", "a", "Σ", "é"],
        vec!["absent", "alpha", "β"],
    ];
    for text in [
        "",
        "aaaa",
        "ALPHA alpha β",
        "ΟΣ Σ é İ i\u{307}",
        "\0alpha\nβ",
        "a",
    ] {
        for normalized in ["", "a", "alpha", "Σ", "absent"] {
            for terms in &terms {
                for min in 0..=terms.len() + 1 {
                    compare(
                        text,
                        &SessionSearchQueryProfile {
                            normalized: normalized.into(),
                            terms: terms.iter().map(|s| s.to_string()).collect(),
                            min_term_matches: min,
                        },
                    );
                }
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn cpu() -> u128 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) },
        0
    );
    t.tv_sec as u128 * 1_000_000_000 + t.tv_nsec as u128
}
#[test]
#[ignore = "optimized alternating Linux thread CPU probe"]
#[cfg(target_os = "linux")]
fn search_traversal_probe() {
    assert!(!cfg!(debug_assertions));
    let cases = [
        ("single-hit", "7".into(), "7"),
        ("single-no-hit", "y".into(), "7"),
        ("overlapping", "aaaaaaa".into(), "aa aaa"),
        ("exact", "alpha beta".into(), "alpha beta"),
        ("term-only", "beta something alpha".into(), "alpha beta"),
        ("no-hits", "nothing relevant".into(), "alpha beta"),
        (
            "many-terms",
            "alpha beta gamma".into(),
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda",
        ),
        (
            "large-ascii-early",
            format!("alpha beta {}", "padding ".repeat(16000)),
            "alpha beta gamma",
        ),
        (
            "large-ascii-late",
            format!("{}alpha beta", "padding ".repeat(16000)),
            "alpha beta gamma",
        ),
        (
            "large-ascii-no-hits",
            "padding ".repeat(16000),
            "alpha beta gamma",
        ),
        (
            "large-unicode-early",
            format!("alpha βeta {}", "ΟΣ İ é 中文 ".repeat(10000)),
            "alpha βeta gamma",
        ),
        (
            "large-unicode",
            format!("{}alpha βeta", "ΟΣ İ é 中文 ".repeat(10000)),
            "alpha βeta gamma",
        ),
        (
            "repeated-hits",
            "alpha beta gamma ".repeat(8000),
            "alpha gamma",
        ),
    ];
    for (name, text, query) in cases {
        let q = SessionSearchQueryProfile::new(query);
        compare(&text, &q);
        for kind in 0..3 {
            let run = |new: bool| match (kind, new) {
                (0, false) => {
                    black_box(legacy::score_session_search_text_match(
                        black_box(&text),
                        black_box(&q),
                    ));
                }
                (0, true) => {
                    black_box(score_session_search_text_match(
                        black_box(&text),
                        black_box(&q),
                    ));
                }
                (1, false) => {
                    black_box(legacy::normalized_session_search_text_matches(
                        black_box(&text),
                        black_box(&q),
                    ));
                }
                (1, true) => {
                    black_box(normalized_session_search_text_matches(
                        black_box(&text),
                        black_box(&q),
                    ));
                }
                (2, false) => {
                    black_box(legacy::session_search_raw_matches_query(
                        black_box(text.as_bytes()),
                        black_box(&q),
                    ));
                }
                (2, true) => {
                    black_box(session_search_raw_matches_query(
                        black_box(text.as_bytes()),
                        black_box(&q),
                    ));
                }
                _ => unreachable!(),
            };
            let mut allocs = Vec::new();
            for new in [false, true] {
                COUNTS.with(|c| c.set(Some((0, 0))));
                run(new);
                allocs.push(COUNTS.with(|c| c.replace(None).unwrap()));
            }
            assert!(allocs[1].0 <= allocs[0].0 && allocs[1].1 <= allocs[0].1);
            let n = (2_000_000 / text.len().max(1)).clamp(16, 20000);
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..21 {
                for slot in 0..2 {
                    let i = (round + slot) % 2;
                    let start = cpu();
                    for _ in 0..n {
                        run(i == 1);
                    }
                    samples[i].push((cpu() - start) / n as u128);
                }
            }
            for s in &mut samples {
                s.sort_unstable();
            }
            println!(
                "{name} kind={kind} legacy={} new={} ratio={:.3} alloc={:?}->{:?}",
                samples[0][10],
                samples[1][10],
                samples[1][10] as f64 / samples[0][10] as f64,
                allocs[0],
                allocs[1]
            );
        }
    }
}
