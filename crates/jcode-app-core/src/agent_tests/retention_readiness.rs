// Deterministic longitudinal synthetic-cohort evaluator for retention readiness.
//
// This deliberately does NOT claim to measure human retention. It asks whether
// jcode has the product properties that make returning likely: a useful first
// result, cheap re-entry, preserved context, durable state, recoverable failure,
// and value that compounds across sessions. Real D1/D7/D30 cohort retention is
// a separate telemetry outcome and is never synthesized here.
//
// The test drives the real Agent, Provider, Session persistence, and restore
// paths through labeled D0/D1/D7 boundaries. The labels are deterministic phase
// boundaries, not wall-clock sleeps. Run the scorecard with:
//
//   cargo test -p jcode-app-core --lib retention_readiness -- --nocapture

#[derive(Clone)]
struct RetentionReadinessProvider {
    fail_d7_once: std::sync::Arc<std::sync::atomic::AtomicBool>,
    transcripts: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

struct RetentionHomeRestore(Option<std::ffi::OsString>);

impl Drop for RetentionHomeRestore {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetentionFactorStatus {
    Scored,
    Deferred,
    ObservedOnly,
}

struct RetentionFactor {
    name: &'static str,
    status: RetentionFactorStatus,
    rationale: &'static str,
}

/// Explicit scope registry. Observed-only outcomes never enter this
/// deterministic score. Known evidence gaps lower coverage rather than vanish.
fn retention_factor_registry() -> [RetentionFactor; 9] {
    use RetentionFactorStatus::{Deferred, ObservedOnly, Scored};
    [
        RetentionFactor {
            name: "assistant-response first value",
            status: Scored,
            rationale: "D0 real Agent turn reaches a deterministic useful answer",
        },
        RetentionFactor {
            name: "return friction",
            status: Scored,
            rationale: "D1 counts context restatement and prompts-to-value",
        },
        RetentionFactor {
            name: "state continuity",
            status: Scored,
            rationale: "D1 disk rehydrate preserves transcript, metadata, and memory marker",
        },
        RetentionFactor {
            name: "restart/failure durability",
            status: Scored,
            rationale: "D7 provider failure leaves the real persisted Session loadable",
        },
        RetentionFactor {
            name: "failure recovery",
            status: Scored,
            rationale: "D7 returns to useful value after one explicit retry",
        },
        RetentionFactor {
            name: "compounding context value",
            status: Scored,
            rationale: "D7 success is conditional on both D0 and D1 context",
        },
        RetentionFactor {
            name: "tool-backed first value",
            status: Deferred,
            rationale: "v1 proves a useful answer, but not yet a successful real tool/file edit",
        },
        RetentionFactor {
            name: "credential/provider/OS return parity",
            status: Deferred,
            rationale: "needs persisted credential reconstruction across a provider x OS matrix",
        },
        RetentionFactor {
            name: "observed D1/D7/D30 meaningful-work retention",
            status: ObservedOnly,
            rationale: "requires privacy-safe real cohorts and must never be synthesized",
        },
    ]
}

impl RetentionReadinessProvider {
    fn new() -> Self {
        Self {
            fail_d7_once: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            transcripts: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn transcript_snapshots(&self) -> Vec<Vec<String>> {
        self.transcripts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl Provider for RetentionReadinessProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let transcript: Vec<String> = messages
            .iter()
            .map(|message| message_text(message).to_string())
            .collect();
        self.transcripts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(transcript.clone());

        let latest = transcript.last().cloned().unwrap_or_default();
        if latest.contains("D7_RECOVER")
            && self
                .fail_d7_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("synthetic provider outage");
        }

        let has_d0 = transcript.iter().any(|text| text.contains("D0_ACTIVATE"));
        let has_d0_value = transcript.iter().any(|text| text.contains("VALUE_D0"));
        let has_d1 = transcript.iter().any(|text| text.contains("D1_RETURN"));
        let has_d1_value = transcript.iter().any(|text| text.contains("CONTINUITY_D1"));
        let answer = if latest.contains("D0_ACTIVATE") {
            "VALUE_D0"
        } else if latest.contains("D1_RETURN") && has_d0 && has_d0_value {
            "CONTINUITY_D1"
        } else if latest.contains("D7_RECOVER") && has_d0 && has_d0_value && has_d1 && has_d1_value
        {
            "COMPOUNDED_D7"
        } else {
            "CONTEXT_LOST"
        };

        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(4);
        tx.send(Ok(StreamEvent::TextDelta(answer.to_string())))
            .await
            .expect("retention provider event receiver");
        tx.send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }))
        .await
        .expect("retention provider event receiver");
        drop(tx);
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "retention-readiness"
    }

    fn model(&self) -> String {
        "retention-fixture-v1".to_string()
    }

