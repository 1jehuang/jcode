use super::*;

#[test]
fn fence_matching_is_token_based_and_case_insensitive() {
    for language in ["d2", "D2", "d2 title=example"] {
        assert!(is_d2_lang(language));
    }
    for language in ["", "d2lang", "d2-example", "mermaid"] {
        assert!(!is_d2_lang(language));
    }
}

#[test]
fn artifact_identity_separates_engines_and_layouts() {
    let source = "a -> b";
    let dagre = Settings::new(false, D2Options::default());
    let tala = Settings::new(
        false,
        D2Options {
            layout: super::super::D2Layout::Tala,
            ..D2Options::default()
        },
    );
    assert_ne!(source_hash(source, &dagre), source_hash(source, &tala));
    assert_ne!(
        source_hash(source, &dagre),
        super::super::hash_content(source)
    );
}

#[cfg(all(unix, feature = "renderer"))]
#[test]
fn production_path_rasterizes_caches_registers_and_preserves_d2_source() {
    use std::os::unix::fs::PermissionsExt;
    let _lock = super::super::IMAGE_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let directory = super::super::d2::PrivateDirectory::new().unwrap();
    let executable = directory.path().join("d2");
    let calls = directory.path().join("calls");
    std::fs::write(&executable, format!("#!/bin/sh\n/bin/cat >/dev/null\nprintf x >> '{}'\nprintf '<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><rect width=\"100\" height=\"50\" fill=\"red\"/></svg>'\n", calls.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let options = D2Options {
        binary: executable,
        ..D2Options::default()
    };
    let source = format!("a -> b: test{}", std::process::id());
    let (hash, path, width, height) =
        match render_d2_sized_with_options(&source, Some(80), options.clone(), false) {
            RenderResult::Image {
                hash,
                path,
                width,
                height,
            } => (hash, path, width, height),
            RenderResult::Error(error) => panic!("{error}"),
        };
    assert!(width > 0 && height > 0 && width <= 4096 && height <= 4096);
    let pixels = image::open(&path).unwrap().into_rgba8();
    assert!(
        pixels
            .pixels()
            .any(|pixel| pixel[0] > 200 && pixel[1] < 20 && pixel[3] > 200)
    );
    assert_eq!(diagram_source_for_hash(hash).unwrap().source, source);
    assert_eq!(
        diagram_source_for_hash(hash).unwrap().language,
        DiagramLanguage::D2
    );
    assert!(
        !super::super::get_active_diagrams()
            .iter()
            .any(|diagram| diagram.hash == hash)
    );
    assert!(matches!(
        render_d2_sized_with_options(&source, Some(80), options, true),
        RenderResult::Image { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(calls).unwrap(),
        "x",
        "warm render launched another compiler"
    );
    assert!(
        super::super::get_active_diagrams()
            .iter()
            .any(|diagram| diagram.hash == hash)
    );
}

#[cfg(feature = "renderer")]
#[test]
fn d2_raster_bounds_explicit_dimensions_and_rejects_images() {
    let directory = super::super::d2::PrivateDirectory::new().unwrap();
    let png = directory.path().join("bounded.png");
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='1000000' height='1000000'><rect width='1000000' height='1000000' fill='red'/></svg>";
    let (width, height) = super::super::svg::write_d2_png(svg, &png, 800).unwrap();
    assert_eq!((width, height), (800, 800));
    assert!(
        super::super::svg::write_d2_png(
            "<svg><image href='/private/forbidden.png'/></svg>",
            &png,
            800
        )
        .is_err()
    );
    assert!(super::super::svg::write_d2_png("not svg", &png, 800).is_err());
}

#[cfg(feature = "renderer")]
#[test]
#[ignore = "requires local d2, set JCODE_TEST_D2 to its executable"]
fn installed_tala_uses_production_png_pipeline() {
    let binary = std::env::var_os("JCODE_TEST_D2")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "d2".into());
    let source = "direction: right\nclient: Terminal\ncompiler: Local D2\nclient -> compiler: SVG\ncompiler -> raster: PNG";
    let result = render_d2_sized_with_options(
        source,
        Some(100),
        D2Options {
            binary,
            layout: super::super::D2Layout::Tala,
            ..D2Options::default()
        },
        false,
    );
    let RenderResult::Image {
        path,
        width,
        height,
        hash,
    } = result
    else {
        if let RenderResult::Error(error) = result {
            panic!("{error}")
        } else {
            unreachable!()
        }
    };
    let pixels = image::open(path).unwrap().into_rgba8();
    let distinct: std::collections::HashSet<_> = pixels
        .pixels()
        .map(|pixel| pixel.0)
        .take(2_000_000)
        .collect();
    assert!(distinct.len() > 10);
    assert!(width > height && height > 0);
    assert_eq!(diagram_source_for_hash(hash).unwrap().source, source);
}
