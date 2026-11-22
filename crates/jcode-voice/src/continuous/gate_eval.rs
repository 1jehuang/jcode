//! All case text is synthetic. Offline checks inspect the actual request, not model behavior.
use super::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    family: String,
    candidate: String,
    stage: String,
    accept: bool,
    history: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    source: String,
    text: String,
    age_s: u64,
}
fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("gate_eval_cases.json")).unwrap()
}

#[test]
fn corpus_has_context_only_contrasts_and_revision() {
    let cases = cases();
    let mut ids = HashSet::new();
    for c in &cases {
        assert!(ids.insert(&c.id), "duplicate {}", c.id);
        assert!(!c.candidate.trim().is_empty());
        assert!(c.history.windows(2).all(|w| w[0].age_s >= w[1].age_s));
        assert!(matches!(c.stage.as_str(), "partial" | "final"));
    }
    for family in ["followup", "stale", "manual", "quotation", "asr"] {
        let pair: Vec<_> = cases.iter().filter(|c| c.family == family).collect();
        assert_eq!(pair.len(), 2, "{family}");
        assert_eq!(pair[0].candidate, pair[1].candidate, "{family}");
        assert_ne!(pair[0].accept, pair[1].accept, "{family}");
        assert_ne!(
            build(pair[0]),
            build(pair[1]),
            "indistinguishable requests: {family}"
        );
    }
    let partial = cases
        .iter()
        .find(|c| c.family == "revision" && c.stage == "partial")
        .unwrap();
    let full = cases
        .iter()
        .find(|c| c.family == "revision" && c.stage == "final")
        .unwrap();
    assert!(full.candidate.starts_with(&partial.candidate));
    assert!(partial.accept && !full.accept);
}

fn build(c: &Case) -> Value {
    let mut ctx = GateContext::default();
    let now = Instant::now();
    for (id, e) in c.history.iter().enumerate() {
        let at = now - Duration::from_secs(e.age_s);
        match e.source.as_str() {
            "ignored" => ctx.remember_at(id as u64, &e.text, at),
            "assistant" => ctx.chat.push(("assistant".into(), e.text.clone())),
            "manual" => {
                ctx.remember_at(id as u64, &e.text, at);
                let (sent, _) = ctx.manual(now).expect("recent manual claim");
                assert_eq!(sent, id as u64);
            }
            "accepted" => {
                ctx.accepted(e.text.clone());
            }
            other => panic!("unknown fixture source: {other}"),
        }
    }
    ctx.state(&c.candidate, false)
}

#[test]
fn wire_keeps_labels_private_and_separates_evidence() {
    for c in cases() {
        let wire = build(&c);
        let s = wire.as_str().unwrap();
        let state: Value = serde_json::from_str(s).unwrap();
        assert_eq!(
            state["recent_acceptance"],
            c.history
                .iter()
                .any(|h| matches!(h.source.as_str(), "manual" | "accepted")),
            "{}",
            c.id
        );
        let encoded_candidate = serde_json::to_string(&c.candidate).unwrap();
        assert!(
            s.ends_with(&format!(",\"candidate\":{encoded_candidate}}}")),
            "candidate not last: {}",
            c.id
        );
        assert_eq!(state["candidate"], c.candidate, "{}", c.id);
        assert!(!s.contains(&c.id));
        assert!(state.get("accept").is_none());
        let background = state["background_transcript"].as_array().unwrap();
        let chat = state["recent_chat"].as_array().unwrap();
        for h in &c.history {
            match h.source.as_str() {
                "ignored" => assert_eq!(
                    background.contains(&Value::String(h.text.clone())),
                    h.age_s <= MANUAL_TRIGGER_WINDOW.as_secs(),
                    "{}",
                    c.id
                ),
                "assistant" => assert!(
                    chat.contains(&serde_json::json!(["assistant", h.text])),
                    "{}",
                    c.id
                ),
                "manual" | "accepted" => assert!(
                    !background.contains(&Value::String(h.text.clone())),
                    "{}",
                    c.id
                ),
                _ => unreachable!(),
            }
        }
        if c.history.is_empty() {
            assert!(background.is_empty() && chat.is_empty());
        }
    }
}

