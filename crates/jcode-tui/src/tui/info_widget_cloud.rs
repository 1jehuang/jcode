//! Cloud-only details. Probe timing is not network RTT, and retained samples are
//! always marked stale after a failed probe or while reconnecting.
use super::cloud_status::{CloudProbeState, CloudStatusInfo};
use super::info_widget::{
    InfoWidgetData,
    frame::{self, Framed},
    render_usage_limit_bar,
};
use ratatui::prelude::*;
use std::time::SystemTime;
use unicode_width::UnicodeWidthChar;

fn fit(line: Line<'static>, width: u16) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let truncated = line.width() > usize::from(width);
    let mut left = usize::from(width).saturating_sub(usize::from(truncated));
    let mut spans = Vec::new();
    for span in line.spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let cells = ch.width().unwrap_or(0);
            if cells > left {
                break;
            }
            text.push(ch);
            left -= cells;
        }
        spans.push(Span::styled(text, span.style));
        if left == 0 {
            break;
        }
    }
    if truncated {
        spans.push(Span::raw("…"));
    }
    Line::from(spans)
}
fn clean(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}
fn row(text: impl Into<String>, width: u16) -> Line<'static> {
    fit(Line::from(clean(&text.into())), width)
}
fn value_style() -> Style {
    Style::default().fg(crate::tui::color_support::rgb(210, 210, 220))
}
/// Dim label, bright value. `label` should include its trailing separator.
fn kv(label: &str, value: impl Into<String>, width: u16) -> Line<'static> {
    fit(
        Line::from(vec![
            frame::dim(clean(label)),
            Span::styled(clean(&value.into()), value_style()),
        ]),
        width,
    )
}
fn state_style(state: &str) -> Style {
    use crate::tui::color_support::rgb;
    match state {
        "Ready" => Style::default().fg(rgb(110, 200, 120)),
        "Loading" | "Reconnecting" => Style::default().fg(rgb(230, 190, 90)),
        _ => Style::default().fg(rgb(230, 100, 100)),
    }
}
/// Show RFC 3339 transfer stamps as local `YYYY-MM-DD HH:MM`.
fn transfer_time(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| raw.to_string())
}
fn path_row(prefix: &str, path: &str, width: u16) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let path = clean(path);
    let full = format!("{prefix}{path}");
    let budget = usize::from(width).saturating_sub(prefix.width() + 1);
    if full.width() <= usize::from(width) || budget == 0 {
        return kv(prefix, path, width);
    }
    let mut suffix = Vec::new();
    let mut cells = 0;
    for ch in path.chars().rev() {
        cells += ch.width().unwrap_or(0);
        if cells > budget {
            break;
        }
        suffix.push(ch);
    }
    kv(
        prefix,
        format!("…{}", suffix.into_iter().rev().collect::<String>()),
        width,
    )
}
fn duration(seconds: u64) -> String {
    if seconds >= 86400 {
        format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600)
    } else if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}
fn elapsed(time: SystemTime) -> String {
    SystemTime::now()
        .duration_since(time)
        .map(|d| duration(d.as_secs()))
        .unwrap_or_else(|_| "unknown".into())
}
fn status<'a>(data: &'a InfoWidgetData, info: &CloudStatusInfo) -> &'a str {
    if let Some(state) = data.cloud_connection.as_deref() {
        return state;
    }
    match info.state {
        CloudProbeState::Loading => "Loading",
        CloudProbeState::Ready => "Ready",
        CloudProbeState::Unavailable => "Unavailable",
    }
}
fn stale(data: &InfoWidgetData, info: &CloudStatusInfo) -> bool {
    info.state != CloudProbeState::Ready || data.cloud_connection.is_some()
}
fn resource(label: &str, used: Option<f64>, width: u16, display_used: bool) -> Line<'static> {
    match used.filter(|v| v.is_finite()) {
        Some(value) => {
            let pct = value.round().clamp(0.0, 100.0) as u8;
            fit(
                render_usage_limit_bar(label, pct, 100 - pct, None, width, display_used),
                width,
            )
        }
        None => kv(&format!("{label}: "), "unavailable", width),
    }
}
fn bytes(value: Option<u64>) -> String {
    value
        .map(|v| format!("{:.0}", v as f64 / 1_073_741_824.0))
        .unwrap_or_else(|| "unknown".into())
}
fn used_ratio(total: Option<u64>, available: Option<u64>) -> Option<f64> {
    let total = total.filter(|v| *v > 0)?;
    let available = available?;
    if available > total {
        return None;
    }
    Some((total - available) as f64 / total as f64 * 100.0)
}

