use jcode_provider_protocol::{
    Frame, ProtocolError, ProviderInfo, WireError, decode, encode, negotiate_capabilities,
};
use serde_json::json;
use std::io::{self, BufRead, Write};

fn write_frame(stdout: &mut impl Write, frame: &Frame) -> Result<(), Box<dyn std::error::Error>> {
    stdout.write_all(&encode(frame)?)?;
    stdout.flush()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let requested_capabilities = vec![
        jcode_provider_protocol::CAP_STREAMING.to_string(),
        jcode_provider_protocol::CAP_NATIVE_TOOLS.to_string(),
        jcode_provider_protocol::CAP_CANCELLATION.to_string(),
    ];

    for line in stdin.lock().lines() {
        let line = line?;
        let frame = decode(
            line.as_bytes(),
            jcode_provider_protocol::DEFAULT_MAX_FRAME_SIZE,
        )?;
        match frame {
            Frame::Hello {
                capabilities,
                max_frame_size,
                ..
            } => {
                let offered = negotiate_capabilities(&capabilities, &requested_capabilities);
                write_frame(
                    &mut stdout,
                    &Frame::HelloOk {
                        protocol_version: jcode_provider_protocol::PROTOCOL_VERSION.to_string(),
                        provider: ProviderInfo {
                            id: "fixture-provider".to_string(),
                            name: "Reference Fixture Provider".to_string(),
                            version: "0.1.0".to_string(),
                        },
                        capabilities: offered,
                    },
                )?;
                if max_frame_size.is_none() {
                    eprintln!("warning: client did not advertise a frame limit");
                }
            }
            Frame::Request { id, .. } => {
                write_frame(
                    &mut stdout,
                    &Frame::Event {
                        protocol_version: jcode_provider_protocol::PROTOCOL_VERSION.to_string(),
                        request_id: id.clone(),
                        event: "text_delta".to_string(),
                        payload: json!({"text": "fixture"}),
                    },
                )?;
                write_frame(
                    &mut stdout,
                    &Frame::Response {
                        protocol_version: jcode_provider_protocol::PROTOCOL_VERSION.to_string(),
                        id,
                        ok: true,
                        result: Some(json!({"text": "fixture response"})),
                        error: None,
                    },
                )?;
            }
            Frame::Cancel { request_id, .. } => {
                write_frame(
                    &mut stdout,
                    &Frame::Response {
                        protocol_version: jcode_provider_protocol::PROTOCOL_VERSION.to_string(),
                        id: request_id,
                        ok: false,
                        result: None,
                        error: Some(WireError {
                            code: "cancelled".to_string(),
                            message: "request cancelled".to_string(),
                            retryable: false,
                            details: json!({}),
                        }),
                    },
                )?;
            }
            Frame::HelloOk { .. } | Frame::Response { .. } | Frame::Event { .. } => {
                return Err(ProtocolError::Malformed(
                    "fixture received a server-only frame".to_string(),
                )
                .into());
            }
        }
    }
    Ok(())
}
