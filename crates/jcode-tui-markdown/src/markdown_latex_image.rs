use image::GenericImageView;
use ratatui::prelude::{Line, Modifier, Span, Style};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use wait_timeout::ChildExt;

const RENDERER_VERSION: u8 = 4;
const MAX_SOURCE_BYTES: usize = 32 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const FOREGROUND: (u8, u8, u8) = super::MATH_FOREGROUND;
const FALLBACK_RENDER_DPI: u16 = 240;
const MIN_RENDER_DPI: u16 = 240;
const MAX_RENDER_DPI: u16 = 480;
const DPI_PER_CELL_PIXEL: u16 = 9;
const DPI_QUANTUM: u16 = 12;

static LOG_HOOK: LazyLock<Mutex<fn(&str)>> = LazyLock::new(|| Mutex::new(|_| {}));
static LAST_REPORTED_ERROR: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
const COPY_SOURCE_CACHE_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LatexCopySource {
    pub source: String,
    pub display: bool,
}

#[derive(Default)]
struct CopySourceCache {
    entries: HashMap<u64, LatexCopySource>,
    order: VecDeque<u64>,
}

static COPY_SOURCES: LazyLock<Mutex<CopySourceCache>> =
    LazyLock::new(|| Mutex::new(CopySourceCache::default()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandtermNativeLatex {
    pub source: String,
    pub display: bool,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Default)]
struct HandtermNativeLatexCache {
    entries: HashMap<u64, HandtermNativeLatex>,
    order: VecDeque<u64>,
}

static HANDTERM_NATIVE_LATEX: LazyLock<Mutex<HandtermNativeLatexCache>> =
    LazyLock::new(|| Mutex::new(HandtermNativeLatexCache::default()));

#[cfg(test)]
thread_local! {
    static TEST_HANDTERM_NATIVE_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
thread_local! {
    static TEST_RENDER_ATTEMPTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_test_render_attempts() {
    TEST_RENDER_ATTEMPTS.with(|attempts| attempts.set(0));
}

#[cfg(test)]
pub(super) fn test_render_attempts() -> u64 {
    TEST_RENDER_ATTEMPTS.with(std::cell::Cell::get)
}

pub(crate) fn set_log_hook(hook: fn(&str)) {
    if let Ok(mut current) = LOG_HOOK.lock() {
        *current = hook;
    }
}

pub(crate) fn report_error(error: &str) {
    let should_report = LAST_REPORTED_ERROR
        .lock()
        .map(|mut last| {
            if last.as_deref() == Some(error) {
                false
            } else {
                *last = Some(error.to_string());
                true
            }
        })
        .unwrap_or(false);
    if should_report && let Ok(hook) = LOG_HOOK.lock() {
        hook(error);
    }
}

fn handterm_terminal_name(term_program: Option<&str>) -> bool {
    term_program.is_some_and(|value| value.eq_ignore_ascii_case("handterm"))
}

fn handterm_native_latex_available() -> bool {
    #[cfg(test)]
    if let Some(enabled) = TEST_HANDTERM_NATIVE_OVERRIDE.with(std::cell::Cell::get) {
        return enabled;
    }

    handterm_terminal_name(std::env::var("TERM_PROGRAM").ok().as_deref())
}

#[cfg(test)]
pub(super) fn with_handterm_native_latex_override<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
    TEST_HANDTERM_NATIVE_OVERRIDE.with(|override_value| {
        let previous = override_value.replace(Some(enabled));
        struct ResetOverride<'a> {
            value: &'a std::cell::Cell<Option<bool>>,
            previous: Option<bool>,
        }

        impl Drop for ResetOverride<'_> {
            fn drop(&mut self) {
                self.value.set(self.previous);
            }
        }

        let _reset = ResetOverride {
            value: override_value,
            previous,
        };
        f()
    })
}

