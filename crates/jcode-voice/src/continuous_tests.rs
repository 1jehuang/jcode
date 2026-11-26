#[test]
fn recovery_is_bounded_and_never_retries_known_permanent_errors() {
    for error in [
        VoiceError::Network,
        VoiceError::Timeout,
        VoiceError::Http(503),
        VoiceError::Http(429),
    ] {
        assert!(recovery_delay(&error, 0) < recovery_delay(&error, 1));
        assert_eq!(
            recovery_delay(&error, usize::MAX),
            Some(Duration::from_secs(60))
        );
    }
    assert_eq!(recovery_delay(&VoiceError::NariRejected, 2), None);
    for error in [
        VoiceError::NariNotConfigured,
        VoiceError::NariCreditsExhausted,
        VoiceError::Http(401),
        VoiceError::Http(402),
        VoiceError::Http(403),
        VoiceError::Http(400),
        VoiceError::Cancelled,
        VoiceError::CaptureFailed,
    ] {
        assert_eq!(recovery_delay(&error, 0), None);
    }
}

#[tokio::test]
async fn rejected_warm_session_never_opens_a_second_connection() {
    for error in [
        VoiceError::NariNotConfigured,
        VoiceError::NariCreditsExhausted,
        VoiceError::NariRejected,
    ] {
        let expected = error.clone();
        let failed = tokio::spawn(async move { Err(error) });
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        let (partial, _) = tokio::sync::watch::channel(String::new());
        // Empty key makes an unintended fallback fail differently, without network I/O.
        let result = stream_job(
            Some(failed),
            String::new(),
            rx,
            Arc::new(AtomicBool::new(false)),
            |text| {
                partial.send_replace(text);
            },
        )
        .await;
        assert_eq!(result, Err(expected));
    }
}

#[test]
fn quiet_window_requires_recent_contiguous_silence() {
    use super::*;
    let start = Instant::now();
    let t = |ms| start + Duration::from_millis(ms);
    let mut quiet = QuietWindow::default();
    for ms in [0, 100, 200] {
        quiet.observe(t(ms), false);
    }
    assert!(!quiet.ready(t(200)));
    quiet.observe(t(250), false);
    assert!(quiet.ready(t(250)));
    assert!(!quiet.ready(t(501)));
    quiet.observe(t(501), false);
    assert!(!quiet.ready(t(501)));
    quiet.observe(t(550), true);
    quiet.observe(t(650), false);
    assert!(!quiet.ready(t(650)));
}
use super::*;

