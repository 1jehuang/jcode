//! Stress probe: build the mermaid source that `jcode_plan::mermaid::
//! swarm_plan_mermaid` (the production swarm plan-graph generator, re-exported
//! to the TUI via `crate::tui::swarm_plan_graph`) emits for realistic +
//! hostile plan data, then render through the real pipeline
//! (`render_mermaid_untracked`).
//!
//!   scripts/dev_cargo.sh run -p jcode-tui-mermaid --features renderer \
//!       --example swarm_plan_stress
//!
//! Cases:
//!   (a) real deep-mode plan fixture (examples/swarm_plan_fixture.json,
//!       snapshot of ~/.kcode/state/swarm/_home_jeremy_jcode__git.json)
//!   (a23) first 23 items of the fixture (the original task shape)
//!   (b) 40-node plan -> truncation with the linked 'more' summary node
//!   (c) labels at exactly the label cap with unicode glyphs
//!   (d) hostile label chars: quotes, backtick, backslash, #, %, &, <, >, |
//!   (e) duplicate sanitized ids (a-1 vs a_1) + self-dependency
//!   (f) an item whose id is literally "more" alongside the summary node
//!   (g) gate hexagons + wide fan-in (deep-mode shape)

use jcode_plan::PlanItem;
use jcode_plan::mermaid::swarm_plan_mermaid;

fn item(id: &str, content: &str, status: &str, blocked_by: &[&str]) -> PlanItem {
    PlanItem {
        content: content.to_string(),
        status: status.to_string(),
        priority: "normal".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
        assigned_to: None,
    }
}