pub fn encode_handterm_latex_apc(source: &str) -> Option<String> {
    if source.bytes().any(|byte| matches!(byte, 0x07 | 0x1b)) {
        return None;
    }
    Some(format!("\x1b_L;{source}\x1b\\"))
}

pub fn handterm_native_latex_for_hash(hash: u64) -> Option<HandtermNativeLatex> {
    HANDTERM_NATIVE_LATEX
        .lock()
        .ok()
        .and_then(|cache| cache.entries.get(&hash).cloned())
}

pub(super) fn render_handterm_native_latex(
    source: &str,
    display: bool,
    max_width: Option<usize>,
) -> Option<Vec<Line<'static>>> {
    if !handterm_native_latex_available() || validate_source(source).is_err() {
        return None;
    }
    encode_handterm_latex_apc(source)?;

    // Use the same library and version as Handterm so the placeholder geometry
    // exactly matches the native APC result. Unsupported expressions retain the
    // existing image/Unicode fallback instead of risking a mismatched region.
    let rendered = mdwright_latex::render_unicode_math(source).ok()?;
    let rows = rendered.lines().len().max(1);
    let cols = rendered.width().max(1);
    let width_limit = max_width
        .unwrap_or(u16::MAX as usize)
        .min(u16::MAX as usize);
    if rows > u16::MAX as usize || cols > width_limit {
        return None;
    }
    let rows = rows as u16;
    let cols = cols as u16;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "handterm-native-latex-v1".hash(&mut hasher);
    source.hash(&mut hasher);
    display.hash(&mut hasher);
    rows.hash(&mut hasher);
    cols.hash(&mut hasher);
    let hash = hasher.finish();

    register_copy_source(hash, source, display);
    if let Ok(mut cache) = HANDTERM_NATIVE_LATEX.lock() {
        if cache.entries.contains_key(&hash) {
            cache.order.retain(|entry| *entry != hash);
        }
        cache.entries.insert(
            hash,
            HandtermNativeLatex {
                source: source.to_string(),
                display,
                rows,
                cols,
            },
        );
        cache.order.push_back(hash);
        while cache.order.len() > COPY_SOURCE_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    }

    let mut lines = vec![Line::from(Span::styled(
        "  math",
        Style::default().add_modifier(Modifier::DIM),
    ))];
    lines.extend(super::mermaid::inline_image_placeholder_lines(
        hash, rows, cols,
    ));
    Some(lines)
}

#[derive(Debug, Clone)]
struct Toolchain {
    latex: PathBuf,
    dvipng: PathBuf,
    pdflatex: PathBuf,
    pdftocairo: PathBuf,
}

impl Toolchain {
    fn from_environment() -> Self {
        Self {
            latex: std::env::var_os("JCODE_LATEX_COMMAND")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("latex")),
            dvipng: std::env::var_os("JCODE_DVIPNG_COMMAND")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("dvipng")),
            pdflatex: std::env::var_os("JCODE_PDFLATEX_COMMAND")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("pdflatex")),
            pdftocairo: std::env::var_os("JCODE_PDFTOCAIRO_COMMAND")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("pdftocairo")),
        }
    }
}

pub(super) fn render_latex_image(
    source: &str,
    display: bool,
    max_width: Option<usize>,
) -> Result<Vec<Line<'static>>, String> {
    #[cfg(test)]
    TEST_RENDER_ATTEMPTS.with(|attempts| attempts.set(attempts.get().saturating_add(1)));
    if !super::mermaid::image_protocol_available() {
        return Err("terminal image protocol unavailable".to_string());
    }
    let dpi = render_dpi(super::mermaid::get_font_size());
    let artifact = render_artifact(source, display, dpi, &Toolchain::from_environment())?;
    let hash =
        super::mermaid::register_external_image(&artifact.path, artifact.width, artifact.height);
    register_copy_source(hash, source, display);
    let mut lines = super::mermaid::result_to_lines(
        super::mermaid::RenderResult::Image {
            hash,
            path: artifact.path,
            width: artifact.width,
            height: artifact.height,
        },
        max_width,
    );
    // Keep a real text row above the image. It gives copy badges a stable host
    // instead of placing them on the invisible marker row that the viewport
    // deliberately clears before drawing terminal graphics.
    lines.insert(
        0,
        Line::from(Span::styled(
            "  math",
            Style::default().add_modifier(Modifier::DIM),
        )),
    );
    Ok(lines)
}

