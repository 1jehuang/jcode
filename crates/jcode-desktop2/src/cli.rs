//! Entry points that run instead of the window.
//!
//! Scripted input, the keymap table, the end-to-end harness, the donut
//! benchmark, offscreen captures, and the state-space profiler. These share
//! nothing with the event loop but the model, and keeping them in `main.rs`
//! left the entry point mostly not-the-entry-point.

use crate::{
    App, DONUT_GRID, Model, ModelId, build_scene, capture, donut, harness, keymap, layout, paint,
    profile, scroll_bench, scroll_profile, states, transcript,
};
use anyhow::Result;
use vello::Scene;

/// Run whatever command-line entry point `args` selects, or `None` when the
/// app should open its window.
///
/// `--version` is answered before anything that can open a window: build
/// tooling validates a fresh binary by running it, and a GUI process that
/// ignores an unknown flag and puts a window up instead of answering hangs
/// that check forever.
pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("jcode-desktop2 {}", jcode_build_meta::version());
        return Some(Ok(()));
    }
    match args.first().map(String::as_str) {
        Some("--script") => Some(run_script(&args[1..])),
        Some("--keys") => {
            print_keys();
            Some(Ok(()))
        }
        Some("--profile-states") => Some(run_profile_states(&args[1..])),
        Some("--profile-transitions") => Some(profile_transitions()),
        Some("--bench-stream") => Some(bench_stream(&args[1..])),
        Some("--bench-scroll") => Some(bench_scroll()),
        Some("--profile-scroll") => Some(profile_scroll()),
        Some("--bench-donut") => Some(bench_donut()),
        Some("--capture") => Some(run_capture(&args[1..])),
        Some("--check-clipboard-image") => Some(check_clipboard_image()),
        Some("--check-primary-selection") => Some(check_primary_selection()),
        Some("--check-connect") => Some(check_connect()),
        Some("--check-new-session") => Some(check_new_session()),
        Some("--check-reconnect") => Some(check_reconnect()),
        Some("--check-resume-scan") => Some(check_resume_scan()),
        Some("--e2e") => Some(run_e2e(
            args.get(1)
                .map(String::as_str)
                .unwrap_or("Reply with exactly the word: pong"),
        )),
        _ => None,
    }
}

/// Exercise the exact worker and command queue used by Ctrl+Shift+N.
fn check_new_session() -> Result<()> {
    use std::time::{Duration, Instant};

    let (updates, outgoing) = harness::spawn(|| {});
    let wait_for_attached = |after: Option<&str>| -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match updates.recv_timeout(Duration::from_millis(100)) {
                Ok(harness::HarnessUpdate::Attached { session_id, .. })
                    if after != Some(session_id.as_str()) =>
                {
                    return Ok(session_id);
                }
                Ok(harness::HarnessUpdate::Failed(message)) => anyhow::bail!(message),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("desktop2 harness disconnected")
                }
            }
        }
        anyhow::bail!("desktop2 did not attach a session within 10 seconds")
    };

    let first = wait_for_attached(None)?;
    println!("desktop2 initial session attached: {first}");
    let started = Instant::now();
    outgoing.send(harness::Command::New)?;
    let second = wait_for_attached(Some(&first))?;
    println!(
        "desktop2 new session ok: {first} -> {second} in {:.2?}",
        started.elapsed()
    );
    Ok(())
}

