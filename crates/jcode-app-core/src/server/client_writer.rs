use crate::protocol::{ServerEvent, encode_event};
use anyhow::Result;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Project wire copies only. Persisted state and opted-in API bridges retain
/// PDF bytes, while old native clients can decode the complete History event.
pub(super) fn side_panel_for_client(
    mut snapshot: crate::side_panel::SidePanelSnapshot,
    supports_pdf_panels: bool,
) -> crate::side_panel::SidePanelSnapshot {
    if !supports_pdf_panels {
        for page in &mut snapshot.pages {
            if page.format == crate::side_panel::SidePanelPageFormat::Pdf {
                page.format = crate::side_panel::SidePanelPageFormat::Markdown;
                // This is a generated fallback, not a linked Markdown file.
                // Otherwise old TUIs re-read binary PDF bytes as text on change.
                page.source = crate::side_panel::SidePanelPageSource::Ephemeral;
            }
            page.pdf_data = None;
        }
    }
    snapshot
}

pub(super) async fn write_direct_event(
    writer: &Arc<Mutex<crate::transport::WriteHalf>>,
    event: &ServerEvent,
) -> Result<()> {
    let json = encode_event(event);
    let mut w = writer.lock().await;
    w.write_all(json.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_panel::{
        SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
    };

    #[test]
    fn pdf_panels_live_projection_preserves_fallback_and_opted_in_payload() {
        let snapshot = SidePanelSnapshot {
            focus_revision: 123,
            focused_page_id: Some("report".into()),
            pages: vec![SidePanelPage {
                id: "report".into(),
                title: "Report".into(),
                format: SidePanelPageFormat::Pdf,
                source: SidePanelPageSource::LinkedFile,
                content: "PDF document fallback".into(),
                pdf_data: Some("JVBERi0xLjQKJSVFT0Y=".into()),
                ..Default::default()
            }],
        };
        let opted = side_panel_for_client(snapshot.clone(), true);
        assert_eq!(opted, snapshot);
        let mut projected = side_panel_for_client(snapshot.clone(), false);
        assert_eq!(projected.focus_revision, 123);
        assert_eq!(projected.focused_page_id, snapshot.focused_page_id);
        assert_eq!(projected.pages[0].content, snapshot.pages[0].content);
        assert_eq!(projected.pages[0].format, SidePanelPageFormat::Markdown);
        assert!(projected.pages[0].pdf_data.is_none());
        assert_eq!(projected.pages[0].source, SidePanelPageSource::Ephemeral);
        assert!(!crate::side_panel::refresh_linked_page_content(
            &mut projected,
            None
        ));

        // The shipped native reader has this closed enum. Unknown fields are
        // tolerated, but a new value in this existing field is not.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum LegacyFormat {
            Markdown,
        }
        #[derive(serde::Deserialize)]
        struct LegacyPage {
            format: LegacyFormat,
        }
        #[derive(serde::Deserialize)]
        struct LegacySnapshot {
            pages: Vec<LegacyPage>,
        }
        #[derive(serde::Deserialize)]
        struct LegacyEvent {
            snapshot: LegacySnapshot,
        }
        let wire = encode_event(&ServerEvent::SidePanelState {
            snapshot: projected,
        });
        let legacy: LegacyEvent = serde_json::from_str(&wire).unwrap();
        assert!(matches!(
            legacy.snapshot.pages[0].format,
            LegacyFormat::Markdown
        ));
        assert!(!wire.contains("pdf_data"));
        let wire = encode_event(&ServerEvent::SidePanelState { snapshot: opted });
        assert!(serde_json::from_str::<LegacyEvent>(&wire).is_err());
        assert!(wire.contains("JVBERi0xLjQKJSVFT0Y="));
    }
}
