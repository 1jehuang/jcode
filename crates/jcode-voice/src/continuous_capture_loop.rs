use super::*;

pub(super) async fn run(
    cancel: Arc<AtomicBool>,
    suppressed: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    level: Arc<AtomicU32>,
    events: Arc<Mutex<Events>>,
    key: String,
    recorder: String,
    config: ContinuousConfig,
    budget: ContinuousBudget,
    paused: Arc<AtomicBool>,
    capturing: Arc<AtomicBool>,
    gate: Option<jcode_base::jev::JevClient>,
) -> Result<(), VoiceError> {
    emit(&events, ContinuousEvent::State(VoiceState::Starting));
    let mut segmenter = Segmenter::default();
    let mut generation = epoch.load(Ordering::SeqCst);
    let mut resume_at = Instant::now();
    // Randomize the process origin so IDs in persisted logs do not restart at 1.
    static NEXT_ID: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    let mut jobs: VecDeque<(
        tokio::task::JoinHandle<Result<Option<ClassifiedCaption>, VoiceError>>,
        Arc<AtomicBool>,
        u64,
    )> = VecDeque::new();
    // Audio sink of the utterance currently streaming to Nari.
    let mut live: Option<(u64, tokio::sync::mpsc::UnboundedSender<Vec<i16>>)> = None;
    let mut warm: Option<Warm> = None;
    let mut warm_retry_at = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    // Device rotation and manual retry share the listener's original hourly ledger.
    let mut consecutive_failures = 0;
    let mut budget_limited = false;
    let mut permission_at = Instant::now();
    let mut permitted = false;
    loop {
        while paused.load(Ordering::SeqCst) || !permitted {
            if cancel.load(Ordering::SeqCst) {
                return Err(VoiceError::Cancelled);
            }
            permitted = session_permitted(&config).await;
            permission_at = Instant::now();
            if !paused.load(Ordering::SeqCst) && permitted {
                break;
            }
            emit(&events, ContinuousEvent::State(VoiceState::Suppressed));
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let setup_cancel = Arc::new(AtomicBool::new(false));
        let setup = open_capture(setup_cancel.clone(), recorder.clone());
        let (mic, mut pcm) = tokio::select! {
            biased;
            _ = crate::nari::cancelled(&cancel) => { setup_cancel.store(true,Ordering::SeqCst); return Err(VoiceError::Cancelled); },
            result = setup => result?,
        };
        capturing.store(true, Ordering::SeqCst);
        let rotate = tokio::time::sleep(Duration::from_secs(240));
        tokio::pin!(rotate);
        let mut last_audio = Instant::now();
        emit(&events, ContinuousEvent::State(VoiceState::Listening));
        let result = loop {
            tokio::select! {
                biased;
                _ = crate::nari::cancelled(&cancel) => break Err(VoiceError::Cancelled),
                _ = &mut rotate => break Ok(()),
                chunk = pcm.recv() => {
                    let Some(chunk) = chunk else { break Err(VoiceError::CaptureFailed); };
                    last_audio = Instant::now();
                    let next = epoch.load(Ordering::SeqCst);
                    if next != generation {
                        generation = next; segmenter = Segmenter::default(); live = None;
                        for (handle,token,_) in jobs.drain(..) { token.store(true,Ordering::SeqCst); handle.abort(); }
                        // Drain stale capture buffers and allow loudspeaker decay after TTS.
                        resume_at = Instant::now() + Duration::from_millis(500);
                    }
                    if paused.load(Ordering::SeqCst) || suppressed.load(Ordering::SeqCst) || Instant::now() < resume_at {
                        level.store(0,Ordering::Relaxed); continue;
                    }
                    let amplitude = rms(&chunk);
                    level.store(amplitude.to_bits(),Ordering::Relaxed);
                    {
                        let mut output = events.lock().unwrap_or_else(|e| e.into_inner());
                        output.quiet.observe(Instant::now(), amplitude >= 0.018 || segmenter.active);
                    }
                    for seg in segmenter.push_stream(&chunk) {
                        match seg {
                            Seg::Start(audio) => {
                                live = None;
                                // Bounded: drop speech rather than build an upload backlog.
                                if jobs.len() >= 2 { continue; }
                                if !budget.0.lock().unwrap_or_else(|e| e.into_inner()).reserve(Instant::now(), audio.len(), &config) {
                                    budget_limited = true;
                                    emit(&events, ContinuousEvent::State(VoiceState::BudgetLimited));
                                    continue;
                                }
                                budget_limited = false;
                                let id = NEXT_ID.get_or_init(|| AtomicU64::new(rand::random())).fetch_add(1, Ordering::Relaxed);
                                trace_turn(id, "speech_onset");
                                let token = Arc::new(AtomicBool::new(false));
                                let gate = gate.clone();
                                let threshold = config.action_probability;
                                let require_jev = config.require_jev;
                                let key = key.clone();
                                let child = token.clone();
                                let warm_session = warm.take().and_then(Warm::take);
                                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                                tx.send(audio).map_err(|_| VoiceError::CaptureFailed)?;
                                live = Some((id, tx));
                                let (events, epoch, paused, suppressed, stopped) = (events.clone(), epoch.clone(), paused.clone(), suppressed.clone(), cancel.clone());
                                jobs.push_back((tokio::spawn(async move {
                                    let (partial_tx, partial_rx) = tokio::sync::watch::channel(String::new());
                                    // Show Nari's revisions as they stream, word by word.
                                    let mut live_rx = partial_tx.subscribe();
                                    let live_events = events.clone();
                                    let caption_feed = tokio::spawn(async move {
                                        while live_rx.changed().await.is_ok() {
                                            let text: String = live_rx.borrow_and_update().trim().chars().take(256).collect();
                                            if text.is_empty() { continue; }
                                            emit(&live_events, ContinuousEvent::Caption { id, text });
                                        }
                                    });
                                    let early = early_trigger(partial_rx, gate.clone(), threshold, require_jev, id, events.clone(), {
                                        let (epoch, paused, suppressed, stopped) = (epoch.clone(), paused.clone(), suppressed.clone(), stopped.clone());
                                        move || !(stopped.load(Ordering::SeqCst) || generation != epoch.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) || suppressed.load(Ordering::SeqCst))
                                    });
                                    tokio::pin!(early);
                                    let stream = stream_job(warm_session, key, rx, child, partial_tx);
                                    tokio::pin!(stream);
                                    // Gate partials while audio streams. Once the final
                                    // transcript lands, any in-flight partial gate is stale.
                                    let mut fired: Option<String> = None;
                                    let mut pending_final = PendingFinal(None);
                                    let text = loop {
                                        tokio::select! {
                                            biased;
                                            // A provisional request is not a successful transcription.
                                            // On failure the recovery event clears deferred replies.
                                            text = &mut stream => break text?,
                                            trigger = &mut early, if fired.is_none() => {
                                                match trigger { Some(t) => { pending_final.0 = Some(id); fired = Some(t); }, None => break (&mut stream).await? }
                                                // Loop again to await the final transcript.
                                            }
                                        }
                                    };
                                    caption_feed.abort();
                                    trace_turn(id, "transcription_finished");
                                    if let Some(trigger) = fired {
                                        pending_final.0 = None;
                                        // Always close a provisional request, even when
                                        // nothing was added, so Jarvis knows to answer.
                                        let full = if text.trim().is_empty() { trigger } else { text };
                                        let mut output = events.lock().unwrap_or_else(|e|e.into_inner());
                                        if stopped.load(Ordering::SeqCst) || generation != epoch.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) || suppressed.load(Ordering::SeqCst) {
                                            trace_discard(id, "continuation_discarded", "epoch_changed");
                                            return Ok(None);
                                        }
                                        trace_turn(id, "continuation_ready");
                                        output.push(ContinuousEvent::Continuation { id, text: full.trim().chars().take(4096).collect() });
                                        return Ok(None);
                                    }
                                    if text.trim().is_empty() { return Ok(None); }
                                    let state = {
                                        let mut output = events.lock().unwrap_or_else(|e|e.into_inner());
                                        if stopped.load(Ordering::SeqCst) || generation != epoch.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) || suppressed.load(Ordering::SeqCst) { return Ok(None); }
                                        output.push(ContinuousEvent::Caption { id, text: text.trim().chars().take(256).collect() });
                                        output.context.inflight = Some((id, text.chars().take(1024).collect()));
                                        output.context.state(&text,require_jev)
                                    };
                                    let gate = gate.as_ref().ok_or(VoiceError::NotConfigured)?;
                                    let score = semantic_gate(gate,&text,threshold,state).await?;
                                    trace_turn(id, "final_gate_decided");
                                    Ok(Some(ClassifiedCaption { id, text, score }))
                                }),token,generation));
                            }
                            Seg::Audio(audio) => {
                                if let Some((_, tx)) = &live {
                                    budget.0.lock().unwrap_or_else(|e| e.into_inner()).extend(audio.len());
                                    if tx.send(audio).is_err() { live = None; }
                                }
                            }
                            Seg::End => {
                                if let Some((id, tx)) = live.take() {
                                    trace_turn(id, "speech_endpoint");
                                    drop(tx);
                                }
                            }
                        }
                    }
                },
                _ = tick.tick() => {
                    if permission_at.elapsed() >= Duration::from_secs(1) {
                        permitted = session_permitted(&config).await;
                        permission_at = Instant::now();
                    }
                    if paused.load(Ordering::SeqCst) || !permitted {
                        epoch.fetch_add(1, Ordering::SeqCst);
                        let mut out = events.lock().unwrap_or_else(|e| e.into_inner());
                        out.context = GateContext::default();
                        out.clear_pending_speech(if permitted { "paused" } else { "permission_lost" });
                        drop(out);
                        live = None; warm = None;
                        for (handle,token,_) in jobs.drain(..) { token.store(true,Ordering::SeqCst); handle.abort(); }
                        break Ok(());
                    }
                    if Instant::now().duration_since(last_audio) > Duration::from_secs(5) { break Err(VoiceError::CaptureFailed); }
                    // Warm spare: connected but idle, it carries no audio until speech onset.
                    if config.warm_nari && live.is_none() && jobs.is_empty() {
                        // A spare that failed authentication must not be replaced forever
                        // while the microphone is idle. Observe its typed failure on expiry.
                        if warm.as_ref().is_some_and(|w| w.since.elapsed() >= WARM_MAX_IDLE) {
                            if let Some(handle) = warm.take().and_then(Warm::take) {
                                if !handle.is_finished() { handle.abort(); break Err(VoiceError::Timeout); }
                                match handle.await {
                                    Ok(Ok(_)) => {},
                                    Ok(Err(error)) => break Err(error),
                                    Err(_) => break Err(VoiceError::Network),
                                }
                            }
                        }
                        if warm.is_none() && Instant::now() >= warm_retry_at {
                            warm = Some(Warm::spawn(&key));
                            warm_retry_at = Instant::now() + Duration::from_secs(5);
                        }
                    }
                    let mut failed = None;
                    while failed.is_none() && let Some((handle,_,job_generation)) = jobs.pop_front_if(|(h,_,_)| h.is_finished()) {
                        let result = handle.await.map_err(|_|VoiceError::CaptureFailed)?;
                        if !cancel.load(Ordering::SeqCst) && job_generation == epoch.load(Ordering::SeqCst) && !suppressed.load(Ordering::SeqCst) && !paused.load(Ordering::SeqCst) {
                            if result.is_ok() { consecutive_failures = 0; }
                            match result {
                                Ok(Some(caption)) => {
                                    let mut output = events.lock().unwrap_or_else(|e|e.into_inner());
                                    if !cancel.load(Ordering::SeqCst) && job_generation == epoch.load(Ordering::SeqCst) && !suppressed.load(Ordering::SeqCst) && !paused.load(Ordering::SeqCst) {
                                        output.classified(caption);
                                    }
                                },
                                Ok(None) => {},
                                Err(error) => failed = Some(error),
                            }
                        }
                    }
                    if let Some(error) = failed { break Err(error); }
                    let state = if suppressed.load(Ordering::SeqCst) { VoiceState::Suppressed }
                        else if budget_limited { VoiceState::BudgetLimited }
                        else if segmenter.active { VoiceState::Speaking }
                        else if !jobs.is_empty() { VoiceState::Transcribing } else { VoiceState::Listening };
                    emit(&events,ContinuousEvent::State(state));
                    emit(&events,ContinuousEvent::Level(if state == VoiceState::Suppressed { 0.0 } else { f32::from_bits(level.load(Ordering::Relaxed)) }));
                },
            }
        };
        if result.is_err() {
            // Invalidate queued speech before waiting for the capture backend to stop.
            // Keep only non-speech telemetry and visible chat, never an unfinished trigger.
            epoch.fetch_add(1, Ordering::SeqCst);
            drop(warm.take());
            drop(live.take());
            for (handle, token, _) in jobs.drain(..) {
                token.store(true, Ordering::SeqCst);
                handle.abort();
            }
            let mut out = events.lock().unwrap_or_else(|e| e.into_inner());
            let chat = std::mem::take(&mut out.context.chat);
            out.context = GateContext {
                chat,
                ..Default::default()
            };
            out.retain_events(
                if cancel.load(Ordering::SeqCst) {
                    "stopped"
                } else {
                    "capture_failed"
                },
                |event| matches!(event, ContinuousEvent::State(_) | ContinuousEvent::Level(_)),
            );
        }
        mic.stop();
        let capture = tokio::task::spawn_blocking(move || mic.finish())
            .await
            .map_err(|_| VoiceError::CaptureFailed)?;
        capturing.store(false, Ordering::SeqCst);
        // Closing the sink commits whatever was said before the device restart.
        live = None;
        if result.is_err() || capture.is_err() {
            warm = None;
            for (handle, token, _) in jobs.drain(..) {
                token.store(true, Ordering::SeqCst);
                handle.abort();
            }
            if let Err(error) = result {
                let Some(delay) = recovery_delay(&error, consecutive_failures) else {
                    return Err(error);
                };
                capture?;
                consecutive_failures += 1;
                emit(&events, ContinuousEvent::Recovering { error, delay });
                emit(&events, ContinuousEvent::State(VoiceState::Suppressed));
                level.store(0, Ordering::Relaxed);
                segmenter = Segmenter::default();
                permitted = false;
                tokio::select! {
                    _ = crate::nari::cancelled(&cancel) => return Err(VoiceError::Cancelled),
                    _ = tokio::time::sleep(delay) => continue,
                }
            }
            capture?;
        }
        // Do not join partial speech across a device restart.
        segmenter = Segmenter::default();
    }
}
