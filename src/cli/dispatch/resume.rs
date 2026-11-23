use super::{Args, Result, output, session};

pub(super) fn resolve_resume_arg(args: &mut Args) -> Result<()> {
    if let Some(ref resume_id) = args.resume {
        let resume_id = resume_id.clone();
        match resolve_resume_id(&resume_id) {
            Ok(full_id) => {
                args.resume = Some(full_id);
            }
            Err(e) => {
                match resume_resolution_failure_action(&resume_id, |key| std::env::var_os(key)) {
                    // During a reload/update/restart handoff the client re-execs
                    // itself with `--resume <id>` and `JCODE_RESUMING=1`. In the
                    // client/server architecture the shared server is the authority
                    // for session lifecycle, so an id that is not in the local store
                    // can still be valid server-side. Hard-exiting here dumped the
                    // user back to a shell with "No session found matching ...",
                    // making jcode unusable after an auto-update (issue #328).
                    // Instead, keep the raw id and let the remote connection resolve
                    // it; if the server cannot find it either, the TUI surfaces a
                    // recoverable message and falls back to a fresh session rather
                    // than killing the process.
                    ResumeResolutionFailureAction::DeferToServer => {
                        crate::logging::warn(&format!(
                            "Resume id '{}' not found locally during reload handoff ({}); deferring resolution to the server instead of exiting",
                            resume_id, e
                        ));
                        // Leave args.resume as the raw id for the server to resolve.
                    }
                    ResumeResolutionFailureAction::Exit => {
                        eprintln!("Error: {}", e);
                        if !output::quiet_enabled() {
                            eprintln!("\nUse `jcode --resume` to list available sessions.");
                        }
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    Ok(())
}

/// What to do when a `--resume <id>` cannot be resolved from the local session
/// store. Extracted as a pure function so the reload-handoff recovery path can
/// be unit-tested without invoking `std::process::exit` (issue #328).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResumeResolutionFailureAction {
    /// Keep the raw id and let the shared server resolve it (reload handoff).
    DeferToServer,
    /// No live handoff in progress; the id is genuinely bad, so exit.
    Exit,
}

pub(super) fn resume_resolution_failure_action<F, V>(
    _resume_id: &str,
    var_os: F,
) -> ResumeResolutionFailureAction
where
    F: Fn(&str) -> Option<V>,
{
    if var_os("JCODE_RESUMING").is_some() {
        ResumeResolutionFailureAction::DeferToServer
    } else {
        ResumeResolutionFailureAction::Exit
    }
}

pub(super) fn resolve_resume_id(resume_id: &str) -> Result<String> {
    match session::find_session_by_name_or_id(resume_id) {
        Ok(full_id) => Ok(full_id),
        Err(native_err) => match crate::import::import_external_resume_id(resume_id)? {
            Some(imported_id) => Ok(imported_id),
            None => Err(native_err),
        },
    }
}
