use super::*;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

fn jsonish(len: usize) -> String {
    let unit = r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"fn main() { println!(\"hi\"); }"}]},"#;
    unit.chars().cycle().take(len).collect()
}

#[test]
fn parses_extension_responses() {
    let a = parse_extension_response(["permessage-deflate"]);
    assert!(a.accepted && a.may_compress_outbound);
    let a = parse_extension_response(["permessage-deflate; client_max_window_bits=10"]);
    assert!(a.accepted && !a.may_compress_outbound);
    let a = parse_extension_response([
        "permessage-deflate; client_max_window_bits=15; server_no_context_takeover",
    ]);
    assert!(a.accepted && a.may_compress_outbound);
    let a = parse_extension_response(["x-webkit-deflate-frame"]);
    assert!(!a.accepted);
    assert!(!parse_extension_response(std::iter::empty()).accepted);
}

#[test]
fn deflate_roundtrips_through_rfc7692_inflate() {
    let text = jsonish(300_000);
    let compressed = deflate_message(text.as_bytes()).unwrap();
    assert!(compressed.len() * 3 < text.len());
    let mut input = compressed.clone();
    input.extend_from_slice(&DEFLATE_TAIL);
    let mut out = Vec::new();
    Inflater::new()
        .inflate(&input, &mut out, MAX_INFLATED_MESSAGE)
        .unwrap();
    assert_eq!(out, text.as_bytes());
}

#[test]
fn inflate_rejects_bombs() {
    let zeros = vec![0u8; 4 << 20];
    let mut input = deflate_message(&zeros).unwrap();
    input.extend_from_slice(&DEFLATE_TAIL);
    let mut out = Vec::new();
    let err = Inflater::new()
        .inflate(&input, &mut out, 1 << 20)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

/// A minimal RFC 7692 server: answers the upgrade (optionally accepting
/// deflate), then for each client data frame records whether RSV1 was set,
/// inflates it, and echoes it back compressed with context takeover (two
/// messages through one shared compressor) when deflate was agreed.
async fn reference_server(
    listener: tokio::net::TcpListener,
    accept_deflate: bool,
    messages: usize,
) -> Vec<(bool, usize, String)> {
    let (mut sock, _) = listener.accept().await.unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut tmp).await.unwrap();
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = find_header_end(&buf) {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    assert!(
        head.to_ascii_lowercase()
            .contains("sec-websocket-extensions: permessage-deflate")
    );
    let key = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("Sec-WebSocket-Key: ")
                .or_else(|| l.strip_prefix("sec-websocket-key: "))
        })
        .unwrap()
        .trim()
        .to_string();
    let mut resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
        derive_accept_key(key.as_bytes())
    );
    if accept_deflate {
        resp.push_str("Sec-WebSocket-Extensions: permessage-deflate\r\n");
    }
    resp.push_str("\r\n");
    sock.write_all(resp.as_bytes()).await.unwrap();
    buf.drain(..head_end);

    let mut inflater = Inflater::new();
    let mut server_compressor = flate2::Compress::new(flate2::Compression::fast(), false);
    let mut seen = Vec::new();
    while seen.len() < messages {
        let header = loop {
            if let Some(h) = parse_frame_header(&buf).unwrap()
                && buf.len() >= h.header_len + h.payload_len
            {
                break h;
            }
            let n = sock.read(&mut tmp).await.unwrap();
            assert!(n > 0, "client closed early");
            buf.extend_from_slice(&tmp[..n]);
        };
        let mut payload = buf[header.header_len..header.header_len + header.payload_len].to_vec();
        buf.drain(..header.header_len + header.payload_len);
        apply_mask(&mut payload, header.mask.expect("client frames are masked"));
        let text = if header.rsv1 {
            payload.extend_from_slice(&DEFLATE_TAIL);
            let mut out = Vec::new();
            inflater
                .inflate(&payload, &mut out, MAX_INFLATED_MESSAGE)
                .unwrap();
            String::from_utf8(out).unwrap()
        } else {
            String::from_utf8(payload.clone()).unwrap()
        };
        seen.push((header.rsv1, header.payload_len, text.clone()));
        // Echo back.
        let mut frame = Vec::new();
        if accept_deflate {
            let mut out = Vec::with_capacity(text.len() + 64);
            server_compressor
                .compress_vec(text.as_bytes(), &mut out, flate2::FlushCompress::Sync)
                .unwrap();
            while out.len() == out.capacity() {
                out.reserve(64 * 1024);
                server_compressor
                    .compress_vec(&[], &mut out, flate2::FlushCompress::Sync)
                    .unwrap();
            }
            assert!(out.ends_with(&DEFLATE_TAIL));
            out.truncate(out.len() - 4);
            write_frame_header(&mut frame, 0x80 | 0x40 | 0x1, None, out.len());
            frame.extend_from_slice(&out);
        } else {
            write_frame_header(&mut frame, 0x80 | 0x1, None, text.len());
            frame.extend_from_slice(text.as_bytes());
        }
        sock.write_all(&frame).await.unwrap();
    }
    seen
}

async fn run_client(
    addr: std::net::SocketAddr,
    mode: CompressionMode,
    texts: Vec<String>,
) -> (bool, Vec<String>) {
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let stream = DeflateStream::new(tcp, mode);
    let mut request = format!("ws://{addr}/").into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Extensions",
        PERMESSAGE_DEFLATE_OFFER.parse().unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap();
    let negotiated = ws.get_ref().deflate_negotiated();
    let mut echoes = Vec::new();
    for text in texts {
        ws.send(Message::Text(text)).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => echoes.push(t),
            other => panic!("unexpected {other:?}"),
        }
    }
    (negotiated, echoes)
}

#[tokio::test]
async fn compresses_large_messages_and_inflates_context_takeover_replies() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let big = jsonish(400_000);
    let big2 = jsonish(250_000) + "tail";
    let small = "{\"type\":\"response.create\"}".to_string();
    let texts = vec![big.clone(), small.clone(), big2.clone()];
    let server = tokio::spawn(reference_server(listener, true, texts.len()));
    let (negotiated, echoes) = run_client(addr, CompressionMode::Always, texts.clone()).await;
    let seen = server.await.unwrap();
    assert!(negotiated);
    assert_eq!(echoes, texts, "inflated replies must match");
    assert!(
        seen[0].0 && seen[0].1 * 3 < big.len(),
        "large message compressed"
    );
    assert!(!seen[1].0, "small message stays raw");
    assert!(seen[2].0, "second large message compressed");
    for (i, (_, _, text)) in seen.iter().enumerate() {
        assert_eq!(text, &texts[i], "server decoded the exact payload");
    }
}

#[tokio::test]
async fn declined_negotiation_is_a_transparent_passthrough() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let big = jsonish(300_000);
    let server = tokio::spawn(reference_server(listener, false, 1));
    let (negotiated, echoes) = run_client(addr, CompressionMode::Always, vec![big.clone()]).await;
    let seen = server.await.unwrap();
    assert!(!negotiated);
    assert_eq!(echoes, vec![big.clone()]);
    assert!(!seen[0].0, "must never set RSV1 without agreement");
    assert_eq!(seen[0].1, big.len());
}

#[tokio::test]
async fn off_mode_never_compresses_but_still_inflates() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let big = jsonish(300_000);
    let server = tokio::spawn(reference_server(listener, true, 1));
    let (negotiated, echoes) = run_client(addr, CompressionMode::Off, vec![big.clone()]).await;
    let seen = server.await.unwrap();
    assert!(negotiated);
    assert_eq!(echoes, vec![big]);
    assert!(!seen[0].0);
}
