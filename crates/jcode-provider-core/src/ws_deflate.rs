//! Client-side WebSocket `permessage-deflate` (RFC 7692) as a byte-stream layer.
//!
//! tungstenite (every release through 0.30) has no extension hook and rejects
//! any frame with RSV1 set, so compression cannot be enabled through its API.
//! [`DeflateStream`] sits between the TLS stream and tungstenite instead:
//!
//! - It watches the HTTP upgrade response and enables itself only when the
//!   server answered `Sec-WebSocket-Extensions: permessage-deflate`. Without
//!   that answer it is a transparent pass-through.
//! - Outbound: whole, unfragmented text/binary frames that the compression
//!   policy selects are unmasked, raw-deflated (fresh context per message, which
//!   is always valid for the sender), re-masked, and sent with RSV1 set.
//! - Inbound: RSV1 messages are inflated with one persistent decompressor
//!   (servers may use context takeover) and handed to tungstenite as ordinary
//!   frames with RSV1 cleared.
//!
//! tungstenite therefore only ever sees uncompressed frames.

use std::io;
use std::io::Write;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::request_compression::{
    CompressionMode, STREAM_MIN_SAMPLE_BYTES, STREAM_SAMPLE_SKIP_BYTES,
    record_external_uplink_sample, should_compress_stream_message,
};

/// Header value a client sends to offer the extension.
pub const PERMESSAGE_DEFLATE_OFFER: &str = "permessage-deflate; client_max_window_bits";

const DEFLATE_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
/// Refuse to inflate a single message beyond this (decompression bomb guard),
/// matching tungstenite's default max message size.
const MAX_INFLATED_MESSAGE: usize = 64 << 20;
/// Largest HTTP upgrade response we are willing to buffer.
const MAX_HANDSHAKE_BYTES: usize = 64 * 1024;
const READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negotiation {
    /// Still inside the HTTP upgrade exchange.
    Handshake,
    /// Server declined (or never offered): transparent pass-through.
    Declined,
    /// Server accepted. `may_compress_outbound` is false when it constrained
    /// our window below 15 bits, which the default flate2 backend cannot honour.
    Accepted { may_compress_outbound: bool },
}

/// Result of parsing a server's `Sec-WebSocket-Extensions` response header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeflateAgreement {
    pub accepted: bool,
    pub may_compress_outbound: bool,
}

/// Parse the extension header values of a `101 Switching Protocols` response.
pub fn parse_extension_response<'a>(values: impl IntoIterator<Item = &'a str>) -> DeflateAgreement {
    let mut agreement = DeflateAgreement {
        accepted: false,
        may_compress_outbound: false,
    };
    for value in values {
        for offer in value.split(',') {
            let mut parts = offer.split(';').map(str::trim);
            if !parts
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("permessage-deflate"))
            {
                continue;
            }
            let mut may_compress = true;
            for param in parts {
                let (key, val) = match param.split_once('=') {
                    Some((k, v)) => (k.trim(), Some(v.trim().trim_matches('"'))),
                    None => (param, None),
                };
                if key.eq_ignore_ascii_case("client_max_window_bits")
                    && val
                        .and_then(|v| v.parse::<u8>().ok())
                        .is_some_and(|bits| bits < 15)
                {
                    may_compress = false;
                }
            }
            agreement = DeflateAgreement {
                accepted: true,
                may_compress_outbound: may_compress,
            };
            return agreement;
        }
    }
    agreement
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn negotiation_from_response(head: &[u8]) -> Negotiation {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let switching = lines
        .next()
        .is_some_and(|status| status.split_whitespace().nth(1) == Some("101"));
    if !switching {
        return Negotiation::Declined;
    }
    let values: Vec<&str> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("sec-websocket-extensions"))
        .map(|(_, value)| value.trim())
        .collect();
    let agreement = parse_extension_response(values);
    if agreement.accepted {
        Negotiation::Accepted {
            may_compress_outbound: agreement.may_compress_outbound,
        }
    } else {
        Negotiation::Declined
    }
}

/// Parsed frame header. `header_len` includes extended length and mask key.
#[derive(Debug, Clone, Copy)]
struct FrameHeader {
    fin: bool,
    rsv1: bool,
    opcode: u8,
    mask: Option<[u8; 4]>,
    header_len: usize,
    payload_len: usize,
}

