use super::{App, DisplayMessage, ProcessingStatus, RunResult};
use crate::replay::{PaneReplayInput, ReplayEvent, TimelineEvent};
use crate::tui::backend::RemoteConnection;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame, Terminal, backend::TestBackend, buffer::Buffer, layout::Rect,
};
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
                if let Some(chunk) = app.stream_buffer.flush() {
                    app.streaming_text.push_str(&chunk);
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

pub(super) async fn run_swarm_replay(
    mut terminal: DefaultTerminal,
    panes: Vec<PaneReplayInput>,
    speed: f64,
    centered_override: Option<bool>,
) -> Result<()> {
    if panes.is_empty() {
        anyhow::bail!("No swarm replay panes to render");
    }

    let mut panes: Vec<SwarmReplayPane> = panes
        .into_iter()
        .map(|pane| SwarmReplayPane::new(pane, centered_override))
        .collect();
    let mut event_stream = EventStream::new();
    let mut redraw_period = Duration::from_millis(16);
    let mut redraw_interval = interval(redraw_period);
    let mut paused = false;
    let mut replay_speed = speed.clamp(0.1, 20.0);
    let mut sim_time_ms = 0.0;
    let mut last_tick = Instant::now();
    let total_duration_ms = panes
        .iter()
        .map(SwarmReplayPane::total_duration_ms)
        .fold(0.0, f64::max);
    let mut should_quit = false;

    loop {
        terminal.draw(|frame| draw_swarm_replay_frame(frame, &mut panes, sim_time_ms))?;

        if should_quit {
            break;
        }

        let replay_done = panes.iter().all(SwarmReplayPane::is_done);
        if !paused && !replay_done {
            let elapsed = last_tick.elapsed();
            sim_time_ms = (sim_time_ms + elapsed.as_secs_f64() * 1000.0 * replay_speed)
                .min(total_duration_ms.max(0.0));
            last_tick = Instant::now();
        } else {
            last_tick = Instant::now();
        }

        tokio::select! {
            _ = redraw_interval.tick() => {}
            event = event_stream.next() => {
                if let Some(Ok(event)) = event {
                    handle_swarm_replay_input(
                        &mut terminal,
                        event,
                        replay_done,
                        &mut should_quit,
                        &mut paused,
                        &mut replay_speed,
                        &mut last_tick,
                    );
                }
            }
        }

        let desired_redraw = if paused {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(16)
        };
        if desired_redraw != redraw_period {
            redraw_period = desired_redraw;
            redraw_interval = interval(redraw_period);
        }
    }

    Ok(())
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
                if let Some(amount) = app.scroll_keys.scroll_amount(key.code, key.modifiers) {
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

fn handle_swarm_replay_input(
    terminal: &mut DefaultTerminal,
    event: Event,
    replay_done: bool,
    should_quit: &mut bool,
    paused: &mut bool,
    replay_speed: &mut f64,
    last_tick: &mut Instant,
) {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *should_quit = true;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                *should_quit = true;
            }
            KeyCode::Char(' ') => {
                *paused = !*paused;
                *last_tick = Instant::now();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                *replay_speed = (*replay_speed * 1.5).min(20.0);
                *last_tick = Instant::now();
            }
            KeyCode::Char('-') => {
                *replay_speed = (*replay_speed / 1.5).max(0.1);
                *last_tick = Instant::now();
            }
            KeyCode::Right if replay_done => {
                *should_quit = true;
            }
            _ => {}
        },
        Event::Resize(_, _) => {
            let _ = terminal.clear();
            *last_tick = Instant::now();
        }
        _ => {}
    }
}

struct SwarmReplayPane {
    app: App,
    remote: RemoteConnection,
    event_schedule: Vec<(f64, ReplayEvent)>,
    event_cursor: usize,
    replay_turn_id: u64,
}

impl SwarmReplayPane {
    fn new(input: PaneReplayInput, centered_override: Option<bool>) -> Self {
        let event_schedule = schedule_replay_events(&input.timeline);
        let mut app = App::new_for_replay(input.session);
        if let Some(centered) = centered_override {
            app.set_centered(centered);
        }
        Self {
            app,
            remote: RemoteConnection::dummy(),
            event_schedule,
            event_cursor: 0,
            replay_turn_id: 0,
        }
    }

    fn total_duration_ms(&self) -> f64 {
        self.event_schedule.last().map(|(t, _)| *t).unwrap_or(0.0)
    }

    fn is_done(&self) -> bool {
        self.event_cursor >= self.event_schedule.len()
    }

    fn advance_to(&mut self, sim_time_ms: f64) {
        while self.event_cursor < self.event_schedule.len()
            && self.event_schedule[self.event_cursor].0 <= sim_time_ms
        {
            let event = self.event_schedule[self.event_cursor].1.clone();
            apply_replay_event(
                &mut self.app,
                &mut self.remote,
                &event,
                &mut self.replay_turn_id,
                Some(sim_time_ms),
            );
            self.event_cursor += 1;
        }

        if self.is_done() {
            self.app.is_processing = false;
            self.app.status = ProcessingStatus::Idle;
        }

        update_replay_elapsed_override(&mut self.app, sim_time_ms);
    }