fn register_copy_source(hash: u64, source: &str, display: bool) {
    let Ok(mut cache) = COPY_SOURCES.lock() else {
        return;
    };
    if cache.entries.contains_key(&hash) {
        cache.order.retain(|entry| *entry != hash);
    }
    cache.entries.insert(
        hash,
        LatexCopySource {
            source: source.trim().to_string(),
            display,
        },
    );
    cache.order.push_back(hash);
    while cache.order.len() > COPY_SOURCE_CACHE_LIMIT {
        if let Some(oldest) = cache.order.pop_front() {
            cache.entries.remove(&oldest);
        }
    }
}

pub(super) fn copy_source_for_placeholder(line: &Line<'_>) -> Option<LatexCopySource> {
    let hash = super::mermaid::parse_inline_image_placeholder(line)
        .map(|(hash, _, _)| hash)
        .or_else(|| super::mermaid::parse_image_placeholder(line))?;
    COPY_SOURCES
        .lock()
        .ok()
        .and_then(|cache| cache.entries.get(&hash).cloned())
}

pub(super) fn copy_text(source: &LatexCopySource) -> String {
    if source.display {
        format!("$$\n{}\n$$", source.source)
    } else {
        format!("${}$", source.source)
    }
}

#[derive(Debug)]
struct Artifact {
    path: PathBuf,
    width: u32,
    height: u32,
}

fn render_dpi(font_size: Option<(u16, u16)>) -> u16 {
    let Some((_, cell_height)) = font_size else {
        return FALLBACK_RENDER_DPI;
    };
    // Computer Modern's visible ink is roughly 8/72 of the requested DPI for
    // ordinary 10pt symbols. Nine DPI per terminal-row pixel therefore keeps
    // simple math close to one full row of ink instead of letting it become
    // smaller as users increase their terminal font size. Quantizing avoids
    // producing redundant cache entries for tiny geometry differences.
    let raw = cell_height
        .max(1)
        .saturating_mul(DPI_PER_CELL_PIXEL)
        .clamp(MIN_RENDER_DPI, MAX_RENDER_DPI);
    raw.saturating_add(DPI_QUANTUM / 2) / DPI_QUANTUM * DPI_QUANTUM
}

fn render_artifact(
    source: &str,
    display: bool,
    dpi: u16,
    toolchain: &Toolchain,
) -> Result<Artifact, String> {
    render_artifact_in(source, display, dpi, toolchain, &cache_dir()?)
}