fn parse_frame_header(buf: &[u8]) -> io::Result<Option<FrameHeader>> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let b0 = buf[0];
    let b1 = buf[1];
    let masked = b1 & 0x80 != 0;
    let (payload_len, mut offset) = match b1 & 0x7f {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let mut len = [0u8; 8];
            len.copy_from_slice(&buf[2..10]);
            (u64::from_be_bytes(len), 10)
        }
        n => (n as u64, 2),
    };
    let payload_len = usize::try_from(payload_len)
        .ok()
        .filter(|len| *len <= MAX_INFLATED_MESSAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "websocket frame too large"))?;
    let mask = if masked {
        if buf.len() < offset + 4 {
            return Ok(None);
        }
        let key = [
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ];
        offset += 4;
        Some(key)
    } else {
        None
    };
    Ok(Some(FrameHeader {
        fin: b0 & 0x80 != 0,
        rsv1: b0 & 0x40 != 0,
        opcode: b0 & 0x0f,
        mask,
        header_len: offset,
        payload_len,
    }))
}

fn write_frame_header(out: &mut Vec<u8>, b0: u8, mask: Option<[u8; 4]>, len: usize) {
    out.push(b0);
    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    if len < 126 {
        out.push(mask_bit | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    if let Some(key) = mask {
        out.extend_from_slice(&key);
    }
}

fn apply_mask(data: &mut [u8], key: [u8; 4]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i & 3];
    }
}

/// Raw-deflate one message with a fresh context and a sync flush, stripping
/// the trailing empty block as RFC 7692 section 7.2.1 requires.
pub fn deflate_message(payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(payload.len() / 2 + 16),
        flate2::Compression::fast(),
    );
    encoder.write_all(payload)?;
    encoder.flush()?;
    let mut out = std::mem::take(encoder.get_mut());
    if out.ends_with(&DEFLATE_TAIL) {
        out.truncate(out.len() - DEFLATE_TAIL.len());
    }
    Ok(out)
}

struct Inflater {
    decompress: flate2::Decompress,
}

impl Inflater {
    fn new() -> Self {
        Self {
            decompress: flate2::Decompress::new(false),
        }
    }

    /// Inflate `input` (one frame's payload, with the RFC 7692 tail appended on
    /// the final frame) into `out`, failing if `out` would exceed `budget`.
    fn inflate(&mut self, input: &[u8], out: &mut Vec<u8>, budget: usize) -> io::Result<()> {
        let mut consumed = 0usize;
        loop {
            out.reserve(READ_CHUNK);
            let before_in = self.decompress.total_in();
            let before_out = out.len();
            self.decompress
                .decompress_vec(&input[consumed..], out, flate2::FlushDecompress::Sync)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let used = (self.decompress.total_in() - before_in) as usize;
            consumed += used;
            if out.len() > budget {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "inflated websocket message exceeds size limit",
                ));
            }
            let spare_left = out.len() < out.capacity();
            // All input consumed and the decoder stopped with room to spare:
            // everything it can emit for this input has been emitted.
            if consumed >= input.len() && spare_left {
                return Ok(());
            }
            if used == 0 && out.len() == before_out && spare_left {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "permessage-deflate stream made no progress",
                ));
            }
        }
    }
}

struct UploadSample {
    total: usize,
    written: usize,
    clock: Option<(usize, Instant)>,
}

/// A transport wrapper adding client-side `permessage-deflate`. Construct with
/// [`DeflateStream::new`] around the (TLS) socket before the WebSocket
/// handshake, and offer [`PERMESSAGE_DEFLATE_OFFER`] in the request.
pub struct DeflateStream<S> {
    inner: S,
    mode: CompressionMode,
    negotiation: Negotiation,
    // Read side.
    read_raw: Vec<u8>,
    read_out: Vec<u8>,
    read_out_pos: usize,
    inbound_message_compressed: bool,
    inbound_message_size: usize,
    inflater: Inflater,
    read_eof: bool,
    // Write side.
    write_handshake_done: bool,
    write_in: Vec<u8>,
    write_out: Vec<u8>,
    write_out_pos: usize,
    outbound_fragmented: bool,
    sample: Option<UploadSample>,
}

