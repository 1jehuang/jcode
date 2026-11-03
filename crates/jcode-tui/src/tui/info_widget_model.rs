//! Runtime widget: the detail layer behind the status line's model segment.
//!
//! The overscroll status line owns the identity facts: model, reasoning
//! effort, provider, auth method, directory, and branch. This widget never
//! repeats those. It shows only the runtime facts the line has no room for:
//! the OpenAI service tier, the upstream route, the transport, live
//! throughput, and which session/swarm this is.

use super::InfoWidgetData;
use super::frame::{self, Framed};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;

/// Content rows the Runtime widget renders, mirroring [`render_model_widget`].
#[cfg(test)]
pub(super) fn runtime_height(data: &InfoWidgetData) -> u16 {
    runtime_frame_rows(data).0.len() as u16
}

/// Content rows in the unframed overview, which retains every runtime fact.
pub(super) fn overview_runtime_height(data: &InfoWidgetData) -> u16 {
    runtime_rows(data).len() as u16
}

pub(super) fn runtime_has_data(data: &InfoWidgetData) -> bool {
    !runtime_rows(data).is_empty()
}

/// Keep a distinct runtime fact on the border without duplicating body facts.
pub(super) fn render_model_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let max_len = inner.width as usize;
    let (rows, footer) = runtime_frame_rows(data);
    let lines = rows.into_iter().map(|row| row.into_line(max_len)).collect();
    Framed::body(lines)
        .title(frame::label("Runtime"))
        .footer_right(footer.into_line(max_len))
}

/// Overview section: every runtime fact as a row, no border.
pub(super) fn render_model_info(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let max_len = inner.width.saturating_sub(2) as usize;
    runtime_rows(data)
        .into_iter()
        .map(|row| row.into_line(max_len))
        .collect()
}

const TPS_ICON: &str = "⏱";

/// Only move a fact when another nonempty fact remains, never a startup spacer.
/// The overview still uses the complete, unchanged runtime row list.
fn runtime_frame_rows(data: &InfoWidgetData) -> (Vec<RuntimeRow>, RuntimeRow) {
    let mut rows = runtime_rows(data);
    if rows.iter().filter(|row| !row.text.is_empty()).count() > 1 {
        let index = [TPS_ICON, "↔", "⚡", "◆", "☁"]
            .iter()
            .find_map(|icon| rows.iter().position(|row| row.icon == *icon));
        if let Some(index) = index {
            let footer = rows.remove(index);
            return (rows, footer);
        }
    }
    let has_tps = rows.iter().any(|row| row.icon == TPS_ICON);
    let footer = RuntimeRow {
        icon: if has_tps { "↔" } else { TPS_ICON },
        icon_color: rgb(140, 180, 255),
        text: if has_tps {
            "transport n/a".into()
        } else if data.startup_runtime_pending {
            "tok/s pending".into()
        } else {
            "tok/s n/a".into()
        },
        text_color: rgb(140, 140, 150),
    };
    (rows, footer)
}

struct RuntimeRow {
    icon: &'static str,
    icon_color: Color,
    text: String,
    text_color: Color,
}

impl RuntimeRow {
    fn into_line(self, max_len: usize) -> Line<'static> {
        let line = if self.icon.is_empty() {
            Line::from(Span::styled(
                self.text,
                Style::default().fg(self.text_color),
            ))
        } else {
            Line::from(vec![
                Span::styled(
                    format!("{} ", self.icon),
                    Style::default().fg(self.icon_color),
                ),
                Span::styled(self.text, Style::default().fg(self.text_color)),
            ])
        };
        jcode_tui_render::truncate_line_with_ellipsis_to_width(&line, max_len)
    }
}

