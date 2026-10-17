//! Cross-width parity probe for the mermaid layout-tier cache (commit
//! 72bd457b, see `mermaid_cache_render.rs`: `LAYOUT_CACHE`,
//! `layout_cache_get`, `render_mermaid_sized_internal`).
//!
//! Every prior parity check (`part_02.rs`
//! `layout_cache_hit_renders_byte_identical_png` and
//! `tests/layout_cache_pixel_parity.rs`) compares a fresh render against a
//! cached-layout render AT THE SAME width, i.e. `raster(L)` vs `raster(L)` of
//! the same `Arc<Layout>` object, and the resize probe only checks counters
//! and speed. This probe closes the remaining core claim: the PRODUCTION
//! resize scenario, where a layout computed during a width-A render is reused
//! at width B, must produce output identical to a fully cold render at width
//! B. Because the cold width-B baseline and the reused layout come from two
//! separate `compute_layout` runs, this also empirically tests
//! `compute_layout` determinism (mermaid-rs-renderer layout code iterates
//! `HashMap`s internally, e.g. `layout/ranking.rs`), which the same-width
//! checks cannot exercise.
//!
//! Per-cell protocol, using only the crate's public API:
//! 1. Render at wide width `WIDE_CELLS` cold -> PNG bytes A (layout-tier
//!    MISS, asserted).
//! 2. `clear_cache()`: clears RENDER_CACHE, the layout tier, and every
//!    on-disk PNG artifact, so the world is fully cold again. NOTE: this
//!    wipes the real shared jcode mermaid cache dir, which is fine for an
//!    explicitly-run probe (the cache is regenerable).
//! 3. Render at narrow width `NARROW_CELLS` (layout-tier MISS, asserted:
//!    proves `clear_cache` cleared the layout tier too).
//! 4. Delete the PNG-tier artifacts for the hash on disk. The memory tier
//!    self-invalidates via the `path.exists()` check in `MermaidCache::get*`;
//!    `discover_on_disk` finds nothing because every fixture embeds a per-run
//!    nonce. The layout tier stays warm.
//! 5. Render at `WIDE_CELLS` again (layout-tier HIT with zero new misses,
//!    asserted) -> PNG bytes B, then require A == B byte-for-byte. On a byte
//!    mismatch the probe decodes both PNGs first: pixel-equal but
//!    byte-different is reported as a PNG-encoder-nondeterminism finding,
//!    while a pixel difference is a real cross-width parity / layout
//!    determinism failure reported with the exact cell, dimensions, first
//!    differing pixel, and differing-pixel bounding box.
//!
//! Cells: flowchart (default profile), sequence (`DiagramData::Sequence`
//! payload, default profile), and flowchart under a non-default aspect
//! profile via `with_preferred_aspect_ratio` (the prior parity matrix only
//! used the default profile).
//!
//! Width choice: with the default 8 px font fallback and the 4-cell width
//! bucket, 90 cells and 200 cells land in PNG width buckets far outside the
//! 85% `CACHE_WIDTH_MATCH_PERCENT` reuse window for both fixture
//! complexities, so step 5 is guaranteed to be a genuine re-rasterize rather
//! than a PNG-cache hit (also enforced by the counter assertions).
//!
//! `#[ignore]`-d: run explicitly, twice for fresh nonces, and once with the
//! legacy SVG-retarget backend (`KCODE_MMDR_SIZE_API_DISABLE=1` is a
//! compile-time toggle consumed by build.rs, so it triggers a rebuild):
//!   cargo test -p jcode-tui-mermaid --test layout_cache_cross_width_parity -- --ignored --nocapture
//!   KCODE_MMDR_SIZE_API_DISABLE=1 cargo test -p jcode-tui-mermaid --test layout_cache_cross_width_parity -- --ignored --nocapture
//! The report header prints `render_size_backend` so runs are
//! distinguishable. Global hit/miss counters are safe to delta-assert because
//! this binary contains exactly one test.

#![cfg(feature = "renderer")]

use std::path::{Path, PathBuf};

use jcode_tui_mermaid::{
    RenderResult, clear_cache, debug_stats, render_mermaid_untracked, with_preferred_aspect_ratio,
};

/// Narrow terminal width (cells): the render whose computed layout gets
/// reused across the width change.
const NARROW_CELLS: u16 = 90;
/// Wide terminal width (cells): the cold baseline and the cached-layout
/// re-render compared for parity.
const WIDE_CELLS: u16 = 200;

/// Flowchart fixture: 6 nodes / 5 edges, exercising branch + merge routing.
const FLOWCHART_TEMPLATE: &str = "flowchart LR\n    A{cell}[Ingest] --> B{cell}{Valid?}\n    B{cell} -->|yes| C{cell}[Store]\n    B{cell} -->|no| D{cell}[Reject]\n    C{cell} --> E{cell}[Index]\n    E{cell} --> F{cell}[Serve]";