/// `--profile-transitions`: time the real daemon boundaries behind navigation.
///
/// Frame profiling cannot explain a slow new-session gesture: that path is an
/// SDK round trip and agent construction, not scene building. Keep this as a
/// small live probe so the two costs remain separately measurable. This creates
/// one real session, which is retained like a session created by the window.
fn profile_transitions() -> Result<()> {
    use std::time::Instant;

    let total = Instant::now();
    let start = Instant::now();
    jcode_sdk::ensure_runtime(&jcode_sdk::LaunchOptions::default(), &|_| {})?;
    let runtime = start.elapsed();

    let start = Instant::now();
    let client = jcode_sdk::JcodeClient::connect(jcode_sdk::ConnectOptions {
        client_name: concat!("jcode-desktop2-profile/", env!("CARGO_PKG_VERSION")).to_string(),
        ensure_runtime: false,
        ..Default::default()
    })?;
    let connect = start.elapsed();

    // Match the window exactly: attachment emits model and status events in
    // addition to its reply, so the event stream must be subscribed first.
    let events = client.events(None);

    let start = Instant::now();
    let session = client.create_session(harness::default_working_dir())?;
    let create = start.elapsed();

    let start = Instant::now();
    client.attach_session(&session.session_id)?;
    let attach = start.elapsed();

    drop(events);
    println!("transition                  elapsed");
    println!("-----------------------------------");
    println!("runtime ready              {:>8.2?}", runtime);
    println!("connect                    {:>8.2?}", connect);
    println!("spawn new session          {:>8.2?}", create);
    println!("attach session             {:>8.2?}", attach);
    println!("-----------------------------------");
    println!("total                      {:>8.2?}", total.elapsed());
    println!("created session: {}", session.session_id);
    Ok(())
}

/// `--check-connect`: exercise the exact startup boundary used by the window.
///
/// `--version` only proves that the GUI executable can start. Desktop2 also
/// requires the harness API bridge beside that executable (or on PATH), and a
/// build missing that companion used to pass activation before every new window
/// sat on "Connecting". Keep this check small enough to run after every self-dev
/// build: start/locate the runtime, complete the protocol handshake, then exit.
fn check_connect() -> Result<()> {
    jcode_sdk::ensure_runtime(&jcode_sdk::LaunchOptions::default(), &|status| {
        println!("[connect] {status}")
    })?;
    let client = jcode_sdk::JcodeClient::connect(jcode_sdk::ConnectOptions {
        client_name: concat!("jcode-desktop2-check/", env!("CARGO_PKG_VERSION")).to_string(),
        ensure_runtime: false,
        ..Default::default()
    })?;
    println!(
        "desktop2 connection ok: {} at {}",
        client.server,
        harness::api_socket_path().display()
    );
    Ok(())
}

/// `--check-clipboard-image`: prove Ctrl+V's image path against the *real*
/// compositor.
///
/// The unit tests keep the system clipboard sandboxed so they cannot read or
/// clobber a developer's clipboard, which means nothing in the suite exercises
/// Wayland image negotiation at all. That is exactly where pasting a screenshot
/// breaks without a single test failing, so this reads whatever image is on the
/// clipboard right now and reports its type, size, and payload cost.
fn check_clipboard_image() -> Result<()> {
    let mut clipboard = crate::clipboard::Clipboard::system();
    let image = clipboard
        .get_image()
        .map_err(|error| anyhow::anyhow!("clipboard image unavailable: {error}"))?
        .ok_or_else(|| anyhow::anyhow!("clipboard does not contain an image"))?;
    println!(
        "clipboard image ok: {} {}, {} bytes, {} base64 chars",
        image.media_type,
        image.label(),
        image.bytes.len(),
        crate::png::base64(&image.bytes).len()
    );
    Ok(())
}

