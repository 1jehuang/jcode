use super::{tests::*, *};

#[tokio::test]
async fn voice_subscription_uses_existing_noul_entitlement_without_bypassing_it() {
    for entitled in [false, true] {
        let mut replies = vec![(
            200,
            json!({"capabilities": {"memory_jev": entitled, "browser_jev": false}}).to_string(),
            vec![],
        )];
        if entitled {
            replies.push((200, response().to_string(), vec![]));
        }
        let (base, worker) = mock_server(replies);
        let mut client = mock_client(&base, JevProvider::Jcode);
        client.purpose = JevPurpose::Voice;
        let result = client
            .evaluate(json!({"transcript": "synthetic"}), questions())
            .await;
        let requests = worker.join().unwrap();
        assert!(requests[0].starts_with("GET /v1/me "));
        if entitled {
            result.unwrap();
            assert_eq!(requests.len(), 2);
            let body: Value =
                serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["model"], "typesafe/jev-1.13");
            assert!(body["state"].is_string());
            assert!(
                !requests[1]
                    .to_ascii_lowercase()
                    .contains("x-jcode-jev-purpose"),
                "only browser decisions use the browser budget"
            );
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("voice"));
            assert!(error.contains("memory_jev"));
            assert_eq!(
                requests.len(),
                1,
                "denied voice entitlement must not send a decision"
            );
        }
    }
}

#[tokio::test]
async fn voice_evaluate_once_never_hedges_or_retries() {
    for status in [200, 429, 503] {
        let (base, seen) = concurrent_mock(vec![(Duration::from_millis(250), status); 8]);
        let mut client = mock_client(&base, JevProvider::TypeSafe);
        client.purpose = JevPurpose::Voice;
        let result = client
            .evaluate_once(json!({"transcript":"synthetic"}), questions())
            .await;
        assert_eq!(result.is_ok(), status == 200);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn voice_hedges_a_slow_request_and_takes_the_first_valid_answer() {
    let (base, seen) = concurrent_mock(vec![(Duration::from_secs(3), 200), (Duration::ZERO, 200)]);
    let mut client = mock_client(&base, JevProvider::TypeSafe);
    client.purpose = JevPurpose::Voice;
    let started = std::time::Instant::now();
    let value = client
        .evaluate(json!({"transcript": "synthetic"}), questions())
        .await
        .unwrap();
    assert!(validate_answers(&value, &questions()).is_ok());
    assert!(started.elapsed() < Duration::from_secs(2), "hedge won");
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn voice_hedge_failure_waits_for_primary_and_memory_never_hedges() {
    let (base, seen) = concurrent_mock(vec![
        (Duration::from_millis(300), 200),
        (Duration::ZERO, 500),
    ]);
    let mut client = mock_client(&base, JevProvider::TypeSafe);
    client.purpose = JevPurpose::Voice;
    client
        .evaluate(json!({"transcript": "synthetic"}), questions())
        .await
        .unwrap();
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);

    let (base, seen) = concurrent_mock(vec![(Duration::from_millis(300), 200)]);
    mock_client(&base, JevProvider::TypeSafe)
        .evaluate(json!({"transcript": "synthetic"}), questions())
        .await
        .unwrap();
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn voice_typesafe_errors_never_retry_or_return_a_partial_report() {
    for status in [401, 402, 403, 500, 302] {
        let (base, worker) = mock_server(vec![(
            status,
            "private-error test-route-secret".into(),
            vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())],
        )]);
        let mut client = mock_client(&base, JevProvider::TypeSafe);
        client.purpose = JevPurpose::Voice;
        let error = crate::voice_intent::classify_with_client("Start a new session", &[], &client)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&status.to_string()));
        assert!(!error.contains("test-route-secret"));
        assert!(!error.contains("private-error"));
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn voice_direct_transport_preserves_typed_state_and_never_sends_openrouter_headers() {
    use crate::voice_intent::{QuickAction, VoiceIntent, classify_with_client, describe_questions};
    let transcript = "Start a new Jcode conversation";
    let answers: Map<String, Value> = describe_questions(transcript, &[])
        .unwrap()
        .into_iter()
        .map(|question| {
            let probability = match question.id.as_str() {
                "quick_action" | "new_session" => 0.99,
                _ => 0.01,
            };
            (question.id, json!({"type": "noul", "noul": probability}))
        })
        .collect();
    let (base, worker) = mock_server(vec![(200, json!({"answers": answers}).to_string(), vec![])]);
    let mut client = mock_client(&base, JevProvider::TypeSafe);
    client.purpose = JevPurpose::Voice;
    client.endpoint = format!("{base}/v1/systemone");
    let report = classify_with_client(transcript, &[], &client)
        .await
        .unwrap();
    assert_eq!(
        report.intent,
        VoiceIntent::QuickAction(QuickAction::NewSession)
    );
    let requests = worker.join().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "direct API has no gateway entitlement preflight"
    );
    let request = &requests[0];
    assert!(request.starts_with("POST /v1/systemone "));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-route-secret\r\n")
    );
    assert!(!request.to_ascii_lowercase().contains("http-referer:"));
    assert!(!request.to_ascii_lowercase().contains("x-title:"));
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["transcript"], transcript);
    assert_eq!(body["questions"].as_object().unwrap().len(), 6);
}

