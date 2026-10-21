use super::*;

#[test]
fn source_guard_rejects_imports_and_asset_forms_before_compilation() {
    for source in [
        "...@/private/example",
        "node: @other",
        "node.icon: /private/example.svg",
        "node.\"icon\": https://example.invalid/icon.svg",
        "node: { shape: image }",
        "node.\\icon: /private/example.svg",
        "node: |md\n![image](example.png)\n|",
        "node: ${asset}",
        "node.*: /private/example.svg",
        "node.i?on: /private/example.svg",
        r#"node."i""con": /private/example.svg"#,
        r#"node.'i''con': /private/example.svg"#,
        "node.IcOn: x",
        "node.İcon: /private/example.svg",
        "node: { İMAGE: x }",
        "# an image is rejected even in comments",
    ] {
        assert!(validate_d2_source(source).is_err(), "accepted {source:?}");
    }
}

#[test]
fn source_guard_accepts_self_contained_graphs_and_tala_positions() {
    for source in [
        "a -> b: SVG",
        "direction: right\nclient: Terminal\nclient -> renderer",
        "client: { top: 100; left: 100 }\nclient -> renderer",
        "日本語: ラベル\n日本語 -> renderer",
    ] {
        assert!(validate_d2_source(source).is_ok(), "rejected {source:?}");
    }
}

#[test]
fn source_guard_bounds_input_and_control_characters() {
    assert!(validate_d2_source("").is_err());
    assert!(validate_d2_source(&"a".repeat(MAX_D2_SOURCE_BYTES + 1)).is_err());
    assert!(validate_d2_source("a -> b\x1b[0m").is_err());
    assert!(validate_d2_source("a -> b\0").is_err());
    assert!(validate_d2_source("a -> b\r\n\t").is_ok());
}

#[test]
fn unsafe_source_is_rejected_before_even_resolving_missing_executable() {
    let options = D2Options {
        binary: PathBuf::from("/nonexistent/jcode-test-d2"),
        ..D2Options::default()
    };
    assert!(
        compile_d2("...@private", &options)
            .unwrap_err()
            .contains("self-contained")
    );
    assert!(
        compile_d2("a -> b", &options)
            .unwrap_err()
            .contains("executable not found")
    );
}

#[test]
fn svg_guard_rejects_external_or_embedded_image_content() {
    for svg in [
        "",
        "not SVG",
        "<!DOCTYPE svg><svg/>",
        "<svg><image href='/private/icon.png'/></svg>",
        "<svg><foreignObject/></svg>",
        "<svg><script/></svg>",
    ] {
        assert!(validate_d2_svg(svg).is_err(), "accepted {svg}");
    }
    assert!(validate_d2_svg("<svg><path d='M0 0 L1 1'/></svg>").is_ok());
}

#[cfg(unix)]
fn shell(
    script: &str,
    source: &[u8],
    timeout: Duration,
    limit: usize,
    cancelled: Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    run_child(
        Path::new("/bin/sh"),
        &["-c".into(), script.into()],
        source,
        timeout,
        limit,
        cancelled,
    )
}

#[cfg(unix)]
#[test]
fn subprocess_deadline_covers_blocked_stdin_and_reaps_process_group() {
    let directory = PrivateDirectory::new().unwrap();
    let pid = directory.path().join("pid");
    let descendant = directory.path().join("descendant");
    let script = format!(
        "echo $$ > '{}'; /bin/sleep 30 & echo $! > '{}'; wait",
        pid.display(),
        descendant.display()
    );
    let started = Instant::now();
    let error = shell(
        &script,
        &vec![b'a'; MAX_D2_SOURCE_BYTES],
        Duration::from_millis(120),
        1024,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    for file in [pid, descendant] {
        let process = std::fs::read_to_string(file).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", process.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&status.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child still running: {process} {state}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
#[test]
fn subprocess_caps_both_output_streams_and_supports_cancellation() {
    for script in [
        "while :; do printf '0123456789'; done",
        "while :; do printf '0123456789' >&2; done",
    ] {
        let started = Instant::now();
        let error = shell(
            script,
            b"",
            Duration::from_secs(2),
            32,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap_err();
        assert!(error.contains("allowed size"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        signal.store(true, Ordering::Release);
    });
    let error = shell(
        "while :; do :; done",
        b"",
        Duration::from_secs(2),
        1024,
        cancelled,
    )
    .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
}

#[cfg(unix)]
#[test]
fn subprocess_drains_success_and_sanitizes_failure() {
    let output = shell(
        "/bin/cat >/dev/null; printf 'ok'; /bin/sleep 30 &",
        b"source",
        Duration::from_secs(2),
        1024,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(output, b"ok");
    let error = shell(
        "printf '\\033[31mfailed\\033[0m\\n' >&2; exit 3",
        b"",
        Duration::from_secs(2),
        1024,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap_err();
    assert!(error.contains("failed"));
    assert!(!error.contains('\x1b'));
}

#[cfg(unix)]
#[test]
fn compiler_passes_fixed_local_flags_and_source_on_stdin() {
    use std::os::unix::fs::PermissionsExt;
    let directory = PrivateDirectory::new().unwrap();
    let executable = directory.path().join("d2");
    let arguments = directory.path().join("args");
    let input = directory.path().join("stdin");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n/bin/cat > '{}'\nprintf '<svg width=\"1\" height=\"1\"/>'\n",
        arguments.display(),
        input.display()
    );
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let options = D2Options {
        binary: executable,
        layout: D2Layout::Tala,
        ..D2Options::default()
    };
    assert!(compile_d2("a -> b", &options).unwrap().contains("<svg"));
    assert_eq!(std::fs::read_to_string(input).unwrap(), "a -> b");
    let args = std::fs::read_to_string(&arguments).unwrap();
    for required in [
        "--layout=tala",
        "--watch=false",
        "--bundle=false",
        "--scale=1",
        "--target=",
        "--timeout=10",
    ] {
        assert!(
            args.lines().any(|arg| arg == required),
            "missing {required}: {args}"
        );
    }
    std::fs::remove_file(&arguments).unwrap();
    assert!(compile_d2("node.icon: /private/path", &options).is_err());
    assert!(!arguments.exists(), "unsafe source launched compiler");
}

#[test]
#[ignore = "requires local d2, set JCODE_TEST_D2 to its executable"]
fn installed_d2_compiles_all_bundled_layouts_and_reports_syntax_errors() {
    let binary = std::env::var_os("JCODE_TEST_D2")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("d2"));
    for layout in [D2Layout::Dagre, D2Layout::Elk, D2Layout::Tala] {
        let options = D2Options {
            binary: binary.clone(),
            layout,
            ..D2Options::default()
        };
        let svg = compile_d2("direction: right\na: Terminal\na -> b: SVG", &options).unwrap();
        assert!(svg.contains("Terminal") && svg.contains("viewBox"));
        let error = compile_d2("a: {", &options).unwrap_err();
        assert!(error.contains("maps must be terminated"), "{error}");
    }
}