/// `--check-primary-selection`: prove auto-copy against the *real* compositor.
///
/// The unit tests deliberately use the in-process fallback so they cannot
/// clobber a developer's clipboard, which means they never exercise arboard or
/// the Wayland/X11 selection protocol at all. That is precisely where this
/// feature can break without a single test failing, so this entry point does
/// the round trip for real: write a sentinel to the primary selection, read it
/// back, and assert the ordinary clipboard was left alone.
fn check_primary_selection() -> Result<()> {
    use crate::clipboard::{Clipboard, Target};

    if !crate::clipboard::has_primary_selection() {
        println!("no primary selection on this platform; nothing to check");
        return Ok(());
    }
    let mut clipboard = Clipboard::system();
    let before = clipboard.get();
    let sentinel = format!("jcode-primary-check-{}", std::process::id());

    clipboard
        .set_to(Target::Primary, &sentinel)
        .map_err(|error| anyhow::anyhow!("could not write the primary selection: {error}"))?;
    // The selection is served asynchronously by the compositor, so a read
    // immediately after the write can legitimately race it.
    std::thread::sleep(std::time::Duration::from_millis(250));

    let read_back = clipboard.get_from(Target::Primary);
    if read_back.as_deref() != Some(sentinel.as_str()) {
        anyhow::bail!(
            "primary selection round trip failed: wrote {sentinel:?}, read {read_back:?}"
        );
    }
    let after = clipboard.get();
    if after != before {
        anyhow::bail!(
            "writing the primary selection clobbered the clipboard: {before:?} -> {after:?}"
        );
    }
    // Ownership of a selection belongs to the process that set it, so a
    // short-lived checker proves the write happened but not that the value
    // survives. Hold it long enough for another process to read it, which is
    // the situation that actually matters: the window stays open.
    if std::env::var_os("JCODE_DESKTOP2_HOLD_SELECTION").is_some() {
        println!("holding {sentinel} for 10s");
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    println!("primary selection round trip ok (clipboard untouched: {before:?})");
    Ok(())
}

/// `--profile-states [budget_us]`: measure every state in the space, and fail
/// when one is over budget or is redoing layout on an unchanged frame.
///
/// Sweeping the state space beats waiting for someone to feel the app lag:
/// `build_scene` is a pure function of the model, so frame cost is something
/// to evaluate over a known space rather than an impression gathered from
/// whatever a person happened to click on.
fn run_profile_states(args: &[String]) -> Result<()> {
    // A mistyped budget must not silently fall back to the default and report
    // a pass against a threshold the caller did not ask for.
    let budget = match args.first() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("invalid budget '{value}': {error}"))?,
        None => profile::WARM_BUDGET_US,
    };
    let costs = profile::sweep();
    if !profile::report(&costs, budget) {
        anyhow::bail!("one or more states are over budget or redoing layout every frame");
    }
    Ok(())
}

/// `--script <chord|text> ...`: drive the app with a keystroke script and
/// print the resulting composer state. Verifies real chord sequences end to
/// end without a compositor, which synthetic-input tools make unreliable.
///
///   jcode-desktop2 --script 'type:alpha beta' ctrl+a shift+right shift+right
///
/// The gesture verbs drive the same handlers the window does. Super+hjkl is
/// direct niri-style motion by default; the held-Super overview stays behind
/// its bench flag:
///
///   jcode-desktop2 --script 'sessions:a=jcode,b=jcode,c=site' super+l super+j
fn run_script(steps: &[String]) -> Result<()> {
    let mut app = App::default();
    app.model.session_id = Some("session_script".into());
    let mut clock = std::time::Instant::now();
    for step in steps {
        if let Some(text) = step.strip_prefix("type:") {
            app.apply(keymap::Action::Insert, Some(text));
            continue;
        }
        // `sessions:id=project,...` seeds the strip, because the overview has
        // nothing to lay out for a lone session and the interesting failures
        // are all about moving between them.
        if let Some(spec) = step.strip_prefix("sessions:") {
            let entries: Vec<crate::strip::Panel> = spec
                .split(',')
                .filter(|part| !part.is_empty())
                .enumerate()
                .map(|(index, part)| {
                    let (id, project) = part.split_once('=').unwrap_or((part, "project"));
                    crate::strip::Panel {
                        session_id: id.to_string(),
                        title: None,
                        working_dir: Some(format!("/w/{project}")),
                        busy: false,
                        weight: 1_000.0 * (index as f64 + 1.0),
                    }
                })
                .collect();
            let first = entries.first().map(|entry| entry.session_id.clone());
            app.model.session_id = first.clone();
            app.model.strips = crate::strip::Strips::build(entries, first.as_deref());
            continue;
        }
        match step.as_str() {
            "super-down" => {
                app.modifiers |= winit::keyboard::ModifiersState::SUPER;
                app.on_super_changed(true, clock);
                continue;
            }
            "super-up" => {
                app.modifiers -= winit::keyboard::ModifiersState::SUPER;
                app.on_super_changed(false, clock);
                // The debounce window that absorbs a remapper's synthetic
                // Super lift also has to expire before the release resolves,
                // and a script has no frames of its own to expire it on.
                clock += crate::SUPER_BOUNCE;
                app.settle_super_release(clock);
                continue;
            }
            // `super-bounce:<chord>` replays exactly what keyd and friends
            // emit for a rewritten Super chord: Super up, the substituted key,
            // Super back down. Without this verb the flicker it used to cause
            // was only reproducible on a machine with a remapper installed.
            other if other.starts_with("super-bounce:") => {
                let inner = other.trim_start_matches("super-bounce:");
                let (key, _) = keymap::parse_chord(inner)
                    .ok_or_else(|| anyhow::anyhow!("unknown chord '{inner}'"))?;
                app.modifiers -= winit::keyboard::ModifiersState::SUPER;
                app.on_super_changed(false, clock);
                app.key_pressed(&key, None);
                clock += std::time::Duration::from_millis(1);
                app.modifiers |= winit::keyboard::ModifiersState::SUPER;
                app.on_super_changed(true, clock);
                app.settle_super_release(clock);
                continue;
            }
            // Run the zoom to rest and move the clock past the tap window, so
            // a release afterwards is a deliberate gesture rather than a tap.
            "settle" => {
                for tick in 1..=40u32 {
                    app.tick_overview(
                        clock + std::time::Duration::from_millis(u64::from(tick) * 16),
                    );
                }
                clock += std::time::Duration::from_millis(640);
                continue;
            }
            _ => {}
        }
        let (key, mods) =
            keymap::parse_chord(step).ok_or_else(|| anyhow::anyhow!("unknown chord '{step}'"))?;
        // Chords go through the window's own dispatch, so the field's claim on
        // the keyboard is exercised rather than bypassed.
        app.modifiers = mods;
        if !app.key_pressed(&key, None) {
            println!("quit");
            return Ok(());
        }
    }
    let editor = &app.model.editor;
    println!("text: {:?}", editor.text());
    println!("cursor: {}", editor.cursor());
    match editor.selected_text() {
        Some(selected) => println!("selected: {selected:?}"),
        None => println!("selected: none"),
    }
    println!("session: {:?}", app.model.session_id);
    println!("overview_open: {}", app.model.overview.is_open());
    println!("overview_focus: {:?}", app.model.overview.focus());
    if let Some(notice) = &app.model.notice {
        println!("notice: {notice}");
    }
    Ok(())
}

