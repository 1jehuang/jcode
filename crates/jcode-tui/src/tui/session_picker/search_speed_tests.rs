//! Manual, deterministic resume-search microbenchmarks. See the baseline guide.
use super::super::{SessionPicker, SessionSource, performance_tests};
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::json;
use std::hint::black_box;

const WORDS: &str = "config deployment performance synchronization throughput ";
const TYPO: &str = "conifg";
const ABSENT: &str = "quartz";

fn repeated_metadata(bytes: usize) -> String {
    let mut text = WORDS.repeat(bytes.div_ceil(WORDS.len()));
    text.truncate(bytes);
    text
}

fn fixtures(count: usize, bytes: usize, missing: &Path) -> Vec<SessionInfo> {
    (0..count)
        .map(|id| {
            let mut session = performance_tests::session(id, 0);
            session.search_index = repeated_metadata(bytes);
            session.first_user_prompt = None;
            session.source = SessionSource::Codex;
            session.resume_target = ResumeTarget::CodexSession {
                session_id: session.id.clone(),
                session_path: missing.to_string_lossy().into_owned(),
            };
            session.external_path = Some(missing.to_string_lossy().into_owned());
            session
        })
        .collect()
}

fn sample_count() -> usize {
    std::env::var("JCODE_SEARCH_SPEED_SAMPLES")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .expect("positive integer sample count")
        })
        .unwrap_or(2)
        .clamp(1, 100)
}

fn report(
    name: &str,
    query: &str,
    sessions: usize,
    bytes: usize,
    cache: &str,
    values: &[Duration],
) {
    let mut ms: Vec<_> = values.iter().map(|v| v.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let p = |percent: usize| ms[(ms.len() * percent).div_ceil(100).saturating_sub(1)];
    eprintln!(
        "SEARCH_SPEED {}",
        json!({
            "case": name, "query": query, "sessions": sessions,
            "bytes_per_session": bytes, "samples": ms.len(), "cache": cache,
            "ms": ms, "p50_ms": p(50), "p95_ms": p(95),
            "assertions": "passed", "debounce_included": false,
        })
    );
}

/// Measures the real worker, including scheduling and polling, not reset/copy time.
/// An absent query has no first result, represented as JSON null, never completion.
fn worker_sample(
    sessions: &[SessionInfo],
    query: &str,
    expected: usize,
) -> (Option<Duration>, Duration) {
    let mut search = TranscriptSearch::default();
    search.reset(query, sessions.iter());
    assert!(search.pending());
    search.started_at = Some(Instant::now());
    let start = Instant::now();
    let mut first = None;
    while search.pending() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "worker timed out for {query}"
        );
        search.poll();
        if first.is_none() && !search.ranks.is_empty() {
            first = Some(start.elapsed());
        }
        if search.pending() {
            std::thread::sleep(Duration::from_micros(100));
        }
    }
    let complete = start.elapsed();
    assert_eq!(
        search.ranks.len(),
        expected,
        "worker result count for {query}"
    );
    assert_eq!(first.is_some(), expected != 0);
    for session in sessions {
        assert_eq!(search.contains(&session.id), expected != 0);
    }
    (first, complete)
}

