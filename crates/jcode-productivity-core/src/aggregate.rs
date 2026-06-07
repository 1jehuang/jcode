//! Fold per-session summaries into the global [`ProductivityReport`], including
//! the shareable "flavor" fields (archetype, badges, power score).

use crate::model::{ProductivityReport, SessionSummary, Tally};
use crate::scan::ScanResult;
use chrono::{Datelike, Local, NaiveDate};
use std::collections::{BTreeSet, HashMap};

fn top_n(map: HashMap<String, u64>, n: usize) -> Vec<Tally> {
    let mut v: Vec<Tally> = map
        .into_iter()
        .map(|(name, count)| Tally { name, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    v.truncate(n);
    v
}

/// Build the full report from a scan result.
pub fn build_report(scan: ScanResult) -> ProductivityReport {
    let summaries = &scan.summaries;

    let mut r = ProductivityReport {
        generated_at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        total_sessions: summaries.len() as u64,
        scanned_files: scan.scanned_files,
        parse_errors: scan.parse_errors,
        scan_secs: scan.scan_secs,
        cache_hits: scan.cache_hits,
        ..Default::default()
    };

    let mut projects: HashMap<String, u64> = HashMap::new();
    let mut tools: HashMap<String, u64> = HashMap::new();
    let mut models: HashMap<String, u64> = HashMap::new();
    let mut providers: HashMap<String, u64> = HashMap::new();
    let mut all_dates: BTreeSet<String> = BTreeSet::new();
    let mut date_counts: HashMap<String, u64> = HashMap::new();

    let mut total_session_msgs: u64 = 0;

    for s in summaries {
        let msgs = (s.user_msgs + s.assistant_msgs) as u64;
        r.total_messages += msgs;
        total_session_msgs += msgs;
        r.user_prompts += s.user_msgs as u64;
        r.assistant_messages += s.assistant_msgs as u64;
        r.total_tool_calls += s.total_tool_calls();
        r.total_images += s.images as u64;

        r.input_tokens += s.input_tokens;
        r.output_tokens += s.output_tokens;
        r.cache_read_tokens += s.cache_read_tokens;
        r.cache_creation_tokens += s.cache_creation_tokens;

        r.user_chars += s.user_chars;
        r.assistant_chars += s.assistant_chars;

        if msgs > r.longest_session_msgs {
            r.longest_session_msgs = msgs;
        }

        if let Some(p) = &s.project
            && !p.is_empty()
        {
            *projects.entry(p.clone()).or_insert(0) += 1;
        }
        if let Some(m) = &s.model
            && !m.is_empty()
            && m != "<synthetic>"
        {
            *models.entry(m.clone()).or_insert(0) += msgs.max(1);
        }
        if let Some(pk) = &s.provider_key
            && !pk.is_empty()
        {
            *providers.entry(pk.clone()).or_insert(0) += msgs.max(1);
        }

        for (name, count) in &s.tools {
            *tools.entry(name.clone()).or_insert(0) += *count as u64;
        }

        for h in 0..24 {
            r.hour_hist[h] += s.hour_hist[h];
        }
        for d in 0..7 {
            r.weekday_hist[d] += s.weekday_hist[d];
        }

        for date in &s.active_dates {
            all_dates.insert(date.clone());
            *date_counts.entry(date.clone()).or_insert(0) += msgs.max(1);
        }
    }

    r.user_words = (r.user_chars as f64 / 5.0).round() as u64;
    r.distinct_projects = projects.len() as u64;
    r.avg_session_msgs = if r.total_sessions > 0 {
        total_session_msgs as f64 / r.total_sessions as f64
    } else {
        0.0
    };

    // Tool-derived activity buckets.
    let tool = |name: &str| tools.get(name).copied().unwrap_or(0);
    r.code_edits = tool("edit") + tool("write") + tool("multiedit") + tool("apply_patch");
    r.commands_run = tool("bash");
    r.searches = tool("grep") + tool("agentgrep") + tool("glob");
    r.web_actions = tool("browser") + tool("websearch") + tool("webfetch");

    r.top_projects = top_n(projects, 8);
    r.top_tools = top_n(tools, 10);
    r.top_models = top_n(models, 6);
    r.top_providers = top_n(providers, 6);

    // Time analysis -------------------------------------------------------
    r.active_days = all_dates.len() as u64;
    r.first_day = all_dates.iter().next().cloned();
    r.last_day = all_dates.iter().next_back().cloned();
    if let (Some(first), Some(last)) = (&r.first_day, &r.last_day)
        && let (Ok(a), Ok(b)) = (
            NaiveDate::parse_from_str(first, "%Y-%m-%d"),
            NaiveDate::parse_from_str(last, "%Y-%m-%d"),
        )
    {
        r.span_days = (b - a).num_days().max(0) as u64 + 1;
    }
    let (cur, longest) = streaks(&all_dates);
    r.current_streak = cur;
    r.longest_streak = longest;
    if let Some((day, count)) = date_counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
    {
        r.busiest_day = Some(Tally {
            name: day.clone(),
            count: *count,
        });
    }
    r.peak_hour = r
        .hour_hist
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| **c)
        .map(|(h, _)| h as u8)
        .unwrap_or(0);
    r.chronotype = chronotype(r.peak_hour).to_string();

    // Flavor --------------------------------------------------------------
    r.power_score = power_score(&r);
    let (archetype, blurb) = archetype(&r);
    r.archetype = archetype;
    r.archetype_blurb = blurb;
    r.badges = badges(&r);

    r
}

/// Compute (current_streak, longest_streak) of consecutive active days ending at
/// today (or yesterday). Current streak counts if the most recent active day is
/// today or yesterday.
fn streaks(dates: &BTreeSet<String>) -> (u64, u64) {
    let parsed: Vec<NaiveDate> = dates
        .iter()
        .filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .collect();
    if parsed.is_empty() {
        return (0, 0);
    }

    // Longest run of consecutive days.
    let mut longest = 1u64;
    let mut run = 1u64;
    for w in parsed.windows(2) {
        if (w[1] - w[0]).num_days() == 1 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }

    // Current streak: walk backwards from the last active day, but only counts
    // as "current" if it reaches today or yesterday.
    let today = Local::now().date_naive();
    let last = *parsed.last().unwrap();
    let mut current = 0u64;
    if (today - last).num_days() <= 1 {
        current = 1;
        let mut idx = parsed.len() - 1;
        while idx > 0 {
            if (parsed[idx] - parsed[idx - 1]).num_days() == 1 {
                current += 1;
                idx -= 1;
            } else {
                break;
            }
        }
    }

    (current, longest)
}

fn chronotype(peak_hour: u8) -> &'static str {
    match peak_hour {
        5..=8 => "Early Bird",
        9..=11 => "Morning Person",
        12..=16 => "Afternoon Operator",
        17..=20 => "Evening Builder",
        21..=23 => "Night Owl",
        _ => "Nocturnal Hacker",
    }
}