impl<S> DeflateStream<S> {
    pub fn new(inner: S, mode: CompressionMode) -> Self {
        Self {
            inner,
            mode,
            negotiation: Negotiation::Handshake,
            read_raw: Vec::new(),
            read_out: Vec::new(),
            read_out_pos: 0,
            inbound_message_compressed: false,
            inbound_message_size: 0,
            inflater: Inflater::new(),
            read_eof: false,
            write_handshake_done: false,
            write_in: Vec::new(),
            write_out: Vec::new(),
            write_out_pos: 0,
            outbound_fragmented: false,
            sample: None,
        }
    }

    /// Whether the server accepted `permessage-deflate`.
    pub fn deflate_negotiated(&self) -> bool {
        matches!(self.negotiation, Negotiation::Accepted { .. })
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    fn push_read_out(&mut self, bytes: &[u8]) {
        if self.read_out_pos == self.read_out.len() {
            self.read_out.clear();
            self.read_out_pos = 0;
        }
        self.read_out.extend_from_slice(bytes);
    }

    /// Move as much of `read_raw` as possible into `read_out`, rewriting
    /// compressed frames.
    fn process_read(&mut self) -> io::Result<()> {
        if self.negotiation == Negotiation::Handshake {
            match find_header_end(&self.read_raw) {
                Some(end) => {
                    let head: Vec<u8> = self.read_raw.drain(..end).collect();
                    self.negotiation = negotiation_from_response(&head);
                    self.push_read_out(&head);
                }
                None if self.read_raw.len() > MAX_HANDSHAKE_BYTES => {
                    // Not an HTTP upgrade we understand: never touch frames.
                    self.negotiation = Negotiation::Declined;
                }
                None => return Ok(()),
            }
        }
        if !self.deflate_negotiated() {
            let raw = std::mem::take(&mut self.read_raw);
            self.push_read_out(&raw);
            return Ok(());
        }
        let mut cursor = 0usize;
        while let Some(header) = parse_frame_header(&self.read_raw[cursor..])? {
            let frame_len = header.header_len + header.payload_len;
            if self.read_raw.len() - cursor < frame_len {
                break;
            }
            let frame = cursor..cursor + frame_len;
            cursor += frame_len;
            let is_data = matches!(header.opcode, 0x0 | 0x1 | 0x2);
            if header.opcode != 0x0 && is_data {
                self.inbound_message_compressed = header.rsv1;
                self.inbound_message_size = 0;
            }
            if !is_data || !self.inbound_message_compressed || header.mask.is_some() {
                let bytes = self.read_raw[frame].to_vec();
                self.push_read_out(&bytes);
                continue;
            }
            let payload_start = frame.start + header.header_len;
            let mut payload = self.read_raw[payload_start..frame.end].to_vec();
            if header.fin {
                payload.extend_from_slice(&DEFLATE_TAIL);
            }
            let mut inflated = Vec::new();
            let budget = MAX_INFLATED_MESSAGE.saturating_sub(self.inbound_message_size);
            self.inflater.inflate(&payload, &mut inflated, budget)?;
            self.inbound_message_size += inflated.len();
            let b0 = self.read_raw[frame.start] & !0x40;
            let mut rewritten = Vec::with_capacity(inflated.len() + 10);
            write_frame_header(&mut rewritten, b0, None, inflated.len());
            rewritten.extend_from_slice(&inflated);
            self.push_read_out(&rewritten);
            if header.fin {
                self.inbound_message_compressed = false;
            }
        }
        self.read_raw.drain(..cursor);
        Ok(())
    }

    /// Accept bytes from tungstenite into the outbound pipeline.
    fn accept_write(&mut self, buf: &[u8]) -> io::Result<()> {
        if !self.write_handshake_done {
            self.write_in.extend_from_slice(buf);
            match find_header_end(&self.write_in) {
                Some(end) => {
                    let head: Vec<u8> = self.write_in.drain(..end).collect();
                    self.write_out.extend_from_slice(&head);
                    self.write_handshake_done = true;
                }
                None if self.write_in.len() > MAX_HANDSHAKE_BYTES => {
                    let raw = std::mem::take(&mut self.write_in);
                    self.write_out.extend_from_slice(&raw);
                    self.write_handshake_done = true;
                    return Ok(());
                }
                None => return Ok(()),
            }
        } else {
            self.write_in.extend_from_slice(buf);
        }
        let may_compress = matches!(
            self.negotiation,
            Negotiation::Accepted {
                may_compress_outbound: true
            }
        );
        let mut cursor = 0usize;
        while let Some(header) = parse_frame_header(&self.write_in[cursor..])? {
            let frame_len = header.header_len + header.payload_len;
            if self.write_in.len() - cursor < frame_len {
                break;
            }
            let frame = cursor..cursor + frame_len;
            cursor += frame_len;
            let whole_data_frame = header.fin
                && !header.rsv1
                && matches!(header.opcode, 0x1 | 0x2)
                && !self.outbound_fragmented;
            if matches!(header.opcode, 0x1 | 0x2) && !header.fin {
                self.outbound_fragmented = true;
            } else if header.opcode == 0x0 && header.fin {
                self.outbound_fragmented = false;
            }
            let compress = may_compress
                && whole_data_frame
                && should_compress_stream_message(header.payload_len, self.mode);
            let wire_start = self.write_out.len();
            if compress {
                let payload_start = frame.start + header.header_len;
                let mut payload = self.write_in[payload_start..frame.end].to_vec();
                if let Some(key) = header.mask {
                    apply_mask(&mut payload, key);
                }
                let started = Instant::now();
                let mut compressed = deflate_message(&payload)?;
                if compressed.len() < payload.len() {
                    if let Some(key) = header.mask {
                        apply_mask(&mut compressed, key);
                    }
                    let b0 = self.write_in[frame.start] | 0x40;
                    write_frame_header(&mut self.write_out, b0, header.mask, compressed.len());
                    self.write_out.extend_from_slice(&compressed);
                    jcode_logging::info(&format!(
                        "request_compression: websocket permessage-deflate raw={}B wire={}B encode={}ms",
                        payload.len(),
                        compressed.len(),
                        started.elapsed().as_millis()
                    ));
                } else {
                    let bytes = self.write_in[frame].to_vec();
                    self.write_out.extend_from_slice(&bytes);
                }
            } else {
                let bytes = self.write_in[frame].to_vec();
                self.write_out.extend_from_slice(&bytes);
            }
            let wire_len = self.write_out.len() - wire_start;
            if self.mode != CompressionMode::Off
                && whole_data_frame
                && wire_len >= STREAM_MIN_SAMPLE_BYTES
                && self.sample.is_none()
            {
                self.sample = Some(UploadSample {
                    total: self.write_out.len() - self.write_out_pos,
                    written: 0,
                    clock: None,
                });
            }
        }
        self.write_in.drain(..cursor);
        Ok(())
    }
}

impl<S: AsyncWrite + Unpin> DeflateStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_out_pos < self.write_out.len() {
            let n = ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.write_out[self.write_out_pos..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.write_out_pos += n;
            if let Some(sample) = self.sample.as_mut() {
                sample.written += n;
                if sample.clock.is_none() && sample.written >= STREAM_SAMPLE_SKIP_BYTES {
                    sample.clock = Some((sample.written, Instant::now()));
                }
                if sample.written >= sample.total {
                    if let Some((from, at)) = sample.clock {
                        record_external_uplink_sample(sample.written - from, at.elapsed());
                    }
                    self.sample = None;
                }
            }
        }
        self.write_out.clear();
        self.write_out_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for DeflateStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.read_out_pos < this.read_out.len() {
                let available = &this.read_out[this.read_out_pos..];
                let n = available.len().min(buf.remaining());
                buf.put_slice(&available[..n]);
                this.read_out_pos += n;
                return Poll::Ready(Ok(()));
            }
            if this.read_eof {
                // Surface any trailing partial bytes so tungstenite reports
                // the truncation itself.
                if !this.read_raw.is_empty() {
                    let raw = std::mem::take(&mut this.read_raw);
                    this.push_read_out(&raw);
                    continue;
                }
                return Poll::Ready(Ok(()));
            }
            let mut chunk = vec![0u8; READ_CHUNK];
            let mut read_buf = ReadBuf::new(&mut chunk);
            ready!(Pin::new(&mut this.inner).poll_read(cx, &mut read_buf))?;
            let filled = read_buf.filled().len();
            if filled == 0 {
                this.read_eof = true;
                continue;
            }
            this.read_raw.extend_from_slice(&chunk[..filled]);
            this.process_read()?;
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for DeflateStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        // Backpressure: never queue more than one batch ahead of the socket.
        ready!(this.poll_drain(cx))?;
        this.accept_write(buf)?;
        // Opportunistically start sending; completion is driven by flush.
        if let Poll::Ready(Err(err)) = this.poll_drain(cx) {
            return Poll::Ready(Err(err));
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "ws_deflate_tests.rs"]
mod tests;