    fn fork(&self) -> std::sync::Arc<dyn Provider> {
        std::sync::Arc::new(self.clone())
    }
}

#[derive(Debug, Clone, Copy)]
struct RetentionDimensionScores {
    activation: f64,
    return_friction: f64,
    continuity: f64,
    durability: f64,
    recovery: f64,
    compounding_value: f64,
}

impl RetentionDimensionScores {
    fn values(self) -> [(&'static str, f64, f64); 6] {
        [
            ("activation / first value", self.activation, 0.25),
            ("return friction", self.return_friction, 0.20),
            ("continuity", self.continuity, 0.20),
            ("durability", self.durability, 0.15),
            ("failure recovery", self.recovery, 0.10),
            ("compounding value", self.compounding_value, 0.10),
        ]
    }

    /// Weighted geometric mean. A zero in any dimension makes the behavioral
    /// score zero, so perfect setup cannot compensate for catastrophic state
    /// loss or inability to produce value.
    fn behavioral_score(self) -> f64 {
        self.values()
            .into_iter()
            .map(|(_, score, weight)| {
                let normalized = (score.clamp(0.0, 100.0) / 100.0).max(f64::MIN_POSITIVE);
                normalized.powf(weight)
            })
            .product::<f64>()
            * 100.0
    }
}

#[derive(Debug)]
struct RetentionJourneyEvidence {
    first_value: bool,
    session_persisted: bool,
    d1_context_available: bool,
    restatement_steps: u32,
    return_prompts_to_value: u32,
    title_preserved: bool,
    working_dir_preserved: bool,
    memory_marker_preserved: bool,
    history_preserved_after_failure: bool,
    outage_surfaced: bool,
    recovery_retries: u32,
    recovered_value: bool,
    compounded_value: bool,
}

fn retention_dimension_scores(e: &RetentionJourneyEvidence) -> RetentionDimensionScores {
    let activation = 70.0 * f64::from(e.first_value) + 30.0 * f64::from(e.session_persisted);

    let return_friction = (100.0
        - e.restatement_steps as f64 * 25.0
        - e.return_prompts_to_value.saturating_sub(1) as f64 * 15.0)
        .max(0.0);

    let continuity = 25.0
        * [
            e.d1_context_available,
            e.title_preserved,
            e.working_dir_preserved,
            e.memory_marker_preserved,
        ]
        .into_iter()
        .filter(|ok| *ok)
        .count() as f64;

    let durability =
        50.0 * f64::from(e.history_preserved_after_failure) + 50.0 * f64::from(e.session_persisted);

    let recovery = if !e.outage_surfaced || !e.recovered_value {
        0.0
    } else {
        (100.0 - e.recovery_retries.saturating_sub(1) as f64 * 20.0).max(0.0)
    };

    let compounding_value = 100.0 * f64::from(e.compounded_value);

    RetentionDimensionScores {
        activation,
        return_friction,
        continuity,
        durability,
        recovery,
        compounding_value,
    }
}

#[tokio::test]
async fn retention_readiness_scorecard() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("retention readiness home");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let _home_restore = RetentionHomeRestore(previous_home);

    let provider_fixture = RetentionReadinessProvider::new();
    let provider: std::sync::Arc<dyn Provider> = std::sync::Arc::new(provider_fixture.clone());
    let registry = Registry::new(provider.clone()).await;

    // D0: one prompt reaches a deterministic useful answer and creates durable
    // state that a return journey can benefit from.
    let mut d0 = Agent::new(provider.clone(), registry.clone());
    d0.session
        .rename_title(Some("Retention cohort project".to_string()));
    d0.session.working_dir = Some("/synthetic/retention-project".to_string());
    d0.session.record_memory_injection(
        "cohort preference".to_string(),
        "Prefer deterministic validation".to_string(),
        1,
        0,
        vec!["retention-memory-v1".to_string()],
    );
    let session_id = d0.session_id().to_string();
    let d0_answer = d0
        .run_once_capture("D0_ACTIVATE explain this project")
        .await
        .expect("D0 activation turn");
    d0.session.save().expect("persist D0 state");
    drop(d0);

    let persisted_d0 = Session::load(&session_id).expect("load D0 session");

    // D1: construct a new Agent from disk, not the old in-memory object. The
    // fixture only returns CONTINUITY_D1 when the real provider transcript still
    // contains both D0's prompt and its useful answer.
    let d1_provider: std::sync::Arc<dyn Provider> = std::sync::Arc::new(provider_fixture.clone());
    let d1_registry = Registry::new(d1_provider.clone()).await;
    let mut d1 = Agent::new_with_session(d1_provider, d1_registry, persisted_d0, None);
    let d1_answer = d1
        .run_once_capture("D1_RETURN continue without restating context")
        .await
        .expect("D1 return turn");
    d1.session.save().expect("persist D1 state");
    drop(d1);

