//! Account at successful filesystem mutation boundaries, never tool previews.
use super::ToolContext;
use jcode_harness_api::SessionEditStats;
use similar::{ChangeTag, TextDiff};

fn changed_lines(old: &str, new: &str, approximate: bool) -> SessionEditStats {
    let mut stats = SessionEditStats {
        approximate,
        ..Default::default()
    };
    for change in TextDiff::from_lines(old, new).iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => stats.added += 1,
            ChangeTag::Delete => stats.removed += 1,
            ChangeTag::Equal => (),
        }
    }
    stats
}

pub(super) async fn record(ctx: &ToolContext, old: &str, new: &str, approximate: bool) {
    let id = ctx.session_id.clone();
    let old = old.to_owned();
    let new = new.to_owned();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let dir = crate::storage::jcode_dir()?.join("sessions");
        jcode_harness_api::record_session_edit(&dir, &id, changed_lines(&old, &new, approximate))?;
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => (),
        other => crate::logging::warn(&format!("Could not persist session edit counts: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_full_file_counts_cover_repetition_truncation_and_noops() {
        let old = "old\n".repeat(100);
        let new = "new\n".repeat(100);
        assert_eq!(
            changed_lines(&old, &new, false),
            SessionEditStats {
                added: 100,
                removed: 100,
                approximate: false
            }
        );
        assert_eq!(
            changed_lines(&new, &new, false),
            SessionEditStats::default()
        );
        assert_eq!(
            changed_lines("", "last line without newline", false).added,
            1
        );
        assert_eq!(changed_lines("a\nb\n", "", false).removed, 2);
        assert!(changed_lines("", "new", true).approximate);
    }
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn successful_tools_persist_exact_counts_and_failed_edits_do_not() {
        use crate::tool::{Tool, ToolExecutionMode};
        use serde_json::json;
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        struct Home(Option<std::ffi::OsString>);
        impl Drop for Home {
            fn drop(&mut self) {
                if let Some(old) = self.0.take() {
                    crate::env::set_var("JCODE_HOME", old);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
            }
        }
        let _home = Home(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        let ctx = ToolContext {
            session_id: "stats_test".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: Some(home.path().into()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        let stats = || -> SessionEditStats {
            serde_json::from_slice(
                &std::fs::read(home.path().join("sessions/edit-stats/stats_test.json")).unwrap(),
            )
            .unwrap()
        };
        let old = "old\n".repeat(100);
        crate::tool::write::WriteTool
            .execute(json!({"file_path":"f","content":old}), ctx.clone())
            .await
            .unwrap();
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 100,
                removed: 0,
                approximate: false
            }
        );
        crate::tool::edit::EditTool
            .execute(
                json!({"file_path":"f","old_string":"old","new_string":"new","replace_all":true}),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 200,
                removed: 100,
                approximate: false
            }
        );
        assert!(
            crate::tool::edit::EditTool
                .execute(
                    json!({"file_path":"f","old_string":"missing","new_string":"bad"}),
                    ctx.clone()
                )
                .await
                .is_err()
        );
        assert_eq!(stats().added, 200);
        assert!(
            crate::tool::edit::EditTool
                .execute(json!({"file_path":"f","edits":[{"old_string":"new","new_string":"changed","replace_all":true},{"old_string":"missing","new_string":"failed"}]}), ctx.clone())
                .await
                .is_err(),
            "a failed edit in the batch must write nothing"
        );
        assert_eq!(stats().added, 200);
        crate::tool::edit::EditTool.execute(json!({"file_path":"f","edits":[{"old_string":"new","new_string":"changed","replace_all":true}]}), ctx.clone()).await.unwrap();
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 300,
                removed: 200,
                approximate: false
            }
        );
        crate::tool::patch::PatchTool
            .execute(
                json!({"patch_text":"--- /dev/null\n+++ g\n@@ -0,0 +1,2 @@\n+a\n+b\n"}),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(stats().added, 302);
        crate::tool::apply_patch::ApplyPatchTool.execute(json!({"patch_text":"*** Begin Patch\n*** Add File: h\n+one\n*** Update File: absent\n@@\n-bad\n+new\n*** End Patch"}), ctx.clone()).await.unwrap();
        assert_eq!(stats().added, 303);
        crate::tool::apply_patch::ApplyPatchTool
            .execute(
                json!({"patch_text":"*** Begin Patch\n*** Delete File: h\n*** End Patch"}),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 303,
                removed: 201,
                approximate: false
            }
        );
        // AddFile may overwrite, so account actual old content rather than
        // counting the whole replacement as a new file.
        crate::tool::apply_patch::ApplyPatchTool
            .execute(
                json!({"patch_text":"*** Begin Patch\n*** Add File: g\n+a\n+c\n*** End Patch"}),
                ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 304,
                removed: 202,
                approximate: false
            }
        );
        crate::tool::apply_patch::ApplyPatchTool.execute(json!({"patch_text":"*** Begin Patch\n*** Update File: g\n*** Move to: moved\n@@\n a\n-c\n+d\n*** End Patch"}), ctx.clone()).await.unwrap();
        assert!(!home.path().join("g").exists());
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 305,
                removed: 203,
                approximate: false
            }
        );
        // A later fatal filesystem error must not erase an earlier mutation.
        std::fs::write(home.path().join("blocker"), "external file").unwrap();
        assert!(crate::tool::apply_patch::ApplyPatchTool.execute(json!({"patch_text":"*** Begin Patch\n*** Add File: before_failure\n+saved\n*** Add File: blocker/child\n+never\n*** End Patch"}), ctx.clone()).await.is_err());
        assert_eq!(
            stats(),
            SessionEditStats {
                added: 306,
                removed: 203,
                approximate: false
            }
        );
        // Another session sharing the worktree has its own counter.
        let other = ToolContext {
            session_id: "other".into(),
            ..ctx.clone()
        };
        crate::tool::write::WriteTool
            .execute(json!({"file_path":"g","content":"else\n"}), other)
            .await
            .unwrap();
        assert_eq!(stats().added, 306);
    }
}
