//! Interactive, animated demo of the inline swarm gallery.
//!
//! This runs as a real full-screen terminal app so you can *see* the inline
//! gallery the way it looks live in the jcode TUI, without touching any running
//! agents. It simulates a handful of mock swarm workers that stream output,
//! change status, and finish over time, rendering them through the exact same
//! `render_swarm_gallery` layout the real TUI uses.
//!
//! Run with:
//!   cargo run --profile selfdev -p jcode-tui-render --example swarm_gallery_live
//!
//! Controls:
//!   q / Esc      quit
//!   + / -        more / fewer agents
//!     [ / ]        shrink / grow the gallery band (the max_pct knob)
//!     space        pause / resume the animation

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use jcode_tui_render::swarm_gallery::{GalleryMember, humanize_age, render_gallery};

/// A simulated worker, mirroring the fields the real adapter reads from a
/// `SwarmMemberStatus` (name, role, status, streamed output tail).
struct MockWorker {
    name: String,
    role: Option<&'static str>,
    status: String,
    /// Full streamed transcript so far; the tile shows the tail.
    transcript: Vec<String>,
    /// Scripted lines this worker will "stream" over time.
    script: Vec<String>,
    next_line: usize,
    /// Ticks until the next line is appended.
    cooldown: u16,
    started: Instant,
}

impl MockWorker {
    fn new(name: &str, role: Option<&'static str>, script: Vec<&str>) -> Self {
        Self {
            name: name.to_string(),
            role,
            status: "spawned".to_string(),
            transcript: Vec::new(),
            script: script.into_iter().map(|s| s.to_string()).collect(),
            next_line: 0,
            cooldown: 2,
            started: Instant::now(),
        }
    }

