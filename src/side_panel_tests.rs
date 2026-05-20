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
        focused_page_id: Some("linked".to_string()),
        pages: vec![
            SidePanelPage {
                id: "linked".to_string(),
                title: "Linked".to_string(),
                file_path: "/tmp/linked.md".to_string(),
                format: SidePanelPageFormat::Markdown,
                source: SidePanelPageSource::LinkedFile,
                content: String::new(),
                updated_at_ms: 2,
            },
            SidePanelPage {
                id: "managed".to_string(),
                title: "Managed".to_string(),
                file_path: "/tmp/managed.md".to_string(),
                format: SidePanelPageFormat::Markdown,
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
        focused_page_id: Some("linked".to_string()),
        pages: vec![SidePanelPage {
            id: "linked".to_string(),
            title: "Linked".to_string(),
            file_path: file_path.display().to_string(),
            format: SidePanelPageFormat::Markdown,
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
