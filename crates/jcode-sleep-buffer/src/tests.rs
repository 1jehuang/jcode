//! Unit tests: buffer, dedup, idempotency, interrupt/resume.
//!
//! Conventions: `tmp_buffer()` gives an isolated buffer per test (tempdir
//! path, never the real `~/.jcode`). Clock-sensitive assertions use
//! explicit `now` values or generous bounds — no sleeps, no flakiness.

use super::*;
use chrono::Duration as ChronoDuration;

fn tmp_buffer() -> (SleepBuffer, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sleep_buffer.json");
    let buf = SleepBuffer::open(&path).expect("open fresh");
    (buf, dir)
}

fn cite(s: &str) -> Vec<String> {
    vec![s.to_string()]
}

// ---------------------------------------------------------------------------
// Buffer basics
// ---------------------------------------------------------------------------

#[test]
fn propose_creates_pending_proposal() {
    let (mut buf, _dir) = tmp_buffer();
    let out = buf
        .propose(
            "Always run cargo test before committing",
            "scoped_rule",
            "sess-a",
            ProposalSource::AgentDistilled,
            &cite("sess-a:turn-12"),
        )
        .expect("propose");
    let id = match out {
        ProposeOutcome::Created { id } => id,
        other => panic!("expected Created, got {other:?}"),
    };
    let p = buf.get(&id).expect("stored");
    assert_eq!(p.status, ProposalStatus::Pending);
    assert_eq!(p.occurrences, 1);
    assert_eq!(p.session_ids, vec!["sess-a".to_string()]);
    assert_eq!(p.kind, "scoped_rule");
    assert_eq!(p.version, SLEEP_BUFFER_VERSION);
    assert!(buf.is_dirty());
    assert_eq!(buf.len(), 1);
}

#[test]
fn propose_rejects_empty_text() {
    let (mut buf, _dir) = tmp_buffer();
    for bad in ["", "   ", "\n\t  \n"] {
        let err = buf
            .propose(bad, "scoped_rule", "s", ProposalSource::User, &[])
            .expect_err("empty must fail");
        assert!(err.to_string().contains("empty"), "{err}");
    }
    assert!(buf.is_empty());
    assert!(!buf.is_dirty());
}

#[test]
fn normalize_collapses_whitespace_and_case() {
    assert_eq!(normalize_text("  Foo\nBAR\t baz "), "foo bar baz");
    assert_eq!(normalize_text("A  B"), "a b");
    assert_eq!(dedup_key_for("a b"), dedup_key_for("a b"));
    assert_ne!(dedup_key_for("a b"), dedup_key_for("a c"));
}

// ---------------------------------------------------------------------------
// Dedup: exact tier
// ---------------------------------------------------------------------------