/// `--keys`: print the keybindings ported from the TUI, and the ones that were
/// deliberately skipped. Makes the parity table discoverable to users instead
/// of living only in the source.
fn print_keys() {
    println!("keybindings (ported from the jcode TUI)\n");
    let width = keymap::PORTED
        .iter()
        .map(|row| row.chord.len())
        .max()
        .unwrap_or(0);
    for row in keymap::PORTED {
        println!(
            "  {:<width$}  {:<20}  {}",
            row.chord,
            format!("{:?}", row.action),
            row.tui,
            width = width
        );
    }
    println!("\nnot ported yet:\n");
    for (chord, reason) in keymap::NOT_PORTED {
        println!("  {chord:<width$}  {reason}", width = width);
    }
}

/// `--e2e [message]`: headless validation of the app's own harness wiring.
/// Uses the same worker (`harness::spawn`) and model updates as the windowed
/// app: connect, attach, send one message, stream the reply, exit 0 on
/// `TurnDone`. Also renders the final model offscreen to prove the full
/// model -> scene path.
fn run_e2e(message: &str) -> Result<()> {
    let (updates, outgoing) = harness::spawn(|| {});
    let mut model = Model::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut sent = false;
    while std::time::Instant::now() < deadline {
        let Ok(update) = updates.recv_timeout(std::time::Duration::from_secs(1)) else {
            continue;
        };
        match update {
            harness::HarnessUpdate::Status(status) => {
                println!("[e2e] status: {status}");
                model.status = status;
            }
            // The worker now retries rather than giving up, but a probe must
            // still fail loudly on the first failure instead of watching it
            // reconnect until the deadline.
            harness::HarnessUpdate::Failed(message) => {
                anyhow::bail!("harness failure: {message}");
            }
            harness::HarnessUpdate::ConnectionLost(message) => {
                anyhow::bail!("harness connection lost: {message}");
            }
            harness::HarnessUpdate::Attached { session_id, .. } => {
                println!("[e2e] attached: {session_id}");
                model.status = format!("attached: {session_id}");
                model.session_id = Some(session_id);
                model.transcript.push(transcript::Message::user(message));
                outgoing.send(harness::Command::Send {
                    content: message.to_string(),
                    images: vec![],
                })?;
                sent = true;
            }
            // The e2e probe drives one session, so another session's tail is
            // nothing it needs to assert on.
            harness::HarnessUpdate::Peek { .. } => {}
            harness::HarnessUpdate::Model {
                provider,
                model: id,
            } => {
                println!("[e2e] model: {provider:?} {id:?}");
                model.model = Some(ModelId {
                    provider,
                    model: id,
                });
            }
            harness::HarnessUpdate::Models { models, current } => {
                model.model_picker.set_models(models, current);
            }
            harness::HarnessUpdate::ModelSelected(selected) => {
                model.model_picker.mark_selected(selected);
            }
            harness::HarnessUpdate::Text(text) => {
                print!("{text}");
                model.transcript.append_assistant(&text);
            }
            harness::HarnessUpdate::Reasoning(text) => {
                // Printed to stderr so the e2e log keeps stdout as the answer
                // alone, while still proving reasoning reached the client.
                eprint!("{text}");
                model.transcript.append_reasoning(&text);
            }
            harness::HarnessUpdate::TurnDone if sent => {
                println!("\n[e2e] turn done");
                let out = std::env::temp_dir().join("jcode-desktop2-e2e.png");
                let mut painter = paint::Painter::default();
                let mut scene = Scene::new();
                build_scene(&mut scene, &mut painter, &model, (1100, 720), 1.0);
                capture::capture_scene_to_png(&scene, 1100, 720, &out)?;
                println!("[e2e] final frame -> {}", out.display());
                println!("[e2e] OK");
                return Ok(());
            }
            harness::HarnessUpdate::TurnDone => {}
            // The e2e path drives one session, so the list is irrelevant here.
            harness::HarnessUpdate::Activity(label) => {
                println!("[e2e] activity: {label}");
                model.activity.set_label(label, std::time::Instant::now());
            }
            harness::HarnessUpdate::Tool { call_id, label } => {
                println!("[e2e] tool: {label}");
                model.transcript.set_live_tool(&call_id, &label);
            }
            harness::HarnessUpdate::Edit(card) => {
                println!("[e2e] edit: +{} -{}", card.added, card.removed);
                model.transcript.push_edit(&card);
            }
            harness::HarnessUpdate::Todo(card) => {
                println!("[e2e] plan updated");
                model.transcript.set_todo(&card);
            }
            harness::HarnessUpdate::Sessions(_) => {}
            // Background progress is folded into the model so the captured
            // frame is the one a user would see, bar and all.
            harness::HarnessUpdate::Progress {
                task_id,
                label,
                summary,
                percent,
                done,
            } => {
                println!("[e2e] progress: {label} · {summary}");
                if done {
                    model.transcript.clear_progress(&task_id);
                } else {
                    model
                        .transcript
                        .set_progress(&task_id, &label, &summary, percent);
                }
            }
            // The probe asserts on the reply, so delivery of the prompt is a
            // log line rather than a state change.
            harness::HarnessUpdate::MessageAccepted => {
                println!("[e2e] message accepted");
            }
        }
    }
    anyhow::bail!("e2e timed out")
}