#[test]
#[ignore = "manual search-speed baseline, run serial with --nocapture"]
fn benchmark_resume_search_speed() {
    let total = Instant::now();
    let samples = sample_count();
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("intentionally-absent.jsonl");
    assert!(!missing.exists());
    for bytes in [16 * 1024, 64 * 1024] {
        let sessions = fixtures(100, bytes, &missing);
        assert!(sessions.iter().all(|s| s.search_index.len() == bytes));
        for (query, expected) in [(TYPO, 100), (ABSENT, 0)] {
            let tokens = search_query_tokens(query);
            let mut metadata = Vec::new();
            let mut first = Vec::new();
            let mut complete = Vec::new();
            for _ in 0..samples {
                let start = Instant::now();
                for session in &sessions {
                    let rank = black_box(fuzzy_metadata_rank(
                        black_box(&session.search_index),
                        &tokens,
                    ));
                    assert!(!rank.literal);
                    assert_eq!(rank.score > 0, expected != 0, "metadata score for {query}");
                }
                metadata.push(start.elapsed());
                let (first_result, completion) = worker_sample(&sessions, query, expected);
                first.extend(first_result);
                complete.push(completion);
            }
            report(
                "metadata_fuzzy_rank",
                query,
                100,
                bytes,
                "resident_metadata_no_cross_query_score_warmup",
                &metadata,
            );
            if first.is_empty() {
                eprintln!(
                    "SEARCH_SPEED {}",
                    json!({"case":"worker_first_result", "query":query,
                    "sessions":100,"bytes_per_session":bytes,"samples":samples,"ms":null,
                    "cache":"resident_metadata_missing_transcript", "reason":"asserted_no_matches",
                    "assertions":"passed","debounce_included":false})
                );
            } else {
                report(
                    "worker_first_result",
                    query,
                    100,
                    bytes,
                    "resident_metadata_missing_transcript",
                    &first,
                );
            }
            report(
                "worker_completion",
                query,
                100,
                bytes,
                "resident_metadata_missing_transcript",
                &complete,
            );
        }
    }

    // One decoded segment with heavily repeated words exposes repeated fuzzy DP.
    // Populate only decoded-text cache, not the fuzzy query's score cache.
    let path = temp.path().join("repeated.jsonl");
    let text = repeated_metadata(128 * 1024);
    std::fs::write(&path, format!("{}\n", json!({"text":text}))).unwrap();
    let paths = [path];
    let cancel = AtomicBool::new(false);
    assert!(!paths_match_mode(&paths, &[ABSENT.into()], &cancel, false));
    let mut scans = Vec::new();
    for _ in 0..samples {
        let start = Instant::now();
        assert_eq!(
            black_box(paths_match_scored(&paths, &[ABSENT.into()], &cancel)),
            None
        );
        scans.push(start.elapsed());
    }
    report(
        "transcript_fuzzy_absent_repeated_words",
        ABSENT,
        1,
        text.len(),
        "decoded_text_warm_fuzzy_query_not_preseeded",
        &scans,
    );

    for count in [100, 1000] {
        let mut picker = SessionPicker::new(fixtures(count, 16 * 1024, &missing));
        picker.search_active = true;
        let mut keys = Vec::new();
        for _ in 0..samples {
            // Prefix setup is outside timing. Measure a real final keystroke.
            picker.search_query = "quart".into();
            picker.rebuild_items();
            let start = Instant::now();
            picker
                .handle_search_key(KeyCode::Char('z'), KeyModifiers::NONE)
                .unwrap();
            keys.push(start.elapsed());
            assert_eq!(picker.search_query, ABSENT);
            assert!(picker.visible_sessions.is_empty());
            assert!(
                picker.transcript_search.receiver.is_none(),
                "keystroke started fuzzy worker"
            );
        }
        report(
            "ui_final_keystroke",
            ABSENT,
            count,
            16 * 1024,
            "resident_metadata_no_worker_started",
            &keys,
        );
    }
    eprintln!(
        "SEARCH_SPEED {}",
        json!({"case":"suite_total","samples":samples,
        "ms":total.elapsed().as_secs_f64()*1000.0,"assertions":"passed"})
    );
}

#[test]
fn search_speed_fixture_preserves_typo_and_absent_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = fixtures(3, 256, &temp.path().join("absent.jsonl"));
    assert_eq!(sessions.len(), 3);
    assert!(fuzzy_metadata_rank(&sessions[0].search_index, &search_query_tokens(TYPO)).score > 0);
    assert_eq!(
        fuzzy_metadata_rank(&sessions[0].search_index, &search_query_tokens(ABSENT)).score,
        0
    );
    let (first, _) = worker_sample(&sessions, TYPO, 3);
    assert!(first.is_some());
    let (first, _) = worker_sample(&sessions, ABSENT, 0);
    assert!(first.is_none());
}
