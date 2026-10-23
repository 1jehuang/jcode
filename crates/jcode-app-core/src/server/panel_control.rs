//! Session-scoped panel controls for native clients, including SSH clients.
use crate::bus::{Bus, BusEvent, SidePanelUpdated};
use crate::protocol::ServerEvent;
use crate::side_panel::SidePanelSnapshot;
use anyhow::{Result, bail, ensure};
use tokio::sync::mpsc;

fn apply_panel_action(session_id: &str, action: &str, page_id: &str) -> Result<SidePanelSnapshot> {
    ensure!(!session_id.is_empty(), "No session is attached");
    match action {
        "focus" => crate::side_panel::focus_page(session_id, page_id),
        "reopen" => crate::side_panel::reopen_page(session_id, page_id),
        "archive" => crate::side_panel::archive_page(session_id, page_id),
        "purge" => crate::side_panel::purge_page(session_id, page_id),
        "refresh" => {
            let mut snapshot = crate::side_panel::snapshot_for_session(session_id)?;
            ensure!(
                snapshot.pages.iter().any(|page| page.id == page_id),
                "Panel not found in this session: {page_id}"
            );
            crate::side_panel::refresh_linked_page_content(&mut snapshot, Some(page_id));
            Ok(snapshot)
        }
        _ => bail!("Unknown panel action: {action}"),
    }
}

fn send_panel_event(events: &mpsc::UnboundedSender<ServerEvent>, event: ServerEvent) {
    if events.send(event).is_err() {
        crate::logging::info("Panel response not delivered: client disconnected");
    }
}

pub(super) fn handle_panel_request(
    id: u64,
    attached_session: &str,
    expected_session: &str,
    action: &str,
    page_id: &str,
    supports_pdf_panels: bool,
    events: &mpsc::UnboundedSender<ServerEvent>,
) {
    if attached_session != expected_session {
        send_panel_event(
            events,
            ServerEvent::Error {
                id,
                message: "Panel action belongs to a different session".to_string(),
                retry_after_secs: None,
            },
        );
        return;
    }
    handle_panel_action(
        id,
        attached_session,
        action,
        page_id,
        supports_pdf_panels,
        events,
    );
}

fn handle_panel_action(
    id: u64,
    session_id: &str,
    action: &str,
    page_id: &str,
    supports_pdf_panels: bool,
    events: &mpsc::UnboundedSender<ServerEvent>,
) {
    match crate::server::run_blocking_io(|| apply_panel_action(session_id, action, page_id)) {
        Ok(snapshot) => {
            Bus::global().publish(BusEvent::SidePanelUpdated(SidePanelUpdated {
                session_id: session_id.to_string(),
                snapshot: snapshot.clone(),
            }));
            send_panel_event(
                events,
                ServerEvent::SidePanelState {
                    snapshot: super::client_writer::side_panel_for_client(
                        snapshot,
                        supports_pdf_panels,
                    ),
                },
            );
            send_panel_event(events, ServerEvent::Done { id });
        }
        Err(error) => {
            send_panel_event(
                events,
                ServerEvent::Error {
                    id,
                    message: format!("Panel action failed: {error:#}"),
                    retry_after_secs: None,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHome(Option<std::ffi::OsString>);
    impl Drop for TestHome {
        fn drop(&mut self) {
            if let Some(previous) = &self.0 {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[test]
    fn remote_panel_controls_are_session_scoped_and_recoverable() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _restore = TestHome(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        crate::side_panel::write_markdown_page("owner", "plan", Some("Plan"), "# Retained", true)
            .unwrap();
        assert!(apply_panel_action("other", "reopen", "plan").is_err());
        assert!(apply_panel_action("other", "purge", "plan").is_err());
        let archived = apply_panel_action("owner", "archive", "plan").unwrap();
        assert!(archived.pages.is_empty());
        assert_eq!(archived.archived.len(), 1);
        let restored = apply_panel_action("owner", "reopen", "plan").unwrap();
        assert_eq!(restored.focused_page().unwrap().content, "# Retained");
        assert!(apply_panel_action("owner", "run_shell", "plan").is_err());
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_panel_action(72, "owner", "focus", "plan", false, &tx);
        let Some(ServerEvent::SidePanelState { snapshot }) = rx.try_recv().ok() else {
            panic!("panel snapshot reply")
        };
        assert_eq!(snapshot.session_id.as_deref(), Some("owner"));
        assert!(matches!(rx.try_recv(), Ok(ServerEvent::Done { id: 72 })));
        handle_panel_action(73, "owner", "reopen", "missing", false, &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerEvent::Error { id: 73, .. })
        ));
        handle_panel_request(74, "other", "owner", "purge", "plan", false, &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerEvent::Error { id: 74, .. })
        ));
        assert!(
            crate::side_panel::snapshot_for_session("owner")
                .unwrap()
                .focused_page()
                .is_some()
        );
    }
}