#[tokio::test]
async fn voice_twenty_candidates_cross_real_transport_in_bounded_same_state_batches() {
    use crate::voice_intent::{
        SessionCandidate, VoiceIntent, classify_with_client, describe_questions,
    };
    let offered: Vec<_> = (0..20)
        .map(|index| SessionCandidate {
            id: format!("private-session-{index}"),
            title: format!("Investigate voice routing request budgets and preserve classification safety for recent conversation {index}: compare candidate metadata, JSON escaping, provider transport limits, and regression coverage"),
            working_dir: Some(r#"C:\Users\example\projects\jcode\"voice routing""#.into()),
        })
        .collect();
    let transcript = "Open conversation 19";
    let questions = describe_questions(transcript, &offered).unwrap();
    assert_eq!(questions.len(), 26);
    // Exercise both object-state and string-state providers, including the
    // gateway's real entitlement preflight on every bounded evaluation.
    for provider in [
        JevProvider::TypeSafe,
        JevProvider::OpenRouter,
        JevProvider::Aimlapi,
        JevProvider::Jcode,
    ] {
        // All batches participate in ranking, with no competing-score veto.
        for competing in [0.01, 0.9, 0.99, 1.0] {
            let batches = questions.chunks(provider.max_questions()).count();
            let per_batch = if provider == JevProvider::Jcode { 2 } else { 1 };
            // Batches run concurrently, so answer whatever each request asks.
            let (base, worker) = mock_server_with(batches * per_batch, move |request| {
                if request.starts_with("GET ") {
                    let me = json!({"capabilities": {"memory_jev": true}});
                    return (200, me.to_string(), vec![]);
                }
                let body: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                let answers: Map<String, Value> = body["questions"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|id| {
                        let probability = match id.as_str() {
                            "navigation" | "candidate_19" => 0.99,
                            "coding_agent" => competing,
                            _ => 0.01,
                        };
                        (id.clone(), json!({"type": "noul", "noul": probability}))
                    })
                    .collect();
                (200, json!({"answers": answers}).to_string(), vec![])
            });
            let result = classify_with_client(transcript, &offered, &mock_client(&base, provider))
                .await
                .unwrap();
            assert_eq!(
                result.intent,
                if competing < 0.99 {
                    VoiceIntent::OpenSession(offered[19].id.clone())
                } else {
                    VoiceIntent::CodingAgent
                }
            );
            assert_eq!(result.answers.len(), 26);
            assert_eq!(
                result.answers.iter().map(|a| &a.id).collect::<Vec<_>>(),
                questions.iter().map(|q| &q.id).collect::<Vec<_>>()
            );
            assert_eq!(
                result
                    .answers
                    .iter()
                    .find(|a| a.id == "candidate_19")
                    .unwrap()
                    .probability,
                0.99
            );
            let requests = worker.join().unwrap();
            let bodies: Vec<Value> = requests
                .iter()
                .filter(|r| r.starts_with("POST "))
                .map(|r| {
                    let wire = r.split_once("\r\n\r\n").unwrap().1;
                    assert!(wire.len() <= MAX_REQUEST_BYTES);
                    assert!(wire.len() < 32 * 1024, "voice batch bytes: {}", wire.len());
                    serde_json::from_str(wire).unwrap()
                })
                .collect();
            assert_eq!(bodies.len(), batches);
            assert!(
                bodies
                    .iter()
                    .all(|body| body["state"] == bodies[0]["state"])
            );
            let state = if let Some(text) = bodies[0]["state"].as_str() {
                serde_json::from_str::<Value>(text).unwrap()
            } else {
                bodies[0]["state"].clone()
            };
            assert_eq!(state["candidates"].as_object().unwrap().len(), 20);
            assert_eq!(state["transcript"], transcript);
            assert!(
                state["policy"]
                    .as_str()
                    .unwrap()
                    .contains("untrusted evidence")
            );
            for (index, candidate) in offered.iter().enumerate() {
                assert_eq!(
                    state["candidates"][format!("candidate_{index}")]["title"],
                    candidate.title
                );
                assert_eq!(
                    state["candidates"][format!("candidate_{index}")]["working_dir"],
                    candidate.working_dir.as_deref().unwrap()
                );
            }
            assert!(!state.to_string().contains("private-session"));
            let mut sent = Map::new();
            for body in &bodies {
                let batch = body["questions"].as_object().unwrap();
                assert!(batch.len() <= provider.max_questions());
                sent.extend(batch.clone());
            }
            assert_eq!(sent.len(), questions.len());
            for question in &questions {
                assert_eq!(sent[&question.id]["instructions"], question.instructions);
                assert_eq!(sent[&question.id]["criteria"]["true"], question.yes);
                assert_eq!(sent[&question.id]["criteria"]["false"], question.no);
            }
        }
    }
}

#[tokio::test]
async fn voice_later_batch_errors_never_return_partial_classification() {
    use crate::voice_intent::{SessionCandidate, classify_with_client, describe_questions};
    let offered: Vec<_> = (0..20)
        .map(|index| SessionCandidate {
            id: format!("session-{index}"),
            title: format!("Conversation {index}"),
            working_dir: None,
        })
        .collect();
    let questions = describe_questions("next conversation", &offered).unwrap();
    let first: Map<String, Value> = questions[..MAX_QUESTIONS]
        .iter()
        .map(|q| {
            (
                q.id.clone(),
                json!({"type": "noul", "noul": if q.id == "coding_agent" { 0.99 } else { 0.01 }}),
            )
        })
        .collect();
    let last: Map<String, Value> = questions[MAX_QUESTIONS..]
        .iter()
        .map(|q| (q.id.clone(), json!({"type": "noul", "noul": 0.01})))
        .collect();
    let mut failures = vec![
        (500, "{}".into(), vec![]),
        (200, json!({"answers": {}}).to_string(), vec![]),
    ];
    for invalid in [
        json!({"type": "noul", "noul": 1.1}),
        json!({"type": "noul", "noul": "0.99"}),
        json!({"type": "choice", "choice": "yes"}),
    ] {
        let mut bad = last.clone();
        bad.insert(questions[MAX_QUESTIONS].id.clone(), invalid);
        failures.push((200, json!({"answers": bad}).to_string(), vec![]));
    }
    let mut wrong_ids = last.clone();
    wrong_ids.remove(&questions[MAX_QUESTIONS].id);
    wrong_ids.insert("invented".into(), json!({"type": "noul", "noul": 0.99}));
    failures.push((200, json!({"answers": wrong_ids}).to_string(), vec![]));
    for failure in failures {
        // Batches run concurrently, so reply by which batch was requested.
        let later = questions[MAX_QUESTIONS].id.clone();
        let good = json!({"answers": first}).to_string();
        let mut failure = Some(failure);
        let (base, worker) = mock_server_with(2, move |request| {
            let body: Value =
                serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            if body["questions"].get(&later).is_some() {
                failure.take().unwrap()
            } else {
                (200, good.clone(), vec![])
            }
        });
        assert!(
            classify_with_client(
                "next conversation",
                &offered,
                &mock_client(&base, JevProvider::OpenRouter)
            )
            .await
            .is_err()
        );
        assert_eq!(worker.join().unwrap().len(), 2);
    }
}