fn runtime_rows(data: &InfoWidgetData) -> Vec<RuntimeRow> {
    let accent = rgb(140, 180, 255);
    let muted = rgb(140, 140, 150);
    let mut rows = Vec::new();

    let is_openai = data
        .provider_name
        .as_deref()
        .is_some_and(|provider| provider.trim().to_ascii_lowercase().starts_with("openai"));
    if is_openai && let Some(tier) = data.service_tier.as_deref().and_then(short_service_tier) {
        rows.push(RuntimeRow {
            icon: "⚡",
            icon_color: rgb(200, 140, 255),
            text: if tier.contains(" ← ") || tier.ends_with(" ?") {
                tier.to_string()
            } else {
                format!("{tier} tier")
            },
            text_color: rgb(200, 140, 255),
        });
    }

    if let Some(upstream) = non_empty(data.upstream_provider.as_deref()) {
        rows.push(RuntimeRow {
            icon: "☁",
            icon_color: accent,
            text: format!("via {upstream}"),
            text_color: rgb(220, 190, 120),
        });
    }

    if let Some(connection) = non_empty(data.connection_type.as_deref()) {
        rows.push(RuntimeRow {
            icon: "↔",
            icon_color: accent,
            text: connection.to_lowercase(),
            text_color: muted,
        });
    }

    if let Some(tps) = data.tokens_per_second
        && tps.is_finite()
        && tps > 0.1
    {
        rows.push(RuntimeRow {
            icon: TPS_ICON,
            icon_color: accent,
            text: format!("{tps:.0} tok/s"),
            text_color: muted,
        });
    }

    let mut session_parts = Vec::new();
    if let Some(name) = non_empty(data.session_name.as_deref()) {
        session_parts.push(name.to_string());
    }
    if let Some(sessions) = data.session_count.filter(|n| *n > 1) {
        session_parts.push(format!("{sessions} sessions"));
    }
    if !session_parts.is_empty() {
        rows.push(RuntimeRow {
            icon: "◆",
            icon_color: accent,
            text: session_parts.join(" · "),
            text_color: muted,
        });
    }

    // The initial remote snapshot normally supplies the tier and session rows.
    // Keep two transient slots so applying it does not push Changes down. Never
    // guess either value, and never retain slots after the owner clears pending.
    if data.startup_runtime_pending {
        while rows.len() < 2 {
            rows.push(RuntimeRow {
                icon: "",
                icon_color: muted,
                text: if rows.is_empty() {
                    "Loading…".into()
                } else {
                    String::new()
                },
                text_color: muted,
            });
        }
    }

    rows
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn short_service_tier(service_tier: &str) -> Option<&str> {
    let service_tier = service_tier.trim();
    if service_tier.is_empty() || service_tier == "off" || service_tier == "default" {
        return None;
    }
    Some(match service_tier {
        "priority" => "fast",
        "ultrafast" => "ultrafast",
        "flex" => "flex",
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::info_widget::{AuthMethod, InfoWidgetData};

    fn data() -> InfoWidgetData {
        InfoWidgetData {
            model: Some("gpt-5-codex".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            provider_name: Some("openai".to_string()),
            auth_method: AuthMethod::OpenAIOAuth,
            working_dir: Some("/home/me/jcode".to_string()),
            ..Default::default()
        }
    }

    fn text(lines: Vec<Line<'static>>) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn runtime_widget_never_repeats_status_line_identity() {
        let mut d = data();
        d.connection_type = Some("websocket".to_string());
        d.tokens_per_second = Some(61.7);
        let out = text(render_model_widget(&d, Rect::new(0, 0, 30, 8)).all_lines());
        for owned in ["GPT", "codex", "high", "(hi)", "openai", "OAuth", "jcode"] {
            assert!(
                !out.contains(owned),
                "{owned:?} belongs to the status line: {out}"
            );
        }
        assert!(out.contains("fast tier"), "{out}");
        assert!(out.contains("websocket"), "{out}");
        assert!(out.contains("62 tok/s"), "{out}");
    }

    #[test]
    fn compact_tier_reports_keep_reported_tier_visible() {
        let mut d = data();
        for status in ["default ← Ultrafast", "Ultrafast ?"] {
            d.service_tier = Some(status.into());
            let out = text(render_model_info(&d, Rect::new(0, 0, 34, 8)));
            assert!(out.contains(status), "{out}");
            assert!(!out.contains(" tier"), "{out}");
            let framed = text(render_model_widget(&d, Rect::new(0, 0, 38, 8)).all_lines());
            assert!(framed.contains(status), "{framed}");
            for width in [18, 24, 34] {
                let lines = render_model_info(&d, Rect::new(0, 0, width, 8));
                for line in &lines {
                    assert!(line.width() <= width as usize, "{line:?}");
                }
                if status.starts_with("default") {
                    assert!(text(lines).contains("default"));
                }
            }
        }
    }

    #[test]
    fn service_tier_only_for_openai() {
        let mut d = data();
        d.provider_name = Some("deepseek".to_string());
        assert!(
            !text(render_model_widget(&d, Rect::new(0, 0, 30, 8)).all_lines()).contains("tier")
        );
        for tier in [None, Some("off"), Some("default")] {
            let mut d = data();
            d.service_tier = tier.map(str::to_string);
            assert!(!runtime_has_data(&d), "tier {tier:?}");
        }
    }

    #[test]
    fn identity_only_session_has_no_runtime_widget() {
        let mut d = data();
        d.service_tier = None;
        assert!(!runtime_has_data(&d));
        assert_eq!(runtime_height(&d), 0);
    }

    #[test]
    fn footer_moves_facts_once_and_preserves_overview() {
        for (connection, tps, session, expected) in [
            (Some("websocket"), None, None, "websocket"),
            (None, None, Some("sauropod"), "fast tier"),
            (Some("websocket"), Some(61.7), Some("sauropod"), "62 tok/s"),
        ] {
            let mut d = data();
            d.connection_type = connection.map(str::to_string);
            d.tokens_per_second = tps;
            d.session_name = session.map(str::to_string);
            let overview = text(render_model_info(&d, Rect::new(0, 0, 80, 8)));
            let (rows, footer) = runtime_frame_rows(&d);
            assert_eq!(footer.text, expected);
            assert!(!rows.is_empty());
            assert!(rows.iter().all(|row| row.text != expected));
            let framed = render_model_widget(&d, Rect::new(0, 0, 80, 8));
            assert_eq!(framed.lines.len() as u16, runtime_height(&d));
            assert_eq!(text(framed.all_lines()).matches(expected).count(), 1);
            assert_eq!(overview.matches(expected).count(), 1);
        }
    }

    #[test]
    fn single_facts_keep_body_and_use_distinct_unavailable_metric() {
        for icon in ["⚡", "↔", "☁", "◆", TPS_ICON] {
            let mut d = data();
            d.service_tier = (icon == "⚡").then(|| "priority".into());
            d.connection_type = (icon == "↔").then(|| "websocket".into());
            d.upstream_provider = (icon == "☁").then(|| "fireworks".into());
            d.session_name = (icon == "◆").then(|| "sauropod".into());
            d.tokens_per_second = (icon == TPS_ICON).then_some(61.7);
            let (rows, footer) = runtime_frame_rows(&d);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].icon, icon);
            assert_ne!(footer.icon, icon);
            assert!(footer.text.ends_with("n/a"));
            assert_eq!(runtime_height(&d), 1);
        }
    }

    #[test]
    fn startup_slots_keep_height_and_report_pending_throughput() {
        for tier in [None, Some("priority")] {
            let mut d = data();
            d.service_tier = tier.map(str::to_string);
            d.startup_runtime_pending = true;
            let (rows, footer) = runtime_frame_rows(&d);
            assert_eq!(rows.len(), 2);
            assert_eq!(runtime_height(&d), 2);
            assert_eq!(footer.text, "tok/s pending");
            assert!(rows.iter().all(|row| row.text != footer.text));
        }
    }

    #[test]
    fn invalid_throughput_is_unavailable_not_a_live_metric() {
        for tps in [0.0, 0.1, -1.0, f32::NAN, f32::INFINITY] {
            let mut d = data();
            d.tokens_per_second = Some(tps);
            let (rows, footer) = runtime_frame_rows(&d);
            assert_eq!(rows.len(), 1);
            assert_eq!(footer.text, "tok/s n/a");
        }
    }

    #[test]
    fn overview_height_counts_all_facts_including_moved_footer() {
        let mut d = data();
        d.connection_type = Some("websocket".into());
        d.tokens_per_second = Some(61.7);
        d.session_name = Some("sauropod".into());
        let overview = render_model_info(&d, Rect::new(0, 0, 80, 8));
        assert_eq!(overview.len() as u16, overview_runtime_height(&d));
        assert_eq!(overview_runtime_height(&d), runtime_height(&d) + 1);
        d.startup_runtime_pending = true;
        d.service_tier = None;
        d.connection_type = None;
        d.tokens_per_second = None;
        d.session_name = None;
        assert_eq!(
            render_model_info(&d, Rect::new(0, 0, 80, 8)).len() as u16,
            overview_runtime_height(&d)
        );
        assert_eq!(overview_runtime_height(&d), 2);
    }

    #[test]
    fn height_matches_rendered_rows() {
        let mut d = data();
        d.upstream_provider = Some("fireworks".to_string());
        d.session_name = Some("sauropod".to_string());
        d.session_count = Some(3);
        let framed = render_model_widget(&d, Rect::new(0, 0, 30, 8));
        assert_eq!(framed.lines.len() as u16, runtime_height(&d));
        assert!(text(framed.all_lines()).contains("sauropod · 3 sessions"));
    }
}