#[cfg(unix)]
#[test]
#[ignore = "Requires synthetic JCODE_TEST_CONTINUOUS_PCM and configured Nari/Jev accounts, no real microphone"]
fn real_continuous_synthetic_audio_produces_one_gated_request() {
    let fixture = std::env::var("JCODE_TEST_CONTINUOUS_PCM").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let key = crate::nari_api_key().unwrap();
    let recording = ContinuousRecording::start_auto(
        Arc::new(AtomicBool::new(false)), &key,
        "cat \"$JCODE_TEST_CONTINUOUS_PCM\"; while :; do dd if=/dev/zero bs=3200 count=1 2>/dev/null; sleep 0.1; done",
        ContinuousConfig {action_probability: 0.97, ..Default::default()},
    ).unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut accepted = 0;
    while Instant::now() < deadline {
        while let Some(event) = recording.try_event() {
            match event {
                ContinuousEvent::Transcript { text, .. } => {
                    assert!(text.to_lowercase().contains("window"));
                    accepted += 1;
                }
                ContinuousEvent::Error(error) => {
                    panic!("Synthetic speech pipeline failed: {error}")
                }
                _ => {}
            }
        }
        if accepted > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    recording.stop();
    assert_eq!(
        accepted, 1,
        "Synthetic request must pass actual transcription and intent gating"
    );
}

#[test]
#[ignore = "Uses configured Jev account for three bounded synthetic classifications, no microphone"]
fn live_semantic_gate_accepts_direct_request_and_rejects_ambient() {
    let client = jcode_base::jev::JevClient::for_voice().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for (text, expected) in [
            ("Jarvis, please tell me which window is focused.", true),
            ("The weather looks nice today and I enjoyed lunch.", false),
            ("In the movie he said, Jarvis please open the door. I am just describing the scene, not asking you to do anything.", false),
        ] {
            let accepted = semantic_gate(&client, text, 0.97, GateContext::default().state(text,false)).await.unwrap().unwrap().1;
            println!("synthetic semantic gate accepted={accepted}, expected={expected}");
            assert_eq!(accepted, expected);
        }
    });
}
#[test]
#[ignore = "Bounded synthetic Jev evaluation; uses a configured account, never opens the microphone"]
fn live_semantic_gate_contextual_cases() {
    // A null expectation denotes a diagnostic probe, not a required dispatch.
    type Case = (Vec<String>, String, bool, Option<bool>);
    let cases: std::collections::BTreeMap<String, Case> =
        serde_json::from_str(include_str!("continuous_gate_cases.json")).unwrap();
    let client = jcode_base::jev::JevClient::for_voice().unwrap();
    let threshold = ContinuousConfig::default().action_probability;
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for (name, (earlier, candidate, require_name, expected)) in cases {
            let mut ctx = GateContext::default();
            for text in earlier {
                ctx.remember(1, &text);
            }
            let (score, accepted) = semantic_gate(
                &client,
                &candidate,
                threshold,
                ctx.state(&candidate, require_name),
            )
            .await
            .unwrap()
            .unwrap();
            println!("{name}: {score:.2} expected={expected:?} accepted={accepted}");
            if let Some(expected) = expected {
                assert_eq!(accepted, expected, "{name}: {score}");
            }
        }
    });
}
#[test]
fn rejected_start_never_opens_capture() {
    assert!(matches!(
        ContinuousRecording::start_auto(
            Arc::new(AtomicBool::new(true)),
            "key",
            "",
            ContinuousConfig::default()
        ),
        Err(VoiceError::Cancelled)
    ));
    assert!(matches!(
        ContinuousRecording::start_auto(
            Arc::new(AtomicBool::new(false)),
            "",
            "",
            ContinuousConfig::default()
        ),
        Err(VoiceError::NariNotConfigured)
    ));
}