#[test]
fn exact_reproposal_bumps_counters_same_id() {
    let (mut buf, _dir) = tmp_buffer();
    let first = buf
        .propose(
            "Prefer fish shell over bash",
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &cite("s1:1"),
        )
        .expect("propose");
    let id = match first {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    // Same text, different casing/whitespace/session: exact tier must hit.
    let second = buf
        .propose(
            "  prefer FISH shell\nover bash ",
            "scoped_rule",
            "s2",
            ProposalSource::AgentDistilled,
            &cite("s2:7"),
        )
        .expect("propose");
    match second {
        ProposeOutcome::DedupedExact { id: id2 } => assert_eq!(id, id2),
        other => panic!("expected DedupedExact, got {other:?}"),
    }
    assert_eq!(buf.len(), 1, "no duplicate record created");
    let p = buf.get(&id).unwrap();
    assert_eq!(p.occurrences, 2);
    assert_eq!(p.session_ids.len(), 2);
    assert!(p.citations.contains(&"s1:1".to_string()));
    assert!(p.citations.contains(&"s2:7".to_string()));
}

#[test]
fn exact_reproposal_same_session_counts_occurrence_not_session() {
    let (mut buf, _dir) = tmp_buffer();
    let text = "Pin GPU jobs to off-peak hours";
    let id = match buf
        .propose(
            text,
            "recurring_fact",
            "s1",
            ProposalSource::ToolIngested,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    // Same session proposes again (e.g. re-scanned transcript).
    match buf
        .propose(
            text,
            "recurring_fact",
            "s1",
            ProposalSource::ToolIngested,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::DedupedExact { id: id2 } => assert_eq!(id, id2),
        other => panic!("{other:?}"),
    }
    let p = buf.get(&id).unwrap();
    assert_eq!(p.occurrences, 2, "volume counts");
    assert_eq!(p.session_ids.len(), 1, "but breadth does not double-count");
}

#[test]
fn source_tier_upgrades_monotone_never_downgrades() {
    let (mut buf, _dir) = tmp_buffer();
    let text = "Use limine-mkinitcpio after kernel param changes";
    let id = match buf
        .propose(text, "scoped_rule", "s1", ProposalSource::ToolIngested, &[])
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    // User correction upgrades.
    match buf
        .propose(text, "scoped_rule", "s2", ProposalSource::User, &[])
        .unwrap()
    {
        ProposeOutcome::DedupedExact { .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(buf.get(&id).unwrap().source, ProposalSource::User);
    // Later low-tier re-proposal must NOT demote.
    match buf
        .propose(text, "scoped_rule", "s3", ProposalSource::ToolIngested, &[])
        .unwrap()
    {
        ProposeOutcome::DedupedExact { .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(buf.get(&id).unwrap().source, ProposalSource::User);
    assert_eq!(buf.get(&id).unwrap().occurrences, 3);
}

// ---------------------------------------------------------------------------
// Dedup: near-duplicate tier (SimHash)
// ---------------------------------------------------------------------------

#[test]
fn simhash_is_deterministic() {
    let n = normalize_text("Always run cargo test before committing changes to memory files");
    assert_eq!(simhash64(&n), simhash64(&n));
    assert_eq!(hamming_similarity(0xAAAA, 0xAAAA), 1.0);
    assert_eq!(hamming_similarity(0xFFFF_FFFF_FFFF_FFFF, 0), 0.0);
}

#[test]
fn near_duplicate_typo_merges() {
    let (mut buf, _dir) = tmp_buffer();
    let a = "always run cargo test before committing changes to memory files";
    // Single insertion deep in a long-enough sentence: typo-level edit.
    let b = "always run cargo test before committing changes to memory files please";
    let sim = hamming_similarity(simhash64(a), simhash64(&normalize_text(b)));
    eprintln!("typo-sim = {sim}");
    assert!(
        sim >= NEAR_DUP_SIMILARITY_THRESHOLD,
        "fixture sanity: typo pair must clear threshold, got {sim}"
    );
    let id_a = match buf
        .propose(
            a,
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &cite("s1:3"),
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    let merged = buf
        .propose(
            b,
            "scoped_rule",
            "s2",
            ProposalSource::AgentDistilled,
            &cite("s2:9"),
        )
        .unwrap();
    let (into_id, new_id) = match merged {
        ProposeOutcome::MergedNearDup { into_id, new_id } => (into_id, new_id),
        other => panic!("expected MergedNearDup, got {other:?}"),
    };
    assert_eq!(into_id, id_a);
    // Winner absorbed the observation.
    let winner = buf.get(&id_a).unwrap();
    assert_eq!(winner.occurrences, 2);
    assert_eq!(winner.session_ids.len(), 2);
    assert!(winner.citations.contains(&"s2:9".to_string()));
    // Tombstone forwards.
    let tomb = buf.get(&new_id).unwrap();
    assert_eq!(tomb.status, ProposalStatus::Merged);
    assert_eq!(tomb.merged_into.as_deref(), Some(id_a.as_str()));
}

#[test]
fn distinct_texts_stay_separate() {
    let (mut buf, _dir) = tmp_buffer();
    let texts = [
        "always run cargo test before committing changes to memory files",
        "prefer fish shell over bash for interactive scripting work",
        "never commit secrets or api keys into any git repository",
    ];
    // Fixture sanity: pairwise similarity must be far below threshold.
    for (i, a) in texts.iter().enumerate() {
        for b in &texts[i + 1..] {
            let sim = hamming_similarity(simhash64(a), simhash64(&normalize_text(b)));
            assert!(
                sim < NEAR_DUP_SIMILARITY_THRESHOLD,
                "fixture sanity: distinct pair sim {sim} must be below threshold"
            );
        }
    }
    let mut ids = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        match buf
            .propose(
                t,
                "scoped_rule",
                &format!("s{i}"),
                ProposalSource::AgentDistilled,
                &[],
            )
            .unwrap()
        {
            ProposeOutcome::Created { id } => ids.push(id),
            other => panic!("distinct text must create, got {other:?}"),
        }
    }
    assert_eq!(buf.len(), 3);
    assert_eq!(buf.ranked_pending(Utc::now()).len(), 3);
}

// ---------------------------------------------------------------------------
// Priority ordering
// ---------------------------------------------------------------------------

#[test]
fn priority_orders_by_signal_then_source_then_age() {
    let (mut buf, _dir) = tmp_buffer();
    // Low signal: single tool-ingested mention.
    let low = match buf
        .propose(
            "low signal candidate about widget rendering performance tuning",
            "recurring_fact",
            "s1",
            ProposalSource::ToolIngested,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    // High signal: user-backed, seen across 3 sessions.
    let high = match buf
        .propose(
            "high signal candidate about kernel boot parameter documentation updates",
            "scoped_rule",
            "s1",
            ProposalSource::User,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    for sess in ["s2", "s3"] {
        match buf
            .propose(
                "high signal candidate about kernel boot parameter documentation updates",
                "scoped_rule",
                sess,
                ProposalSource::AgentDistilled,
                &[],
            )
            .unwrap()
        {
            ProposeOutcome::DedupedExact { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    // Mid: agent-distilled, two occurrences.
    let mid = match buf
        .propose(
            "mid signal candidate about ambient usage log rotation policy details",
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    match buf
        .propose(
            "mid signal candidate about ambient usage log rotation policy details",
            "scoped_rule",
            "s2",
            ProposalSource::AgentDistilled,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::DedupedExact { .. } => {}
        other => panic!("{other:?}"),
    }

    let now = Utc::now();
    let ranked = buf.ranked_pending(now);
    let order: Vec<&str> = ranked.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(order, vec![high.as_str(), mid.as_str(), low.as_str()]);

    // Scores are monotone with the ordering and positive for pending.
    let scores: Vec<f64> = ranked.iter().map(|p| p.priority_score(now)).collect();
    assert!(scores[0] > scores[1] && scores[1] > scores[2]);
    assert!(scores.iter().all(|s| s.is_finite() && *s > 0.0));
}

#[test]
fn priority_decays_with_age() {
    let (mut buf, _dir) = tmp_buffer();
    let id = match buf
        .propose(
            "aging candidate about display server configuration file locations",
            "scoped_rule",
            "s1",
            ProposalSource::User,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    let p = buf.get(&id).unwrap();
    let fresh = p.priority_score(Utc::now());
    let old = p.priority_score(Utc::now() + ChronoDuration::days(60));
    assert!(fresh > old, "decay: fresh {fresh} must exceed 60d {old}");
    // Halving shape: ~30 days halves the fresh score (breadth constant).
    let month = p.priority_score(p.first_seen_at + ChronoDuration::days(30));
    let at_birth = p.priority_score(p.first_seen_at);
    assert!(
        (month - at_birth / 2.0).abs() < 1e-9,
        "month {month} vs half-birth {}",
        at_birth / 2.0
    );
}

#[test]
fn terminal_proposals_never_rank() {
    let (mut buf, _dir) = tmp_buffer();
    let id = match buf
        .propose(
            "doomed candidate about obsolete terminal emulator keybindings",
            "scoped_rule",
            "s1",
            ProposalSource::User,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(buf.mark_promoted(&id));
    let p = buf.get(&id).unwrap();
    assert_eq!(p.priority_score(Utc::now()), f64::NEG_INFINITY);
    assert!(buf.ranked_pending(Utc::now()).is_empty());
    assert!(buf.promotion_queue(Utc::now(), 1, 10).is_empty());
}

#[test]
fn promotion_queue_respects_threshold_and_limit() {
    let (mut buf, _dir) = tmp_buffer();
    let mut mk = |text: &str, occ_extra: usize| {
        let id = match buf
            .propose(
                text,
                "scoped_rule",
                "s0",
                ProposalSource::AgentDistilled,
                &[],
            )
            .unwrap()
        {
            ProposeOutcome::Created { id } => id,
            other => panic!("{other:?}"),
        };
        for i in 0..occ_extra {
            match buf
                .propose(
                    text,
                    "scoped_rule",
                    &format!("sx{i}"),
                    ProposalSource::AgentDistilled,
                    &[],
                )
                .unwrap()
            {
                ProposeOutcome::DedupedExact { .. } => {}
                other => panic!("{other:?}"),
            }
        }
        id
    };
    let once = mk(
        "threshold probe alpha about cron schedule documentation wording",
        0,
    );
    let _twice = mk(
        "threshold probe beta about hook payload size limit conventions",
        1,
    );
    let _thrice = mk(
        "threshold probe gamma about session presence marker cleanup rules",
        2,
    );

    let now = Utc::now();
    let q = buf.promotion_queue(now, DEFAULT_PROMOTION_THRESHOLD, 10);
    assert_eq!(q.len(), 2);
    assert!(
        !q.iter().any(|p| p.id == once),
        "single-occurrence excluded"
    );

    let limited = buf.promotion_queue(now, 1, 1);
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].id, _thrice, "limit keeps top-ranked");
}

// ---------------------------------------------------------------------------
// Idempotency: transitions, flush/reopen, double-run convergence
// ---------------------------------------------------------------------------

#[test]
fn transitions_are_idempotent() {
    let (mut buf, _dir) = tmp_buffer();
    assert!(!buf.mark_promoted("missing"), "unknown id is a no-op false");
    let id = match buf
        .propose(
            "transition probe about notification email template subjects",
            "scoped_rule",
            "s1",
            ProposalSource::User,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(buf.mark_promoted(&id));
    assert!(!buf.mark_promoted(&id), "second promote is a no-op");
    assert!(
        !buf.mark_rejected(&id),
        "promoted never leaves terminal state"
    );
    assert_eq!(buf.get(&id).unwrap().status, ProposalStatus::Promoted);
}

#[test]
fn flush_reopen_roundtrip_is_identical() {
    let (mut buf, dir) = tmp_buffer();
    let texts = [
        "roundtrip alpha about power profile switching on ac events",
        "roundtrip beta about display scaling across mango sessions",
    ];
    for t in texts {
        let _ = buf.propose(
            t,
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &cite("s1:1"),
        );
    }
    // Exact re-proposal before flush (exercises dirty path twice).
    let _ = buf.propose(
        texts[0],
        "scoped_rule",
        "s2",
        ProposalSource::User,
        &cite("s2:2"),
    );
    buf.flush().expect("flush");
    assert!(!buf.is_dirty());

    let path = dir.path().join("sleep_buffer.json");
    let reopened = SleepBuffer::open(&path).expect("reopen");
    assert_eq!(reopened.len(), 2);
    for p in reopened.ranked_pending(Utc::now()) {
        let orig = buf.get(&p.id).expect("same id");
        assert_eq!(p.dedup_key, orig.dedup_key);
        assert_eq!(p.simhash, orig.simhash);
        assert_eq!(p.occurrences, orig.occurrences);
        assert_eq!(p.session_ids, orig.session_ids);
        assert_eq!(p.source, orig.source);
        assert_eq!(p.citations, orig.citations);
    }
    // No stray tmp file left behind.
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn rerun_converges_double_propose_after_reopen() {
    // The idempotency contract: running the same session-end pass twice
    // (even across a restart) converges to identical state.
    let (mut buf, dir) = tmp_buffer();
    let text = "convergent candidate about fingerprint reader modprobe ordering";
    let _ = buf.propose(
        text,
        "scoped_rule",
        "s9",
        ProposalSource::AgentDistilled,
        &cite("s9:4"),
    );
    buf.flush().unwrap();
    let path = dir.path().join("sleep_buffer.json");

    let mut again = SleepBuffer::open(&path).unwrap();
    match again
        .propose(
            text,
            "scoped_rule",
            "s9",
            ProposalSource::AgentDistilled,
            &cite("s9:4"),
        )
        .unwrap()
    {
        ProposeOutcome::DedupedExact { .. } => {}
        other => panic!("re-run must dedup, got {other:?}"),
    }
    again.flush().unwrap();

    let final_buf = SleepBuffer::open(&path).unwrap();
    assert_eq!(final_buf.len(), 1);
    let p = final_buf.ranked_pending(Utc::now());
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].occurrences, 2);
    assert_eq!(p[0].session_ids.len(), 1, "same session: volume only");
}

#[test]
fn corrupt_file_fails_open_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sleep_buffer.json");
    fs::write(&path, "{ this is not json").unwrap();
    let (buf, loaded_ok) = SleepBuffer::open_with_status(&path).unwrap();
    assert!(buf.is_empty());
    assert!(!loaded_ok, "corruption must be reported");
    // Buffer still usable after fail-open.
    let mut buf = buf;
    match buf
        .propose(
            "post-corruption candidate about tmpfs size guard rails",
            "scoped_rule",
            "s1",
            ProposalSource::User,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { .. } => {}
        other => panic!("{other:?}"),
    }
    buf.flush().unwrap();
    let (reopened, ok) = SleepBuffer::open_with_status(&path).unwrap();
    assert!(ok);
    assert_eq!(reopened.len(), 1);
}

#[test]
fn version_mismatch_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sleep_buffer.json");
    fs::write(&path, r#"{"version":999,"proposals":{}}"#).unwrap();
    let (buf, loaded_ok) = SleepBuffer::open_with_status(&path).unwrap();
    assert!(buf.is_empty());
    assert!(!loaded_ok);
}

// ---------------------------------------------------------------------------
// Interrupt / resume
// ---------------------------------------------------------------------------

#[test]
fn unflushed_work_is_lost_but_state_stays_consistent() {
    // Kill -9 between propose and flush: reopen sees pre-crash state, and
    // re-proposing the lost candidate re-creates it cleanly (no half-row).
    let (mut buf, dir) = tmp_buffer();
    let kept = "durable candidate about sudo password piping conventions";
    let _ = buf.propose(kept, "scoped_rule", "s1", ProposalSource::User, &[]);
    buf.flush().unwrap();
    let path = dir.path().join("sleep_buffer.json");

    let lost = "interrupted candidate about heredoc stdin conflict avoidance";
    let _ = buf.propose(
        lost,
        "scoped_rule",
        "s2",
        ProposalSource::AgentDistilled,
        &[],
    );
    assert!(buf.is_dirty());
    drop(buf); // simulated crash: no flush

    let mut resumed = SleepBuffer::open(&path).unwrap();
    assert_eq!(resumed.len(), 1, "only flushed work survives");
    assert!(
        resumed
            .ranked_pending(Utc::now())
            .iter()
            .all(|p| p.text == kept)
    );
    // Resume re-proposes the lost candidate: converges, no duplicate.
    match resumed
        .propose(
            lost,
            "scoped_rule",
            "s2",
            ProposalSource::AgentDistilled,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { .. } => {}
        other => panic!("lost work re-creates cleanly, got {other:?}"),
    }
    assert_eq!(resumed.len(), 2);
}

#[test]
fn interrupted_flush_never_corrupts() {
    // A stale tmp file (previous crash between create and rename) must not
    // affect open: only the last fully-renamed file is read.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sleep_buffer.json");
    let mut buf = SleepBuffer::open(&path).unwrap();
    let _ = buf.propose(
        "crash-safe candidate about worktree isolation per worker",
        "scoped_rule",
        "s1",
        ProposalSource::User,
        &[],
    );
    buf.flush().unwrap();
    // Plant a garbage tmp leftover as a crashed flush would.
    fs::write(path.with_extension("json.tmp"), "garbage-half-write").unwrap();
    let reopened = SleepBuffer::open(&path).unwrap();
    assert_eq!(reopened.len(), 1);
    // Next flush overwrites the stale tmp and renames cleanly.
    let mut reopened = reopened;
    reopened.flush().unwrap(); // clean: no-op
    let _ = reopened.propose(
        "second crash-safe candidate about coordinator owned merges",
        "scoped_rule",
        "s2",
        ProposalSource::AgentDistilled,
        &[],
    );
    reopened.flush().unwrap();
    assert_eq!(SleepBuffer::open(&path).unwrap().len(), 2);
}

#[test]
fn prune_terminal_keeps_pending_forever() {
    let (mut buf, _dir) = tmp_buffer();
    let keep = match buf
        .propose(
            "eternal pending candidate about branch naming discipline",
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    let drop_me = match buf
        .propose(
            "doomed terminal candidate about retired desktop widget ids",
            "scoped_rule",
            "s1",
            ProposalSource::AgentDistilled,
            &[],
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(buf.mark_rejected(&drop_me));
    // Far-future cutoff: terminal goes, pending survives regardless of age.
    let pruned = buf.prune_terminal(Utc::now() + ChronoDuration::days(365));
    assert_eq!(pruned, 1);
    assert!(buf.get(&keep).is_some());
    assert!(buf.get(&drop_me).is_none());
    assert!(
        !buf.ranked_pending(Utc::now())
            .iter()
            .any(|p| p.id == drop_me)
    );
}

#[test]
fn citations_capped_and_deduped() {
    let (mut buf, _dir) = tmp_buffer();
    let many: Vec<String> = (0..MAX_CITATIONS + 5)
        .map(|i| format!("sess:turn-{i}"))
        .collect();
    let id = match buf
        .propose(
            "citation heavy candidate about log rotation retention windows",
            "recurring_fact",
            "s1",
            ProposalSource::AgentDistilled,
            &many,
        )
        .unwrap()
    {
        ProposeOutcome::Created { id } => id,
        other => panic!("{other:?}"),
    };
    let p = buf.get(&id).unwrap();
    assert_eq!(p.citations.len(), MAX_CITATIONS);
    // Re-proposing with duplicate citations does not grow the list.
    match buf
        .propose(
            "citation heavy candidate about log rotation retention windows",
            "recurring_fact",
            "s2",
            ProposalSource::AgentDistilled,
            &many[0..3],
        )
        .unwrap()
    {
        ProposeOutcome::DedupedExact { .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(buf.get(&id).unwrap().citations.len(), MAX_CITATIONS);
}
