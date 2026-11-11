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
    w.write_all(json.as_bytes()).await.map_err(|error| {
        let operation = super::client_error_context::DirectEventWrite::new(&error);
        anyhow::Error::new(error).context(operation)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_panel::{
        SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
    };
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    #[tokio::test]
    async fn direct_event_peer_close_preserves_operation_and_io_cause() {
        let (server, client) = crate::transport::Stream::pair().expect("socket pair");
        let (mut server_reader, writer) = server.into_split();
        let writer = Arc::new(Mutex::new(writer));
        let (reader, client_writer) = client.into_split();
        let mut reader = BufReader::new(reader);
        write_direct_event(&writer, &ServerEvent::Ack { id: 41 })
            .await
            .expect("connected peer accepts a response");
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_line(&mut line),
        )
        .await
        .expect("response deadline")
        .expect("response bytes");
        assert!(matches!(
            serde_json::from_str::<ServerEvent>(&line).expect("response event"),
            ServerEvent::Ack { id: 41 }
        ));
        drop(reader);
        drop(client_writer);

        let mut byte = [0];
        let closed = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            server_reader.read(&mut byte),
        )
        .await
        .expect("peer-close observation deadline");
        match closed {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            other => panic!("expected peer closure before response write, got {other:?}"),
        }

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            write_direct_event(&writer, &ServerEvent::Ack { id: 42 }),
        )
        .await
        .expect("peer-close deadline")
        .expect_err("closed peer must reject the response");
        assert!(format!("{error:#}").contains("operation=write_direct_event"));
        let native = error
            .downcast_ref::<std::io::Error>()
            .expect("native IO cause");
        assert!(
            error
                .to_string()
                .contains(&format!("io_error_kind={:?}", native.kind()))
        );
        assert!(
            error
                .to_string()
                .contains(&format!("raw_os_error={:?}", native.raw_os_error()))
        );
    }

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
