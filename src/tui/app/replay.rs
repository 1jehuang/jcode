use super::{App, DisplayMessage, ProcessingStatus, RunResult};
use crate::replay::{ReplayEvent, TimelineEvent};
use crate::tui::backend::RemoteConnection;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};
use tokio::time::interval;

pub(super) async fn run_replay(
    mut app: App,
    mut terminal: DefaultTerminal,
    timeline: Vec<TimelineEvent>,
    speed: f64,
) -> Result<RunResult> {
    let mut event_stream = EventStream::new();
    let mut redraw_period = super::super::redraw_interval(&app);
    let mut redraw_interval = interval(redraw_period);
    let mut remote = RemoteConnection::dummy();

    let replay_events = crate::replay::timeline_to_replay_events(&timeline);
    let mut event_index: usize = 0;
    let mut paused = false;
    let mut replay_speed = speed;
    let mut next_event_at: Option<tokio::time::Instant> = Some(tokio::time::Instant::now());
    let mut replay_turn_id: u64 = 0;

    loop {
        let desired_redraw = super::super::redraw_interval(&app);
        if desired_redraw != redraw_period {
            redraw_period = desired_redraw;
            redraw_interval = interval(redraw_period);
        }

        terminal.draw(|frame| crate::tui::ui::draw(frame, &app))?;

        if app.should_quit {
            break;
        }

        let replay_done = event_index >= replay_events.len();

        tokio::select! {
            _ = redraw_interval.tick() => {
                if app.stream_buffer.should_flush() {
                    if let Some(chunk) = app.stream_buffer.flush() {
                        app.streaming_text.push_str(&chunk);
                    }
                }
            }
            event = event_stream.next() => {
                if let Some(Ok(event)) = event {
                    handle_replay_input(&mut app, &mut terminal, event, replay_done, &mut paused, &mut replay_speed, &mut next_event_at);
                }
            }
            _ = async {
                if let Some(target) = next_event_at {
                    tokio::time::sleep_until(target).await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if !paused && !replay_done => {
                if event_index < replay_events.len() {
                    let replay_event = replay_events[event_index].1.clone();
                    apply_replay_event(&mut app, &mut remote, &replay_event, &mut replay_turn_id, None);

                    event_index += 1;

                    if event_index < replay_events.len() {
                        let next_delay = replay_events[event_index].0;
                        let adjusted = (next_delay as f64 / replay_speed) as u64;
                        next_event_at = Some(tokio::time::Instant::now() + Duration::from_millis(adjusted));
                    } else {
                        next_event_at = None;
                        app.is_processing = false;
                        app.status = ProcessingStatus::Idle;
                    }
                }
            }
        }
    }

    Ok(RunResult {
        reload_session: None,
        rebuild_session: None,
        update_session: None,
        restart_session: None,
        exit_code: None,
        session_id: if app.is_remote {
            app.remote_session_id.clone()
        } else {
            Some(app.session.id.clone())
        },
    })
}

fn handle_replay_input(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    event: Event,
    replay_done: bool,
    paused: &mut bool,
    replay_speed: &mut f64,
    next_event_at: &mut Option<tokio::time::Instant>,
) {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                app.should_quit = true;
            }
            KeyCode::Char(' ') => {
                *paused = !*paused;
                if !*paused && !replay_done {
                    *next_event_at = Some(tokio::time::Instant::now());
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                *replay_speed = (*replay_speed * 1.5).min(20.0);
            }
            KeyCode::Char('-') => {
                *replay_speed = (*replay_speed / 1.5).max(0.1);
            }
            _ => {
                if let Some(amount) = app
                    .scroll_keys
                    .scroll_amount(key.code, key.modifiers)
                {
                    if amount < 0 {
                        app.scroll_up((-amount) as usize);
                    } else {
                        app.scroll_down(amount as usize);
                    }
                }
            }
        },
        Event::Mouse(mouse) => {
            app.handle_mouse_event(mouse);
        }
        Event::Resize(_, _) => {
            let _ = terminal.clear();
        }
        _ => {}
    }
}

pub(super) fn apply_replay_event(
    app: &mut App,
    remote: &mut RemoteConnection,
    replay_event: &ReplayEvent,
    replay_turn_id: &mut u64,
    replay_processing_started_ms: Option<f64>,
) {
    match replay_event {
        ReplayEvent::UserMessage { text } => {
            app.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: text.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }
        ReplayEvent::StartProcessing => {
            *replay_turn_id += 1;
            app.current_message_id = Some(*replay_turn_id);
            app.is_processing = true;
            app.processing_started = Some(Instant::now());
            app.status = ProcessingStatus::Thinking(Instant::now());
            app.streaming_tps_start = Some(Instant::now());
            app.streaming_tps_elapsed = Duration::ZERO;
            app.streaming_total_output_tokens = 0;
            app.replay_processing_started_ms = replay_processing_started_ms;
        }
        ReplayEvent::Server(server_event) => {
            if let crate::protocol::ServerEvent::TextDelta { text } = server_event {
                if !text.is_empty() {
                    app.streaming_text.push_str(text);
                    if matches!(app.status, ProcessingStatus::Thinking(_)) {
                        app.status = ProcessingStatus::Streaming;
                    }
                    app.last_stream_activity = Some(Instant::now());
                }
            } else {
                app.handle_server_event(server_event.clone(), remote);
            }
        }
    }
}

pub(super) fn update_replay_elapsed_override(app: &mut App, sim_time_ms: f64) {
    if let Some(start_ms) = app.replay_processing_started_ms {
        let elapsed_ms = (sim_time_ms - start_ms).max(0.0);
        app.replay_elapsed_override = Some(Duration::from_millis(elapsed_ms as u64));
    } else {
        app.replay_elapsed_override = None;
    }
}