    fn render_buffer(&self, width: u16, height: u16) -> Result<Buffer> {
        let backend = TestBackend::new(width.max(1), height.max(1));
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|frame| crate::tui::render_frame(frame, &self.app))?;
        Ok(terminal.backend().buffer().clone())
    }
}

fn schedule_replay_events(timeline: &[TimelineEvent]) -> Vec<(f64, ReplayEvent)> {
    let mut abs_time_ms = 0.0;
    crate::replay::timeline_to_replay_events(timeline)
        .into_iter()
        .map(|(delay_ms, event)| {
            abs_time_ms += delay_ms as f64;
            (abs_time_ms, event)
        })
        .collect()
}

fn draw_swarm_replay_frame(frame: &mut Frame<'_>, panes: &mut [SwarmReplayPane], sim_time_ms: f64) {
    let area = frame.area().intersection(*frame.buffer_mut().area());
    crate::tui::color_support::clear_buf(area, frame.buffer_mut());
    if panes.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let pane_count = panes.len() as u16;
    let cols = if pane_count <= 2 { pane_count } else { 2 };
    let rows = ((pane_count + cols - 1) / cols).max(1);
    let pane_width = (area.width / cols).max(1);
    let pane_height = (area.height / rows).max(1);

    for (idx, pane) in panes.iter_mut().enumerate() {
        pane.advance_to(sim_time_ms);

        let idx = idx as u16;
        let col = idx % cols;
        let row = idx / cols;
        let x = area.x + col * pane_width;
        let y = area.y + row * pane_height;
        let pane_area = Rect::new(
            x,
            y,
            if col == cols - 1 {
                area.width - (x - area.x)
            } else {
                pane_width
            },
            if row == rows - 1 {
                area.height - (y - area.y)
            } else {
                pane_height
            },
        );

        if let Ok(buf) = pane.render_buffer(pane_area.width, pane_area.height) {
            blit_buffer(frame.buffer_mut(), pane_area, &buf);
        }
    }
}

fn blit_buffer(dst: &mut Buffer, area: Rect, src: &Buffer) {
    for sy in 0..area.height.min(src.area.height) {
        for sx in 0..area.width.min(src.area.width) {
            let dx = area.x + sx;
            let dy = area.y + sy;
            if let (Some(src_cell), Some(dst_cell)) = (src.cell((sx, sy)), dst.cell_mut((dx, dy))) {
                *dst_cell = src_cell.clone();
            }
        }
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
        ReplayEvent::MemoryInjection {
            summary,
            content,
            count: _,
        } => {
            let display = DisplayMessage::memory(summary.clone(), content.clone());
            app.push_display_message(display);
        }
        ReplayEvent::DisplayMessage {
            role,
            title,
            content,
        } => {
            if role == "swarm" {
                app.swarm_enabled = true;
            }
            app.push_display_message(DisplayMessage {
                role: role.clone(),
                content: content.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: title.clone(),
                tool_data: None,
            });
        }
        ReplayEvent::SwarmStatus { members } => {
            app.swarm_enabled = true;
            app.remote_swarm_members = members.clone();
        }
        ReplayEvent::SwarmPlan {
            swarm_id,
            version,
            items,
        } => {
            app.swarm_enabled = true;
            app.swarm_plan_swarm_id = Some(swarm_id.clone());
            app.swarm_plan_version = Some(*version);
            app.swarm_plan_items = items.clone();
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

#[cfg(test)]
mod tests {
    use super::schedule_replay_events;
    use crate::replay::{ReplayEvent, TimelineEvent, TimelineEventKind};

    #[test]
    fn schedule_replay_events_accumulates_relative_delays() {
        let timeline = vec![
            TimelineEvent {
                t: 0,
                kind: TimelineEventKind::UserMessage {
                    text: "hi".to_string(),
                },
            },
            TimelineEvent {
                t: 250,
                kind: TimelineEventKind::Thinking { duration: 250 },
            },
            TimelineEvent {
                t: 500,
                kind: TimelineEventKind::StreamText {
                    text: "there".to_string(),
                    speed: 80,
                },
            },
        ];

        let scheduled = schedule_replay_events(&timeline);
        assert_eq!(scheduled.len(), 4);
        assert_eq!(scheduled[0].0, 0.0);
        assert_eq!(scheduled[1].0, 250.0);
        assert_eq!(scheduled[2].0, 500.0);
        assert!(scheduled[3].0 > scheduled[2].0);
        assert!(matches!(scheduled[0].1, ReplayEvent::UserMessage { .. }));
        assert!(matches!(scheduled[1].1, ReplayEvent::StartProcessing));
        assert!(matches!(scheduled[2].1, ReplayEvent::Server(_)));
        assert!(matches!(scheduled[3].1, ReplayEvent::Server(_)));
    }
}
