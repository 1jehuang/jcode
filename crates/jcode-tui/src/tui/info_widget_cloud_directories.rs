//! Cached directory inventory for an attached cloud VM, separate from telemetry.
use super::cloud_status::CloudProbeState;
use super::info_widget::{
    InfoWidgetData,
    frame::{self, Framed},
};
use ratatui::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_ENTRIES: usize = 6;

// Truncate by terminal cells, never by bytes. Strip controls from remote paths.
fn fit(text: &str, width: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.width() <= width {
        return clean;
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for c in clean.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells > width - 1 {
            break;
        }
        result.push(c);
        used += cells;
    }
    result.push('…');
    result
}

pub(super) fn render_cloud_directories(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(info) = data.cloud_info.as_ref() else {
        return Framed::default();
    };
    let width = usize::from(inner.width);
    let mut card = Framed::default().title(frame::label("Cloud Directories"));
    let Some(snapshot) = info.directories.as_ref() else {
        let message = match info.state {
            CloudProbeState::Loading => "Loading directories…",
            _ => "Directories unavailable",
        };
        card.lines.push(Line::from(frame::dim(fit(message, width))));
        return card.footer(frame::dim("no snapshot"));
    };
    let current_path = snapshot.current_path.as_deref().unwrap_or(&info.remote_dir);
    let mut entries: Vec<_> = snapshot.entries.iter().collect();
    entries.sort_by(|a, b| {
        (b.path == current_path)
            .cmp(&(a.path == current_path))
            .then_with(|| (b.busy > 0).cmp(&(a.busy > 0)))
            .then_with(|| (b.agents > 0).cmp(&(a.agents > 0)))
            .then_with(|| a.path.cmp(&b.path))
    });
    let shown = entries.len().min(MAX_ENTRIES);
    let agents_width = entries
        .iter()
        .take(shown)
        .map(|e| e.agents.to_string().len())
        .max()
        .unwrap_or(1)
        .max(6);
    let busy_width = entries
        .iter()
        .take(shown)
        .map(|e| e.busy.to_string().len())
        .max()
        .unwrap_or(1)
        .max(4);
    let suffix_width = agents_width + busy_width + 2;
    let path_width = width.saturating_sub(suffix_width);
    // Reserve numeric columns before shortening paths. Extremely narrow cards
    // show compact counts instead of wrapping or silently clipping a count.
    let table_row = |path: &str, agents: &str, busy: &str| -> Line<'static> {
        let text = if path_width > 0 {
            let path = fit(path, path_width);
            format!(
                "{path}{} {agents:>agents_width$} {busy:>busy_width$}",
                " ".repeat(path_width.saturating_sub(path.width()))
            )
        } else {
            let counts = format!("{agents}/{busy}");
            fit(&counts, width)
        };
        Line::from(Span::styled(
            text,
            Style::default().fg(crate::tui::color_support::rgb(170, 170, 185)),
        ))
    };
    card.lines.push(if path_width > 0 {
        table_row("Directory", "Agents", "Busy")
    } else {
        Line::from(frame::dim(fit("Agents/Busy", width)))
    });
    for entry in entries.iter().take(MAX_ENTRIES) {
        let marker = if entry.path == current_path { ">" } else { " " };
        card.lines.push(table_row(
            &format!("{marker} {}", entry.path),
            &entry.agents.to_string(),
            &entry.busy.to_string(),
        ));
    }
    if entries.is_empty() {
        card.lines
            .push(Line::from(frame::dim(fit("No directories", width))));
    }
    let hidden = entries.len().saturating_sub(shown);
    let overflow = match (hidden, snapshot.truncated) {
        (0, false) => String::new(),
        (0, true) => " · more available".to_string(),
        (_, false) => format!(" · +{hidden} more"),
        (_, true) => format!(" · +{hidden} or more"),
    };
    let stale = info.directories_stale
        || info.state != CloudProbeState::Ready
        || data.cloud_connection.is_some()
        || info.last_success.is_none();
    card.footer(frame::dim(format!(
        "{}{}",
        if stale {
            "stale · > current"
        } else {
            "> current"
        },
        overflow
    )))
}

