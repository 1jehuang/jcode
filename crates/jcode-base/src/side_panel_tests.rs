use super::*;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[test]
fn side_panel_pages_persist_and_focus_latest() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let session_id = "ses_side_panel_test";
    let first = write_markdown_page(session_id, "notes", Some("Notes"), "# Notes", true)
        .expect("write notes");
    assert_eq!(first.focused_page_id.as_deref(), Some("notes"));
    assert_eq!(first.pages.len(), 1);

    let second =
        write_markdown_page(session_id, "plan", Some("Plan"), "# Plan", true).expect("write plan");
    assert_eq!(second.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(second.pages.len(), 2);
    assert_eq!(
        second.focused_page().map(|p| p.title.as_str()),
        Some("Plan")
    );

    let appended =
        append_markdown_page(session_id, "notes", None, "- item", false).expect("append notes");
    let notes = appended
        .pages
        .iter()
        .find(|page| page.id == "notes")
        .expect("notes page");
    assert!(notes.content.contains("- item"));
    assert_eq!(appended.focused_page_id.as_deref(), Some("plan"));

    let focused = focus_page(session_id, "notes").expect("focus notes");
    assert_eq!(focused.focused_page_id.as_deref(), Some("notes"));

    let reloaded = snapshot_for_session(session_id).expect("reload snapshot");
    assert_eq!(reloaded.focused_page_id.as_deref(), Some("notes"));
    assert_eq!(reloaded.pages.len(), 2);
}

#[test]
fn side_panel_delete_falls_back_to_most_recent_page() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let session_id = "ses_side_panel_delete";
    write_markdown_page(session_id, "one", Some("One"), "# One", true).expect("page one");
    write_markdown_page(session_id, "two", Some("Two"), "# Two", true).expect("page two");

    let after_delete = delete_page(session_id, "two").expect("delete page two");
    assert_eq!(after_delete.pages.len(), 1);
    assert_eq!(after_delete.focused_page_id.as_deref(), Some("one"));
}

#[test]
fn load_markdown_file_uses_source_path_content() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let source = temp.path().join("guide.md");
    std::fs::write(&source, "# Guide\n\nHello").expect("write source file");

    let snapshot = load_markdown_file("ses_side_panel_load", "guide", Some("Guide"), &source, true)
        .expect("load markdown file");

    assert_eq!(snapshot.focused_page_id.as_deref(), Some("guide"));
    let page = snapshot
        .pages
        .iter()
        .find(|page| page.id == "guide")
        .expect("guide page");
    assert_eq!(page.title, "Guide");
    assert_eq!(page.source, SidePanelPageSource::LinkedFile);
    assert_eq!(page.content, "# Guide\n\nHello");
    assert_eq!(
        Path::new(&page.file_path),
        source.canonicalize().expect("canonical path")
    );

    std::fs::write(&source, "# Guide\n\nUpdated").expect("update source file");
    let reloaded = snapshot_for_session("ses_side_panel_load").expect("reload snapshot");
    let page = reloaded
        .pages
        .iter()
        .find(|page| page.id == "guide")
        .expect("guide page");
    assert_eq!(page.content, "# Guide\n\nUpdated");
}

#[test]
fn load_markdown_file_rejects_non_markdown_extensions() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let source = temp.path().join("notes.txt");
    std::fs::write(&source, "not markdown").expect("write source file");

    let err = load_markdown_file("ses_side_panel_load", "notes", Some("Notes"), &source, true)
        .expect_err("non-markdown load should fail");
    assert!(err.to_string().contains("only supports markdown files"));
}

#[test]
fn status_output_marks_linked_and_managed_pages() {
    let snapshot = SidePanelSnapshot {
        focus_revision: 0,
        focused_page_id: Some("linked".to_string()),
        pages: vec![
            SidePanelPage {
                id: "linked".to_string(),
                title: "Linked".to_string(),
                file_path: "/tmp/linked.md".to_string(),
                format: SidePanelPageFormat::Markdown,
                pdf_data: None,
                source: SidePanelPageSource::LinkedFile,
                content: String::new(),
                updated_at_ms: 2,
            },
            SidePanelPage {
                id: "managed".to_string(),
                title: "Managed".to_string(),
                file_path: "/tmp/managed.md".to_string(),
                format: SidePanelPageFormat::Markdown,
                pdf_data: None,
                source: SidePanelPageSource::Managed,
                content: String::new(),
                updated_at_ms: 1,
            },
        ],
    };

    let output = status_output(&snapshot);
    assert!(output.contains("source: linked_file"));
    assert!(output.contains("source: managed"));
}