// A recording owner with an inert worker. No audio hardware, network, or timer task.
fn inert_recording() -> ContinuousRecording {
    ContinuousRecording {
        paused: Default::default(),
        capturing: Default::default(),
        cancel: Default::default(),
        suppressed: Default::default(),
        epoch: Default::default(),
        level: Default::default(),
        events: Arc::new(Mutex::new(Events {
            history: Default::default(),
            context: Default::default(),
            queue: Default::default(),
            quiet: Default::default(),
            notify: Default::default(),
        })),
        worker: std::thread::spawn(|| {}),
    }
}
#[test]
fn bounded_roles_and_caption_order_do_not_cross_recordings_or_pause() {
    let rec = inert_recording();
    let other = inert_recording();
    let read = |r: &ContinuousRecording| {
        let wire = r.events.lock().unwrap().context.state("candidate", false);
        serde_json::from_str::<Value>(wire.as_str().unwrap()).unwrap()
    };
    let mut chat = (0..8)
        .map(|i| ("user".to_owned(), format!("t{i}")))
        .collect::<Vec<_>>();
    chat.push(("assistant".into(), "語".repeat(1025)));
    chat.push(("system".into(), "not a user turn".into()));
    rec.set_chat_context(chat);
    {
        let mut events = rec.events.lock().unwrap();
        for id in 0..10 {
            events.context.remember(id, &format!("caption{id}"));
        }
    }
    let state = read(&rec);
    assert_eq!(
        state["recent_chat"],
        serde_json::json!([
            ["user", "t5"],
            ["user", "t6"],
            ["user", "t7"],
            ["assistant", "語".repeat(512)]
        ])
    );
    assert_eq!(
        state["background_transcript"],
        serde_json::json!(["caption6", "caption7", "caption8", "caption9"])
    );
    let empty = read(&other);
    assert_ne!(state, empty);
    rec.set_paused(true);
    rec.set_chat_context(vec![("assistant".into(), "must not survive pause".into())]);
    rec.set_paused(false);
    assert_eq!(read(&rec), empty);
}

#[test]
#[ignore = "Explicit opt-in only: sends synthetic text to the configured Jev provider"]
fn provider_replay() {
    assert_eq!(
        std::env::var("JCODE_RUN_JEV_EVAL").as_deref(),
        Ok("1"),
        "set JCODE_RUN_JEV_EVAL=1 to consent to provider calls"
    );
    let client = jcode_base::jev::JevClient::for_voice().expect("configure a Jev voice account");
    println!(
        "provider={} model={}",
        client.provider_name(),
        client.model_id()
    );
    let threshold = ContinuousConfig::default().action_probability;
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (mut fp, mut fn_, mut errors) = (0, 0, 0);
        for c in cases() {
            let state = build(&c);
            match semantic_gate(&client, &c.candidate, threshold, state).await {
                Ok(Some((score, actual))) => {
                    let outcome = match (c.accept, actual) {
                        (false, true) => {
                            fp += 1;
                            "FP"
                        }
                        (true, false) => {
                            fn_ += 1;
                            "FN"
                        }
                        (true, true) => "TP",
                        (false, false) => "TN",
                    };
                    println!(
                        "{}: {outcome} score={score:.4} threshold={threshold:.4}",
                        c.id
                    );
                }
                other => {
                    errors += 1;
                    eprintln!("{}: unscored {other:?}", c.id);
                }
            }
        }
        println!("false_positive={fp} false_negative={fn_} unscored={errors}");
        assert_eq!((fp, fn_, errors), (0, 0, 0));
    });
}
