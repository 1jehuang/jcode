use super::*;

#[test]
fn agent_edited_paths_come_from_transcript_edit_tools() {
    use crate::tui::TuiState;
    let _lock = scroll_render_test_lock();
    let (mut app, _terminal) = create_scroll_test_app(80, 20, 0, 4);
    app.session.working_dir = Some("/repo/crates".to_string());
    let tool = |name: &str, input: serde_json::Value| {
        DisplayMessage::tool(
            "ok",
            crate::message::ToolCall {
                id: name.into(),
                name: name.into(),
                input,
                ..Default::default()
            },
        )
    };
    app.display_messages
        .push(tool("edit", serde_json::json!({"file_path": "a/src/x.rs"})));
    app.display_messages.push(tool(
        "apply_patch",
        serde_json::json!({"patch_text": "*** Begin Patch\n*** Update File: /repo/README.md\n@@\n-a\n+b\n*** End Patch"}),
    ));
    app.display_messages
        .push(tool("read", serde_json::json!({"file_path": "a/src/y.rs"})));
    app.bump_display_messages_version();

    let data = app.info_widget_data();
    let set = &data.agent_edited;
    assert!(
        set.contains(std::path::Path::new("/repo/crates/a/src/x.rs")),
        "{set:?}"
    );
    assert!(
        set.contains(std::path::Path::new("/repo/README.md")),
        "{set:?}"
    );
    assert!(
        !set.contains(std::path::Path::new("/repo/crates/a/src/y.rs")),
        "reads are not edits"
    );

    // Cached until the transcript changes, then refreshed.
    let again = app.info_widget_data().agent_edited;
    assert!(std::sync::Arc::ptr_eq(&data.agent_edited, &again));
    app.display_messages.push(tool(
        "write",
        serde_json::json!({"file_path": "/repo/new.rs"}),
    ));
    app.bump_display_messages_version();
    assert!(
        app.info_widget_data()
            .agent_edited
            .contains(std::path::Path::new("/repo/new.rs"))
    );
}