/// `--bench-stream [history_turns] [reply_chars] [delta_ms]`: replay a
/// scripted token stream through the real per-frame path and report every
/// frame class. This is "the streaming feels laggy" turned into numbers: a
/// per-frame cost distribution, the early-vs-late growth curve, and the exact
/// count of frames that did layout work they should not have.
fn bench_stream(args: &[String]) -> Result<()> {
    use crate::stream_bench::{Config, FrameKind, run};

    let mut config = Config::default();
    let parse = |value: &String| -> Result<u64> {
        value
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("invalid number '{value}': {error}"))
    };
    if let Some(value) = args.first() {
        config.history_turns = parse(value)? as usize;
    }
    if let Some(value) = args.get(1) {
        config.reply_chars = parse(value)? as usize;
    }
    if let Some(value) = args.get(2) {
        config.delta_interval_ms = parse(value)?;
    }

    println!(
        "streaming {} chars into a {}-turn session, one delta per {}ms, 8ms frames\n",
        config.reply_chars, config.history_turns, config.delta_interval_ms
    );
    let report = run(&config);

    println!(
        "{:<8} {:>7} {:>9} {:>9} {:>9} {:>9}",
        "frames", "count", "mean", "p50", "p95", "max"
    );
    println!("{}", "-".repeat(56));
    for (kind, name) in [
        (FrameKind::Delta, "delta"),
        (FrameKind::Reveal, "reveal"),
        (FrameKind::Idle, "idle"),
    ] {
        let Some(stats) = report.stats(kind) else {
            continue;
        };
        let ms = |us: u64| us as f64 / 1000.0;
        println!(
            "{:<8} {:>7} {:>7.2}ms {:>7.2}ms {:>7.2}ms {:>7.2}ms",
            name,
            stats.count,
            ms(stats.mean_us),
            ms(stats.p50_us),
            ms(stats.p95_us),
            ms(stats.max_us)
        );
    }

    println!();
    if let Some((early, late)) = report.delta_growth() {
        println!(
            "delta cost, first quarter of the reply: {:.2}ms -> last quarter: {:.2}ms ({:.1}x)",
            early as f64 / 1000.0,
            late as f64 / 1000.0,
            late as f64 / early.max(1) as f64
        );
    }
    println!(
        "frames over 8.3ms (120Hz): {} | over 16.6ms (60Hz): {}",
        report.over(8_333),
        report.over(16_600)
    );

    // The exact gates: these fail the command on any machine, because they
    // count work rather than time.
    let wasteful = report.wasteful_animation_frames();
    let overworked = report.overworked_delta_frames();
    if !wasteful.is_empty() {
        anyhow::bail!(
            "{} reveal/idle frames re-laid messages: animation frames must do no layout work",
            wasteful.len()
        );
    }
    if !overworked.is_empty() {
        anyhow::bail!(
            "{} delta frames laid out more than the tail message",
            overworked.len()
        );
    }
    println!("exact gates ok: animation frames did no layout, deltas re-laid only the tail");
    Ok(())
}