#[test]
fn refresh_linked_page_content_updates_snapshot_in_memory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("linked.md");
    std::fs::write(&file_path, "# First").expect("write initial");

    let mut snapshot = SidePanelSnapshot {
        focus_revision: 0,
        focused_page_id: Some("linked".to_string()),
        pages: vec![SidePanelPage {
            id: "linked".to_string(),
            title: "Linked".to_string(),
            file_path: file_path.display().to_string(),
            format: SidePanelPageFormat::Markdown,
            pdf_data: None,
            source: SidePanelPageSource::LinkedFile,
            content: "# Stale".to_string(),
            updated_at_ms: 1,
        }],
    };

    assert!(refresh_linked_page_content(&mut snapshot, None));
    assert_eq!(
        snapshot.focused_page().map(|page| page.content.as_str()),
        Some("# First")
    );

    let unchanged_revision = snapshot
        .focused_page()
        .map(|page| page.updated_at_ms)
        .unwrap_or(0);
    assert!(!refresh_linked_page_content(&mut snapshot, None));
    assert_eq!(
        snapshot.focused_page().map(|page| page.updated_at_ms),
        Some(unchanged_revision)
    );

    std::fs::write(&file_path, "# Second").expect("write update");
    assert!(refresh_linked_page_content(&mut snapshot, None));
    assert_eq!(
        snapshot.focused_page().map(|page| page.content.as_str()),
        Some("# Second")
    );
}

/// A complete one-page PDF with real object offsets and a cross-reference table.
fn synthetic_pdf(label: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 50 100 Td ({label}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    pdf
}

#[test]
fn pdf_load_hydrate_refresh_focus_delete_and_replace() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let path = temp.path().join("Report.PdF");
    let bytes = synthetic_pdf("Hello");
    std::fs::write(&path, &bytes).unwrap();
    let mut snapshot = load_file("pdf-session", "report", Some("Report"), &path, true).unwrap();
    let page = snapshot.focused_page().unwrap();
    assert_eq!(page.format, SidePanelPageFormat::Pdf);
    assert_eq!(page.source, SidePanelPageSource::LinkedFile);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(page.pdf_data.as_ref().unwrap())
            .unwrap(),
        bytes
    );
    assert!(page.content.contains("desktop app"));
    assert!(!page.content.contains(page.pdf_data.as_ref().unwrap()));
    assert_eq!(snapshot_for_session("pdf-session").unwrap(), snapshot);
    assert!(!refresh_linked_page_content(&mut snapshot, None));
    let updated = synthetic_pdf("Updated report");
    std::fs::write(&path, &updated).unwrap();
    assert!(refresh_linked_page_content(&mut snapshot, Some("report")));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(snapshot.pages[0].pdf_data.as_ref().unwrap())
            .unwrap(),
        updated
    );
    write_markdown_page("pdf-session", "notes", None, "Notes", true).unwrap();
    assert_eq!(
        focus_page("pdf-session", "report")
            .unwrap()
            .focused_page_id
            .as_deref(),
        Some("report")
    );
    assert!(
        append_markdown_page("pdf-session", "report", None, "bad", true)
            .unwrap_err()
            .to_string()
            .contains("cannot append")
    );
    assert_eq!(std::fs::read(&path).unwrap(), updated);
    let replacement =
        write_markdown_page("pdf-session", "report", None, "Replacement", true).unwrap();
    assert_eq!(
        replacement.focused_page().unwrap().format,
        SidePanelPageFormat::Markdown
    );
    assert!(replacement.focused_page().unwrap().pdf_data.is_none());
    load_file("pdf-session", "report", None, &path, true).unwrap();
    let remaining = delete_page("pdf-session", "report").unwrap();
    assert_eq!(remaining.pages.len(), 1);
    assert_eq!(remaining.focused_page_id.as_deref(), Some("notes"));
    assert!(
        path.exists(),
        "deleting a linked panel must not delete its source"
    );
}

#[test]
fn pdf_rejects_invalid_oversize_missing_and_clears_stale_payload() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let path = temp.path().join("bad.pdf");
    assert!(load_file("bad-pdf", "bad", None, &path, true).is_err());
    std::fs::write(&path, b"not a PDF").unwrap();
    assert!(
        load_file("bad-pdf", "bad", None, &path, true)
            .unwrap_err()
            .to_string()
            .contains("signature")
    );
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_PDF_BYTES + 1).unwrap();
    assert!(
        load_file("bad-pdf", "bad", None, &path, true)
            .unwrap_err()
            .to_string()
            .contains("20 MiB")
    );
    assert!(snapshot_for_session("bad-pdf").unwrap().pages.is_empty());
    std::fs::write(&path, synthetic_pdf("valid")).unwrap();
    let mut snapshot = load_file("bad-pdf", "bad", None, &path, true).unwrap();
    for failure in ["malformed", "oversize", "missing"] {
        match failure {
            "malformed" => std::fs::write(&path, b"corrupt").unwrap(),
            "oversize" => std::fs::File::create(&path)
                .unwrap()
                .set_len(MAX_PDF_BYTES + 1)
                .unwrap(),
            _ => std::fs::remove_file(&path).unwrap(),
        }
        assert!(refresh_linked_page_content(&mut snapshot, None));
        assert!(snapshot.pages[0].pdf_data.is_none());
        assert!(snapshot.pages[0].content.contains("Unable to load pdf"));
        assert_eq!(snapshot_for_session("bad-pdf").unwrap(), snapshot);
    }
}

