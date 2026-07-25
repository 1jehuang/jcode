//! Corpus-driven evaluation of the webfetch HTML extractor.
//!
//! These tests validate extraction against a *diverse corpus* of real pages
//! (wikis, API docs, specs, forums, news, SPAs, and non-HTML payloads) so that
//! extraction rules are held to general standards rather than tuned to whatever
//! page happened to be tried by hand.
//!
//! The corpus is not vendored: fetch it with `scripts/webfetch_corpus.sh` and
//! point `WEBFETCH_CORPUS` at the output directory. Without it these tests skip,
//! so offline and CI runs stay green.
//!
//! Every assertion here is a *site-agnostic invariant*. If an invariant needs a
//! per-site exception, that is a signal the extraction rule is not general.

use std::path::{Path, PathBuf};

use super::{html_to_markdown, html_to_text};

fn corpus_dir() -> Option<PathBuf> {
    let dir = std::env::var("WEBFETCH_CORPUS").ok()?;
    let path = PathBuf::from(dir);
    path.is_dir().then_some(path)
}

struct Page {
    name: String,
    html: String,
}

fn load_corpus() -> Vec<Page> {
    let Some(dir) = corpus_dir() else {
        return Vec::new();
    };
    let mut pages = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return pages;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let Ok(html) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        pages.push(Page { name, html });
    }
    pages.sort_by(|a, b| a.name.cmp(&b.name));
    pages
}

fn is_html(html: &str) -> bool {
    let head: String = html.chars().take(2000).collect::<String>().to_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body")
}

/// Ratio of markup/URL characters to prose characters. A good extractor leaves
/// mostly prose behind.
fn punctuation_noise_ratio(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let noise = text
        .chars()
        .filter(|c| matches!(c, '<' | '>' | '{' | '}' | '\\' | '%' | '|' | '=' | '~'))
        .count();
    noise as f64 / text.chars().count() as f64
}