/// `--bench-donut`: measure the donut's per-frame CPU cost, split between the
/// SDF raymarch (the luminance field) and building the halftone path. The
/// website's budget is under 9ms of main-thread time per frame; this prints the
/// same number so a regression is a measurement, not an impression.
fn bench_donut() -> Result<()> {
    const FRAMES: u32 = 120;
    let mut field = donut::Donut::new(DONUT_GRID);
    let frame = layout::Frame::new((2200, 1440), 2.0);
    let Some(hero) = frame.hero() else {
        anyhow::bail!("no hero block at the bench window size");
    };

    let start = std::time::Instant::now();
    for i in 0..FRAMES {
        field.render(i as f32 / 60.0, 0.0);
    }
    let march = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);

    let mut painter = paint::Painter::default();
    let model = Model::default();
    let start = std::time::Instant::now();
    for i in 0..FRAMES {
        field.render(i as f32 / 60.0, 0.0);
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, &model, (2200, 1440), 2.0);
    }
    let full = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);

    println!("donut grid       : {DONUT_GRID}x{DONUT_GRID}");
    println!("halftone box     : {:.0}pt square", hero.donut.width());
    println!("sdf raymarch     : {march:.3} ms/frame");
    println!("full scene build : {full:.3} ms/frame");
    println!("scene minus march: {:.3} ms/frame", full - march);
    println!("budget           : 9.000 ms/frame (website's main-thread budget)");
    if full > 9.0 {
        anyhow::bail!("donut frame cost {full:.3}ms exceeds the 9ms budget");
    }
    Ok(())
}

