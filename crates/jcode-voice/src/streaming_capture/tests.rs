use super::*;

#[tokio::test]
async fn poisoned_event_queue_preserves_original_wakeup_and_finished_event() {
    let events = Arc::new(Mutex::new(Events::default()));
    let ready = events.lock().unwrap().ready.clone();
    let poisoned = events.clone();
    assert!(
        thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("simulate an interrupted event producer");
        })
        .join()
        .is_err()
    );
    Events::lock(&events).push(NariEvent::Finished(Err(VoiceError::CaptureFailed)));
    tokio::time::timeout(Duration::from_secs(1), ready.notified())
        .await
        .unwrap();
    assert!(!events.is_poisoned());
    assert!(matches!(
        Events::lock(&events).queue.pop_front(),
        Some(NariEvent::Finished(Err(VoiceError::CaptureFailed)))
    ));
}
#[cfg(feature = "voice-capture")]
fn level_chunker() -> Chunker {
    let (tx, _rx) = nari_pcm_channel();
    Chunker {
        level: Arc::new(AtomicU32::new(0)),
        resampler: Resampler::new(48000).unwrap(),
        chunk: Vec::new(),
        tx,
        failed: false,
        samples: 0,
        first_callback: false,
        first_voice: false,
    }
}
#[cfg(feature = "voice-capture")]
fn level(c: &Chunker) -> f32 {
    f32::from_bits(c.level.load(Ordering::Relaxed))
}
#[cfg(feature = "voice-capture")]
#[test]
fn callback_level_is_latest_rms_not_peak_or_signed_average() {
    let mut c = level_chunker();
    assert_eq!(level(&c), 0.0);
    c.push(&[0.5f32, -0.5, 0.5, -0.5], 1);
    assert_eq!(level(&c), 0.5);
    c.push(&[1.0f32, 0.0, -1.0, 0.0], 1);
    assert!((level(&c) - 0.5f32.sqrt()).abs() < 1e-6);
    c.push(&[0.0f32; 4], 1);
    assert_eq!(level(&c), 0.0);
    c.push::<f32>(&[], 1);
    assert_eq!(level(&c), 0.0);
}
#[cfg(feature = "voice-capture")]
#[test]
fn callback_level_downmixes_and_normalizes_native_formats() {
    let mut c = level_chunker();
    c.push(&[0.75f32, 0.25, -0.75, -0.25], 2);
    assert_eq!(level(&c), 0.5);
    c.push(&[1.0f32, -1.0], 2);
    assert_eq!(level(&c), 0.0);
    c.push(&[16384i16, -16384], 1);
    assert_eq!(level(&c), 0.5);
    c.push(&[32768u16; 2], 1);
    assert_eq!(level(&c), 0.0);
    c.push(&[49152u16, 16384], 1);
    assert_eq!(level(&c), 0.5);
    c.push(&[0.25f64, -0.25], 1);
    assert_eq!(level(&c), 0.25);
}
#[cfg(feature = "voice-capture")]
#[test]
fn callback_level_is_finite_and_bounded_for_invalid_float_samples() {
    let mut c = level_chunker();
    c.push(&[2.0f32, -2.0], 1);
    assert_eq!(level(&c), 1.0);
    c.push(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY], 1);
    assert_eq!(level(&c), 0.0);
}
#[cfg(feature = "voice-capture")]
#[test]
fn recording_level_reads_callback_atomic_without_consuming_events() {
    let mut c = level_chunker();
    let (release, wait) = mpsc::channel();
    let recording = NariRecording {
        cancel: Arc::new(AtomicBool::new(false)),
        stop: Arc::new(AtomicBool::new(false)),
        level: Arc::new(std::sync::OnceLock::from(c.level.clone())),
        events: Arc::new(Mutex::new(Events::default())),
        worker: thread::spawn(move || {
            wait.recv().unwrap();
        }),
        mic_stop: Arc::new(Mutex::new(None)),
    };
    recording.events.lock().unwrap().push(NariEvent::Started);
    thread::spawn(move || c.push(&[0.5f32; 100], 1))
        .join()
        .unwrap();
    assert_eq!(recording.audio_level(), 0.5);
    assert_eq!(recording.audio_level(), 0.5);
    assert!(matches!(recording.try_event(), Some(NariEvent::Started)));
    recording.cancel.store(true, Ordering::SeqCst);
    assert_eq!(recording.audio_level(), 0.0);
    recording.cancel.store(false, Ordering::SeqCst);
    recording.stop();
    assert_eq!(recording.audio_level(), 0.0);
    recording.stop.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    while !recording.is_finished() {
        thread::yield_now();
    }
    assert_eq!(recording.audio_level(), 0.0);
}
#[cfg(feature = "voice-capture")]
#[test]
fn cancelled_constructors_never_open_microphone() {
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        MicrophoneRecording::start_pcm_cancellable(cancel.clone()),
        Err(VoiceError::Cancelled)
    ));
    assert!(matches!(
        NariRecording::start_cancellable(cancel, "test"),
        Err(VoiceError::Cancelled)
    ));
}
#[cfg(feature = "voice-capture")]
#[test]
fn bounded_chunks_downmix_and_fail_on_backpressure() {
    let (tx, mut rx) = nari_pcm_channel();
    let mut c = Chunker {
        level: Arc::new(AtomicU32::new(0)),
        resampler: Resampler::new(48000).unwrap(),
        chunk: Vec::new(),
        tx,
        failed: false,
        samples: 0,
        first_callback: false,
        first_voice: false,
    };
    c.push(&vec![0.5f32; 48000 * 2], 2);
    let mut count = 0;
    while let Ok(chunk) = rx.try_recv() {
        assert!(chunk.len() <= 1601);
        assert!(chunk.iter().skip(50).all(|s| (*s - 16384).abs() < 2));
        count += chunk.len();
    }
    assert!(count > 14000);
    assert!(!c.failed);
    c.push(&vec![0.0f32; 48000 * 6], 2);
    assert!(c.failed);
}
#[test]
fn stop_unparks_capture_thread_immediately() {
    // A long park stands in for the capture loop. stop() must wake it
    // rather than waiting out the timeout.
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let worker = thread::spawn(move || {
        while !s.load(Ordering::SeqCst) {
            thread::park_timeout(Duration::from_secs(5));
        }
        Ok(())
    });
    let recording = PcmRecording {
        level: Arc::new(AtomicU32::new(0)),
        stop,
        cancel: Arc::new(AtomicBool::new(false)),
        worker: Some(worker),
    };
    let started = std::time::Instant::now();
    recording.finish().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
}
#[tokio::test]
async fn event_ready_wakes_on_push_and_keeps_early_permit() {
    let events = Arc::new(Mutex::new(Events::default()));
    let recording = NariRecording {
        cancel: Arc::new(AtomicBool::new(false)),
        stop: Arc::new(AtomicBool::new(false)),
        level: Arc::new(std::sync::OnceLock::new()),
        events: events.clone(),
        worker: thread::spawn(|| {}),
        mic_stop: Arc::new(Mutex::new(None)),
    };
    // Pushed before anyone waits: the permit must not be lost.
    events.lock().unwrap().push(NariEvent::Started);
    tokio::time::timeout(Duration::from_millis(200), recording.event_ready())
        .await
        .expect("early event wakes the next wait");
    let wait = recording.event_ready();
    let e = events.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        e.lock()
            .unwrap()
            .push(NariEvent::Finished(Ok("done".into())));
    });
    tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .expect("finish wakes a pending wait");
}
#[test]
fn events_coalesce_but_preserve_lifecycle() {
    let mut e = Events::default();
    e.push(NariEvent::Started);
    for _ in 0..1000 {
        e.push(NariEvent::Transcript("latest".into()));
    }
    e.push(NariEvent::Finished(Ok("latest".into())));
    assert_eq!(e.queue.len(), 3);
    assert!(matches!(e.queue.front(), Some(NariEvent::Started)));
}
#[test]
fn stop_preserves_token_drop_cancels_without_joining() {
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let s = stop.clone();
    let worker = thread::spawn(move || {
        while !c.load(Ordering::SeqCst) && !s.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    });
    let recording = PcmRecording {
        level: Arc::new(AtomicU32::new(0)),
        cancel: cancel.clone(),
        stop,
        worker: Some(worker),
    };
    recording.stop();
    recording.finish().unwrap();
    assert!(!cancel.load(Ordering::SeqCst));
    let c = cancel.clone();
    let worker = thread::spawn(move || {
        while !c.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    });
    drop(PcmRecording {
        level: Arc::new(AtomicU32::new(0)),
        cancel: cancel.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        worker: Some(worker),
    });
    assert!(cancel.load(Ordering::SeqCst));
}
fn fake_capture(
    cancel: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
) -> (PcmRecording, tokio::sync::mpsc::Receiver<Vec<i16>>) {
    let (tx, rx) = nari_pcm_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let c = cancel.clone();
    let worker = thread::spawn(move || {
        while !s.load(Ordering::SeqCst) && !c.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        drop(tx);
        released.store(true, Ordering::SeqCst);
        Ok(())
    });
    (
        PcmRecording {
            level: Arc::new(AtomicU32::new(0)),
            stop,
            cancel,
            worker: Some(worker),
        },
        rx,
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestration_opens_capture_during_handshake_then_releases_on_network_error() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let opened = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    let o = opened.clone();
    let r = released.clone();
    let starting = tokio::task::spawn_blocking(move || {
        NariRecording::start_with(
            Arc::new(AtomicBool::new(false)),
            "test",
            &url,
            move |cancel| {
                o.store(true, Ordering::SeqCst);
                let capture = fake_capture(cancel, r);
                capture.0.level.store(0.375f32.to_bits(), Ordering::Relaxed);
                Ok(capture)
            },
        )
    });
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.next().await.unwrap().unwrap();
    // Speech during the handshake must be captured, so the microphone and
    // the recording handle are both live before session.configured.
    let recording = starting.await.unwrap().unwrap();
    assert!(opened.load(Ordering::SeqCst));
    assert!(!recording.is_finished());
    ws.send(Message::Text(
        serde_json::json!({"type":"session.configured"}).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(recording.audio_level(), 0.375);
    ws.send(Message::Text(
        serde_json::json!({"type":"error","error":{"message":"never expose this"}}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !recording.is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(released.load(Ordering::SeqCst));
    assert_eq!(recording.audio_level(), 0.0);
    let mut finished = 0;
    while let Some(event) = recording.try_event() {
        if let NariEvent::Finished(result) = event {
            assert_eq!(result, Err(VoiceError::NariRejected));
            finished += 1;
        }
    }
    assert_eq!(finished, 1);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestration_setup_error_releases_capture_and_reports_provider_error() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let released = Arc::new(AtomicBool::new(false));
    let r = released.clone();
    let starting = tokio::task::spawn_blocking(move || {
        NariRecording::start_with(Arc::new(AtomicBool::new(false)), "test", &url, move |c| {
            Ok(fake_capture(c, r))
        })
    });
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.next().await.unwrap().unwrap();
    ws.send(Message::Text(
        serde_json::json!({"type":"error","error":{"code":"INSUFFICIENT_CREDITS"}}).to_string(),
    ))
    .await
    .unwrap();
    let recording = starting.await.unwrap().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(NariEvent::Finished(result)) = recording.try_event() {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result, Err(VoiceError::NariCreditsExhausted));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !released.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[cfg(feature = "voice-capture")]
#[test]
fn filter_tail_never_exceeds_duration_cap() {
    for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
        let (tx, mut rx) = nari_pcm_channel();
        let mut c = Chunker {
            level: Arc::new(AtomicU32::new(0)),
            resampler: Resampler::new(rate).unwrap(),
            chunk: Vec::new(),
            tx,
            failed: false,
            samples: 0,
            first_callback: false,
            first_voice: false,
        };
        c.push(&vec![0.2f32; rate as usize / 5], 1);
        let cap = c.samples;
        c.finish(cap);
        let mut total = 0;
        while let Ok(chunk) = rx.try_recv() {
            total += chunk.len();
        }
        assert_eq!(total, cap, "rate {rate}");
        assert!(!c.failed);
    }
}

#[cfg(feature = "voice-capture")]
#[test]
fn upsampling_final_frame_cannot_exceed_cap() {
    let (tx, _rx) = nari_pcm_channel();
    let cap = 16000 * MAX_RECORDING_DURATION.as_secs() as usize;
    let mut c = Chunker {
        level: Arc::new(AtomicU32::new(0)),
        resampler: Resampler::new(11025).unwrap(),
        chunk: Vec::new(),
        tx,
        failed: false,
        samples: cap - 1,
        first_callback: false,
        first_voice: false,
    };
    c.push(&[0.5f32; 100], 1);
    assert_eq!(c.samples, cap);
    assert_eq!(c.chunk.len(), 1);
}
#[test]
fn construction_returns_before_microphone_or_network_are_ready() {
    // Unroutable address: the handshake cannot complete during this test.
    let (release, blocked) = mpsc::channel::<()>();
    let started = std::time::Instant::now();
    let recording = NariRecording::start_with(
        Arc::new(AtomicBool::new(false)),
        "test",
        "ws://10.255.255.1:9",
        move |c| {
            let _ = blocked.recv_timeout(Duration::from_secs(2));
            Ok(fake_capture(c, Arc::new(AtomicBool::new(false))))
        },
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(!recording.is_finished());
    assert_eq!(recording.audio_level(), 0.0);
    drop(recording);
    let _ = release.send(());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_slow_factory_finishes_cancelled_without_starting_stale_capture() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let entered = Arc::new(AtomicBool::new(false));
    let e = entered.clone();
    let stale_prevented = Arc::new(AtomicBool::new(false));
    let prevented = stale_prevented.clone();
    let (release, blocked) = mpsc::channel();
    let starting = tokio::task::spawn_blocking(move || {
        NariRecording::start_with(c, "test", &url, move |setup_cancel| {
            e.store(true, Ordering::SeqCst);
            blocked.recv_timeout(Duration::from_secs(2)).unwrap();
            prevented.store(setup_cancel.load(Ordering::SeqCst), Ordering::SeqCst);
            Err(VoiceError::Cancelled)
        })
    });
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.next().await.unwrap().unwrap();
    ws.send(Message::Text(
        serde_json::json!({"type":"session.configured"}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    cancel.store(true, Ordering::SeqCst);
    // Construction never waits for the microphone or network.
    let recording = tokio::time::timeout(Duration::from_millis(200), starting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            if let Some(NariEvent::Finished(result)) = recording.try_event() {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result, Err(VoiceError::Cancelled));
    tokio::time::sleep(Duration::from_millis(40)).await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !stale_prevented.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unified_stop_finalizes_and_drop_after_finished_preserves_token() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let released = Arc::new(AtomicBool::new(false));
    let r = released.clone();
    let starting = tokio::task::spawn_blocking(move || {
        NariRecording::start_with(c, "test", &url, move |c| Ok(fake_capture(c, r)))
    });
    let (tcp, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    ws.next().await.unwrap().unwrap();
    ws.send(Message::Text(
        serde_json::json!({"type":"session.configured"}).to_string(),
    ))
    .await
    .unwrap();
    let recording = starting.await.unwrap().unwrap();
    recording.stop();
    let message = tokio::time::timeout(Duration::from_secs(1), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let commit: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(commit["type"], "input_audio_buffer.commit");
    ws.send(Message::Text(serde_json::json!({"type":"input_audio_buffer.commit_empty","client_event_id":commit["event_id"]}).to_string())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(NariEvent::Finished(result)) = recording.try_event() {
                assert_eq!(result, Ok(String::new()));
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    drop(recording);
    assert!(!cancel.load(Ordering::SeqCst));
    assert!(released.load(Ordering::SeqCst));
}
