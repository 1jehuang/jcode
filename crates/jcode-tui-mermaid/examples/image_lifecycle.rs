//! Exercise the real image widget pipeline without a daemon, model, or network.
//!
//! Run inside a graphics-capable terminal or Zellij 0.46+:
//! `cargo run -p jcode-tui-mermaid --example image_lifecycle -- --auto`
//! Omit `--auto` to inspect each phase with keys 1-7. Press q to quit.
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use image::{Rgba, RgbaImage};
use jcode_tui_mermaid as images;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    widgets::{Clear, Paragraph},
};

struct Cleanup {
    path: PathBuf,
    raw: bool,
    alternate: bool,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        images::clear_image_state();
        let mut stdout = io::stdout();
        let _ = stdout.write_all(images::take_terminal_image_cleanup_payload().as_bytes());
        if self.alternate {
            let _ = execute!(stdout, Show, LeaveAlternateScreen);
        }
        if self.raw {
            let _ = disable_raw_mode();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let auto = std::env::args().any(|arg| arg == "--auto");
    let path =
        std::env::temp_dir().join(format!("jcode-image-lifecycle-{}.png", std::process::id()));
    let mut image = RgbaImage::new(640, 360);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = if x % 80 < 3 || y % 60 < 3 {
            Rgba([255, 255, 255, 255])
        } else {
            Rgba([
                (40 + x * 190 / 640) as u8,
                (40 + y * 190 / 360) as u8,
                if (x / 80 + y / 60) % 2 == 0 { 210 } else { 60 },
                255,
            ])
        };
    }
    image.save(&path)?;
    // Register cleanup before terminal setup, including partial setup failures.
    let mut cleanup = Cleanup {
        path: path.clone(),
        raw: false,
        alternate: false,
    };
    images::init_picker();
    if !images::native_image_protocol_available() {
        return Err(format!("No native image protocol detected ({:?}). In a terminal that hides capabilities, try JCODE_MERMAID_PICKER_PROBE=1.", images::protocol_type()).into());
    }
    let hash = images::register_external_image(&path, image.width(), image.height());
    enable_raw_mode()?;
    cleanup.raw = true;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    cleanup.alternate = true;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let started = Instant::now();
    let mut phase = 0usize;
    let names = [
        "fit",
        "moved",
        "cropped / panned",
        "1000% zoom",
        "two placements",
        "overlay",
        "removed",
    ];
    loop {
        if auto {
            phase = (started.elapsed().as_millis() / 2500) as usize;
            if phase >= names.len() {
                break;
            }
        }
        terminal.draw(|frame| {
            images::begin_terminal_image_frame();
            let bounds = frame.area();
            frame.render_widget(
                Paragraph::new(format!(
                    "Image lifecycle: {} | {:?} | 1-7 phases, q quit",
                    names[phase],
                    images::protocol_type(),
                )),
                Rect::new(bounds.x, bounds.y, bounds.width, bounds.height.min(1)),
            );
            let area = Rect::new(
                bounds.x.saturating_add(2),
                bounds.y.saturating_add(3),
                bounds.width.saturating_sub(6).min(64),
                bounds.height.saturating_sub(7).min(20),
            )
            .intersection(bounds);
            match phase {
                0 | 1 | 4 | 5 => {
                    let area = if phase == 1 {
                        Rect {
                            x: area.x.saturating_add(3),
                            y: area.y.saturating_add(2),
                            width: area.width.saturating_sub(3),
                            height: area.height.saturating_sub(2),
                        }
                    } else {
                        area
                    };
                    let area = if phase == 4 {
                        Rect {
                            width: area.width / 2,
                            ..area
                        }
                    } else {
                        area
                    };
                    images::render_image_widget_fit_stable(
                        hash,
                        area,
                        frame.buffer_mut(),
                        area.width,
                        area.height,
                        0,
                        false,
                        false,
                    );
                    if phase == 4 {
                        let second = Rect {
                            x: area.x.saturating_add(area.width).saturating_add(1),
                            ..area
                        }
                        .intersection(bounds);
                        images::render_image_widget_fit_stable(
                            hash,
                            second,
                            frame.buffer_mut(),
                            area.width,
                            area.height,
                            0,
                            false,
                            false,
                        );
                    }
                    if phase == 5 && !area.is_empty() {
                        let overlay = Rect {
                            x: area.x + area.width / 3,
                            y: area.y + area.height / 3,
                            width: area.width / 2,
                            height: area.height / 2,
                        };
                        frame.render_widget(Clear, overlay);
                        frame.render_widget(
                            Paragraph::new(
                                "Overlay covers image.\nNo pixels should cover this text.",
                            ),
                            overlay,
                        );
                    }
                }
                2 | 3 => {
                    images::render_image_widget_viewport_precise(
                        hash,
                        area,
                        frame.buffer_mut(),
                        8,
                        4,
                        if phase == 3 { 1000 } else { 150 },
                        false,
                    );
                }
                _ => {
                    frame.render_widget(
                        Paragraph::new("Image removed. No graphics should remain."),
                        area,
                    );
                }
            }
            if bounds.height > 1 {
                frame.render_widget(
                    Paragraph::new(
                        "Grid colors and white lines must stay inside the image region.",
                    ),
                    Rect::new(bounds.x, bounds.bottom() - 1, bounds.width, 1),
                );
            }
            images::finish_terminal_image_frame(frame.buffer_mut());
        })?;
        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char(value @ '1'..='7') if !auto => {
                            phase = value as usize - '1' as usize
                        }
                        KeyCode::Char(' ') if !auto => phase = (phase + 1) % names.len(),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}