pub(super) fn render_cloud_summary(data: &InfoWidgetData, width: u16) -> Vec<Line<'static>> {
    let Some(info) = &data.cloud_info else {
        return Vec::new();
    };
    vec![row(
        format!(
            "Cloud: {}{}",
            status(data, info),
            if stale(data, info) && info.machine.is_some() {
                " · stale resources"
            } else {
                ""
            }
        ),
        width,
    )]
}

pub(super) fn render_cloud_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(info) = &data.cloud_info else {
        return Framed::default();
    };
    let w = inner.width;
    let machine = info.machine.as_ref();
    let host = machine
        .and_then(|m| m.hostname.as_deref())
        .unwrap_or(&info.host);
    let cores = machine
        .and_then(|m| m.cpu_count)
        .map(|n| format!(" · {n} CPU"))
        .unwrap_or_default();
    let stale_note = if stale(data, info) && machine.is_some() {
        " · stale"
    } else {
        ""
    };
    let mut lines = vec![kv(
        &format!("Machine{stale_note}: "),
        format!("{host}{cores}"),
        w,
    )];
    lines.push(resource(
        "CPU",
        machine.and_then(|m| m.cpu_percent),
        w,
        data.usage_display_used,
    ));
    lines.push(resource(
        "RAM",
        machine.and_then(|m| used_ratio(m.memory_total_bytes, m.memory_available_bytes)),
        w,
        data.usage_display_used,
    ));
    lines.push(resource(
        "Disk",
        machine.and_then(|m| used_ratio(m.disk_total_bytes, m.disk_available_bytes)),
        w,
        data.usage_display_used,
    ));
    let total = machine.and_then(|m| m.memory_total_bytes);
    let used = total
        .zip(machine.and_then(|m| m.memory_available_bytes))
        .and_then(|(t, a)| t.checked_sub(a));
    lines.push(fit(
        Line::from(vec![
            frame::dim("RAM "),
            Span::styled(
                format!("{}/{} GiB", bytes(used), bytes(total)),
                value_style(),
            ),
            frame::dim(" · disk "),
            Span::styled(
                format!(
                    "{} GiB",
                    bytes(machine.and_then(|m| m.disk_available_bytes))
                ),
                value_style(),
            ),
            frame::dim(" free"),
        ]),
        w,
    ));
    let state = status(data, info).to_string();
    lines.push(fit(
        Line::from(vec![
            frame::dim("Connection: "),
            Span::styled(clean(&state), state_style(&state)),
        ]),
        w,
    ));
    let age = info
        .last_success
        .map(|t| format!("{} ago", elapsed(t)))
        .unwrap_or_else(|| "never".into());
    let probe = info
        .probe_duration_ms
        .map(|v| format!("{v}ms"))
        .unwrap_or_else(|| "unavailable".into());
    lines.push(fit(
        Line::from(vec![
            frame::dim("Updated "),
            Span::styled(age, value_style()),
            frame::dim(" · probe "),
            Span::styled(probe, value_style()),
        ]),
        w,
    ));
    lines.push(kv(
        "Session: ",
        if data.cloud_working {
            "Working"
        } else {
            "Idle"
        },
        w,
    ));
    let uptime = machine
        .and_then(|m| m.uptime_secs)
        .map(duration)
        .unwrap_or_else(|| "unavailable".into());
    lines.push(fit(
        Line::from(vec![
            frame::dim("Uptime "),
            Span::styled(uptime, value_style()),
            frame::dim(" · attached "),
            Span::styled(elapsed(info.started_at), value_style()),
        ]),
        w,
    ));
    lines.push(kv("Sleep: ", "policy unavailable", w));
    lines.push(fit(Line::from(frame::label("Workspace")), w));
    lines.push(path_row("Cloud: ", &info.remote_dir, w));
    lines.push(path_row(
        if info.local_is_source {
            "Local (source): "
        } else {
            "Local: "
        },
        info.local_dir.as_deref().unwrap_or("unavailable"),
        w,
    ));
    lines.push(kv(
        "Transferred: ",
        info.last_transfer_at
            .as_deref()
            .map(transfer_time)
            .unwrap_or_else(|| "unknown".into()),
        w,
    ));
    Framed::body(lines).title(frame::label("Cloud VM"))
}