#[test]
fn legacy_panel_serde_defaults_and_pdf_roundtrip() {
    let legacy = serde_json::json!({"id":"old", "title":"Old", "file_path":"old.md"});
    let page: SidePanelPage = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(page.format, SidePanelPageFormat::Markdown);
    assert!(page.pdf_data.is_none());
    assert!(
        serde_json::to_value(&page)
            .unwrap()
            .get("pdf_data")
            .is_none()
    );
    let persisted: PersistedSidePanelPage = serde_json::from_value({
        let mut v = legacy;
        v["updated_at_ms"] = serde_json::json!(1);
        v
    })
    .unwrap();
    assert_eq!(persisted.format, SidePanelPageFormat::Markdown);
    let pdf = SidePanelPage {
        format: SidePanelPageFormat::Pdf,
        pdf_data: Some(base64::engine::general_purpose::STANDARD.encode(synthetic_pdf("serde"))),
        ..page
    };
    assert_eq!(
        serde_json::from_value::<SidePanelPage>(serde_json::to_value(&pdf).unwrap()).unwrap(),
        pdf
    );
}

#[test]
fn focus_revision_tracks_explicit_intent_only() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let initial = write_markdown_page("focus-rev", "a", None, "A", false).unwrap();
    assert_eq!(initial.focus_revision, 0);
    assert!(initial.focused_page_id.is_none());
    let focused = focus_page("focus-rev", "a").unwrap();
    assert!(focused.focus_revision > 0);
    let refocused = focus_page("focus-rev", "a").unwrap();
    assert!(refocused.focus_revision > focused.focus_revision);
    let updated = write_markdown_page("focus-rev", "a", None, "B", false).unwrap();
    assert_eq!(updated.focus_revision, refocused.focus_revision);
    assert_eq!(
        snapshot_for_session("focus-rev").unwrap().focus_revision,
        refocused.focus_revision
    );
    let explicit = write_markdown_page("focus-rev", "a", None, "C", true).unwrap();
    assert!(explicit.focus_revision > refocused.focus_revision);
    let legacy: SidePanelSnapshot = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.focus_revision, 0);
}

#[test]
fn pdf_aggregate_budget_rejects_before_mutation_and_bounds_hydration() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let first = temp.path().join("one.pdf");
    let second = temp.path().join("two.pdf");
    let make_pdf = |path: &Path, size: u64| {
        std::fs::write(path, synthetic_pdf("budget")).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(size)
            .unwrap();
    };
    make_pdf(&first, 16 * 1024 * 1024);
    make_pdf(&second, 16 * 1024 * 1024);
    load_file("budget", "one", None, &first, true).unwrap();
    let initial = load_file("budget", "two", None, &second, true).unwrap();
    let index = std::fs::read(state_file("budget").unwrap()).unwrap();
    assert!(
        load_file("budget", "three", None, &second, true)
            .unwrap_err()
            .to_string()
            .contains("aggregate")
    );
    assert_eq!(std::fs::read(state_file("budget").unwrap()).unwrap(), index);
    make_pdf(&second, 17 * 1024 * 1024);
    assert!(
        load_file("budget", "two", None, &second, true)
            .unwrap_err()
            .to_string()
            .contains("aggregate")
    );
    assert_eq!(std::fs::read(state_file("budget").unwrap()).unwrap(), index);
    let hydrated = snapshot_for_session("budget").unwrap();
    assert_eq!(
        hydrated
            .pages
            .iter()
            .filter(|p| p.pdf_data.is_some())
            .count(),
        1
    );
    assert!(
        hydrated
            .pages
            .iter()
            .any(|p| p.content.contains("aggregate"))
    );
    let mut refreshed = initial;
    assert!(refresh_linked_page_content(&mut refreshed, Some("two")));
    assert!(
        refreshed
            .pages
            .iter()
            .filter_map(|p| p.pdf_data.as_deref())
            .map(pdf_byte_len)
            .sum::<u64>()
            <= MAX_SESSION_PDF_BYTES
    );
    // The focused PDF's revision is unchanged, but shrinking another linked
    // PDF frees enough shared budget to restore its previously rejected data.
    make_pdf(&first, 15 * 1024 * 1024);
    assert!(refresh_linked_page_content(&mut refreshed, Some("two")));
    assert_eq!(
        refreshed
            .pages
            .iter()
            .filter(|p| p.pdf_data.is_some())
            .count(),
        2
    );
}