fn render_artifact_in(
    source: &str,
    display: bool,
    dpi: u16,
    toolchain: &Toolchain,
    cache_dir: &Path,
) -> Result<Artifact, String> {
    validate_source(source)?;
    fs::create_dir_all(cache_dir).map_err(|e| format!("create LaTeX cache: {e}"))?;
    let cache_path = cache_dir.join(format!("{:016x}.png", cache_key(source, display, dpi)));
    if let Ok(artifact) = load_artifact(&cache_path) {
        return Ok(artifact);
    }

    let work = tempfile::Builder::new()
        .prefix("jcode-latex-")
        .tempdir()
        .map_err(|e| format!("create LaTeX workspace: {e}"))?;
    let tex_path = work.path().join("formula.tex");
    fs::write(&tex_path, latex_document(source, display))
        .map_err(|e| format!("write LaTeX source: {e}"))?;

    let dpi_arg = dpi.to_string();
    let foreground_arg = format!("rgb {} {} {}", FOREGROUND.0, FOREGROUND.1, FOREGROUND.2);
    let dvi_result = run_command(
        &toolchain.latex,
        [
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-no-shell-escape",
            "formula.tex",
        ],
        work.path(),
    )
    .and_then(|_| {
        run_command(
            &toolchain.dvipng,
            [
                "-D",
                dpi_arg.as_str(),
                "-T",
                "tight",
                "-bg",
                "Transparent",
                "-fg",
                foreground_arg.as_str(),
                "-o",
                "formula.png",
                "formula.dvi",
            ],
            work.path(),
        )
    });
    if let Err(dvi_error) = dvi_result {
        render_with_pdf_toolchain(toolchain, work.path(), dpi).map_err(|pdf_error| {
            format!("DVI renderer failed ({dvi_error}); PDF renderer failed ({pdf_error})")
        })?;
    }

    let rendered = work.path().join("formula.png");
    load_artifact(&rendered)?;
    let temporary_cache_path = cache_path.with_extension(format!("{}.tmp", std::process::id()));
    fs::copy(&rendered, &temporary_cache_path).map_err(|e| format!("cache rendered LaTeX: {e}"))?;
    if let Err(error) = fs::rename(&temporary_cache_path, &cache_path) {
        if !cache_path.exists() {
            let _ = fs::remove_file(&temporary_cache_path);
            return Err(format!("publish rendered LaTeX: {error}"));
        }
        let _ = fs::remove_file(&temporary_cache_path);
    }
    load_artifact(&cache_path)
}

fn render_with_pdf_toolchain(
    toolchain: &Toolchain,
    working_dir: &Path,
    dpi: u16,
) -> Result<(), String> {
    run_command(
        &toolchain.pdflatex,
        [
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-no-shell-escape",
            "formula.tex",
        ],
        working_dir,
    )?;
    let dpi_arg = dpi.to_string();
    run_command(
        &toolchain.pdftocairo,
        [
            "-png",
            "-singlefile",
            "-r",
            dpi_arg.as_str(),
            "formula.pdf",
            "formula",
        ],
        working_dir,
    )?;
    recolor_and_crop(&working_dir.join("formula.png"), dpi)
}

fn recolor_and_crop(path: &Path, dpi: u16) -> Result<(), String> {
    let image = image::open(path)
        .map_err(|e| format!("read PDF-rendered LaTeX PNG: {e}"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in image.enumerate_pixels() {
        let luminance =
            (u16::from(pixel[0]) * 54 + u16::from(pixel[1]) * 183 + u16::from(pixel[2]) * 19) / 256;
        if pixel[3] > 0 && luminance < 250 {
            bounds = Some(match bounds {
                Some((min_x, min_y, max_x, max_y)) => {
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                }
                None => (x, y, x, y),
            });
        }
    }
    let (min_x, min_y, max_x, max_y) =
        bounds.ok_or_else(|| "rendered formula is blank".to_string())?;
    let padding = u32::from(dpi).saturating_mul(4).div_ceil(180).max(4);
    let left = min_x.saturating_sub(padding);
    let top = min_y.saturating_sub(padding);
    let right = max_x.saturating_add(padding).min(width.saturating_sub(1));
    let bottom = max_y.saturating_add(padding).min(height.saturating_sub(1));
    let mut cropped = image::imageops::crop_imm(
        &image,
        left,
        top,
        right.saturating_sub(left).saturating_add(1),
        bottom.saturating_sub(top).saturating_add(1),
    )
    .to_image();
    for pixel in cropped.pixels_mut() {
        let luminance =
            (u16::from(pixel[0]) * 54 + u16::from(pixel[1]) * 183 + u16::from(pixel[2]) * 19) / 256;
        let alpha = 255u16.saturating_sub(luminance) as u8;
        *pixel = image::Rgba([FOREGROUND.0, FOREGROUND.1, FOREGROUND.2, alpha]);
    }
    cropped
        .save(path)
        .map_err(|e| format!("write cropped LaTeX PNG: {e}"))
}

fn cache_dir() -> Result<PathBuf, String> {
    dirs::cache_dir()
        .map(|path| path.join("jcode").join("latex"))
        .ok_or_else(|| "no user cache directory is available".to_string())
}

fn load_artifact(path: &Path) -> Result<Artifact, String> {
    let image = image::open(path).map_err(|e| format!("read rendered LaTeX PNG: {e}"))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err("rendered LaTeX PNG is empty".to_string());
    }
    Ok(Artifact {
        path: path.to_path_buf(),
        width,
        height,
    })
}

fn run_command<const N: usize>(
    executable: &Path,
    args: [&str; N],
    working_dir: &Path,
) -> Result<(), String> {
    let output_path = working_dir.join(".jcode-command-output.log");
    let stdout = File::create(&output_path)
        .map_err(|e| format!("create {} diagnostics: {e}", executable.display()))?;
    let stderr = stdout
        .try_clone()
        .map_err(|e| format!("capture {} diagnostics: {e}", executable.display()))?;
    let mut child = Command::new(executable)
        .args(args)
        .current_dir(working_dir)
        .env("openin_any", "p")
        .env("openout_any", "p")
        .env("TEXMFOUTPUT", working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| format!("start {}: {e}", executable.display()))?;
    match child
        .wait_timeout(COMMAND_TIMEOUT)
        .map_err(|e| format!("wait for {}: {e}", executable.display()))?
    {
        Some(status) if status.success() => {
            let _ = fs::remove_file(&output_path);
            Ok(())
        }
        Some(status) => Err(format!(
            "{} exited with {status}: {}",
            executable.display(),
            command_diagnostics(&output_path)
        )),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("{} timed out", executable.display()))
        }
    }
}