#[cfg(test)]
mod tests {
    use super::super::cloud_status::{
        CloudDirectoryEntry, CloudDirectorySnapshot, CloudStatusInfo,
    };
    use super::super::info_widget::{Margins, WidgetKind, calculate_widget_height};
    use super::*;
    use std::time::SystemTime;
    fn fixture() -> InfoWidgetData {
        InfoWidgetData {
            cloud_info: Some(CloudStatusInfo {
                host: "vm".into(),
                remote_dir: "/current".into(),
                local_dir: None,
                local_is_source: false,
                started_at: SystemTime::now(),
                last_success: Some(SystemTime::now()),
                probe_duration_ms: None,
                state: CloudProbeState::Ready,
                machine: None,
                last_transfer_at: None,
                directories_stale: false,
                directories: Some(CloudDirectorySnapshot {
                    current_path: None,
                    entries: vec![
                        CloudDirectoryEntry {
                            path: "/z-idle".into(),
                            agents: 0,
                            busy: 0,
                        },
                        CloudDirectoryEntry {
                            path: "/busy".into(),
                            agents: 123,
                            busy: 42,
                        },
                        CloudDirectoryEntry {
                            path: "/current".into(),
                            agents: 1,
                            busy: 0,
                        },
                        CloudDirectoryEntry {
                            path: "/active".into(),
                            agents: 1,
                            busy: 0,
                        },
                    ],
                    truncated: false,
                }),
            }),
            ..Default::default()
        }
    }
    fn render(data: &InfoWidgetData, width: u16) -> Framed {
        render_cloud_directories(data, Rect::new(0, 0, width, 20))
    }
    #[test]
    fn cloud_directories_local_absent_and_loading() {
        let mut data = InfoWidgetData::default();
        assert!(!data.has_data_for(WidgetKind::CloudDirectories));
        assert!(render(&data, 40).is_empty());
        data = fixture();
        data.cloud_info.as_mut().unwrap().directories = None;
        data.cloud_info.as_mut().unwrap().state = CloudProbeState::Loading;
        assert!(data.has_data_for(WidgetKind::CloudDirectories));
        assert_eq!(
            render(&data, 40).lines[0].to_string(),
            "Loading directories…"
        );
    }
    #[test]
    fn cloud_directories_order_counts_and_staleness() {
        let mut data = fixture();
        let card = render(&data, 38);
        assert!(card.lines[0].to_string().contains("Directory"));
        assert!(card.lines[1].to_string().starts_with("> /current"));
        assert!(card.lines[2].to_string().contains("/busy"));
        assert!(card.lines[2].to_string().ends_with("   123   42"));
        assert!(card.lines[3].to_string().contains("/active"));
        assert!(!card.footer.unwrap().to_string().contains("stale"));
        data.cloud_info.as_mut().unwrap().directories_stale = true;
        assert!(
            render(&data, 38)
                .footer
                .unwrap()
                .to_string()
                .contains("stale")
        );
        data.cloud_info.as_mut().unwrap().directories_stale = false;
        data.cloud_info
            .as_mut()
            .unwrap()
            .directories
            .as_mut()
            .unwrap()
            .current_path = Some("/active".into());
        assert!(
            render(&data, 38).lines[1]
                .to_string()
                .starts_with("> /active")
        );
        data.cloud_connection = Some("Reconnecting".into());
        assert!(
            render(&data, 38)
                .footer
                .unwrap()
                .to_string()
                .contains("stale")
        );
        data.cloud_connection = None;
        data.cloud_info.as_mut().unwrap().last_success = None;
        assert!(
            render(&data, 38)
                .footer
                .unwrap()
                .to_string()
                .contains("stale")
        );
    }
    #[test]
    fn cloud_directories_unicode_widths_and_bounded_overflow() {
        let mut data = fixture();
        let snapshot = data
            .cloud_info
            .as_mut()
            .unwrap()
            .directories
            .as_mut()
            .unwrap();
        snapshot.entries[1].path = "/雲雲雲/very-long\npath".into();
        for i in 0..10 {
            snapshot.entries.push(CloudDirectoryEntry {
                path: format!("/extra-{i}"),
                ..Default::default()
            });
        }
        for width in 0..=60 {
            let card = render(&data, width);
            assert_eq!(card.lines.len(), 7);
            assert!(card.lines.iter().all(|l| l.width() <= usize::from(width)));
            assert!(
                card.lines
                    .iter()
                    .all(|l| !l.to_string().chars().any(char::is_control))
            );
            if width >= 13 {
                assert!(card.lines[2].to_string().ends_with("   123   42"));
            }
            assert_eq!(
                calculate_widget_height(WidgetKind::CloudDirectories, &data, width + 2, 40),
                9
            );
        }
        assert!(
            render(&data, 38)
                .footer
                .unwrap()
                .to_string()
                .contains("+8 more")
        );
        data.cloud_info
            .as_mut()
            .unwrap()
            .directories
            .as_mut()
            .unwrap()
            .truncated = true;
        assert!(
            render(&data, 38)
                .footer
                .unwrap()
                .to_string()
                .contains("+8 or more")
        );
    }
    #[test]
    fn cloud_directories_empty_and_layout() {
        let mut data = fixture();
        data.cloud_info.as_mut().unwrap().directories = Some(CloudDirectorySnapshot::default());
        assert_eq!(render(&data, 38).lines[1].to_string(), "No directories");
        let margins = Margins {
            left_widths: vec![40; 60],
            right_widths: vec![40; 60],
            centered: true,
            ..Default::default()
        };
        let result = super::super::info_widget_layout::calculate_placements_anchored(
            Rect::new(0, 0, 180, 60),
            &margins,
            &data,
            true,
            &[],
        );
        let card = result
            .visible
            .iter()
            .find(|p| p.kind == WidgetKind::CloudDirectories)
            .expect("separate directory card fits");
        assert_eq!(card.rect.height, 4);
        assert!(result.visible.iter().any(|p| p.kind == WidgetKind::CloudVm));
    }
}
