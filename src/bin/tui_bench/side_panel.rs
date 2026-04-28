use super::{SidePanelSource, make_text};
use anyhow::{Context, Result};
use jcode::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use std::fs;
use std::path::PathBuf;

pub(super) fn make_bench_file(idx: usize, approx_len: usize) -> Result<PathBuf> {
    let base_dir = std::env::temp_dir().join("jcode_tui_bench");
    fs::create_dir_all(&base_dir).with_context(|| {
        format!(
            "failed to create TUI bench directory {}",
            base_dir.display()
        )
    })?;
    let file_path = base_dir.join(format!("file_diff_{idx}.rs"));

    let mut content = String::from("fn bench_file() {\n");
    let repeated = make_text(approx_len);
    for line_idx in 0..120 {
        if line_idx == idx % 120 {
            content.push_str(&format!(
                "    let line_{line_idx} = \"target line {idx}\";\n"
            ));
        } else {
            content.push_str(&format!("    let line_{line_idx} = \"{}\";\n", repeated));
        }
    }
    content.push_str("}\n");

    fs::write(&file_path, content)
        .with_context(|| format!("failed to write bench file {}", file_path.display()))?;
    Ok(file_path)
}

pub(super) fn make_bench_side_panel(
    approx_len: usize,
    source: SidePanelSource,
    mermaid_count: usize,
    bench_file_paths: &mut Vec<PathBuf>,
) -> Result<SidePanelSnapshot> {
    let content = make_side_panel_content(approx_len, mermaid_count.max(1));
    let source_kind = match source {
        SidePanelSource::Managed => SidePanelPageSource::Managed,
        SidePanelSource::LinkedFile => SidePanelPageSource::LinkedFile,
    };

    let file_path = match source {
        SidePanelSource::Managed => std::env::temp_dir()
            .join("jcode_tui_bench")
            .join("side_panel_managed.md"),
        SidePanelSource::LinkedFile => std::env::temp_dir()
            .join("jcode_tui_bench")
            .join("side_panel_linked.md"),
    };
    fs::create_dir_all(
        file_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )
    .with_context(|| {
        format!(
            "failed to create side-panel bench directory for {}",
            file_path.display()
        )
    })?;
    fs::write(&file_path, &content).with_context(|| {
        format!(
            "failed to write side-panel bench file {}",
            file_path.display()
        )
    })?;
    bench_file_paths.push(file_path.clone());

    Ok(SidePanelSnapshot {
        focused_page_id: Some("bench_side_panel".to_string()),
        pages: vec![SidePanelPage {
            id: "bench_side_panel".to_string(),
            title: format!(
                "Bench Side Panel ({})",
                match source {
                    SidePanelSource::Managed => "managed",
                    SidePanelSource::LinkedFile => "linked-file",
                }
            ),
            file_path: file_path.display().to_string(),
            format: SidePanelPageFormat::Markdown,
            source: source_kind,
            content,
            updated_at_ms: 1,
        }],
    })
}

pub(super) fn make_side_panel_refresh_content(generation: usize) -> String {
    format!(
        "# Linked Refresh Benchmark\n\nGeneration: {generation}\n\n{}\n\n```mermaid\nflowchart TD\n    A[Refresh {generation}] --> B[Read file]\n    B --> C[Update snapshot]\n    C --> D[Reuse width cache]\n```\n",
        make_text(360)
    )
}

fn make_side_panel_content(approx_len: usize, mermaid_count: usize) -> String {
    let mut out = String::new();
    out.push_str("# Side Panel Benchmark\n\n");
    for idx in 0..mermaid_count {
        out.push_str(&format!("## Section {}\n\n", idx + 1));
        out.push_str(&make_text(approx_len));
        out.push_str("\n\n");
        out.push_str("```mermaid\nflowchart TD\n");
        out.push_str(&format!(
            "    A{idx}[Start {idx}] --> B{idx}[Load content]\n    B{idx} --> C{idx}{{Scroll?}}\n    C{idx} -- Yes --> D{idx}[Render viewport]\n    C{idx} -- No --> E{idx}[Reuse cache]\n    D{idx} --> F{idx}[Done]\n    E{idx} --> F{idx}[Done]\n"
        ));
        out.push_str("```\n\n");
        out.push_str("- scroll interaction\n- markdown wrapping\n- image viewport rendering\n\n");
    }
    out.push_str("## Final Notes\n\n");
    for idx in 0..24 {
        out.push_str(&format!("- Bench line {:02}: {}\n", idx + 1, make_text(64)));
    }
    out
}