fn command_diagnostics(path: &Path) -> String {
    const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;
    let Ok(output) = fs::read(path) else {
        return "no diagnostic output".to_string();
    };
    let start = output.len().saturating_sub(MAX_DIAGNOSTIC_BYTES);
    String::from_utf8_lossy(&output[start..])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn cache_key(source: &str, display: bool, dpi: u16) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    RENDERER_VERSION.hash(&mut hasher);
    source.hash(&mut hasher);
    display.hash(&mut hasher);
    dpi.hash(&mut hasher);
    FOREGROUND.hash(&mut hasher);
    hasher.finish()
}

fn latex_document(source: &str, display: bool) -> String {
    let source = source.trim();
    let math = if display {
        format!("\\[\\displaystyle\n{source}\n\\]")
    } else {
        format!("${source}$")
    };
    format!(
        "\\documentclass{{article}}\n\\pagestyle{{empty}}\n\\usepackage{{amsmath,amssymb}}\n\\begin{{document}}\n\\noindent {math}\n\\end{{document}}\n"
    )
}

fn validate_source(source: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("LaTeX source is empty".to_string());
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!("LaTeX source exceeds {MAX_SOURCE_BYTES} bytes"));
    }
    let lowered = source.to_ascii_lowercase();
    const FORBIDDEN: &[&str] = &[
        "\\input",
        "\\include",
        "\\openin",
        "\\openout",
        "\\read",
        "\\write",
        "\\immediate",
        "\\usepackage",
        "\\documentclass",
        "\\special",
        "\\catcode",
    ];
    if let Some(command) = FORBIDDEN.iter().find(|command| lowered.contains(**command)) {
        return Err(format!("unsafe LaTeX command is not allowed: {command}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_wraps_inline_and_display_math_without_shell_escape() {
        let inline = latex_document(r"x^2 + \\alpha", false);
        assert!(inline.contains(r"$x^2 + \\alpha$"));
        assert!(!inline.contains("shell-escape"));
        let display = latex_document("\n\\frac{a}{b}\n", true);
        assert!(display.contains("\\[\\displaystyle"));
        assert!(display.contains(r"\frac{a}{b}"));
        assert!(display.contains("\\displaystyle\n\\frac{a}{b}\n\\]"));
        assert!(!display.contains("\\displaystyle\n\n"));
    }

    #[test]
    fn cache_key_is_stable_and_separates_inline_from_display() {
        assert_eq!(cache_key("x", false, 240), cache_key("x", false, 240));
        assert_ne!(cache_key("x", false, 240), cache_key("x", true, 240));
        assert_ne!(cache_key("x", false, 240), cache_key("y", false, 240));
        assert_ne!(cache_key("x", false, 240), cache_key("x", false, 312));
    }

    #[cfg(feature = "mermaid-renderer")]
    #[test]
    fn image_placeholder_extracts_math_copy_target_with_source_delimiters() {
        let hash = 0x1a7e_c0de_u64;
        register_copy_source(hash, r"\frac{x+1}{y}", true);
        let mut lines = vec![Line::from("  math")];
        lines.extend(super::super::mermaid::inline_image_placeholder_lines(
            hash, 3, 20,
        ));
        lines.push(super::super::mermaid::text_image_fallback_note_line());

        let targets =
            super::super::render_support::extract_copy_targets_from_rendered_lines(&lines);
        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(
            target.kind,
            super::super::CopyTargetKind::Math { display: true }
        );
        assert_eq!(target.content, "$$\n\\frac{x+1}{y}\n$$");
        assert_eq!(target.start_raw_line, 0);
        assert_eq!(target.end_raw_line, 4);
        assert_eq!(target.badge_raw_line, 0);
        assert!(
            super::super::render_support::line_plain_text(&lines[4])
                .contains(super::super::mermaid::TERMINAL_IMAGE_FALLBACK_NOTE),
            "the fallback note should remain outside the math copy target"
        );
    }

    #[test]
    fn inline_math_copy_text_preserves_inline_delimiters() {
        let source = LatexCopySource {
            source: r"x^2 + \alpha".to_string(),
            display: false,
        };
        assert_eq!(copy_text(&source), r"$x^2 + \alpha$");
    }

    #[test]
    fn render_dpi_tracks_terminal_cell_height_with_readable_bounds() {
        assert_eq!(render_dpi(None), 240);
        assert_eq!(render_dpi(Some((8, 16))), 240);
        assert_eq!(render_dpi(Some((15, 34))), 312);
        assert_eq!(render_dpi(Some((20, 60))), 480);
    }

    #[test]
    fn unsafe_empty_and_oversized_sources_are_rejected() {
        assert!(validate_source(" ").is_err());
        assert!(validate_source(r"\\input{/etc/passwd}").is_err());
        assert!(validate_source(&"x".repeat(MAX_SOURCE_BYTES + 1)).is_err());
        assert!(validate_source(r"\\frac{x}{y}").is_ok());
    }

    #[test]
    fn handterm_terminal_detection_is_explicit_and_case_insensitive() {
        assert!(handterm_terminal_name(Some("handterm")));
        assert!(handterm_terminal_name(Some("HandTerm")));
        assert!(!handterm_terminal_name(Some("kitty")));
        assert!(!handterm_terminal_name(None));
    }

    #[test]
    fn handterm_apc_encoding_is_byte_exact_and_rejects_terminators() {
        assert_eq!(
            encode_handterm_latex_apc(r"\sqrt{x^2+y^2}").as_deref(),
            Some("\x1b_L;\\sqrt{x^2+y^2}\x1b\\")
        );
        assert!(encode_handterm_latex_apc("x\x1by").is_none());
        assert!(encode_handterm_latex_apc("x\x07y").is_none());
    }

    #[test]
    fn handterm_native_render_registers_exact_layout_without_external_toolchain() {
        reset_test_render_attempts();
        let lines = with_handterm_native_latex_override(true, || {
            render_handterm_native_latex(r"\frac{a}{b}", true, Some(80))
                .expect("supported math should use Handterm native rendering")
        });

        assert_eq!(test_render_attempts(), 0);
        assert_eq!(lines[0].spans[0].content.as_ref(), "  math");
        let (hash, rows, cols) = super::super::mermaid::parse_inline_image_placeholder(&lines[1])
            .expect("native math should use a reserved inline placeholder");
        assert_eq!((rows, cols), (3, 1));
        assert_eq!(lines.len(), 4);
        assert_eq!(
            handterm_native_latex_for_hash(hash),
            Some(HandtermNativeLatex {
                source: r"\frac{a}{b}".to_string(),
                display: true,
                rows: 3,
                cols: 1,
            })
        );
        let copy = copy_source_for_placeholder(&lines[1]).expect("copy source should be retained");
        assert_eq!(copy_text(&copy), "$$\n\\frac{a}{b}\n$$");
    }

    #[test]
    fn latex_image_dispatch_prefers_handterm_native_rendering() {
        reset_test_render_attempts();
        let lines = with_handterm_native_latex_override(true, || {
            super::super::latex_image_lines(r"\sqrt{x}", false, Some(80))
                .expect("native Handterm rendering should produce reserved lines")
        });

        assert_eq!(test_render_attempts(), 0);
        let (hash, rows, cols) = super::super::mermaid::parse_inline_image_placeholder(&lines[1])
            .expect("dispatch should return a native placeholder");
        assert_eq!((rows, cols), (1, 2));
        assert_eq!(
            handterm_native_latex_for_hash(hash).map(|native| native.source),
            Some(r"\sqrt{x}".to_string())
        );
    }

    #[test]
    fn non_handterm_and_unsupported_math_keep_existing_fallback_path() {
        assert!(with_handterm_native_latex_override(false, || {
            render_handterm_native_latex("x^2", false, Some(80)).is_none()
        }));
        assert!(with_handterm_native_latex_override(true, || {
            render_handterm_native_latex(r"\color{red}{x}", false, Some(80)).is_none()
        }));
        assert!(with_handterm_native_latex_override(true, || {
            render_handterm_native_latex(r"\frac{a}{b}", true, Some(0)).is_none()
        }));
    }

    #[test]
    fn missing_toolchain_returns_an_error_without_panicking() {
        let cache = tempfile::tempdir().unwrap();
        let missing = PathBuf::from("/definitely/missing/jcode-latex-command");
        let result = render_artifact_in(
            "unique_missing_toolchain_test_4815162342",
            false,
            240,
            &Toolchain {
                latex: missing.clone(),
                dvipng: missing.clone(),
                pdflatex: missing.clone(),
                pdftocairo: missing,
            },
            cache.path(),
        );
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn toolchain_output_is_validated_cached_and_reused() {
        use image::{ImageBuffer, Rgba};
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("fixture.png");
        ImageBuffer::from_pixel(7, 3, Rgba([130u8, 210, 235, 255]))
            .save(&fixture)
            .unwrap();
        let latex = root.path().join("latex-ok");
        let dvipng = root.path().join("dvipng-ok");
        fs::write(&latex, "#!/bin/sh\n: > formula.dvi\n").unwrap();
        fs::write(
            &dvipng,
            format!("#!/bin/sh\ncp '{}' formula.png\n", fixture.display()),
        )
        .unwrap();
        fs::set_permissions(&latex, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&dvipng, fs::Permissions::from_mode(0o755)).unwrap();
        let cache = root.path().join("cache");
        let artifact = render_artifact_in(
            r"\frac{x+1}{y}",
            true,
            240,
            &Toolchain {
                latex,
                dvipng,
                pdflatex: PathBuf::from("/unused/pdflatex"),
                pdftocairo: PathBuf::from("/unused/pdftocairo"),
            },
            &cache,
        )
        .unwrap();
        assert_eq!((artifact.width, artifact.height), (7, 3));
        assert!(artifact.path.starts_with(&cache));

        let missing = PathBuf::from("/definitely/missing/jcode-latex-command");
        let cached = render_artifact_in(
            r"\frac{x+1}{y}",
            true,
            240,
            &Toolchain {
                latex: missing.clone(),
                dvipng: missing.clone(),
                pdflatex: missing.clone(),
                pdftocairo: missing,
            },
            &cache,
        )
        .expect("the second render should use the validated PNG cache");
        assert_eq!((cached.width, cached.height), (7, 3));
        assert_eq!(cached.path, artifact.path);
    }

    #[cfg(unix)]
    #[test]
    fn pdf_fallback_crops_recolors_and_produces_a_cached_png() {
        use image::{ImageBuffer, Rgba};
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("pdf-page.png");
        let mut page = ImageBuffer::from_pixel(20, 10, Rgba([255u8, 255, 255, 255]));
        for y in 2..=6 {
            for x in 5..=10 {
                page.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        page.save(&fixture).unwrap();

        let failing_latex = root.path().join("latex-fail");
        let unused_dvipng = root.path().join("dvipng-unused");
        let pdflatex = root.path().join("pdflatex-ok");
        let pdftocairo = root.path().join("pdftocairo-ok");
        fs::write(&failing_latex, "#!/bin/sh\nexit 1\n").unwrap();
        fs::write(&unused_dvipng, "#!/bin/sh\nexit 99\n").unwrap();
        fs::write(&pdflatex, "#!/bin/sh\n: > formula.pdf\n").unwrap();
        fs::write(
            &pdftocairo,
            format!("#!/bin/sh\ncp '{}' formula.png\n", fixture.display()),
        )
        .unwrap();
        for path in [&failing_latex, &unused_dvipng, &pdflatex, &pdftocairo] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let artifact = render_artifact_in(
            r"x^2 + \alpha",
            false,
            240,
            &Toolchain {
                latex: failing_latex,
                dvipng: unused_dvipng,
                pdflatex,
                pdftocairo,
            },
            &root.path().join("cache"),
        )
        .unwrap();
        assert!(artifact.width < 20, "white page margins should be cropped");
        assert!(artifact.height <= 10);
        let rendered = image::open(&artifact.path).unwrap().into_rgba8();
        assert!(rendered.pixels().any(|pixel| pixel[3] == 255));
        assert!(rendered.pixels().all(|pixel| {
            [pixel[0], pixel[1], pixel[2]] == [FOREGROUND.0, FOREGROUND.1, FOREGROUND.2]
        }));
    }

    #[test]
    fn installed_toolchain_renders_gaussian_integral_when_available() {
        let toolchain = Toolchain::from_environment();
        let has_pdf_fallback = Command::new(&toolchain.pdflatex)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
            && Command::new(&toolchain.pdftocairo)
                .arg("-v")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
        if !has_pdf_fallback {
            return;
        }
        let cache = tempfile::tempdir().unwrap();
        let artifact = render_artifact_in(
            "\n\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}\n",
            true,
            312,
            &toolchain,
            cache.path(),
        )
        .expect("installed PDF toolchain should render the Gaussian integral");
        assert!(artifact.width > 0 && artifact.height > 0);
    }

    #[test]
    fn installed_toolchain_scales_simple_math_for_tall_terminal_cells_when_available() {
        let toolchain = Toolchain::from_environment();
        let has_pdf_fallback = Command::new(&toolchain.pdflatex)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
            && Command::new(&toolchain.pdftocairo)
                .arg("-v")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
        if !has_pdf_fallback {
            return;
        }

        let cache = tempfile::tempdir().unwrap();
        let baseline = render_artifact_in("x", false, 180, &toolchain, cache.path())
            .expect("installed toolchain should render baseline inline math");
        let readable = render_artifact_in("x", false, 312, &toolchain, cache.path())
            .expect("installed toolchain should render cell-aware inline math");

        assert!(readable.width > baseline.width);
        assert!(readable.height > baseline.height);
        assert!(
            readable.height * 10 >= baseline.height * 14,
            "312 DPI should materially increase ink height: baseline={} readable={}",
            baseline.height,
            readable.height
        );
    }
}
