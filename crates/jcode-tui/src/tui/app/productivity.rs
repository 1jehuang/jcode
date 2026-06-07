//! `/productivity` command: scan local transcripts, render a shareable usage
//! report (markdown + PNG dashboard), copy the dashboard image to the clipboard,
//! and open it in the default image viewer.
//!
//! Generation runs off the UI thread; the result is delivered back via
//! [`BusEvent::ProductivityReportReady`].

use super::*;
use crate::bus::{Bus, BusEvent, ProductivityReportPayload, ProductivityReportReady};
use std::io::Write;
use std::path::Path;

/// Handle `/productivity` (aliases: `/wrapped`, `/stats`).
pub(super) fn handle_productivity_command(app: &mut App, trimmed: &str) -> bool {
    let is_match = matches!(
        trimmed,
        "/productivity" | "/wrapped" | "/stats" | "/productivity report"
    );
    if !is_match {
        return false;
    }

    if app.productivity_refreshing {
        app.set_status_notice("Productivity report already generating…");
        return true;
    }
    app.productivity_refreshing = true;

    app.push_display_message(DisplayMessage::system(
        "📊 Generating your productivity report… scanning transcripts (first run may take a few seconds).".to_string(),
    ));
    app.set_status_notice("Productivity → scanning");

    let session_id = app.session.id.clone();
    std::thread::spawn(move || {
        let result = jcode_productivity_core::generate()
            .map(|out| ProductivityReportPayload {
                markdown: out.markdown,
                png: out.png,
                png_path: out.png_path,
            })
            .map_err(|e| e.to_string());
        Bus::global().publish(BusEvent::ProductivityReportReady(ProductivityReportReady {
            session_id,
            result,
        }));
    });

    true
}

impl App {
    pub(super) fn handle_productivity_report_ready(&mut self, event: ProductivityReportReady) {
        if event.session_id != self.session.id {
            return;
        }
        self.productivity_refreshing = false;

        match event.result {
            Ok(payload) => {
                let copied = copy_image_to_clipboard(&payload.png_path, &payload.png);
                let opened = open::that_detached(&payload.png_path).is_ok();

                let mut md = payload.markdown;
                let mut footer = format!(
                    "\n\n🖼️ Dashboard saved to `{}`.",
                    payload.png_path.display()
                );
                if copied {
                    footer.push_str(" Copied to your clipboard - paste it anywhere to share.");
                } else {
                    footer.push_str(" (Could not access the clipboard; share the file directly.)");
                }
                if opened {
                    footer.push_str(" Opened in your image viewer.");
                }
                md.push_str(&footer);

                self.push_display_message(DisplayMessage::assistant(md));
                let notice = if copied {
                    "Productivity report ready · image copied to clipboard"
                } else {
                    "Productivity report ready"
                };
                self.set_status_notice(notice);
            }
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to generate productivity report: {err}"
                )));
                self.set_status_notice("Productivity report failed");
            }
        }
    }
}

/// Copy a PNG image to the system clipboard.
///
/// On Wayland we use `wl-copy -t image/png`; otherwise fall back to `xclip`,
/// then arboard (which expects raw RGBA, so we decode the PNG for it).
fn copy_image_to_clipboard(path: &Path, png: &[u8]) -> bool {
    // Wayland: wl-copy from a file is most reliable.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && copy_image_wl(png) {
        return true;
    }

    // X11: xclip selection clipboard with image/png target.
    if copy_image_xclip(path) {
        return true;
    }

    // Cross-platform fallback via arboard (needs decoded RGBA pixels).
    copy_image_arboard(png)
}

fn copy_image_wl(png: &[u8]) -> bool {
    let child = std::process::Command::new("wl-copy")
        .arg("-t")
        .arg("image/png")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut()
        && stdin.write_all(png).is_ok()
    {
        drop(child.stdin.take());
        // wl-copy forks a server and exits; treat a clean spawn+write as success.
        return child.wait().map(|s| s.success()).unwrap_or(true);
    }
    false
}

fn copy_image_xclip(path: &Path) -> bool {
    std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "image/png", "-i"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn copy_image_arboard(png: &[u8]) -> bool {
    let Ok(decoded) = image::load_from_memory(png) else {
        return false;
    };
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    let img = arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(rgba.into_raw()),
    };
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_image(img))
        .is_ok()
}