/// `--capture <node|all> [out.png|out_dir]`: render state-space nodes
/// offscreen to PNG for visual verification without a window or compositor.
fn run_capture(args: &[String]) -> Result<()> {
    // Capture at HiDPI so reviewed frames match what the window shows.
    const SCALE: f64 = 2.0;
    const WIDTH: u32 = 2200;
    const HEIGHT: u32 = 1440;
    // Nodes pin the light palette so a capture cannot depend on the machine
    // that ran it. Dark is still a shipped theme, and a state nobody can
    // render is a state nobody reviews, so `--dark` re-tints the node after
    // it is built. It stays an explicit flag rather than reading the system
    // preference, for the same determinism reason.
    let dark = args.iter().any(|arg| arg == "--dark");
    let positional: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with("--"))
        .collect();
    let node = positional.first().copied().unwrap_or("all");
    let retint = move |mut model: Model| -> Model {
        if dark {
            model.theme = crate::theme::Theme::print_dark();
            model.theme_preference = crate::theme::ThemeMode::Dark;
        }
        model
    };
    let mut painter = paint::Painter::default();
    let mut render_node = |name: &str, model: &Model, path: &std::path::Path| -> Result<()> {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, model, (WIDTH, HEIGHT), SCALE);
        capture::capture_scene_to_png(&scene, WIDTH, HEIGHT, path)?;
        println!("captured {name} -> {}", path.display());
        Ok(())
    };
    if node == "all" {
        let dir = std::path::PathBuf::from(positional.get(1).copied().unwrap_or("captures"));
        std::fs::create_dir_all(&dir)?;
        for name in states::names() {
            let model = retint(states::by_name(name).expect("listed node"));
            render_node(name, &model, &dir.join(format!("{name}.png")))?;
        }
        return Ok(());
    }
    let Some(model) = states::by_name(node).map(retint) else {
        anyhow::bail!(
            "unknown node '{node}'; available: {}",
            states::names().join(", ")
        );
    };
    let out = std::path::PathBuf::from(
        positional
            .get(1)
            .map(|path| (*path).to_string())
            .unwrap_or_else(|| format!("{node}.png")),
    );
    render_node(node, &model, &out)
}

/// `--check-reconnect`: prove the client survives losing the harness.
///
/// This is the "no network" report at the transport layer: the connection dies
/// mid-session, and the app has to say so and then come back rather than sitting
/// there accepting input into nothing. Checked against the real runtime because
/// the bug was in the wiring, not in a pure function: attach, drop the bridge,
/// then require both a reported failure and a re-attach to *the same* session.
/// Scan the real session store the way Ctrl+R does, and report what it found.
///
/// A unit test scans a directory it wrote itself, which proves the parser and
/// nothing about the store on this machine: record shapes have changed over
/// months of sessions, and a picker that silently resolved no working
/// directories would look like an empty store rather than a parse that stopped
/// matching. This is the check that runs against the real thing.
fn check_resume_scan() -> Result<()> {
    let dir = crate::resume::sessions_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot locate the session store: no HOME"))?;
    let started = std::time::Instant::now();
    let records = crate::resume::scan(&dir, crate::resume::SCAN_LIMIT);
    let elapsed = started.elapsed();
    if records.is_empty() {
        anyhow::bail!("scanned {} and found no sessions", dir.display());
    }
    let picker = crate::resume::Picker::pinned(records.clone(), 0, "");
    let rows = picker.rows();
    let projects = rows
        .iter()
        .filter(|row| matches!(row, crate::resume::Row::Group { .. }))
        .count();
    let with_dir = records
        .iter()
        .filter(|record| record.working_dir.is_some())
        .count();
    println!(
        "resume scan ok: {} sessions in {} projects from {} in {:.0}ms; {} resolved a directory",
        records.len(),
        projects,
        dir.display(),
        elapsed.as_secs_f64() * 1000.0,
        with_dir,
    );
    for row in rows.iter().take(12) {
        match row {
            crate::resume::Row::Group { label, count, .. } => println!("  {label} ({count})"),
            crate::resume::Row::Session { index } => {
                if let Some(record) = records.get(*index) {
                    println!(
                        "    {} · {}",
                        record.label(),
                        crate::resume::human_bytes(record.bytes)
                    );
                }
            }
        }
    }
    // A store where nothing resolved a directory is a parse that stopped
    // matching the records, not a user with no projects: the picker would file
    // every session under "(unknown project)" and be useless.
    if with_dir == 0 {
        anyhow::bail!("no session resolved a working directory; the record shape may have changed");
    }
    Ok(())
}