#[cfg(test)]
mod tests {
    use super::super::{
        cloud_status::CloudMachineStats,
        info_widget::{WidgetKind, calculate_widget_height},
        info_widget_overview::compute_page_layout,
    };
    use super::*;
    fn fixture() -> InfoWidgetData {
        InfoWidgetData {
            cloud_info: Some(CloudStatusInfo {
                host: "雲-vm".into(),
                remote_dir: "/home/雲/project".into(),
                local_dir: Some("/home/local/project".into()),
                local_is_source: true,
                started_at: SystemTime::now(),
                last_success: Some(SystemTime::now()),
                probe_duration_ms: Some(120),
                state: CloudProbeState::Ready,
                machine: Some(CloudMachineStats {
                    hostname: Some("雲-vm".into()),
                    cpu_percent: Some(25.0),
                    cpu_count: Some(4),
                    memory_total_bytes: Some(100),
                    memory_available_bytes: Some(60),
                    disk_total_bytes: Some(200),
                    disk_available_bytes: Some(100),
                    uptime_secs: Some(3600),
                }),
                last_transfer_at: Some("2026-10-20T07:00:00Z".into()),
            }),
            ..Default::default()
        }
    }
    fn text(data: &InfoWidgetData) -> String {
        render_cloud_widget(data, Rect::new(0, 0, 100, 20))
            .all_lines()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn cloud_local_absent() {
        let data = InfoWidgetData::default();
        assert!(!data.has_data_for(WidgetKind::CloudVm));
        assert!(render_cloud_widget(&data, Rect::default()).is_empty());
        assert!(render_cloud_summary(&data, 60).is_empty());
    }
    #[test]
    fn cloud_populated_and_used_convention() {
        let mut data = fixture();
        let output = text(&data);
        for expected in [
            "Cloud VM",
            "Machine",
            "CPU",
            "RAM",
            "Disk",
            "75% left",
            "Connection: Ready",
            "probe 120ms",
            "Uptime 1h",
            "attached",
            "Sleep: policy unavailable",
            "/home/雲/project",
            "Local (source)",
            "Transferred: 2026",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
        data.usage_display_used = true;
        assert!(text(&data).contains("25% used"));
    }
    #[test]
    fn cloud_loading_error_and_connection_override() {
        let mut data = fixture();
        data.cloud_info.as_mut().unwrap().state = CloudProbeState::Unavailable;
        assert!(text(&data).contains("stale"));
        assert!(text(&data).contains("75% left"));
        data.cloud_connection = Some("Reconnecting".into());
        data.cloud_info.as_mut().unwrap().state = CloudProbeState::Ready;
        assert!(text(&data).contains("Connection: Reconnecting"));
        assert!(text(&data).contains("stale"));
        data.cloud_connection = None;
        let info = data.cloud_info.as_mut().unwrap();
        info.state = CloudProbeState::Loading;
        info.machine = None;
        info.last_success = None;
        info.probe_duration_ms = None;
        let output = text(&data);
        assert!(output.contains("CPU: unavailable"));
        assert!(output.contains("Connection: Loading"));
        assert!(output.contains("Updated never"));
        assert!(!output.contains("0%"));
    }
    #[test]
    fn cloud_controls_invalid_metrics_and_path_tail() {
        let mut data = fixture();
        let info = data.cloud_info.as_mut().unwrap();
        info.remote_dir = "/very/long/雲/directory/project-tail".into();
        info.local_dir = Some("/tmp/line\nbreak\t\u{1b}[31m".into());
        info.last_transfer_at = Some("today\r\nforged".into());
        let machine = info.machine.as_mut().unwrap();
        machine.memory_available_bytes = Some(200);
        machine.cpu_percent = Some(f64::NAN);
        let rendered = render_cloud_widget(&data, Rect::new(0, 0, 30, 20));
        for line in &rendered.lines {
            assert!(!line.to_string().chars().any(char::is_control));
            assert!(line.width() <= 30);
        }
        let output = text(&data);
        assert!(output.contains("CPU: unavailable"));
        assert!(output.contains("RAM: unavailable"));
        assert!(output.contains("RAM unknown/"));
        assert!(
            rendered
                .lines
                .iter()
                .any(|l| l.to_string().ends_with("project-tail"))
        );
        assert_eq!(used_ratio(Some(100), Some(101)), None);
        assert_eq!(used_ratio(Some(0), Some(0)), None);
    }
    #[test]
    fn cloud_layout_preserves_dual_git_and_full_card() {
        use super::super::info_widget::{DualGitInfo, GitInfo, Margins, RepoState};
        use super::super::info_widget_layout::calculate_placements_anchored;
        let mut data = fixture();
        data.dual_git_info = Some(DualGitInfo {
            local_is_source: true,
            local: RepoState::Ready(GitInfo::default()),
            cloud: RepoState::Ready(GitInfo::default()),
        });
        let margins = Margins {
            left_widths: vec![40; 55],
            right_widths: vec![40; 55],
            centered: true,
            ..Default::default()
        };
        let result =
            calculate_placements_anchored(Rect::new(0, 0, 180, 55), &margins, &data, true, &[]);
        assert!(
            result
                .visible
                .iter()
                .any(|p| p.kind == WidgetKind::Overview || p.kind == WidgetKind::GitStatus)
        );
        let cloud = result
            .visible
            .iter()
            .find(|p| p.kind == WidgetKind::CloudVm)
            .expect("cloud card fits beside dual Git");
        assert_eq!(cloud.rect.height, 16);
        assert_eq!(
            compute_page_layout(&data, 40, 55).max_page_height,
            super::super::info_widget::changes_section_height(&data) + 1
        );
    }
    #[test]
    fn cloud_capacity_fits_default_width() {
        let mut data = fixture();
        let m = data.cloud_info.as_mut().unwrap().machine.as_mut().unwrap();
        let gib = 1_073_741_824;
        m.memory_total_bytes = Some(62 * gib);
        m.memory_available_bytes = Some(42 * gib);
        m.disk_available_bytes = Some(290 * gib);
        let card = render_cloud_widget(&data, Rect::new(0, 0, 38, 20));
        assert_eq!(
            card.lines[4].to_string(),
            "RAM 20/62 GiB · disk 290 GiB free"
        );
    }
    #[test]
    fn cloud_widths_and_layout_match() {
        let data = fixture();
        for width in 0..=60 {
            let framed = render_cloud_widget(&data, Rect::new(0, 0, width, 20));
            assert_eq!(framed.lines.len(), 14);
            assert!(
                framed
                    .lines
                    .iter()
                    .all(|line| line.width() <= usize::from(width)),
                "width {width}"
            );
            assert_eq!(
                calculate_widget_height(WidgetKind::CloudVm, &data, width + 2, 30),
                16
            );
            let summary = render_cloud_summary(&data, width);
            assert!(
                summary
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
            assert_eq!(
                compute_page_layout(&data, width.into(), 1).max_page_height,
                1
            );
        }
        assert!(WidgetKind::GitStatus.priority() < WidgetKind::CloudVm.priority());
    }
}
