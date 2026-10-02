//! Frame-reading limits: the bridge must not let one client's malformed or
//! unbounded input cost it memory or silence.

use super::*;

async fn read_all(input: &'static str) -> Vec<std::io::Result<String>> {
    let mut reader = BufReader::new(input.as_bytes());
    let mut out = Vec::new();
    loop {
        let mut line = String::new();
        match read_frame(&mut reader, &mut line).await {
            Ok(0) => return out,
            Ok(_) => out.push(Ok(line)),
            Err(error) => {
                out.push(Err(error));
                return out;
            }
        }
    }
}

#[tokio::test]
async fn frames_are_split_on_newlines() {
    let frames = read_all("{\"a\":1}\n{\"b\":2}\n").await;
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].as_ref().unwrap(), "{\"a\":1}\n");
    assert_eq!(frames[1].as_ref().unwrap(), "{\"b\":2}\n");
}

#[tokio::test]
async fn a_final_frame_without_a_newline_is_still_returned() {
    let frames = read_all("{\"a\":1}").await;
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].as_ref().unwrap(), "{\"a\":1}");
}

/// `read_line` grows its buffer until it finds a newline. Without a cap, a
/// client that opens a connection and sends bytes forever makes the bridge
/// allocate forever, and the bridge serves every API client on the machine.
#[tokio::test]
async fn an_unterminated_frame_is_refused_rather_than_buffered_forever() {
    let mut reader = BufReader::new(tokio::io::repeat(b'A'));
    let mut line = String::new();
    let error = read_frame(&mut reader, &mut line)
        .await
        .expect_err("an endless frame must fail, not buffer");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("exceeds"),
        "error should say why: {error}"
    );
    assert!(
        line.len() as u64 <= MAX_FRAME_BYTES,
        "buffered {} bytes, above the {MAX_FRAME_BYTES} cap",
        line.len()
    );
}

/// The cap must not clip legitimate traffic: a frame just under it, images
/// included, has to pass intact.
#[tokio::test]
async fn a_large_but_terminated_frame_is_accepted() {
    let payload = "B".repeat(1024 * 1024);
    let input = format!("{payload}\n");
    let mut reader = BufReader::new(input.as_bytes());
    let mut line = String::new();
    let read = read_frame(&mut reader, &mut line)
        .await
        .expect("a 1 MiB frame is legitimate");
    assert_eq!(read, payload.len() + 1);
    assert_eq!(line.trim_end(), payload);
}

/// Each call must start from an empty buffer. Reusing a `String` across frames
/// is the natural way to write the read loop, and forgetting to clear it
/// concatenates every request into one unparseable blob.
#[tokio::test]
async fn each_frame_starts_from_a_clean_buffer() {
    let input = "first\nsecond\n";
    let mut reader = BufReader::new(input.as_bytes());
    let mut line = String::from("stale contents");
    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line, "first\n");
    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line, "second\n");
}

#[tokio::test]
async fn aggregate_pdf_base64_payload_fits_transport_frame() {
    // 32 MiB decoded budget expands to 44,739,244 base64 bytes.
    let payload = "A".repeat((32 * 1024 * 1024_usize).div_ceil(3) * 4);
    let input = format!("{{\"pdf_data\":\"{payload}\"}}\n");
    let mut reader = BufReader::new(input.as_bytes());
    let mut line = String::new();
    assert_eq!(
        read_frame(&mut reader, &mut line).await.unwrap(),
        input.len()
    );
    assert_eq!(line, input);
}

#[tokio::test]
async fn cancelled_frame_reads_preserve_utf8_prefix_and_report_full_length() {
    let (mut writer, read) = tokio::io::duplex(128);
    let mut reader = BufReader::new(read);
    let mut frame = Vec::new();
    let bytes = "{\"text\":\"é\"}\n".as_bytes();
    let split = bytes.iter().position(|b| *b == 0xc3).unwrap() + 1;
    writer.write_all(&bytes[..split]).await.unwrap();
    tokio::select! {
        biased;
        result = read_frame_bytes(&mut reader, &mut frame) => panic!("partial frame completed: {result:?}"),
        _ = std::future::ready(()) => {}
    }
    assert_eq!(frame, bytes[..split]);
    writer.write_all(&bytes[split..]).await.unwrap();
    assert_eq!(
        read_frame_bytes(&mut reader, &mut frame).await.unwrap(),
        bytes.len()
    );
    assert_eq!(frame, bytes);
}

#[tokio::test]
async fn cancellation_does_not_reset_the_accumulated_frame_limit() {
    let mut frame = vec![b'A'; MAX_FRAME_BYTES as usize - 2];
    let mut reader = BufReader::new(&b"BCDE\n"[..]);
    let error = read_frame_bytes(&mut reader, &mut frame).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(frame.len() as u64, MAX_FRAME_BYTES);
}