    // D7: inject one deterministic provider outage. The failed user turn must be
    // durable, the process-like rehydrate must work, and one retry must produce
    // value that depends on BOTH earlier sessions.
    let persisted_d1 = Session::load(&session_id).expect("load D1 session");
    let d1_message_count = persisted_d1.messages.len();
    let title_preserved = persisted_d1.custom_title.as_deref() == Some("Retention cohort project");
    let working_dir_preserved =
        persisted_d1.working_dir.as_deref() == Some("/synthetic/retention-project");
    let memory_marker_preserved = persisted_d1
        .injected_memory_ids()
        .iter()
        .any(|id| id == "retention-memory-v1");
    let d7_provider: std::sync::Arc<dyn Provider> = std::sync::Arc::new(provider_fixture.clone());
    let d7_registry = Registry::new(d7_provider.clone()).await;
    let mut d7 = Agent::new_with_session(d7_provider, d7_registry, persisted_d1, None);
    let outage = d7
        .run_once_capture("D7_RECOVER finish the longitudinal task")
        .await;
    drop(d7);

    let after_failure = Session::load(&session_id).expect("session survives outage");
    let history_preserved_after_failure = after_failure.messages.len() > d1_message_count
        && after_failure.messages.iter().any(|stored| {
            message_text(&stored.to_message()).contains("D7_RECOVER finish the longitudinal task")
        });
    let recovered_provider: std::sync::Arc<dyn Provider> =
        std::sync::Arc::new(provider_fixture.clone());
    let recovered_registry = Registry::new(recovered_provider.clone()).await;
    let mut recovered =
        Agent::new_with_session(recovered_provider, recovered_registry, after_failure, None);
    let d7_answer = recovered
        .run_once_capture("D7_RECOVER retry once")
        .await
        .expect("D7 recovery turn");
    recovered
        .session
        .save()
        .expect("persist recovered D7 state");

    let snapshots = provider_fixture.transcript_snapshots();
    let d1_context_available = snapshots.iter().any(|snapshot| {
        snapshot.iter().any(|text| text.contains("D1_RETURN"))
            && snapshot.iter().any(|text| text.contains("D0_ACTIVATE"))
            && snapshot.iter().any(|text| text.contains("VALUE_D0"))
    });

    let evidence = RetentionJourneyEvidence {
        first_value: d0_answer.contains("VALUE_D0"),
        session_persisted: Session::load(&session_id).is_ok(),
        d1_context_available: d1_context_available && d1_answer.contains("CONTINUITY_D1"),
        restatement_steps: 0,
        return_prompts_to_value: 1,
        title_preserved,
        working_dir_preserved,
        memory_marker_preserved,
        history_preserved_after_failure,
        outage_surfaced: outage.is_err(),
        recovery_retries: 1,
        recovered_value: d7_answer.contains("COMPOUNDED_D7"),
        compounded_value: d7_answer.contains("COMPOUNDED_D7"),
    };
    let scores = retention_dimension_scores(&evidence);
    let behavioral = scores.behavioral_score();

    // Coverage is reported separately and gates the headline. V1 scores six
    // deterministic factors but deliberately defers tool-backed first value and
    // credential/provider/OS return parity. A perfect covered journey therefore
    // cannot be misreported as perfect evidence about retention overall.
    let factors = retention_factor_registry();
    let scored_factors = factors
        .iter()
        .filter(|factor| factor.status == RetentionFactorStatus::Scored)
        .count();
    let acknowledged_factors = factors
        .iter()
        .filter(|factor| factor.status != RetentionFactorStatus::ObservedOnly)
        .count();
    let evidence_coverage = scored_factors as f64 / acknowledged_factors as f64 * 100.0;
    let coverage_adjusted = behavioral * evidence_coverage / 100.0;

    println!("\n================ RETENTION READINESS (SYNTHETIC COHORT) ================");
    println!("This is a deterministic product-readiness proxy, NOT observed user retention.\n");
    println!("journey   boundary   outcome");
    println!(
        "D0        activate   {}",
        if evidence.first_value {
            "useful value"
        } else {
            "FAIL"
        }
    );
    println!(
        "D1        return     {}",
        if evidence.d1_context_available {
            "context continued"
        } else {
            "FAIL"
        }
    );
    println!(
        "D7        outage     {}",
        if evidence.outage_surfaced {
            "surfaced"
        } else {
            "FAIL"
        }
    );
    println!(
        "D7        retry      {}",
        if evidence.recovered_value {
            "recovered + compounded"
        } else {
            "FAIL"
        }
    );
    println!("\n-- dimensions (weighted geometric mean) --");
    for (name, score, weight) in scores.values() {
        println!(
            "{name:<26} {score:>5.1} / 100  weight={weight:.0}%",
            weight = weight * 100.0
        );
    }
    println!("\nBEHAVIORAL READINESS  : {behavioral:>5.1} / 100");
    println!(
        "EVIDENCE COVERAGE     : {evidence_coverage:>5.1} / 100 ({scored_factors}/{acknowledged_factors} factors)"
    );
    println!("COVERAGE-ADJUSTED     : {coverage_adjusted:>5.1} / 100");
    for factor in factors
        .iter()
        .filter(|factor| factor.status == RetentionFactorStatus::Deferred)
    {
        println!(
            "Deferred              : {} ({})",
            factor.name, factor.rationale
        );
    }
    println!("Observed counterpart  : D1/D7/D30 meaningful-work cohorts (telemetry, separate)\n");