fn check_reconnect() -> Result<()> {
    // Its own *bridge* socket, on the shared daemon. The check works by killing
    // the bridge, and the developer's live desktop windows talk to the shared
    // one: taking that down to test a client would be a check that breaks the
    // thing it is checking. The daemon is left shared because it is expensive
    // to start and this check does not disturb it.
    let runtime = std::env::temp_dir().join(format!("jcode-reconnect-{}", std::process::id()));
    std::fs::create_dir_all(&runtime)?;
    // SAFETY: single-threaded, before any connection thread is spawned.
    unsafe {
        std::env::set_var("JCODE_API_SOCKET", runtime.join("api.sock"));
    }
    let bridge = |kill: bool| -> Option<u32> {
        let listening = std::process::Command::new("pgrep")
            .args(["-f", "jcode-harness-api-bridge"])
            .output()
            .ok()?;
        let pids: Vec<u32> = String::from_utf8_lossy(&listening.stdout)
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect();
        // Only ever the bridge this check started: the newest pid, and only
        // one that was not running before we began.
        let mine = pids.into_iter().max()?;
        if kill {
            let _ = std::process::Command::new("kill")
                .arg(mine.to_string())
                .status();
        }
        Some(mine)
    };
    let pre_existing = bridge(false);

    let (updates, _outgoing) = harness::spawn(|| {});
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut first_session: Option<String> = None;
    let mut failure: Option<String> = None;
    let mut dropped = false;

    while std::time::Instant::now() < deadline {
        let Ok(update) = updates.recv_timeout(std::time::Duration::from_secs(1)) else {
            continue;
        };
        match update {
            harness::HarnessUpdate::Status(status) => println!("[reconnect] status: {status}"),
            harness::HarnessUpdate::Failed(message) => {
                println!("[reconnect] failure: {message}");
                failure = Some(message);
            }
            harness::HarnessUpdate::Attached { session_id, .. } => match first_session.clone() {
                None => {
                    println!("[reconnect] attached: {session_id}");
                    first_session = Some(session_id);
                    // Drop the bridge this check started. It comes back on the
                    // next attempt (`ensure_runtime` starts it), so this is the
                    // same shape as the runtime restart a rebuild produces.
                    match bridge(false) {
                        Some(pid) if Some(pid) != pre_existing => {
                            bridge(true);
                            dropped = true;
                            println!("[reconnect] dropped the bridge (pid {pid})");
                        }
                        _ => anyhow::bail!(
                            "could not identify this check's own bridge process to drop"
                        ),
                    }
                }
                Some(previous) => {
                    if !dropped {
                        continue;
                    }
                    let reported = failure.clone().ok_or_else(|| {
                        anyhow::anyhow!("reconnected without ever reporting a failure")
                    })?;
                    if session_id != previous {
                        anyhow::bail!(
                            "reconnect landed in a different session: {previous} -> {session_id}"
                        );
                    }
                    println!("[reconnect] re-attached {session_id} after: {reported}");
                    println!("[reconnect] OK");
                    let _ = std::fs::remove_dir_all(&runtime);
                    return Ok(());
                }
            },
            _ => {}
        }
    }
    anyhow::bail!(
        "reconnect check timed out (attached={first_session:?} dropped={dropped} \
         failure={failure:?})"
    )
}

/// `--bench-scroll`: replay the scroll gestures a hand actually makes and
/// report how the view answered them.
///
/// "The scrollwheel feels wrong" is an impression, and impressions cannot be
/// tuned against: every constant in `scroll.rs` trades against another one, so
/// tightening the ease for a wheel notch quietly ruins a trackpad drag. This
/// turns the feel into numbers (latency, tracking error, travel ratio, jerk)
/// over a fixed set of gestures, so a change can be judged instead of felt.
fn bench_scroll() -> Result<()> {
    let reports = scroll_bench::sweep();
    if !scroll_bench::report(&reports) {
        anyhow::bail!("the scroll misbehaved on one or more gestures");
    }
    Ok(())
}

/// `--profile-scroll`: attribute a scroll frame's cost to its phases, swept
/// over history length.
///
/// Separate from `--bench-scroll` because the two answer different questions:
/// that one says whether the motion is right, this one says where the frame's
/// time went and whether any of it grows with how long the session is.
fn profile_scroll() -> Result<()> {
    let costs = scroll_profile::sweep();
    if !scroll_profile::report(&costs) {
        anyhow::bail!("scroll frames are re-laying text or scale with history length");
    }
    Ok(())
}