#[test]
fn corpus_extraction_invariants() {
    let pages = load_corpus();
    if pages.is_empty() {
        eprintln!("skipping: set WEBFETCH_CORPUS (see scripts/webfetch_corpus.sh)");
        return;
    }

    let mut failures: Vec<String> = Vec::new();
    for page in &pages {
        if !is_html(&page.html) {
            continue;
        }
        let md = html_to_markdown(&page.html);
        let text = html_to_text(&page.html);

        // 1. No unclosed markup may survive into the output. Leftover `<tag`
        //    sequences mean the tag matcher failed on this page's markup.
        for (label, out) in [("markdown", &md), ("text", &text)] {
            if let Some((idx, tag)) = find_residual_tag(&page.html, out) {
                failures.push(format!(
                    "{}: residual html tag {tag:?} in {label} near {:?}",
                    page.name,
                    snippet(out, idx)
                ));
            }
        }

        // 2. Script/style bodies and attribute payloads must not appear as prose.
        //    Output occurrences are compared against the number that appear as
        //    legitimate *content* in the source (entity-escaped, or inside
        //    <pre>/<code> samples), because pages that document HTML/CSS/JS
        //    genuinely render these strings.
        for needle in [
            "@media ",
            "!important",
            "addEventListener",
            "data-mw=",
            "data-testid=",
            "aria-hidden=",
            "srcset=",
        ] {
            let allowed = count_escaped_occurrences(&page.html, needle);
            let in_output = md.matches(needle).count();
            if in_output > allowed {
                failures.push(format!(
                    "{}: non-prose leaked ({needle}): {in_output} in output vs \
                     {allowed} legitimate in source",
                    page.name
                ));
            }
        }

        // 4. Empty links convey nothing and must be dropped.
        if md.contains("[](") {
            failures.push(format!("{}: empty-text link retained", page.name));
        }

        // 5. No single line may be dominated by one enormous URL.
        if let Some(len) = md.lines().map(|l| l.len()).max()
            && len > 20_000
        {
            failures.push(format!("{}: pathological line length {len}", page.name));
        }

        // 6. Output must be mostly prose, not markup residue.
        let ratio = punctuation_noise_ratio(&md);
        if ratio > 0.12 {
            failures.push(format!(
                "{}: noise ratio {ratio:.3} exceeds 0.12",
                page.name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "corpus extraction invariants violated ({} pages):\n{}",
        pages.len(),
        failures.join("\n")
    );
}

/// Count occurrences of `needle` that appear *entity-escaped* in the source, i.e.
/// as page content rather than as live markup. Pages documenting HTML, CSS, or JS
/// (specs, tutorials, MDN) legitimately render these strings, so they form the
/// allowance against which output occurrences are compared.
fn count_escaped_occurrences(html: &str, needle: &str) -> usize {
    // Strip source tags first. Syntax highlighters interleave `<span>`s inside
    // escaped samples (`&lt;</span><span>a</span>`), so the escaped needle is
    // only contiguous once markup is removed.
    let source_text = strip_tags(html);
    let escaped = needle
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    let mut count = source_text.matches(&escaped).count();
    if escaped != needle {
        return count;
    }
    // For needles without escapable characters (e.g. `addEventListener`), treat
    // occurrences inside <pre>/<code> sample blocks as legitimate content.
    count = count.max(count_in_code_blocks(html, needle));
    count
}

/// Remove all html tags without decoding entities, yielding the source's text
/// content in its still-escaped form.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for ch in html.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Occurrences of `needle` inside `<pre>` or `<code>` blocks, which are rendered
/// content even when they look like script source.
fn count_in_code_blocks(html: &str, needle: &str) -> usize {
    let lower = html.to_lowercase();
    let mut total = 0usize;
    for open_tag in ["<pre", "<code"] {
        let close_tag = if open_tag == "<pre" {
            "</pre"
        } else {
            "</code"
        };
        let mut cursor = 0usize;
        while let Some(rel) = lower[cursor..].find(open_tag) {
            let start = cursor + rel;
            let Some(rel_end) = lower[start..].find(close_tag) else {
                break;
            };
            let end = start + rel_end;
            total += html[start..end].matches(needle).count();
            cursor = end + close_tag.len();
        }
    }
    total
}

/// Locate a residual html tag that survived stripping, returning its offset and
/// which tag matched.
///
/// Tags that appear entity-escaped in the source are page *content* (a spec
/// quoting sample markup), not leakage, so they are allowed in proportion to
/// their escaped occurrences.
fn find_residual_tag(html: &str, out: &str) -> Option<(usize, &'static str)> {
    const TAGS: [&str; 10] = [
        "<div", "<span", "<p ", "<a ", "<li", "<td", "<tr", "<img", "<meta", "<link",
    ];
    let lower_out = out.to_lowercase();
    let source_text = strip_tags(html).to_lowercase();
    for tag in TAGS {
        let in_output = lower_out.matches(tag).count();
        if in_output == 0 {
            continue;
        }
        let escaped = tag.replace('<', "&lt;");
        let allowed = source_text.matches(&escaped).count();
        if in_output > allowed {
            let idx = lower_out.find(tag)?;
            // Require the tag to terminate, so prose like "a < b" is not counted.
            if out.as_bytes()[idx..].iter().take(200).any(|&b| b == b'>') {
                return Some((idx, tag));
            }
        }
    }
    None
}

fn snippet(text: &str, idx: usize) -> String {
    let start = idx.saturating_sub(40);
    let end = (idx + 80).min(text.len());
    let mut s = start;
    while s < text.len() && !text.is_char_boundary(s) {
        s += 1;
    }
    let mut e = end;
    while e > s && !text.is_char_boundary(e) {
        e -= 1;
    }
    text[s..e].replace('\n', " ")
}

/// Reports per-page extraction stats. Run with `--ignored --nocapture` to
/// inspect how a change affects the whole corpus rather than one page.
#[test]
#[ignore = "reporting harness; run manually with --nocapture"]
fn corpus_report() {
    let pages = load_corpus();
    if pages.is_empty() {
        eprintln!("skipping: set WEBFETCH_CORPUS (see scripts/webfetch_corpus.sh)");
        return;
    }
    println!(
        "{:<20} {:>10} {:>10} {:>7} {:>7}",
        "page", "html", "markdown", "keep%", "noise"
    );
    let mut total_html = 0usize;
    let mut total_md = 0usize;
    for page in &pages {
        let md = html_to_markdown(&page.html);
        let pct = 100.0 * md.len() as f64 / page.html.len().max(1) as f64;
        println!(
            "{:<20} {:>10} {:>10} {:>6.1}% {:>7.3}",
            page.name,
            page.html.len(),
            md.len(),
            pct,
            punctuation_noise_ratio(&md)
        );
        total_html += page.html.len();
        total_md += md.len();
    }
    println!(
        "{:<20} {:>10} {:>10} {:>6.1}%",
        "TOTAL",
        total_html,
        total_md,
        100.0 * total_md as f64 / total_html.max(1) as f64
    );
}

#[test]
fn corpus_script_matches_test_expectations() {
    // Guard against the harness and fetch script drifting apart.
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/webfetch_corpus.sh")
        .canonicalize();
    if let Ok(path) = script {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            body.contains("WEBFETCH_CORPUS") || body.contains("webfetch-corpus"),
            "corpus script should document the directory the tests read"
        );
    }
}

/// Writes post-extraction output for each corpus page to `WEBFETCH_DUMP`, so
/// external tooling (e.g. a tokenizer) can measure real context cost.
#[test]
#[ignore = "dump harness; run manually"]
fn dump_outputs() {
    let Some(out_dir) = std::env::var("WEBFETCH_DUMP").ok() else {
        eprintln!("set WEBFETCH_DUMP");
        return;
    };
    std::fs::create_dir_all(&out_dir).unwrap();
    for page in load_corpus() {
        let md = if is_html(&page.html) {
            html_to_markdown(&page.html)
        } else {
            page.html.clone()
        };
        let (capped, _) = super::truncate_output(md.clone());
        std::fs::write(format!("{out_dir}/{}.uncapped", page.name), &md).unwrap();
        std::fs::write(format!("{out_dir}/{}.capped", page.name), &capped).unwrap();
    }
}
