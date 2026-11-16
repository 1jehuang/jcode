use super::super::merge_progress;
use super::*;

fn progress(revision: Option<[u64; 4]>, uploaded: usize, mtime_ms: u64) -> SessionProgress {
    SessionProgress {
        wire_version: 2,
        uploaded,
        mtime_ms,
        revision,
        fingerprint: Some(42),
    }
}

#[test]
fn borrowed_commit_matches_materialized_merge_bytes() {
    let variants = [
        progress(None, 2, 1),
        progress(None, 3, 2),
        progress(Some([1_000_000, 1, 0, 0]), 1, 1),
        progress(Some([1_000_000, 2, 0, 0]), 2, 1),
        progress(Some([0, 0, 2_000_000, 3]), 0, 2),
        progress(Some([3_000_000, 4, 0, 0]), 4, 3),
    ];
    for old in &variants {
        for update in &variants {
            for phone_messages_off in [false, true] {
                let state = MobileSyncState {
                    enabled: true,
                    generation: 7,
                    phone_messages_off,
                    sessions: ["b", "d", "f"]
                        .map(|id| (id.to_string(), old.clone()))
                        .into(),
                };
                // New keys before, between and after existing keys, and one untouched key.
                let updates = ["a", "b", "c", "f", "z"]
                    .map(|id| (id.to_string(), update.clone()))
                    .into();
                let mut expected = state.clone();
                assert!(merge_progress(&mut expected, &updates));
                let commit = ProgressCommit::new(&state, 7, &updates).unwrap();
                assert_eq!(
                    serde_json::to_vec(&commit).unwrap(),
                    serde_json::to_vec(&expected).unwrap()
                );
                assert_eq!(
                    state.sessions["b"], *old,
                    "overlay must not mutate its snapshot"
                );
            }
        }
    }
}

#[test]
fn borrowed_commit_skips_noops_and_generation_fences() {
    let old = progress(Some([20, 1, 0, 0]), 3, 1);
    let mut state = MobileSyncState {
        enabled: true,
        generation: 7,
        sessions: [("session".into(), old.clone())].into(),
        ..Default::default()
    };
    assert!(ProgressCommit::new(&state, 7, &BTreeMap::new()).is_none());
    let mut updates = [("session".into(), old)].into();
    assert!(ProgressCommit::new(&state, 7, &updates).is_none());
    updates.insert("session".into(), progress(Some([19, 9, 0, 0]), 9, 9));
    assert!(ProgressCommit::new(&state, 7, &updates).is_none());
    updates.insert("session".into(), progress(Some([20, 2, 0, 0]), 2, 1));
    assert!(ProgressCommit::new(&state, 7, &updates).is_none());
    updates.insert("new".into(), SessionProgress::default());
    assert!(ProgressCommit::new(&state, 8, &updates).is_none());
    state.enabled = false;
    assert!(ProgressCommit::new(&state, 7, &updates).is_none());
}

#[test]
#[ignore = "paired in-memory progress commit benchmark"]
fn benchmark_borrowed_progress_commit() {
    use std::hint::black_box;
    use std::time::Instant;
    let state = MobileSyncState {
        enabled: true,
        generation: 7,
        sessions: (0..4000)
            .map(|i| {
                (
                    format!("session-{i:05}"),
                    progress(Some([20, 1, 0, 0]), 3, 1),
                )
            })
            .collect(),
        ..Default::default()
    };
    let updates = [("session-02000".into(), progress(Some([21, 1, 0, 0]), 4, 1))].into();
    // Warm both implementations before timing identical serialization work.
    let baseline = || {
        let mut next = black_box(&state).clone();
        merge_progress(&mut next, black_box(&updates));
        black_box(serde_json::to_vec(&next).unwrap());
    };
    let borrowed = || {
        let commit = ProgressCommit::new(black_box(&state), 7, black_box(&updates)).unwrap();
        black_box(serde_json::to_vec(&commit).unwrap());
    };
    baseline();
    borrowed();
    let started = Instant::now();
    for _ in 0..100 {
        baseline();
    }
    let old = started.elapsed();
    let started = Instant::now();
    for _ in 0..100 {
        borrowed();
    }
    let new = started.elapsed();
    eprintln!(
        "4000 sessions, 100 commits: materialized={old:?}, borrowed={new:?}, ratio={:.3}",
        new.as_secs_f64() / old.as_secs_f64()
    );
    let unchanged = [(
        "session-02000".into(),
        state.sessions["session-02000"].clone(),
    )]
    .into();
    let started = Instant::now();
    for _ in 0..100 {
        let mut next = black_box(&state).clone();
        assert!(!merge_progress(&mut next, black_box(&unchanged)));
        black_box(next);
    }
    let old = started.elapsed();
    let started = Instant::now();
    for _ in 0..100 {
        assert!(ProgressCommit::new(black_box(&state), 7, black_box(&unchanged)).is_none());
    }
    let new = started.elapsed();
    eprintln!("4000 sessions, 100 unchanged commits: materialized={old:?}, borrowed={new:?}");
}