/// A single headline number that's fun to compare. Weighted toward output and
/// breadth rather than raw token spend.
fn power_score(r: &ProductivityReport) -> u64 {
    let from_tools = r.total_tool_calls as f64 * 1.0;
    let from_edits = r.code_edits as f64 * 4.0;
    let from_prompts = r.user_prompts as f64 * 2.0;
    let from_days = r.active_days as f64 * 25.0;
    let from_streak = r.longest_streak as f64 * 15.0;
    let from_projects = r.distinct_projects as f64 * 10.0;
    let from_tokens = (r.output_tokens as f64 / 1000.0) * 1.0;
    (from_tools + from_edits + from_prompts + from_days + from_streak + from_projects + from_tokens)
        .round() as u64
}

/// Pick a primary archetype from the dominant activity mix.
fn archetype(r: &ProductivityReport) -> (String, String) {
    let edits = r.code_edits as f64;
    let cmds = r.commands_run as f64;
    let search = r.searches as f64;
    let web = r.web_actions as f64;
    let prompts = r.user_prompts.max(1) as f64;

    // Ratios relative to total prompts give a stable signal across volume.
    let edit_ratio = edits / prompts;
    let cmd_ratio = cmds / prompts;
    let search_ratio = search / prompts;
    let web_ratio = web / prompts;

    let mut candidates: Vec<(&str, f64, &str)> = vec![
        (
            "The Shipper",
            edit_ratio,
            "You turn conversations into committed code. Edits per prompt off the charts.",
        ),
        (
            "The Operator",
            cmd_ratio,
            "Terminal-first. You drive builds, tests, and tooling like a control panel.",
        ),
        (
            "The Explorer",
            search_ratio,
            "You read before you write. Codebase navigation is your superpower.",
        ),
        (
            "The Researcher",
            web_ratio * 3.0,
            "You pull the wider world into your work, browsing and searching the web.",
        ),
    ];
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top = candidates[0];
    if top.1 <= 0.0001 {
        return (
            "The Conversationalist".to_string(),
            "Mostly thinking out loud with your agent. The ideas come first.".to_string(),
        );
    }
    (top.0.to_string(), top.2.to_string())
}

fn badges(r: &ProductivityReport) -> Vec<String> {
    let mut b = Vec::new();
    if r.longest_streak >= 7 {
        b.push(format!("🔥 {}-day streak", r.longest_streak));
    }
    if r.distinct_projects >= 10 {
        b.push(format!("🗂️ {} projects", r.distinct_projects));
    }
    if r.output_tokens >= 10_000_000 {
        b.push("📈 10M+ tokens generated".to_string());
    }
    if r.code_edits >= 1000 {
        b.push(format!("🛠️ {} code edits", r.code_edits));
    }
    if matches!(r.peak_hour, 0..=4 | 22..=23) {
        b.push("🦉 Night owl".to_string());
    }
    if r.total_sessions >= 1000 {
        b.push(format!("💬 {} sessions", r.total_sessions));
    }
    // Weekend warrior: meaningful share of activity on Sat/Sun.
    let weekend: u32 = r.weekday_hist[5] + r.weekday_hist[6];
    let total: u32 = r.weekday_hist.iter().sum();
    if total > 0 && (weekend as f64 / total as f64) > 0.30 {
        b.push("🏕️ Weekend warrior".to_string());
    }
    b
}

#[allow(dead_code)]
fn weekday_label(idx: usize) -> &'static str {
    match idx {
        0 => "Mon",
        1 => "Tue",
        2 => "Wed",
        3 => "Thu",
        4 => "Fri",
        5 => "Sat",
        _ => "Sun",
    }
}

/// Convenience for callers that just want today's full report.
pub fn report_from_summaries(summaries: Vec<SessionSummary>) -> ProductivityReport {
    build_report(ScanResult {
        scanned_files: summaries.len() as u64,
        parse_errors: 0,
        cache_hits: 0,
        scan_secs: 0.0,
        summaries,
    })
}

/// Local weekday index (Mon=0) for today, used by some renderers.
#[allow(dead_code)]
pub(crate) fn today_weekday() -> usize {
    Local::now().weekday().num_days_from_monday() as usize
}
