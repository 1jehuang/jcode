//! Entry points that run instead of the window.
//!
//! Scripted input, the keymap table, the end-to-end harness, the donut
//! benchmark, offscreen captures, and the state-space profiler. These share
//! nothing with the event loop but the model, and keeping them in `main.rs`
//! left the entry point mostly not-the-entry-point.

use crate::{
    App, DONUT_GRID, Model, ModelId, build_scene, capture, donut, harness, keymap, layout, paint,
    profile, states, transcript,
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
        Some("--bench-donut") => Some(bench_donut()),
        Some("--capture") => Some(run_capture(&args[1..])),
        Some("--check-primary-selection") => Some(check_primary_selection()),
        Some("--e2e") => Some(run_e2e(
            args.get(1)
                .map(String::as_str)
                .unwrap_or("Reply with exactly the word: pong"),
        )),
        _ => None,
    }
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
fn run_script(steps: &[String]) -> Result<()> {
    let mut app = App::default();
    app.model.session_id = Some("session_script".into());
    for step in steps {
        if let Some(text) = step.strip_prefix("type:") {
            app.apply(keymap::Action::Insert, Some(text));
            continue;
        }
        let (key, mods) =
            keymap::parse_chord(step).ok_or_else(|| anyhow::anyhow!("unknown chord '{step}'"))?;
        let action = keymap::resolve(&key, mods).unwrap_or(keymap::Action::Insert);
        if !app.apply(action, None) {
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
                if status.starts_with("disconnected") || status.starts_with("error") {
                    anyhow::bail!("harness failure: {status}");
                }
                model.status = status;
            }
            harness::HarnessUpdate::Attached { session_id, .. } => {
                println!("[e2e] attached: {session_id}");
                model.status = format!("attached: {session_id}");
                model.session_id = Some(session_id);
                model.transcript.push(transcript::Message::user(message));
                outgoing.send(harness::Command::Send(message.to_string()))?;
                sent = true;
            }
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
            harness::HarnessUpdate::Text(text) => {
                print!("{text}");
                model.transcript.append_assistant(&text);
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
            harness::HarnessUpdate::Sessions(_) => {}
        }
    }
    anyhow::bail!("e2e timed out")
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
    let node = args.first().map(String::as_str).unwrap_or("all");
    let mut painter = paint::Painter::default();
    let mut render_node = |name: &str, model: &Model, path: &std::path::Path| -> Result<()> {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut painter, model, (WIDTH, HEIGHT), SCALE);
        capture::capture_scene_to_png(&scene, WIDTH, HEIGHT, path)?;
        println!("captured {name} -> {}", path.display());
        Ok(())
    };
    if node == "all" {
        let dir = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("captures"));
        std::fs::create_dir_all(&dir)?;
        for name in states::names() {
            let model = states::by_name(name).expect("listed node");
            render_node(name, &model, &dir.join(format!("{name}.png")))?;
        }
        return Ok(());
    }
    let Some(model) = states::by_name(node) else {
        anyhow::bail!(
            "unknown node '{node}'; available: {}",
            states::names().join(", ")
        );
    };
    let out = std::path::PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| format!("{node}.png")),
    );
    render_node(node, &model, &out)
}
