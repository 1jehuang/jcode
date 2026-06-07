//! Markdown rendering of a [`ProductivityReport`] for the chat transcript.

use crate::model::ProductivityReport;

/// Human-readable big-number formatting: 1234 -> "1.2K", 1_500_000 -> "1.5M".
pub fn human(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn hour_label(h: u8) -> String {
    let suffix = if h < 12 { "am" } else { "pm" };
    let display = match h % 12 {
        0 => 12,
        other => other,
    };
    format!("{}{}", display, suffix)
}

/// Public alias of [`hour_label`] for use by the dashboard renderer.
pub fn hour_label_pub(h: u8) -> String {
    hour_label(h)
}

/// Render the full markdown report.
pub fn render_markdown(r: &ProductivityReport) -> String {
    let mut o = String::new();

    o.push_str("# 📊 Your jcode Productivity Report\n\n");
    o.push_str(&format!(
        "**{}** · _{}_\n\n",
        r.archetype, r.archetype_blurb
    ));
    o.push_str(&format!(
        "> ⚡ **Power Score: {}**\n\n",
        human(r.power_score)
    ));

    if !r.badges.is_empty() {
        o.push_str(&r.badges.join("  ·  "));
        o.push_str("\n\n");
    }

    // Headline grid.
    o.push_str("## At a glance\n\n");
    o.push_str("| Metric | Value |\n|---|---|\n");
    o.push_str(&format!("| 💬 Sessions | {} |\n", human(r.total_sessions)));
    o.push_str(&format!(
        "| 🙋 Prompts sent | {} |\n",
        human(r.user_prompts)
    ));
    o.push_str(&format!(
        "| 🛠️ Tool calls | {} |\n",
        human(r.total_tool_calls)
    ));
    o.push_str(&format!("| ✍️ Code edits | {} |\n", human(r.code_edits)));
    o.push_str(&format!(
        "| 🧠 Tokens out / in | {} / {} |\n",
        human(r.output_tokens),
        human(r.input_tokens)
    ));
    o.push_str(&format!(
        "| ♻️ Cache reads | {} |\n",
        human(r.cache_read_tokens)
    ));
    o.push_str(&format!(
        "| 📅 Active days | {} (of {} day span) |\n",
        r.active_days, r.span_days
    ));
    o.push_str(&format!(
        "| 🔥 Streak | {} now · {} best |\n",
        r.current_streak, r.longest_streak
    ));
    o.push_str(&format!(
        "| 🗂️ Projects | {} |\n",
        human(r.distinct_projects)
    ));
    o.push_str(&format!(
        "| ⏰ Peak hour | {} ({}) |\n",
        hour_label(r.peak_hour),
        r.chronotype
    ));
    o.push('\n');

    // Human effort framing.
    o.push_str(&format!(
        "You've typed about **{} words** of prompts and your agent produced **{} characters** back.\n\n",
        human(r.user_words),
        human(r.assistant_chars)
    ));

    // Top projects.
    if !r.top_projects.is_empty() {
        o.push_str("## 🗂️ Top projects\n\n");
        for (i, t) in r.top_projects.iter().enumerate() {
            o.push_str(&format!(
                "{}. **{}** — {} sessions\n",
                i + 1,
                t.name,
                t.count
            ));
        }
        o.push('\n');
    }

    // Top tools with mini bars.
    if !r.top_tools.is_empty() {
        o.push_str("## 🧰 Most-used tools\n\n");
        let max = r.top_tools.first().map(|t| t.count).unwrap_or(1).max(1);
        for t in &r.top_tools {
            let bar = bar_for(t.count, max, 20);
            o.push_str(&format!("- `{:<10}` {} {}\n", t.name, bar, human(t.count)));
        }
        o.push('\n');
    }

    // Models.
    if !r.top_models.is_empty() {
        o.push_str("## 🤖 Models\n\n");
        for t in &r.top_models {
            o.push_str(&format!("- {} ({})\n", t.name, human(t.count)));
        }
        o.push('\n');
    }

    // Daily rhythm: 24h sparkline.
    o.push_str("## 🕒 Daily rhythm\n\n");
    o.push_str("```\n");
    o.push_str(&sparkline(&r.hour_hist));
    o.push_str("\n0h          6h          12h         18h      23h\n");
    o.push_str("```\n\n");

    if let Some(busy) = &r.busiest_day {
        o.push_str(&format!(
            "Busiest day: **{}** ({} actions).\n\n",
            busy.name,
            human(busy.count)
        ));
    }

    o.push_str(&format!(
        "_Generated {} · scanned {} sessions in {:.1}s ({} cached). 🖼️ Dashboard image copied to your clipboard._\n",
        r.generated_at,
        human(r.scanned_files),
        r.scan_secs,
        human(r.cache_hits)
    ));

    o
}

fn bar_for(count: u64, max: u64, width: usize) -> String {
    let filled = ((count as f64 / max as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::new();
    for _ in 0..filled {
        s.push('█');
    }
    for _ in filled..width {
        s.push('░');
    }
    s
}

/// Render a unicode block sparkline over a 24-slot histogram.
pub fn sparkline(hist: &[u32; 24]) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = hist.iter().copied().max().unwrap_or(0).max(1);
    let mut s = String::new();
    for &v in hist.iter() {
        if v == 0 {
            s.push(' ');
        } else {
            let idx = (((v as f64 / max as f64) * (BLOCKS.len() - 1) as f64).round() as usize)
                .min(BLOCKS.len() - 1);
            s.push(BLOCKS[idx]);
        }
        s.push(' ');
    }
    s.trim_end().to_string()
}