    // Hard gates: no weighted average may hide loss of first value, continuity,
    // durable state, recovery, or compounded context.
    assert!(evidence.first_value, "D0 did not reach useful first value");
    assert!(
        evidence.session_persisted,
        "synthetic cohort session was not durable"
    );
    assert!(
        evidence.d1_context_available,
        "D1 return lost prior context"
    );
    assert!(
        evidence.title_preserved
            && evidence.working_dir_preserved
            && evidence.memory_marker_preserved,
        "D1 return lost persisted metadata or memory state"
    );
    assert!(
        evidence.history_preserved_after_failure,
        "provider outage lost session history"
    );
    assert!(
        evidence.outage_surfaced && evidence.recovered_value,
        "provider outage did not recover in one retry"
    );
    assert!(
        evidence.compounded_value,
        "D7 result did not depend on D0 + D1 context"
    );
    assert!(
        behavioral >= 80.0,
        "behavioral retention readiness regressed: {behavioral:.1}"
    );
}

#[test]
fn retention_readiness_scoring_is_monotonic_and_non_compensating() {
    let perfect = RetentionJourneyEvidence {
        first_value: true,
        session_persisted: true,
        d1_context_available: true,
        restatement_steps: 0,
        return_prompts_to_value: 1,
        title_preserved: true,
        working_dir_preserved: true,
        memory_marker_preserved: true,
        history_preserved_after_failure: true,
        outage_surfaced: true,
        recovery_retries: 1,
        recovered_value: true,
        compounded_value: true,
    };
    let perfect_scores = retention_dimension_scores(&perfect);
    let baseline = perfect_scores.behavioral_score();
    let total_weight: f64 = perfect_scores
        .values()
        .into_iter()
        .map(|(_, _, weight)| weight)
        .sum();
    assert!((total_weight - 1.0).abs() < f64::EPSILON);

    let worse_cases = [
        RetentionJourneyEvidence {
            first_value: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            session_persisted: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            restatement_steps: 1,
            ..perfect
        },
        RetentionJourneyEvidence {
            return_prompts_to_value: 2,
            ..perfect
        },
        RetentionJourneyEvidence {
            d1_context_available: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            title_preserved: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            working_dir_preserved: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            memory_marker_preserved: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            history_preserved_after_failure: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            outage_surfaced: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            recovery_retries: 3,
            ..perfect
        },
        RetentionJourneyEvidence {
            recovered_value: false,
            ..perfect
        },
        RetentionJourneyEvidence {
            compounded_value: false,
            ..perfect
        },
    ];
    for worse in &worse_cases {
        assert!(
            retention_dimension_scores(worse).behavioral_score() < baseline,
            "making one retention factor worse must lower the score"
        );
    }

    let catastrophic = retention_dimension_scores(&RetentionJourneyEvidence {
        compounded_value: false,
        ..perfect
    });
    assert!(
        catastrophic.behavioral_score() < 1.0,
        "a zero dimension must not be compensated by perfect sibling dimensions"
    );
}

#[test]
fn retention_readiness_factor_registry_has_explicit_scope_and_rationales() {
    let factors = retention_factor_registry();
    assert_eq!(
        factors
            .iter()
            .filter(|f| f.status == RetentionFactorStatus::Scored)
            .count(),
        6
    );
    assert_eq!(
        factors
            .iter()
            .filter(|f| f.status == RetentionFactorStatus::Deferred)
            .count(),
        2
    );
    assert_eq!(
        factors
            .iter()
            .filter(|f| f.status == RetentionFactorStatus::ObservedOnly)
            .count(),
        1
    );
    let mut names = std::collections::BTreeSet::new();
    for factor in factors {
        assert!(!factor.name.trim().is_empty());
        assert!(
            names.insert(factor.name),
            "duplicate factor: {}",
            factor.name
        );
        assert!(
            !factor.rationale.trim().is_empty(),
            "retention factor '{}' has no rationale",
            factor.name
        );
    }
}