/// End to end on a real git repository: the production gather computes
/// status, line counts, totals, and newest-first order for every kind of
/// change, and a real App frame renders the Changes widget with the agent
/// dot on exactly the file its transcript edited.
#[test]
fn changes_widget_end_to_end_on_real_git_repo() {
    use std::process::Command;
    let _lock = scroll_render_test_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&root)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    let write = |rel: &str, body: &[u8]| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    let sleep = || std::thread::sleep(std::time::Duration::from_millis(20));

    git(&["init", "-q", "-b", "main"]);
    write("src/lib.rs", b"a\nb\nc\nd\n");
    write("src/old_name.rs", b"x\ny\n");
    write("gone.txt", b"1\n2\n3\n");
    write("logo.bin", &[0u8, 1, 2, 3]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);

    // Oldest to newest modification.
    write("logo.bin", &[0u8, 9, 9, 9, 9]); // binary modify
    sleep();
    std::fs::remove_file(root.join("gone.txt")).unwrap(); // delete (no mtime)
    git(&["mv", "src/old_name.rs", "src/new_name.rs"]); // staged rename
    sleep();
    write("nested/dir/new.rs", b"1\n2\n3\n"); // untracked in new dir
    sleep();
    write("src/lib.rs", b"a\nB\nc\nd\ne\nf\n"); // +3 -1, newest

    let info = crate::tui::app::helpers::gather_git_info_in(Some(&root)).expect("repo");
    let find = |p: &str| {
        info.dirty_files
            .iter()
            .find(|f| f.path.ends_with(p))
            .cloned()
    };

    let lib = find("src/lib.rs").expect("lib");
    assert_eq!(
        (lib.status, lib.added, lib.removed),
        ('M', Some(3), Some(1))
    );
    let untracked = find("nested/dir/new.rs").expect("untracked listed individually");
    assert_eq!(
        (untracked.status, untracked.added, untracked.removed),
        ('?', Some(3), Some(0))
    );
    let gone = find("gone.txt").expect("deleted");
    assert_eq!(
        (gone.status, gone.added, gone.removed),
        ('D', Some(0), Some(3))
    );
    let renamed = find("new_name.rs").expect("renamed");
    assert_eq!(renamed.status, 'R');
    let bin = find("logo.bin").expect("binary");
    assert_eq!(
        (bin.added, bin.removed),
        (None, None),
        "binary has no counts"
    );
    assert_eq!(info.dirty_total, 5);
    assert_eq!(info.added_total, 3 + 3 + renamed.added.unwrap_or(0));
    assert_eq!(info.removed_total, 1 + 3 + renamed.removed.unwrap_or(0));
    assert_eq!(info.dirty_files[0].path, "src/lib.rs", "newest first");
    assert_eq!(
        info.dirty_files.last().unwrap().path,
        "gone.txt",
        "no mtime sorts last"
    );
    assert_eq!(info.repo_root.as_deref(), Some(root.as_path()));
    // Commits widget data comes from the same probe against real git output.
    assert_eq!(info.recent_commits.len(), 1, "{:?}", info.recent_commits);
    let init = &info.recent_commits[0];
    assert_eq!(init.subject, "init");
    assert_eq!(
        (init.added, init.removed),
        (Some(9), Some(0)),
        "text lines only"
    );
    assert!(!init.unpushed, "no upstream means ahead=0");
    assert!(init.timestamp > 0 && init.hash.len() >= 7);

    // Real App frame: transcript edited src/lib.rs relative to the repo.
    crate::tui::app::helpers::seed_git_info_cache_for_tests(Some(info));
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let (mut app, mut terminal) = create_scroll_test_app(140, 40, 0, 0);
    app.session.working_dir = Some(root.join("src").to_string_lossy().into_owned());
    app.display_messages.push(DisplayMessage::tool(
        "ok",
        crate::message::ToolCall {
            id: "e1".into(),
            name: "edit".into(),
            input: serde_json::json!({"file_path": "lib.rs"}),
            ..Default::default()
        },
    ));
    app.bump_display_messages_version();
    let mut frame = String::new();
    for _ in 0..3 {
        frame = render_and_snap(&app, &mut terminal);
    }
    crate::tui::app::helpers::seed_git_info_cache_for_tests(None);

    let row = |needle: &str| {
        frame
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} missing:\n{frame}"))
            .to_string()
    };
    assert!(
        row("lib.rs").contains("M● src/lib.rs"),
        "agent dot on edited file:\n{frame}"
    );
    assert!(row("new_name.rs").contains("R  new_name.rs"), "{frame}");
    assert!(row("gone.txt").contains("D  gone.txt"), "{frame}");
    assert!(row("lib.rs").contains("+3 −1"), "{frame}");
    assert!(
        row("new.rs").contains("?  new.rs"),
        "no dot on files agent did not edit:\n{frame}"
    );
    assert!(row("new.rs").contains("+3 −0"), "{frame}");
    assert!(
        !row("logo.bin").contains('+'),
        "binary shows no counts:\n{frame}"
    );
    assert!(
        frame.contains("● agent"),
        "legend explains the dot when one is shown:\n{frame}"
    );

    // Without any agent edits there is no dot, so no legend either.
    let info = crate::tui::app::helpers::gather_git_info_in(Some(&root)).expect("repo");
    crate::tui::app::helpers::seed_git_info_cache_for_tests(Some(info));
    crate::tui::info_widget::clear_widget_placements_for_tests();
    let (app2, mut terminal2) = create_scroll_test_app(140, 40, 0, 0);
    let mut frame2 = String::new();
    for _ in 0..3 {
        frame2 = render_and_snap(&app2, &mut terminal2);
    }
    crate::tui::app::helpers::seed_git_info_cache_for_tests(None);
    assert!(frame2.contains("src/lib.rs"), "{frame2}");
    assert!(!frame2.contains("● agent"), "{frame2}");
    let changes_rows: Vec<_> = frame2
        .lines()
        .filter(|line| {
            [
                "src/lib.rs",
                "new.rs",
                "logo.bin",
                "new_name.rs",
                "gone.txt",
            ]
            .iter()
            .any(|path| line.contains(path))
        })
        .collect();
    assert_eq!(changes_rows.len(), 5, "{frame2}");
    assert!(
        changes_rows.iter().all(|line| !line.contains('●')),
        "no agent-edit dot on clean Changes rows:\n{frame2}"
    );
}
