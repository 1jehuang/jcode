//! Productivity report generation for jcode.
//!
//! Scans local session transcripts (with an incremental cache), computes
//! interesting + shareable usage statistics, and renders them as both Markdown
//! and a PNG dashboard suitable for sharing.
//!
//! High-level entry point: [`generate`].

mod aggregate;
mod dashboard;
mod markdown;
mod model;
mod scan;

pub use model::{ProductivityReport, SessionSummary, Tally};

use anyhow::Result;
use std::path::PathBuf;

/// Everything a caller needs to display and share the report.
pub struct ProductivityOutput {
    pub report: ProductivityReport,
    /// Rendered Markdown for the chat transcript.
    pub markdown: String,
    /// PNG dashboard bytes (also written to `png_path`).
    pub png: Vec<u8>,
    /// Where the PNG was saved on disk.
    pub png_path: PathBuf,
}

/// Scan transcripts, compute the report, render markdown + PNG, and persist the
/// PNG to `~/.jcode/generated-images/productivity-<timestamp>.png`.
pub fn generate() -> Result<ProductivityOutput> {
    let report = compute_report()?;
    let markdown = markdown::render_markdown(&report);
    let png = dashboard::render_png(&report)?;
    let png_path = save_png(&png)?;
    Ok(ProductivityOutput {
        report,
        markdown,
        png,
        png_path,
    })
}

/// Scan + aggregate only (no rendering). Useful for tests and JSON export.
pub fn compute_report() -> Result<ProductivityReport> {
    let scan = scan::scan_all()?;
    Ok(aggregate::build_report(scan))
}

/// Render markdown for a report.
pub fn render_markdown(report: &ProductivityReport) -> String {
    markdown::render_markdown(report)
}

/// Render the dashboard PNG bytes for a report.
pub fn render_png(report: &ProductivityReport) -> Result<Vec<u8>> {
    dashboard::render_png(report)
}

/// Render the dashboard SVG string for a report (useful for debugging/preview).
pub fn render_svg(report: &ProductivityReport) -> String {
    dashboard::render_svg(report)
}

fn save_png(png: &[u8]) -> Result<PathBuf> {
    let dir = jcode_storage::jcode_dir()?.join("generated-images");
    std::fs::create_dir_all(&dir)?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("productivity-{ts}.png"));
    std::fs::write(&path, png)?;
    Ok(path)
}

/// Build a report directly from in-memory summaries. Exposed for tests.
pub fn report_from_summaries(summaries: Vec<SessionSummary>) -> ProductivityReport {
    aggregate::report_from_summaries(summaries)
}

#[cfg(test)]
mod tests;