/// Sequence fixture: a genuinely different layout path with a
/// `DiagramData::Sequence` payload (lifelines + message arrows).
const SEQUENCE_TEMPLATE: &str = "sequenceDiagram\n    participant U as User{cell}\n    participant S as Server{cell}\n    participant D as Db{cell}\n    U->>S: request {cell}\n    S->>D: query\n    D-->>S: rows\n    S-->>U: response";

struct Rendered {
    hash: u64,
    path: PathBuf,
    bytes: Vec<u8>,
}

/// Render and read back the PNG, panicking on render or read errors.
fn render_png(label: &str, content: &str, width_cells: u16) -> Rendered {
    match render_mermaid_untracked(content, Some(width_cells)) {
        RenderResult::Image { hash, path, .. } => {
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("{label}: read {} failed: {e}", path.display()));
            Rendered { hash, path, bytes }
        }
        RenderResult::Error(error) => {
            panic!("{label}: render at {width_cells} cells failed: {error}")
        }
    }
}

/// Delete every cached PNG for `hash` (all width variants and profile
/// suffixes) from the on-disk cache. The in-memory RENDER_CACHE entry is
/// invalidated on its next lookup because `path.exists()` fails, so this
/// evicts both PNG tiers while leaving the layout tier warm.
fn evict_pngs_for_hash(cache_dir: &Path, hash: u64) -> usize {
    let prefix = format!("{hash:016x}_");
    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_cell_png = path.extension().and_then(|e| e.to_str()) == Some("png")
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix));
            if is_cell_png && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn decode_rgba(label: &str, bytes: &[u8]) -> image::RgbaImage {
    image::load_from_memory(bytes)
        .unwrap_or_else(|e| panic!("{label}: PNG decode failed: {e}"))
        .to_rgba8()
}

