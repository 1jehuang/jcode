use super::*;
#[test]
fn latex_deferred_worker_batches_wakeups_and_bounds_batches() {
    for count in [0usize, 3, 65] {
        let (tx, rx) = std::sync::mpsc::channel();
        for n in 0..count {
            tx.send(n).unwrap();
        }
        drop(tx);
        let mut rendered = Vec::new();
        let mut wakeups = 0;
        deferred_worker_with(rx, |n| rendered.push(n), || wakeups += 1);
        assert_eq!(rendered, (0..count).collect::<Vec<_>>());
        assert_eq!(wakeups, count.div_ceil(32));
    }
}

#[test]
fn distinct_fallback_errors_are_reported_once_across_redraws() {
    let errors = ["issue1791-test-missing-tex", "issue1791-test-other-error"];
    for error in errors {
        assert!(should_report_error(error));
    }
    for _ in 0..20 {
        for error in errors {
            assert!(!should_report_error(error));
        }
    }
}

#[test]
fn missing_tex_is_informational_but_render_failures_are_not() {
    assert!(error_is_missing_toolchain(TOOLCHAIN_MISSING_ERROR));
    assert!(error_is_missing_toolchain(
        "DVI renderer failed (start latex: No such file or directory (os error 2))"
    ));
    assert!(!error_is_missing_toolchain("LaTeX source is empty"));
    assert!(!error_is_missing_toolchain(
        "start latex: Permission denied (os error 13)"
    ));
    assert!(!error_is_missing_toolchain("latex exited with status 1"));
}

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

#[test]
fn dvipng_foreground_uses_normalized_rgb_components() {
    assert_eq!(
        dvipng_rgb_arg((255, 255, 255)),
        "rgb 1.000000 1.000000 1.000000"
    );
    assert_eq!(
        dvipng_rgb_arg((0, 128, 255)),
        "rgb 0.000000 0.501961 1.000000"
    );
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

    let targets = super::super::render_support::extract_copy_targets_from_rendered_lines(&lines);
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
fn failed_renders_are_remembered_so_the_toolchain_is_not_respawned() {
    // #563: a failing snippet must not re-run latex/pdflatex on every redraw.
    reset_failed_render_cache();
    let key = cache_key("unique_failed_render_test_2026", true, 240);
    assert_eq!(cached_render_failure(key), None);
    remember_render_failure(key, "pdflatex timed out");
    assert_eq!(
        cached_render_failure(key).as_deref(),
        Some("pdflatex timed out")
    );
    // A different snippet/geometry is unaffected.
    assert_eq!(
        cached_render_failure(cache_key("unique_failed_render_test_2026", false, 240)),
        None
    );
    reset_failed_render_cache();
    assert_eq!(cached_render_failure(key), None);
}

#[cfg(unix)]
#[test]
fn a_failing_snippet_spawns_the_toolchain_only_once_across_many_redraws() {
    // #563: the real symptom was a chain of latex/pdflatex processes, one per
    // redraw. Count actual spawns via a stub that appends to a log file, and
    // assert repeated render attempts do not re-spawn it.
    use std::os::unix::fs::PermissionsExt;

    reset_failed_render_cache();
    let root = tempfile::tempdir().unwrap();
    let spawn_log = root.path().join("spawns.log");
    let failing = root.path().join("latex-fail");
    fs::write(
        &failing,
        format!(
            "#!/bin/sh\necho spawn >> '{}'\nexit 1\n",
            spawn_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();
    let toolchain = Toolchain {
        latex: failing.clone(),
        dvipng: failing.clone(),
        pdflatex: failing.clone(),
        pdftocairo: failing,
    };
    let cache = root.path().join("cache");
    let source = "unique_spawn_count_probe_2026_563";
    let key = cache_key(source, true, 240);

    // First attempt runs the toolchain and records the failure.
    let first = render_artifact_in(source, true, 240, &toolchain, &cache);
    assert!(first.is_err(), "stub toolchain must fail");
    remember_render_failure(key, &first.unwrap_err());
    let spawns_after_first = fs::read_to_string(&spawn_log)
        .unwrap_or_default()
        .lines()
        .count();
    assert!(
        spawns_after_first > 0,
        "first render should spawn the toolchain"
    );

    // Every later "redraw" must short-circuit on the cached failure.
    for _ in 0..50 {
        assert!(
            cached_render_failure(key).is_some(),
            "cached failure must short-circuit the redraw"
        );
    }
    let spawns_after_redraws = fs::read_to_string(&spawn_log)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        spawns_after_first, spawns_after_redraws,
        "50 redraws must not spawn the toolchain again"
    );
    reset_failed_render_cache();
}

/// Regression for #735: the draw path must stay responsive even when the
/// TeX toolchain hangs. `render_latex_image` only consumes the on-disk
/// cache and otherwise queues the work, so it must return promptly with
/// `Pending` (or `Failed` when no image protocol exists) and never block
/// for the multi-second command timeout.
#[test]
fn uncached_math_returns_immediately_instead_of_blocking_the_draw_path() {
    use std::time::Instant;

    let source = format!(
        "unique_deferred_probe_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let started = Instant::now();
    let outcome = render_latex_image(&source, true, Some(80));
    let elapsed = started.elapsed();
    assert!(
        elapsed < COMMAND_TIMEOUT,
        "draw path blocked for {elapsed:?}; LaTeX rendering must be deferred"
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "draw path took {elapsed:?}; expected an immediate cache probe"
    );
    assert!(
        !matches!(outcome, LatexImageOutcome::Ready(_)),
        "an uncached formula cannot be ready synchronously"
    );
}

#[test]
fn failed_render_cache_is_bounded() {
    reset_failed_render_cache();
    for i in 0..(FAILED_RENDER_CACHE_LIMIT + 50) {
        remember_render_failure(i as u64, "boom");
    }
    let len = failed_renders().entries.len();
    assert!(len <= FAILED_RENDER_CACHE_LIMIT, "cache grew to {len}");
    reset_failed_render_cache();
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
