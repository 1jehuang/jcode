use crate::model::SessionSummary;
use crate::{render_markdown, render_png, render_svg, report_from_summaries};
use std::collections::BTreeMap;

fn summary(project: &str, user: u32, asst: u32, tools: &[(&str, u32)]) -> SessionSummary {
    let mut t = BTreeMap::new();
    for (k, v) in tools {
        t.insert(k.to_string(), *v);
    }
    SessionSummary {
        project: Some(project.to_string()),
        working_dir: Some(format!("/home/u/{project}")),
        provider_key: Some("openai".to_string()),
        model: Some("gpt-5.5".to_string()),
        user_msgs: user,
        assistant_msgs: asst,
        user_chars: (user as u64) * 50,
        assistant_chars: (asst as u64) * 200,
        tools: t,
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 2000,
        active_dates: vec!["2026-06-01".to_string(), "2026-06-02".to_string()],
        ..Default::default()
    }
}

#[test]
fn aggregates_basic_totals() {
    let summaries = vec![
        summary("alpha", 3, 3, &[("read", 5), ("edit", 2), ("bash", 4)]),
        summary("alpha", 2, 2, &[("read", 1), ("apply_patch", 3)]),
        summary("beta", 5, 5, &[("agentgrep", 7), ("browser", 1)]),
    ];
    let r = report_from_summaries(summaries);

    assert_eq!(r.total_sessions, 3);
    assert_eq!(r.user_prompts, 10);
    assert_eq!(r.assistant_messages, 10);
    // read 6 + edit 2 + bash 4 + apply_patch 3 + agentgrep 7 + browser 1 = 23
    assert_eq!(r.total_tool_calls, 23);
    // edit 2 + apply_patch 3 = 5
    assert_eq!(r.code_edits, 5);
    assert_eq!(r.commands_run, 4);
    assert_eq!(r.searches, 7);
    assert_eq!(r.web_actions, 1);
    assert_eq!(r.distinct_projects, 2);
    assert!(r.power_score > 0);
    assert!(!r.archetype.is_empty());
}

#[test]
fn top_lists_sorted_desc() {
    let summaries = vec![
        summary("alpha", 1, 1, &[("read", 10)]),
        summary("alpha", 1, 1, &[("read", 5)]),
        summary("gamma", 1, 1, &[("bash", 1)]),
    ];
    let r = report_from_summaries(summaries);
    assert_eq!(r.top_projects.first().unwrap().name, "alpha");
    assert_eq!(r.top_projects.first().unwrap().count, 2);
    assert_eq!(r.top_tools.first().unwrap().name, "read");
    assert_eq!(r.top_tools.first().unwrap().count, 15);
}

#[test]
fn streaks_and_active_days_dedup() {
    let mut s = summary("alpha", 1, 1, &[("read", 1)]);
    s.active_dates = vec![
        "2026-05-10".to_string(),
        "2026-05-11".to_string(),
        "2026-05-12".to_string(),
        "2026-05-20".to_string(),
    ];
    let r = report_from_summaries(vec![s]);
    assert_eq!(r.active_days, 4);
    assert_eq!(r.longest_streak, 3);
    assert_eq!(r.first_day.as_deref(), Some("2026-05-10"));
    assert_eq!(r.last_day.as_deref(), Some("2026-05-20"));
}

#[test]
fn renders_markdown_and_png() {
    let summaries = vec![summary(
        "alpha",
        4,
        4,
        &[("read", 5), ("edit", 3), ("bash", 2)],
    )];
    let r = report_from_summaries(summaries);

    let md = render_markdown(&r);
    assert!(md.contains("Productivity Report"));
    assert!(md.contains("Power Score"));

    let svg = render_svg(&r);
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("</text>"));

    // PNG rendering depends on system fonts; ensure it produces a valid PNG.
    let png = render_png(&r).expect("png render");
    assert!(png.len() > 1000, "png too small: {}", png.len());
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn empty_report_is_safe() {
    let r = report_from_summaries(vec![]);
    assert_eq!(r.total_sessions, 0);
    let md = render_markdown(&r);
    assert!(md.contains("Productivity Report"));
    let png = render_png(&r).expect("png render");
    assert_eq!(&png[1..4], b"PNG");
}