#[cfg(unix)]
#[test]
fn silent_external_capture_has_heartbeat_suppresses_and_stops_without_network() {
    // This synthetic recorder emits silence, not microphone audio. An invalid
    // provider key would fail if the engine accidentally uploaded silence.
    let recording = ContinuousRecording::start_auto(
        Arc::new(AtomicBool::new(false)),
        "synthetic-invalid-key",
        "while :; do dd if=/dev/zero bs=3200 count=1 2>/dev/null; sleep 0.1; done",
        ContinuousConfig {
            bypass_external_guards: true,
            ..Default::default()
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut listening, mut heartbeat) = (false, false);
    while !(listening && heartbeat) && Instant::now() < deadline {
        while let Some(event) = recording.try_event() {
            match event {
                ContinuousEvent::State(VoiceState::Listening) => listening = true,
                ContinuousEvent::Level(value) => {
                    assert_eq!(value, 0.0);
                    heartbeat = true;
                }
                ContinuousEvent::Error(error) => panic!("unexpected capture error: {error}"),
                ContinuousEvent::Transcript { .. } => panic!("silence was transcribed"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        listening && heartbeat,
        "capture must report real readiness and telemetry"
    );
    recording.set_suppressed(true);
    assert_eq!(recording.audio_level(), 0.0);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_suppressed = false;
    while !saw_suppressed && Instant::now() < deadline {
        while let Some(event) = recording.try_event() {
            if matches!(event, ContinuousEvent::State(VoiceState::Suppressed)) {
                saw_suppressed = true;
            }
            assert!(!matches!(
                event,
                ContinuousEvent::Error(_) | ContinuousEvent::Transcript { .. }
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_suppressed);
    recording.set_paused(true);
    let deadline = Instant::now() + Duration::from_secs(2);
    while recording.is_capturing() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!recording.is_capturing(), "pause must close recorder");
    recording.set_paused(false);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !recording.is_capturing() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(recording.is_capturing(), "resume must reopen recorder");
    recording.set_suppressed(false);
    let stopped_at = Instant::now();
    recording.stop();
    while !recording.is_finished() && stopped_at.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        recording.is_finished(),
        "stop must tear down recorder promptly"
    );
    assert_eq!(recording.audio_level(), 0.0);
    let mut stopped = false;
    while let Some(event) = recording.try_event() {
        assert!(!matches!(
            event,
            ContinuousEvent::Error(_) | ContinuousEvent::Transcript { .. }
        ));
        stopped |= matches!(event, ContinuousEvent::Stopped);
    }
    assert!(stopped);
}

#[test]
fn gate_response_is_typed_and_fail_closed() {
    for bad in [
        serde_json::json!({}),
        serde_json::json!({"answers":{"directed":{"noul":"1"}}}),
        serde_json::json!({"answers":{"directed":{"noul":1.1}}}),
    ] {
        assert!(gate_answer(&bad, 0.95).is_err());
    }
    assert!(
        !gate_answer(
            &serde_json::json!({"answers":{"directed":{"noul":0.94}}}),
            0.95
        )
        .unwrap()
        .1
    );
    assert!(
        gate_answer(
            &serde_json::json!({"answers":{"directed":{"noul":0.95}}}),
            0.95
        )
        .unwrap()
        .1
    );
    assert!(!ContinuousConfig::default().require_jev);
    assert_eq!(
        directed_transcript("Jarvis open settings", true).as_deref(),
        Some("open settings")
    );
}
#[test]
fn silence_never_segments() {
    let mut s = Segmenter::default();
    for _ in 0..1000 {
        assert!(s.push(&[0; 1600]).is_empty());
    }
    assert!(s.pre.len() <= 4800);
}
#[test]
fn speech_has_preroll_and_trailing_silence() {
    let mut s = Segmenter::default();
    s.push(&[0; 4800]);
    assert!(s.push(&[3000; 16000]).is_empty());
    let out = s.push(&[0; (PAUSE_MS + COMMIT_GRACE_MS) * 16]);
    assert_eq!(out.len(), 1);
    assert!(out[0].len() <= 320000);
    assert!(!s.active);
}
#[test]
fn resumed_speech_cancels_commit_without_opening_another_stream() {
    let mut s = Segmenter::default();
    let mut transitions = Vec::new();
    for (value, ms) in [(3000, 400), (0, 1280), (3000, 200), (0, 1280)] {
        transitions.extend(s.push_stream(&vec![value; ms * 16]));
    }
    transitions.retain(|e| !matches!(e, Seg::Audio(_)));
    assert!(matches!(
        transitions.as_slice(),
        [Seg::Start(_), Seg::Pause, Seg::Resume, Seg::Pause]
    ));
    assert!(matches!(
        s.push_stream(&[0; 320]).as_slice(),
        [Seg::Audio(_), Seg::End]
    ));
    assert!(s.push_stream(&[0; 16000]).is_empty());
}
#[tokio::test]
async fn long_network_outage_then_pause_resume_commits_only_fresh_audio() {
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/voice/realtime", listener.local_addr().unwrap());
    const OFFLINE_ATTEMPTS: usize = 8;
    let server = tokio::spawn(async move {
        // Drop each TCP connection before the upgrade while the provider is offline.
        for _ in 0..OFFLINE_ATTEMPTS {
            drop(listener.accept().await.unwrap().0);
        }
        let mut ws = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let mut samples = 0;
        while let Some(msg) = ws.next().await {
            let v: Value = serde_json::from_str(msg.unwrap().to_text().unwrap()).unwrap();
            let response = match v["type"].as_str().unwrap() {
                "session.configure" => json!({"type":"session.configured"}),
                "input_audio_buffer.append" => {
                    use base64::Engine;
                    samples += base64::engine::general_purpose::STANDARD
                        .decode(v["audio"].as_str().unwrap())
                        .unwrap()
                        .len()
                        / 2;
                    continue;
                }
                "input_audio_buffer.commit" => {
                    assert_eq!(samples, (400 + 1000 + 200 + 1300) * 16);
                    ws.send(Message::Text(json!({"type":"input_audio_buffer.committed","item_id":"a","client_event_id":v["event_id"]}).to_string())).await.unwrap();
                    json!({"type":"transcript.completed","item_id":"a","transcript":"both halves"})
                }
                other => panic!("unexpected message {other}"),
            };
            ws.send(Message::Text(response.to_string())).await.unwrap();
            if v["type"] == "input_audio_buffer.commit" {
                return;
            }
        }
        panic!("stream closed before final commit");
    });
    let cancel = Arc::new(AtomicBool::new(false));
    for failure in 0..OFFLINE_ATTEMPTS {
        let endpoint = url.clone();
        let stopped = cancel.clone();
        let warm =
            tokio::spawn(async move { NariSession::connect_to(&endpoint, "test", stopped).await });
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(vec![42; 320]).unwrap();
        drop(tx);
        let error = stream_job(Some(warm), String::new(), rx, cancel.clone(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(error, VoiceError::Network);
        assert_eq!(
            recovery_delay(&error, failure).unwrap().as_secs(),
            [2, 8, 60][failure.min(2)]
        );
    }
    let connect_cancel = cancel.clone();
    let mut warm = Some(tokio::spawn(async move {
        NariSession::connect_relay(&url, "test", connect_cancel).await
    }));
    let mut segmenter = Segmenter::default();
    let mut live = None;
    let mut job = None;
    for (value, ms) in [(3000, 400), (0, 1000), (3000, 200), (0, 1300)] {
        for event in segmenter.push_stream(&vec![value; ms * 16]) {
            match event {
                Seg::Start(audio) => {
                    assert!(job.is_none());
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    tx.send(audio).unwrap();
                    live = Some(tx);
                    let warm = warm.take().unwrap();
                    let cancel = cancel.clone();
                    job = Some(tokio::spawn(async move {
                        stream_job(Some(warm), String::new(), rx, cancel, |_| {}).await
                    }));
                }
                Seg::Audio(audio) => live.as_ref().unwrap().send(audio).unwrap(),
                Seg::Pause | Seg::Resume => assert!(live.is_some()),
                Seg::End => drop(live.take().unwrap()),
            }
        }
    }
    assert!(live.is_none());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), job.unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "both halves"
    );
    server.await.unwrap();
}
#[test]
fn brief_noise_is_not_speech() {
    let mut s = Segmenter::default();
    s.push(&[10000; 1600]);
    assert!(s.push(&[0; 16000]).is_empty());
}
#[test]
fn maximum_utterance_is_bounded() {
    let mut s = Segmenter::default();
    let out = s.push(&vec![3000; 400000]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), 320000);
}
#[test]
fn conservative_gate() {
    assert_eq!(
        directed_transcript("Hey Jev, open settings", true).as_deref(),
        Some("open settings")
    );
    for text in [
        "open settings",
        "thanks Jev",
        "Jev",
        "Jev is great",
        "I told Jev to open settings",
        "Jev, okay",
        "okay Jev",
    ] {
        assert!(directed_transcript(text, true).is_none(), "{text}");
    }
    assert!(directed_transcript("open settings", false).is_some());
    assert!(directed_transcript("that's interesting", false).is_none());
}
#[test]
fn gate_separates_candidate_history_and_chat() {
    let mut ctx = GateContext::default();
    ctx.chat.push(("assistant".into(), "Which one?".into()));
    for i in 0..20 {
        ctx.remember(i, &i.to_string());
    }
    let trigger = "  The \"second\" one.  ";
    let wire = ctx.state(trigger, false);
    let state: serde_json::Value = serde_json::from_str(wire.as_str().unwrap()).unwrap();
    assert_eq!(state["candidate"], trigger);
    assert_eq!(state["recent_chat"][0][1], "Which one?");
    assert_eq!(
        state["background_transcript"],
        serde_json::json!(["16", "17", "18", "19"])
    );
    let next = ctx.state("changed", false);
    let prefix = |s: &str| s.rsplit_once(",\"candidate\":").unwrap().0.to_owned();
    assert_eq!(
        prefix(wire.as_str().unwrap()),
        prefix(next.as_str().unwrap())
    );
    let payload: serde_json::Value = serde_json::from_str(&ctx.accepted(trigger.into())).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({"background_transcript": (4..20).map(|i| i.to_string()).collect::<Vec<_>>(), "trigger":trigger})
    );
    assert!(ctx.background.is_empty());
    assert_eq!(ctx.chat.len(), 1);
}
#[test]
fn scored_captions_precede_dispatch_and_unscored_speech_stays_background() {
    let mut events = Events {
        history: history::History::default(),
        context: GateContext::default(),
        queue: VecDeque::new(),
        quiet: QuietWindow::default(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    events.history.reset(Some(true));
    for (id, text, raw) in [
        (1, "ambient", Some(0.943)),
        (2, "unscored", None),
        (3, "request", Some(0.972)),
    ] {
        events.push(ContinuousEvent::Caption {
            id,
            text: text.into(),
        });
        let score = raw.map(|p| {
            gate_answer(
                &serde_json::json!({"answers":{"directed":{"noul":p}}}),
                0.97,
            )
            .unwrap()
        });
        let mut audio = events.history.begin();
        audio.append(&[id as i16, -9]);
        events.classified(ClassifiedCaption {
            audio,
            id,
            text: text.into(),
            score,
        });
        assert!(
            matches!(events.queue.pop_front(), Some(ContinuousEvent::Caption { id: n, .. }) if n == id)
        );
        if let Some(p) = raw {
            assert!(
                matches!(events.queue.pop_front(), Some(ContinuousEvent::CaptionScore { id: n, probability, accepted }) if n == id && probability == p && accepted == (id == 3))
            );
        }
        if id == 3 {
            let Some(ContinuousEvent::Transcript {
                id: 3,
                text: payload,
            }) = events.queue.pop_front()
            else {
                panic!("missing accepted request")
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
                serde_json::json!({"trigger":"request","background_transcript":["ambient","unscored"]})
            );
        }
        assert!(events.queue.is_empty());
    }
    assert_eq!(events.history.status().1, [3]);
    assert_eq!(
        events.history.clip(3).unwrap(),
        (vec![3, -9], "request".into())
    );
}
#[test]
fn manual_trigger_sends_only_recent_speech() {
    let mut ctx = GateContext::default();
    let now = Instant::now();
    assert!(ctx.manual(now).is_none(), "nothing heard");
    ctx.remember_at(
        1,
        "old",
        now - MANUAL_TRIGGER_WINDOW - Duration::from_secs(1),
    );
    ctx.remember_at(2, "first", now - Duration::from_secs(10));
    ctx.remember_at(3, "latest", now);
    let (id, text) = ctx.manual(now).unwrap();
    assert_eq!(id, 3);
    let payload: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({"trigger":"latest","background_transcript":["first"]})
    );
    assert!(ctx.manual(now).is_none(), "each utterance is sent once");
    let probe = |ctx: &GateContext| {
        let wire = ctx.state("and the second?", false);
        serde_json::from_str::<serde_json::Value>(wire.as_str().unwrap()).unwrap()
    };
    assert_eq!(probe(&ctx)["recent_acceptance"], true);
    assert_eq!(probe(&ctx)["background_transcript"], serde_json::json!([]));
    assert!(!probe(&ctx).to_string().contains("latest"));
    ctx.last_acceptance = Some(now - MANUAL_TRIGGER_WINDOW - Duration::from_secs(1));
    assert_eq!(probe(&ctx)["recent_acceptance"], false);
    assert_eq!(probe(&GateContext::default())["recent_acceptance"], false);
    // Speech still at the gate is claimed, and its later score dropped.
    let mut events = Events {
        history: history::History::default(),
        context: GateContext::default(),
        queue: VecDeque::new(),
        quiet: QuietWindow::default(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    events.context.remember_at(6, "before", now);
    events.context.inflight = Some((7, "just said".into()));
    assert!(events.manual_trigger());
    let Some(ContinuousEvent::Transcript {
        id: 7,
        text: payload,
    }) = events.queue.pop_front()
    else {
        panic!("manual trigger queued nothing")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
        serde_json::json!({"trigger":"just said","background_transcript":["before"]})
    );
    events.classified(ClassifiedCaption {
        audio: Default::default(),
        id: 7,
        text: "just said".into(),
        score: Some((0.99, true)),
    });
    assert!(events.queue.is_empty(), "claimed caption is not sent twice");
    assert!(events.context.background.is_empty());
    for i in 0..20 {
        ctx.remember_at(i, &i.to_string(), now);
    }
    assert_eq!((ctx.background.len(), ctx.heard.len()), (16, 16));
}
#[test]
#[ignore = "Live latency probe: JCODE_TEST_CONTINUOUS_PCM raw 16 kHz s16le, uses Nari and Jev accounts"]
fn live_latency_profile() {
    let pcm = std::fs::read(std::env::var("JCODE_TEST_CONTINUOUS_PCM").unwrap()).unwrap();
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let key = crate::nari_api_key().unwrap();
    let gate = jcode_base::jev::JevClient::for_voice().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        for round in 0..3 {
            let cancel = Arc::new(AtomicBool::new(false));
            let t0 = Instant::now();
            let session = NariSession::connect(&key, cancel).await.unwrap();
            let connected = t0.elapsed();
            let (tx, rx) = crate::nari_pcm_channel();
            let chunks: Vec<Vec<i16>> = samples
                .chunks(crate::NARI_PCM_CHUNK_SAMPLES)
                .map(<[i16]>::to_vec)
                .collect();
            let send = async move {
                for chunk in chunks {
                    let _ = tx.send(chunk).await;
                }
            };
            let t1 = Instant::now();
            let (_, text) = tokio::join!(send, session.run(rx, |_| {}));
            let text = text.unwrap();
            let transcribed = t1.elapsed();
            let mut ctx = GateContext::default();
            ctx.chat.push(("assistant".into(), "Hi.".into()));
            let state = ctx.state(&text, false);
            let t2 = Instant::now();
            let score = semantic_gate(&gate, &text, 0.5, state).await.unwrap();
            let gated = t2.elapsed();
            eprintln!(
                "round {round}: nari connect {:>4} ms, transcribe {:>4} ms, gate {:>4} ms, score {score:?}",
                connected.as_millis(),
                transcribed.as_millis(),
                gated.as_millis()
            );
        }
    });
}
#[test]
#[ignore = "Live probe: does an idle Nari session survive and stay fast? Uses Nari account"]
fn live_nari_idle_probe() {
    let pcm = std::fs::read(std::env::var("JCODE_TEST_CONTINUOUS_PCM").unwrap()).unwrap();
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let key = crate::nari_api_key().unwrap();
    let idle: Vec<u64> = std::env::var("JCODE_TEST_NARI_IDLE")
        .unwrap_or_else(|_| "0,30,120".into())
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        for secs in &idle {
            let session = NariSession::connect(&key, Arc::new(AtomicBool::new(false)))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(*secs)).await;
            let (tx, rx) = crate::nari_pcm_channel();
            let chunks: Vec<Vec<i16>> = samples
                .chunks(crate::NARI_PCM_CHUNK_SAMPLES)
                .map(<[i16]>::to_vec)
                .collect();
            let send = async move {
                for chunk in chunks {
                    let _ = tx.send(chunk).await;
                }
            };
            let t = Instant::now();
            let (_, text) = tokio::join!(send, session.run(rx, |_| {}));
            eprintln!(
                "idle {secs:>4} s: {:>4} ms {:?}",
                t.elapsed().as_millis(),
                text.map(|t| t.chars().take(40).collect::<String>())
            );
        }
    });
}
/// Real-time paced end-to-end latency: speech end to accepted request.
#[cfg(unix)]
#[test]
#[ignore = "Live latency e2e: JCODE_TEST_CONTINUOUS_PCM, Nari/Jev accounts, no real microphone"]
fn live_e2e_latency() {
    let fixture = std::env::var("JCODE_TEST_CONTINUOUS_PCM").unwrap();
    let rounds: usize = std::env::var("JCODE_TEST_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let warm = std::env::var("JCODE_TEST_WARM").map_or(true, |v| v != "0");
    let marker = std::env::temp_dir().join(format!("jcode-e2e-{}", std::process::id()));
    let key = crate::nari_api_key().unwrap();
    // Pace 100 ms chunks in real time, stamp the speech end, then feed silence.
    // Each round waits idle first so the warm spare (if enabled) is connected.
    let script = format!(
        "rounds={rounds}; f=\"{fixture}\"; n=$(( $(stat -c%s \"$f\") / 3200 )); \
         idle() {{ i=0; while [ $i -lt $1 ]; do dd if=/dev/zero bs=3200 count=1 2>/dev/null; sleep 0.1; i=$((i+1)); done; }}; \
         r=0; while [ $r -lt $rounds ]; do idle 40; i=0; while [ $i -lt $n ]; do dd if=\"$f\" bs=3200 skip=$i count=1 2>/dev/null; sleep 0.1; i=$((i+1)); done; \
         date +%s%N >> {m}; idle 60; r=$((r+1)); done; while :; do idle 10; done",
        m = marker.display()
    );
    let _ = std::fs::remove_file(&marker);
    let recording = ContinuousRecording::start_auto(
        Arc::new(AtomicBool::new(false)),
        &key,
        &script,
        ContinuousConfig {
            action_probability: 0.5,
            warm_nari: warm,
            ..Default::default()
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20 * rounds as u64 + 20);
    let mut seen = Vec::new();
    let mut fired: Option<i128> = None;
    let now_ns = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i128
    };
    while Instant::now() < deadline && seen.len() < rounds {
        let ends = std::fs::read_to_string(&marker).unwrap_or_default();
        let ends: Vec<i128> = ends.lines().filter_map(|l| l.trim().parse().ok()).collect();
        // Pair the trigger with this round's speech end; negative means it
        // fired on a partial before the user stopped talking.
        if let (Some(at), Some(end)) = (fired, ends.get(seen.len())) {
            let ms = (at - end) / 1_000_000;
            eprintln!(
                "warm={warm} round {}: speech end -> accepted request {ms} ms",
                seen.len()
            );
            seen.push(ms);
            fired = None;
        }
        while let Some(event) = recording.try_event() {
            match event {
                ContinuousEvent::Transcript { .. } if fired.is_none() => fired = Some(now_ns()),
                ContinuousEvent::Caption { id, text } => {
                    eprintln!("caption {id} +{}ms: {text}", now_ns() / 1_000_000 % 100_000)
                }
                ContinuousEvent::Continuation { text, .. } => {
                    eprintln!("continuation: {} words", text.split_whitespace().count())
                }
                ContinuousEvent::Error(error) => panic!("pipeline failed: {error}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    recording.stop();
    let _ = std::fs::remove_file(&marker);
    assert_eq!(seen.len(), rounds);
}
#[test]
fn pending_requests_survive_telemetry_but_not_invalidation() {
    let mut e = Events {
        history: history::History::default(),
        context: GateContext::default(),
        queue: VecDeque::new(),
        quiet: QuietWindow::default(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let request = |id| ContinuousEvent::Transcript {
        id,
        text: "private".into(),
    };
    let final_text = |id| ContinuousEvent::Continuation {
        id,
        text: "private".into(),
    };
    assert_eq!(request(7).discard_marker(), Some((7, "accepted_discarded")));
    assert_eq!(
        final_text(7).discard_marker(),
        Some((7, "continuation_discarded"))
    );
    for id in 1..=32 {
        e.push(request(id));
    }
    e.push(ContinuousEvent::Caption {
        id: 34,
        text: "ambient".into(),
    });
    assert_eq!(
        e.queue.front().unwrap().discard_marker(),
        Some((1, "accepted_discarded"))
    );
    e.push(final_text(32));
    assert_eq!(
        e.queue.front().unwrap().discard_marker(),
        Some((2, "accepted_discarded"))
    );
    e.clear_pending_speech("suppressed");
    assert!(e.queue.is_empty());
    e.push(ContinuousEvent::Stopped);
    e.push(final_text(9));
    e.clear_pending_speech("stopped");
    assert!(matches!(
        e.queue.pop_front(),
        Some(ContinuousEvent::Stopped)
    ));
    assert!(e.queue.is_empty());
}
#[test]
fn caption_revision_keeps_score_after_row_even_before_poll() {
    use super::*;
    let mut e = Events {
        history: history::History::default(),
        context: GateContext::default(),
        queue: VecDeque::new(),
        quiet: QuietWindow::default(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let caption = |id, text: &str| ContinuousEvent::Caption {
        id,
        text: text.into(),
    };
    e.push(caption(1, "start"));
    e.push(ContinuousEvent::CaptionScore {
        id: 1,
        probability: 0.98,
        accepted: true,
    });
    e.push(caption(2, "another"));
    e.push(caption(1, "revised"));
    assert!(
        matches!(e.queue.pop_front(), Some(ContinuousEvent::Caption { id: 1, text }) if text == "revised")
    );
    assert!(matches!(
        e.queue.pop_front(),
        Some(ContinuousEvent::CaptionScore {
            id: 1,
            accepted: true,
            ..
        })
    ));
    assert!(matches!(
        e.queue.pop_front(),
        Some(ContinuousEvent::Caption { id: 2, .. })
    ));
    assert!(e.queue.is_empty());
}

#[test]
fn bounded_events() {
    let mut e = Events {
        history: history::History::default(),
        context: GateContext::default(),
        queue: VecDeque::new(),
        quiet: QuietWindow::default(),
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    for _ in 0..1000 {
        e.push(ContinuousEvent::Rejected("no".into()));
        e.push(ContinuousEvent::Level(0.1));
        e.push(ContinuousEvent::Caption {
            id: 1,
            text: "ambient".into(),
        });
    }
    assert!(e.queue.len() <= 32);
    assert_eq!(
        e.queue
            .iter()
            .filter(|e| matches!(e, ContinuousEvent::Caption { .. }))
            .count(),
        1
    );
    assert_eq!(
        e.queue
            .iter()
            .filter(|e| matches!(e, ContinuousEvent::Level(_)))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_wake_respects_deadline_and_cancellation() {
    let stopped = AtomicBool::new(false);
    let delay = Duration::from_secs(60);
    for (watch, expected) in [(true, 3), (false, 60)] {
        let start = tokio::time::Instant::now();
        wait_for_recovery(
            &stopped,
            delay,
            watch,
            tokio::time::sleep(Duration::from_secs(3)),
        )
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(expected));
    }
    let start = tokio::time::Instant::now();
    wait_for_recovery(&stopped, delay, true, std::future::pending())
        .await
        .unwrap();
    assert_eq!(start.elapsed(), delay);
    let (result, ()) = tokio::join!(
        wait_for_recovery(&stopped, delay, true, std::future::pending()),
        async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            stopped.store(true, Ordering::SeqCst);
        }
    );
    assert_eq!(result, Err(VoiceError::Cancelled));
    assert!(start.elapsed() < delay + Duration::from_secs(2));
}
