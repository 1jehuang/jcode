//! Adapter from swarm member status into the inline gallery layout.
//!
//! All presentation logic (status colors, role glyphs, age formatting, header,
//! sorting, layout config) lives in the shared
//! [`jcode_tui_render::swarm_gallery`] module so the live TUI and the
//! `swarm_gallery_live` demo render identically. This adapter only handles
//! turning a [`SwarmMemberStatus`] into a renderer-agnostic
//! [`GalleryMember`] (label + body lines).

use crate::protocol::SwarmMemberStatus;
use jcode_tui_render::swarm_gallery::{
    GalleryMember, SwarmStripHint, humanize_age, render_gallery, render_swarm_panel,
    render_swarm_strip,
};
use ratatui::prelude::*;

fn member_label(member: &SwarmMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id.chars().take(8).collect())
}

/// Build the body lines shown inside a member's viewport. Prefers live streamed
/// output (the tail) when present; otherwise surfaces the latest detail plus a
/// status-age hint.
fn member_body(member: &SwarmMemberStatus) -> Vec<String> {
    // Live streamed output wins: show the worker's in-progress assistant text.
    if let Some(tail) = member.output_tail.as_ref().filter(|t| !t.trim().is_empty()) {
        let mut body: Vec<String> = tail.lines().map(|l| l.to_string()).collect();
        if let Some(age) = member.status_age_secs {
            body.push(format!("· {} ago", humanize_age(age)));
        }
        return body;
    }
    let mut body: Vec<String> = Vec::new();
    if let Some(detail) = member.detail.as_ref().filter(|d| !d.trim().is_empty()) {
        body.push(detail.clone());
    }
    if let Some(age) = member.status_age_secs {
        body.push(format!("· {} ago", humanize_age(age)));
    }
    body
}

/// Convert swarm members into renderer-agnostic gallery members.
fn members_to_gallery(members: &[SwarmMemberStatus]) -> Vec<GalleryMember> {
    members
        .iter()
        .map(|member| GalleryMember {
            label: member_label(member),
            status: member.status.clone(),
            role: member.role.clone(),
            body: member_body(member),
            sort_key: member.session_id.clone(),
        })
        .collect()
}

/// Render the inline swarm gallery for the given members into `area`-width lines.
#[allow(dead_code)]
pub(crate) fn render_swarm_gallery_lines(
    members: &[SwarmMemberStatus],
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_gallery(&members_to_gallery(members), width, max_height)
}

/// Render the list+detail swarm panel: a compact list of managed agents plus a
/// detail viewport for the `selected` one. `focused` adds an interaction hint.
#[allow(dead_code)]
pub(crate) fn render_swarm_panel_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    focused: bool,
    width: usize,
    max_height: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    render_swarm_panel(
        &members_to_gallery(members),
        selected,
        focused,
        width,
        max_height,
    )
}

/// Render the compact swarm strip (agent chips + optional keybinding hint line)
/// shown directly above the status line. `focus_key` is the configured chord to
/// focus the panel (e.g. "alt+w"), shown as the first hint.
pub(crate) fn render_swarm_strip_lines(
    members: &[SwarmMemberStatus],
    selected: usize,
    focused: bool,
    focus_key: &str,
    width: usize,
) -> Vec<Line<'static>> {
    if members.is_empty() {
        return Vec::new();
    }
    let hints = vec![
        SwarmStripHint {
            key: focus_key.to_string(),
            label: if focused {
                "unfocus".into()
            } else {
                "focus".into()
            },
        },
        SwarmStripHint {
            key: "j/k".into(),
            label: "select".into(),
        },
        SwarmStripHint {
            key: "o".into(),
            label: "pop out".into(),
        },
        SwarmStripHint {
            key: "enter".into(),
            label: "open".into(),
        },
        SwarmStripHint {
            key: "esc".into(),
            label: "back".into(),
        },
    ];
    render_swarm_strip(
        &members_to_gallery(members),
        selected,
        focused,
        &hints,
        width,
    )
}

/// Session ids of `members` in the same order the panel/gallery displays them
/// (coordinator first, then worktree manager, then by session id). Lets the TUI
/// map a selected panel index back to a concrete session for pop-out.
pub(crate) fn members_display_order(members: &[SwarmMemberStatus]) -> Vec<String> {
    fn role_rank(role: Option<&str>) -> u8 {
        match role {
            Some("coordinator") => 0,
            Some("worktree_manager") => 1,
            _ => 2,
        }
    }
    let mut idx: Vec<&SwarmMemberStatus> = members.iter().collect();
    idx.sort_by(|a, b| {
        role_rank(a.role.as_deref())
            .cmp(&role_rank(b.role.as_deref()))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    idx.into_iter().map(|m| m.session_id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tui_render::swarm_gallery::members_to_tiles;

    fn member(
        id: &str,
        status: &str,
        detail: Option<&str>,
        role: Option<&str>,
    ) -> SwarmMemberStatus {
        SwarmMemberStatus {
            session_id: id.to_string(),
            friendly_name: Some(id.to_string()),
            status: status.to_string(),
            detail: detail.map(str::to_string),
            role: role.map(str::to_string),
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(3),
            output_tail: None,
            report_back_to_session_id: None,
        }
    }

    #[test]
    fn coordinator_sorts_first() {
        let members = vec![
            member("zeta", "running", None, None),
            member("alpha", "running", None, Some("coordinator")),
        ];
        let tiles = members_to_tiles(&members_to_gallery(&members));
        assert_eq!(tiles[0].title, "alpha");
        assert_eq!(tiles[0].role_glyph.as_deref(), Some("★"));
    }

    #[test]
    fn renders_header_and_boxes() {
        let members = vec![
            member("alpha", "running", Some("editing config.rs"), None),
            member("beta", "done", Some("reviewed"), None),
        ];
        let lines = render_swarm_gallery_lines(&members, 80, 12);
        assert!(!lines.is_empty());
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.contains("swarm · 2 agents"), "got: {header}");
        for line in &lines {
            assert!(line.width() <= 80);
        }
    }

    #[test]
    fn empty_members_render_nothing() {
        assert!(render_swarm_gallery_lines(&[], 80, 12).is_empty());
    }

    #[test]
    fn output_tail_takes_priority_over_detail() {
        let mut m = member("alpha", "running", Some("the detail line"), None);
        m.output_tail = Some("line one\nline two".to_string());
        let body = member_body(&m);
        assert_eq!(body[0], "line one");
        assert_eq!(body[1], "line two");
        assert!(!body.iter().any(|l| l.contains("the detail line")));
    }
}