    fn tick(&mut self) {
        if self.next_line >= self.script.len() {
            if self.status != "completed" {
                self.status = "completed".to_string();
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            // While waiting, alternate running/thinking to show the badge change.
            if self.transcript.is_empty() {
                self.status = "thinking".to_string();
            }
            return;
        }
        let line = self.script[self.next_line].clone();
        self.status = if line.starts_with("! ") {
            "blocked".to_string()
        } else {
            "running".to_string()
        };
        self.transcript
            .push(line.trim_start_matches("! ").to_string());
        self.next_line += 1;
        self.cooldown = 2 + (self.next_line as u16 % 3);
    }

    fn age_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

fn workers_to_members(workers: &[MockWorker]) -> Vec<GalleryMember> {
    workers
        .iter()
        .map(|w| {
            let mut body: Vec<String> = w.transcript.clone();
            body.push(format!("· {} ago", humanize_age(w.age_secs())));
            GalleryMember {
                label: w.name.clone(),
                icon: None,
                status: w.status.clone(),
                task: None,
                role: w.role.map(str::to_string),
                body,
                sort_key: w.name.clone(),
                todo: None,
                todo_items: Vec::new(),
                model: None,
                provider: None,
                auth_method: None,
                effort: None,
                elapsed_secs: Some(w.age_secs()),
            }
        })
        .collect()
}

fn make_workers(n: usize) -> Vec<MockWorker> {
    let pool: Vec<(&str, Option<&'static str>, Vec<&str>)> = vec![
        (
            "researcher",
            Some("coordinator"),
            vec![
                "Searching the codebase for the auth flow...",
                "Found 12 candidate files.",
                "Reading crates/jcode-app-core/src/auth.rs",
                "The OAuth callback is handled in handle_login()",
                "Now cross-referencing the token refresh path.",
                "Refresh happens in refresh_session() (line 412).",
                "Summarizing findings for the implementer.",
            ],
        ),
        (
            "implementer",
            None,
            vec![
                "Editing crates/jcode-base/src/config.rs",
                "Added swarm_spawn_mode = inline",
                "Running cargo check...",
                "warning: unused import `Foo`",
                "Fixing the import.",
                "cargo check clean.",
                "Wiring the gallery into the draw path.",
            ],
        ),
        (
            "reviewer",
            None,
            vec![
                "Waiting for the implementer to finish.",
                "Reviewing the diff...",
                "Reviewed 4 files.",
                "! Blocked: needs a test for the new branch.",
                "Test added, re-reviewing.",
                "No blocking issues found.",
                "LGTM",
            ],
        ),
        (
            "tester",
            None,
            vec![
                "Building the selfdev profile...",
                "Compiling jcode-tui-render",
                "Running 5 tests",
                "test cells_are_width_bounded ... ok",
                "test many_agents_form_multiple_columns ... ok",
                "All tests passed.",
            ],
        ),
        (
            "doc-writer",
            None,
            vec![
                "Drafting the config docs.",
                "Documenting swarm_gallery_max_pct.",
                "Adding an example to default_file.rs.",
                "Proofreading.",
                "Docs ready.",
            ],
        ),
        (
            "packager",
            None,
            vec![
                "Preparing the release worktree.",
                "Bumping version.",
                "Staging changes.",
                "Commit drafted.",
            ],
        ),
        (
            "benchmarker",
            None,
            vec![
                "Warming up the bench harness.",
                "Running memory_recall_bench...",
                "p50 latency 18ms",
                "p99 latency 64ms",
                "Results recorded.",
            ],
        ),
        (
            "linter",
            None,
            vec![
                "Running clippy...",
                "warning: needless clone (3)",
                "Auto-fixing.",
                "clippy clean.",
            ],
        ),
    ];
    pool.into_iter()
        .take(n.max(1))
        .map(|(name, role, script)| MockWorker::new(name, role, script))
        .collect()
}

fn render_gallery_lines(
    workers: &[MockWorker],
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    // Delegate to the exact same shared renderer the live TUI uses; only the
    // member data (built from mock workers) differs.
    render_gallery(&workers_to_members(workers), width, max_height)
}

fn draw(f: &mut Frame, workers: &[MockWorker], max_pct: usize, paused: bool) {
    let area = f.area();

    // Reserve a top band like the real TUI does: a configurable share of the
    // chat column height, capped, with a >=5 row floor before it shows.
    let budget = ((area.height as usize * max_pct) / 100).clamp(0, 18);
    let lines = if budget >= 5 && area.width >= 24 {
        render_gallery_lines(workers, area.width as usize, budget)
    } else {
        Vec::new()
    };

    let band_h = if lines.is_empty() {
        0u16
    } else {
        ((lines.len() as u16) + 1).min(area.height / 2)
    };

    let band = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: band_h,
    };
    let chat = Rect {
        x: area.x,
        y: area.y + band_h,
        width: area.width,
        height: area.height.saturating_sub(band_h),
    };

    if band_h > 0 {
        f.render_widget(Clear, band);
        f.render_widget(Paragraph::new(lines), band);
    }

    // Mock chat / instructions below the band.
    let help = vec![
        Line::from(vec![Span::styled(
            "  inline swarm gallery — live demo (no real agents touched)",
            Style::default()
                .fg(Color::Rgb(200, 200, 210))
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  The band above is the same renderer the jcode TUI shows above your chat",
            Style::default().fg(Color::Rgb(150, 150, 160)),
        )]),
        Line::from(vec![Span::styled(
            "  when you run a swarm with  swarm_spawn_mode = \"inline\".",
            Style::default().fg(Color::Rgb(150, 150, 160)),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  band size: ",
                Style::default().fg(Color::Rgb(150, 150, 160)),
            ),
            Span::styled(
                format!("{max_pct}% "),
                Style::default().fg(Color::Rgb(255, 200, 100)),
            ),
            Span::styled(
                "(agents.swarm_gallery_max_pct)",
                Style::default().fg(Color::Rgb(110, 110, 120)),
            ),
            Span::styled(
                if paused { "   [PAUSED]" } else { "" },
                Style::default().fg(Color::Rgb(255, 170, 80)),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  keys:  q/Esc quit   +/- agents   [ / ] band size   space pause",
            Style::default().fg(Color::Rgb(120, 120, 130)),
        )]),
    ];
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Rgb(70, 70, 80)))
        .title(Span::styled(
            " chat ",
            Style::default().fg(Color::Rgb(120, 120, 130)),
        ));
    f.render_widget(
        Paragraph::new(help).block(block).wrap(Wrap { trim: false }),
        chat,
    );
}

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut() as &mut CrosstermBackend<Stdout>,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    res
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut n_agents = 4usize;
    let mut workers = make_workers(n_agents);
    let mut max_pct = 40usize;
    let mut paused = false;
    let tick = Duration::from_millis(350);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &workers, max_pct, paused))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            let ctrl_c =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                _ if ctrl_c => break,
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    n_agents = (n_agents + 1).min(8);
                    workers = make_workers(n_agents);
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    n_agents = n_agents.saturating_sub(1).max(1);
                    workers = make_workers(n_agents);
                }
                KeyCode::Char(']') => max_pct = (max_pct + 5).min(90),
                KeyCode::Char('[') => max_pct = max_pct.saturating_sub(5).max(5),
                KeyCode::Char(' ') => paused = !paused,
                KeyCode::Char('r') => {
                    workers = make_workers(n_agents);
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick {
            if !paused {
                for w in workers.iter_mut() {
                    w.tick();
                }
                // When everything is done, loop the demo after a short beat.
                if workers.iter().all(|w| w.status == "completed")
                    && workers.iter().all(|w| w.age_secs() > 1)
                {
                    // restart so the demo keeps streaming
                    workers = make_workers(n_agents);
                }
            }
            last_tick = Instant::now();
        }
    }
    Ok(())
}