fn probe(name: &str, src: &str) -> bool {
    match jcode_tui_mermaid::render_mermaid_untracked(src, Some(100)) {
        jcode_tui_mermaid::RenderResult::Image {
            width,
            height,
            path,
            ..
        } => {
            // Degenerate aspect ratios (slivers) are unreadable even when the
            // renderer technically succeeds.
            let sliver = width < 120 || height < 60;
            println!(
                "{}   {name}: {width}x{height} -> {}",
                if sliver { "THIN" } else { "OK  " },
                path.display()
            );
            true
        }
        jcode_tui_mermaid::RenderResult::Error(err) => {
            println!("FAIL {name}: {err}");
            println!("---- offending source ----\n{src}\n--------------------------");
            false
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ok = true;

    // Ad hoc mode: pass a path to a JSON array of plan items to render just
    // that plan (prints the generated mermaid source and probes it).
    if let Some(path) = std::env::args().nth(1) {
        let raw = std::fs::read_to_string(&path)?;
        let items: Vec<PlanItem> = serde_json::from_str(&raw)?;
        println!("external plan: {} items", items.len());
        let src = swarm_plan_mermaid(&items).ok_or("graph render returned None")?;
        println!("--- external plan mermaid ---\n{src}");
        let ok = probe("external-plan", &src);
        std::process::exit(if ok { 0 } else { 1 });
    }

    // (a) real plan fixture (35 items at snapshot time, so it also exercises
    // real truncation) + (a23) the original 23-item shape.
    let fixture = include_str!("swarm_plan_fixture.json");
    let real: Vec<PlanItem> = serde_json::from_str(fixture)?;
    println!("fixture items: {}", real.len());
    let src_a = swarm_plan_mermaid(&real).ok_or("graph render returned None")?;
    println!(
        "--- (a) real plan mermaid ({} lines) ---\n{src_a}",
        src_a.lines().count()
    );
    ok &= probe("a-real-plan-full", &src_a);
    let src_a23 =
        swarm_plan_mermaid(&real[..real.len().min(23)]).ok_or("graph render returned None")?;
    ok &= probe("a23-real-plan-23", &src_a23);

    // (b) truncation: 40 nodes, chain deps, linked 'more' summary node.
    let items_b: Vec<PlanItem> = (0..40)
        .map(|i: usize| {
            let dep = format!("t{}", i.saturating_sub(1));
            let deps: Vec<&str> = if i == 0 { vec![] } else { vec![dep.as_str()] };
            item(
                &format!("t{i}"),
                &format!("task number {i}"),
                "queued",
                &deps,
            )
        })
        .collect();
    let src_b = swarm_plan_mermaid(&items_b).ok_or("graph render returned None")?;
    assert!(src_b.contains("…and 10 more tasks"), "summary node missing");
    assert!(src_b.contains("-.-> more"), "summary node must be linked");
    ok &= probe("b-truncation-40", &src_b);

    // (c) labels at/over the cap with unicode glyphs.
    let exact: String = "日本語テスト🎯émü→".chars().cycle().take(42).collect();
    let over: String = "日本語テスト🎯émü→".chars().cycle().take(43).collect();
    let items_c = vec![
        item("uni-exact", &exact, "running", &[]),
        item("uni-over", &over, "queued", &["uni-exact"]),
    ];
    let src_c = swarm_plan_mermaid(&items_c).ok_or("graph render returned None")?;
    println!("--- (c) unicode mermaid ---\n{src_c}");
    ok &= probe("c-unicode-max-label", &src_c);

    // (d) hostile label chars, including the quote-shatter hazard.
    let items_d = vec![
        item("h1", "backtick `code` and backslash \\ path", "queued", &[]),
        item("h2", "hash # percent % amp & semi;colon", "queued", &["h1"]),
        item("h3", "angle <b>bold</b> pipe | (parens)", "queued", &["h2"]),
        item(
            "h4",
            "entity-ish &lt;#35; #quot; %%{init}%%",
            "queued",
            &["h3"],
        ),
        item(
            "h5",
            "verify the work of 'fix-swarm-member-task' with a lone \" quote",
            "queued",
            &["h4"],
        ),
    ];
    let src_d = swarm_plan_mermaid(&items_d).ok_or("graph render returned None")?;
    println!("--- (d) hostile-label mermaid ---\n{src_d}");
    assert!(
        !src_d.contains('\''),
        "raw apostrophes must never reach the renderer"
    );
    ok &= probe("d-hostile-labels", &src_d);

    // (d2) each hostile char in isolation so a failure names the culprit.
    for (tag, s) in [
        ("apostrophe", "it's a lone quote"),
        ("double-quote", "a \"quoted\" bit"),
        ("odd-quote", "task 'unbalanced"),
        ("backtick", "a `b` c"),
        ("backslash", "a \\ b"),
        ("hash", "a # b"),
        ("percent", "a % b"),
        ("amp", "a & b"),
        ("lt-gt", "a <b> c"),
        ("pipe", "a | b"),
        ("semicolon", "a ; b"),
        ("parens", "a (b) c"),
        ("entity", "&amp; &#35;"),
        ("percent-directive", "%%{init: {'theme':'dark'}}%%"),
    ] {
        let its = vec![item("only", s, "queued", &[])];
        let src = swarm_plan_mermaid(&its).ok_or("graph render returned None")?;
        ok &= probe(&format!("d2-{tag}"), &src);
    }

    // (e) duplicate sanitized ids: a-1 and a_1 both sanitize to t_a_1 and
    // must be suffixed apart; the self-dependency must be dropped.
    let items_e = vec![
        item("a-1", "first flavor of a1", "completed", &["a-1"]),
        item("a_1", "second flavor of a1", "running", &["a-1"]),
        item("b", "depends on both", "queued", &["a-1", "a_1"]),
    ];
    let src_e = swarm_plan_mermaid(&items_e).ok_or("graph render returned None")?;
    println!("--- (e) duplicate-id + self-dep mermaid ---\n{src_e}");
    assert!(
        src_e.contains("t_a_1_2["),
        "colliding sanitized ids must be suffixed"
    );
    assert!(
        !src_e.contains("t_a_1 --> t_a_1\n"),
        "self-dependency edge must be dropped"
    );
    ok &= probe("e-dup-ids-self-dep", &src_e);

    // (f) an item whose id is exactly "more" plus enough items to force the
    // truncation summary node also named `more` (unprefixed).
    let mut items_f: Vec<PlanItem> = (0..29)
        .map(|i| item(&format!("f{i}"), &format!("filler {i}"), "queued", &[]))
        .collect();
    items_f.insert(
        0,
        item("more", "a task literally named more", "running", &[]),
    );
    for i in 0..5 {
        items_f.push(item(
            &format!("extra{i}"),
            &format!("extra {i}"),
            "queued",
            &[],
        ));
    }
    let src_f = swarm_plan_mermaid(&items_f).ok_or("graph render returned None")?;
    assert!(src_f.contains("t_more["), "prefixed more node missing");
    assert!(src_f.contains("\n    more["), "summary more node missing");
    ok &= probe("f-more-id-collision", &src_f);

    // (g) deep-mode shape: hexagonal gate collecting a wide fan-in.
    let mut items_g: Vec<PlanItem> = (0..12)
        .map(|i| {
            let mut it = item(
                &format!("child{i}"),
                &format!("implement piece {i}"),
                "completed",
                &[],
            );
            it.assigned_to = Some(format!("session_worker{i}_1783199147688_8fa34a84b95fe291"));
            it
        })
        .collect();
    let deps: Vec<String> = (0..12).map(|i| format!("child{i}")).collect();
    let dep_refs: Vec<&str> = deps.iter().map(String::as_str).collect();
    items_g.push(item(
        "parent::gate",
        "Critique the work adversarially",
        "queued",
        &dep_refs,
    ));
    let src_g = swarm_plan_mermaid(&items_g).ok_or("graph render returned None")?;
    println!("--- (g) gate fan-in mermaid ---\n{src_g}");
    assert!(
        src_g.starts_with("flowchart LR"),
        "wide fan-in must switch to LR"
    );
    assert!(src_g.contains("{{\""), "gate must render as hexagon");
    ok &= probe("g-gate-fan-in", &src_g);

    println!(
        "\nresult: {}",
        if ok { "ALL OK" } else { "FAILURES PRESENT" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