/// `None` when the images are pixel-identical; otherwise a precise
/// description of the divergence (dimensions, differing-pixel count, first
/// differing pixel, and the bounding box of all differing pixels).
fn pixel_diff_details(a: &image::RgbaImage, b: &image::RgbaImage) -> Option<String> {
    if a.dimensions() != b.dimensions() {
        let (aw, ah) = a.dimensions();
        let (bw, bh) = b.dimensions();
        return Some(format!("dimensions differ: {aw}x{ah} vs {bw}x{bh}"));
    }
    if a.as_raw() == b.as_raw() {
        return None;
    }
    let (width, height) = a.dimensions();
    let mut first = None;
    let mut count: u64 = 0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0u32, 0u32);
    for y in 0..height {
        for x in 0..width {
            if a.get_pixel(x, y) != b.get_pixel(x, y) {
                if first.is_none() {
                    first = Some((x, y));
                }
                count += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    let (fx, fy) = first.expect("raw buffers differ, so some pixel must differ");
    Some(format!(
        "{count} of {width}x{height} pixels differ; first differing pixel at ({fx},{fy}); \
         differing region x {min_x}..={max_x}, y {min_y}..={max_y}"
    ))
}

struct CellOutcome {
    report: String,
    encoder_finding: Option<String>,
    parity_failure: Option<String>,
}

/// Run the five-step cross-width protocol for one fixture cell. Counter and
/// mechanism assertions panic (probe setup failures); the parity comparison
/// itself is returned so every cell in the matrix gets evaluated and reported
/// before the final verdict.
fn run_cell(label: &str, content: &str, expected_path_marker: Option<&str>) -> CellOutcome {
    // (1) Cold render at the wide width: the parity baseline.
    let stats0 = debug_stats();
    let baseline = render_png(label, content, WIDE_CELLS);
    let stats1 = debug_stats();
    assert_eq!(
        stats1.layout_cache_misses,
        stats0.layout_cache_misses + 1,
        "{label}: cold wide render must compute the layout (layout-tier miss)"
    );
    assert_eq!(
        stats1.layout_cache_hits, stats0.layout_cache_hits,
        "{label}: cold wide render must not hit the layout tier"
    );
    if let Some(marker) = expected_path_marker {
        let name = baseline.path.file_name().and_then(|n| n.to_str());
        assert!(
            name.is_some_and(|n| n.contains(marker)),
            "{label}: non-default profile must be live (cache file {name:?} lacks {marker:?})"
        );
    }

    // (2) Full reset: render cache, layout tier, and disk artifacts all cold.
    clear_cache().unwrap_or_else(|e| panic!("{label}: clear_cache failed: {e}"));
    assert!(
        !baseline.path.exists(),
        "{label}: clear_cache must remove the wide PNG from disk"
    );

    // (3) Narrow render: computes the layout that step 5 will reuse.
    let narrow = render_png(label, content, NARROW_CELLS);
    assert_eq!(
        narrow.hash, baseline.hash,
        "{label}: content hash must be stable across renders"
    );
    let stats2 = debug_stats();
    assert_eq!(
        stats2.layout_cache_misses,
        stats1.layout_cache_misses + 1,
        "{label}: post-clear narrow render must recompute the layout \
         (clear_cache must clear the layout tier)"
    );
    assert_eq!(
        stats2.layout_cache_hits, stats1.layout_cache_hits,
        "{label}: post-clear narrow render must not hit the layout tier"
    );
    assert_ne!(
        narrow.path,
        baseline.path,
        "{label}: narrow and wide renders must land in different PNG width buckets \
         (both mapped to {})",
        narrow.path.display()
    );

    // (4) Evict ONLY the PNG tier; the layout tier stays warm.
    let cache_dir = narrow
        .path
        .parent()
        .expect("cached PNG must live in a cache dir")
        .to_path_buf();
    let removed = evict_pngs_for_hash(&cache_dir, narrow.hash);
    assert!(
        removed >= 1,
        "{label}: PNG-tier eviction must remove at least the narrow PNG (removed {removed})"
    );

    // (5) Wide render again: must reuse the narrow render's layout and
    // rasterize output identical to the cold wide baseline.
    let cached = render_png(label, content, WIDE_CELLS);
    let stats3 = debug_stats();
    assert_eq!(
        stats3.layout_cache_hits,
        stats2.layout_cache_hits + 1,
        "{label}: wide re-render must reuse the layout computed during the narrow render \
         (layout-tier hit)"
    );
    assert_eq!(
        stats3.layout_cache_misses, stats2.layout_cache_misses,
        "{label}: wide re-render must not recompute the layout"
    );
    assert_eq!(
        cached.path, baseline.path,
        "{label}: both wide renders must target the same cache path \
         (same width bucket and profile)"
    );

    let byte_equal = baseline.bytes == cached.bytes;
    let mut encoder_finding = None;
    let mut parity_failure = None;
    let mut pixel_equal = byte_equal;
    if !byte_equal {
        let img_a = decode_rgba(label, &baseline.bytes);
        let img_b = decode_rgba(label, &cached.bytes);
        match pixel_diff_details(&img_a, &img_b) {
            None => {
                pixel_equal = true;
                encoder_finding = Some(format!(
                    "{label}: pixels identical but bytes differ ({} vs {} bytes): \
                     PNG encoder nondeterminism",
                    baseline.bytes.len(),
                    cached.bytes.len()
                ));
            }
            Some(details) => {
                parity_failure = Some(format!(
                    "{label}: cross-width parity failure (cold wide render vs \
                     narrow-computed cached layout at {WIDE_CELLS} cells): {details}"
                ));
            }
        }
    }

    CellOutcome {
        report: format!(
            "{label:<24} hash={:016x} cold-wide={} bytes, cached-layout-wide={} bytes, \
             byte_equal={byte_equal}, pixel_equal={pixel_equal}",
            baseline.hash,
            baseline.bytes.len(),
            cached.bytes.len()
        ),
        encoder_finding,
        parity_failure,
    }
}

#[test]
#[ignore = "cross-width parity probe: run explicitly with --ignored --nocapture"]
fn layout_cache_cross_width_parity() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // (kind, template, aspect profile, expected cache-path marker).
    let cells: [(&str, &str, Option<f32>, Option<&str>); 3] = [
        ("flowchart", FLOWCHART_TEMPLATE, None, None),
        ("sequence", SEQUENCE_TEMPLATE, None, None),
        (
            "flowchart-aspect2.0",
            FLOWCHART_TEMPLATE,
            Some(2.0),
            Some("_a2000"),
        ),
    ];

    let mut reports = Vec::new();
    let mut encoder_findings = Vec::new();
    let mut parity_failures = Vec::new();

    for (index, (kind, template, aspect, path_marker)) in cells.into_iter().enumerate() {
        let cell_token = format!("N{nonce}X{index}");
        let content = template.replace("{cell}", &cell_token);
        let label = format!("{kind}@{NARROW_CELLS}->{WIDE_CELLS}");
        let outcome =
            with_preferred_aspect_ratio(aspect, || run_cell(&label, &content, path_marker));
        reports.push(outcome.report);
        encoder_findings.extend(outcome.encoder_finding);
        parity_failures.extend(outcome.parity_failure);
    }

    eprintln!("--- layout cache cross-width parity probe ---");
    eprintln!(
        "render_size_backend: {} (build profile: {})",
        debug_stats().render_size_backend,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    for line in &reports {
        eprintln!("{line}");
    }
    if encoder_findings.is_empty() {
        eprintln!("encoder determinism: all cells byte-identical");
    } else {
        eprintln!("FINDING: PNG encoder nondeterminism (pixel-equal, byte-different):");
        for line in &encoder_findings {
            eprintln!("  {line}");
        }
    }

    assert_eq!(reports.len(), 3, "probe must cover every matrix cell");
    assert!(
        parity_failures.is_empty(),
        "CROSS-WIDTH PARITY FAILURES (a layout computed at width A, reused at width B, \
         diverged from a fully cold width-B render, i.e. compute_layout nondeterminism \
         is observable in output):\n{}",
        parity_failures.join("\n")
    );
}
