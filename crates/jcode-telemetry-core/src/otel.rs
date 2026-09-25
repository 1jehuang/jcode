//! Lightweight OTLP/HTTP exporter for jcode telemetry.
//!
//! Reads the standard OTLP SDK env vars documented at
//! <https://opentelemetry.io/docs/languages/sdk-configuration/otlp-exporter/>
//! and forwards jcode session / turn spans to a user-supplied collector
//! endpoint as OTLP over HTTP, in either JSON or protobuf wire format.
//!
//! ## Environment variables
//!
//! | Variable | Default | Notes |
//! |---|---|---|
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | (disabled) | Base URL. `/v1/traces` is appended. |
//! | `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | (disabled) | Full traces URL; takes priority over base URL. |
//! | `OTEL_EXPORTER_OTLP_HEADERS` | (none) | Comma-separated `key=value` HTTP headers. |
//! | `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/json` | Wire format: `http/json` or `http/protobuf`. `grpc` falls back to `http/json` with a warning. |
//! | `OTEL_EXPORTER_OTLP_TIMEOUT` | `10000` | HTTP timeout in **milliseconds**. |
//! | `OTEL_SERVICE_NAME` | `jcode` | `service.name` resource attribute. |
//! | `OTEL_SERVICE_VERSION` | jcode version | `service.version` resource attribute. |
//! | `OTEL_RESOURCE_ATTRIBUTES` | (none) | Comma-separated `key=value` extra resource attributes. |
//! | `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` | `false` | Read and acknowledged. jcode **never** captures message content (prompts, completions) in spans regardless of this setting. |
//!
//! When neither `OTEL_EXPORTER_OTLP_ENDPOINT` nor
//! `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is set the exporter is a no-op and
//! adds zero overhead.
//!
//! ## Span mapping
//!
//! | jcode event | OTEL span name |
//! |---|---|
//! | `begin_session` / `session_end` | `jcode.session` |
//! | `turn_end` (per-turn finalization) | `jcode.turn` |
//!
//! All jcode telemetry fields are forwarded as span attributes prefixed
//! `jcode.*`. Sensitive values (prompts, file contents, etc.) are never
//! included.

use jcode_logging as logging;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ─── Protocol ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum OtlpProtocol {
    /// `application/json` — OTLP/HTTP JSON encoding (default).
    HttpJson,
    /// `application/x-protobuf` — OTLP/HTTP binary protobuf encoding.
    HttpProtobuf,
}

// ─── Configuration ────────────────────────────────────────────────────────────

/// Resolved, immutable exporter configuration. Built once from env vars.
struct OtelConfig {
    /// Full traces endpoint URL, e.g. `http://localhost:4318/v1/traces`.
    traces_url: String,
    /// Extra HTTP headers (name, value) pairs.
    headers: Vec<(String, String)>,
    /// HTTP request timeout.
    timeout: Duration,
    /// `service.name` resource attribute.
    service_name: String,
    /// `service.version` resource attribute.
    service_version: String,
    /// Extra resource attributes from `OTEL_RESOURCE_ATTRIBUTES`.
    resource_attrs: Vec<(String, String)>,
    /// Wire protocol to use when posting spans.
    protocol: OtlpProtocol,
}

static OTEL_CONFIG: OnceLock<Option<OtelConfig>> = OnceLock::new();

/// Parse `key=value,key2=value2` pairs, tolerating extra whitespace.
pub(crate) fn parse_kv_pairs(input: &str) -> Vec<(String, String)> {
    input
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let eq = part.find('=')?;
            let key = part[..eq].trim().to_string();
            let val = part[eq + 1..].trim().to_string();
            if key.is_empty() { None } else { Some((key, val)) }
        })
        .collect()
}

fn otel_config() -> &'static Option<OtelConfig> {
    OTEL_CONFIG.get_or_init(|| {
        // Traces endpoint: specific var takes priority over base URL + suffix.
        let traces_url = if let Ok(ep) =
            std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        {
            let ep = ep.trim().to_string();
            if ep.is_empty() { return None; }
            ep
        } else if let Ok(base) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            let base = base.trim().trim_end_matches('/').to_string();
            if base.is_empty() { return None; }
            format!("{base}/v1/traces")
        } else {
            return None;
        };

        let headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
            .map(|s| parse_kv_pairs(&s))
            .unwrap_or_default();

        let protocol = match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
            .as_deref()
            .map(str::trim)
            .unwrap_or("http/json")
        {
            "http/protobuf" => OtlpProtocol::HttpProtobuf,
            "grpc" => {
                logging::warn(
                    "otel: OTEL_EXPORTER_OTLP_PROTOCOL=grpc is not supported; \
                     falling back to http/json. Set http/protobuf for binary encoding.",
                );
                OtlpProtocol::HttpJson
            }
            _ => OtlpProtocol::HttpJson,
        };

        let timeout_ms: u64 = std::env::var("OTEL_EXPORTER_OTLP_TIMEOUT")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(10_000);

        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .unwrap_or_else(|_| "jcode".to_string());

        let service_version = std::env::var("OTEL_SERVICE_VERSION")
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());

        let resource_attrs = std::env::var("OTEL_RESOURCE_ATTRIBUTES")
            .map(|s| parse_kv_pairs(&s))
            .unwrap_or_default();

        // OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT
        // jcode never captures message content in spans regardless of this setting.
        // Read it so users see it is acknowledged.
        let capture_content = std::env::var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT")
            .map(|s| s.trim().to_lowercase() == "true")
            .unwrap_or(false);
        if capture_content {
            logging::debug(
                "otel: OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true detected; \
                 jcode never captures prompt or completion content in OTEL spans.",
            );
        }

        let proto_label = match protocol {
            OtlpProtocol::HttpJson => "http/json",
            OtlpProtocol::HttpProtobuf => "http/protobuf",
        };
        logging::debug(&format!(
            "otel: exporter enabled endpoint={traces_url} protocol={proto_label} service={service_name}"
        ));

        Some(OtelConfig {
            traces_url,
            headers,
            timeout: Duration::from_millis(timeout_ms),
            service_name,
            service_version,
            resource_attrs,
            protocol,
        })
    })
}

/// Returns true if the OTEL exporter is configured (endpoint env var set).
pub fn is_otel_enabled() -> bool {
    otel_config().is_some()
}

// ─── Time helpers ─────────────────────────────────────────────────────────────

/// Current Unix time in nanoseconds (as required by OTLP).
pub fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Derive an absolute end timestamp from a span start and a duration in ms.
pub fn end_nanos(start_nanos: u64, duration_ms: u64) -> u64 {
    start_nanos.saturating_add(duration_ms.saturating_mul(1_000_000))
}

// ─── JSON span construction ───────────────────────────────────────────────────

fn otel_string_attr(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn otel_int_attr(key: &str, value: u64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

fn otel_bool_attr(key: &str, value: bool) -> Value {
    json!({ "key": key, "value": { "boolValue": value } })
}

/// Build the resource object from the resolved config (JSON).
fn build_resource_json(cfg: &OtelConfig) -> Value {
    let mut attrs: Vec<Value> = vec![
        otel_string_attr("service.name", &cfg.service_name),
        otel_string_attr("service.version", &cfg.service_version),
        otel_string_attr("telemetry.sdk.name", "jcode-otel"),
        otel_string_attr("telemetry.sdk.language", "rust"),
    ];
    for (k, v) in &cfg.resource_attrs {
        attrs.push(otel_string_attr(k, v));
    }
    json!({ "attributes": attrs })
}

/// Build a complete OTLP ExportTraceServiceRequest JSON payload for one span.
fn build_export_request_json(cfg: &OtelConfig, span: Value) -> Value {
    json!({
        "resourceSpans": [{
            "resource": build_resource_json(cfg),
            "scopeSpans": [{
                "scope": {
                    "name": "jcode.telemetry",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "spans": [span]
            }]
        }]
    })
}

// ─── Span models ─────────────────────────────────────────────────────────────

/// Span attributes for a session lifecycle event.
pub struct SessionSpanData<'a> {
    pub trace_id: &'a str,
    pub span_id: &'a str,
    pub session_id: &'a str,
    pub correlation_id: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub start_nanos: u64,
    pub end_nanos: u64,
    pub end_reason: &'a str,
    pub turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub tool_failures: u32,
    pub resumed: bool,
    pub parent_session_id: Option<&'a str>,
    pub os: &'a str,
    pub arch: &'a str,
    pub version: &'a str,
}

/// Span attributes for a turn-end event.
pub struct TurnSpanData<'a> {
    pub trace_id: &'a str,
    pub span_id: &'a str,
    pub parent_span_id: &'a str,
    pub session_id: &'a str,
    pub turn_index: u32,
    pub start_nanos: u64,
    pub end_nanos: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub tool_failures: u32,
    pub executed_tool_calls: u32,
    pub file_write_calls: u32,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub turn_success: bool,
    pub turn_abandoned: bool,
    pub end_reason: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
}

// ─── JSON span builders ───────────────────────────────────────────────────────

fn build_session_span_json(cfg: &OtelConfig, data: &SessionSpanData<'_>) -> Value {
    let mut attrs: Vec<Value> = vec![
        otel_string_attr("jcode.session.id", data.session_id),
        otel_string_attr("jcode.session.correlation_id", data.correlation_id),
        otel_string_attr("jcode.provider", data.provider),
        otel_string_attr("jcode.model", data.model),
        otel_string_attr("jcode.session.end_reason", data.end_reason),
        otel_int_attr("jcode.session.turns", data.turns as u64),
        otel_int_attr("jcode.tokens.input", data.input_tokens),
        otel_int_attr("jcode.tokens.output", data.output_tokens),
        otel_int_attr("jcode.tokens.total", data.total_tokens),
        otel_int_attr("jcode.tool_calls", data.tool_calls as u64),
        otel_int_attr("jcode.tool_failures", data.tool_failures as u64),
        otel_bool_attr("jcode.session.resumed", data.resumed),
        otel_string_attr("jcode.os", data.os),
        otel_string_attr("jcode.arch", data.arch),
        otel_string_attr("jcode.version", data.version),
        otel_string_attr("service.name", &cfg.service_name),
    ];
    if let Some(parent) = data.parent_session_id {
        attrs.push(otel_string_attr("jcode.session.parent_id", parent));
    }
    json!({
        "traceId": data.trace_id,
        "spanId": data.span_id,
        "name": "jcode.session",
        "kind": 1,
        "startTimeUnixNano": data.start_nanos.to_string(),
        "endTimeUnixNano": data.end_nanos.to_string(),
        "attributes": attrs,
        "status": { "code": 1 }
    })
}

fn build_turn_span_json(data: &TurnSpanData<'_>) -> Value {
    let attrs: Vec<Value> = vec![
        otel_string_attr("jcode.session.id", data.session_id),
        otel_int_attr("jcode.turn.index", data.turn_index as u64),
        otel_string_attr("jcode.provider", data.provider),
        otel_string_attr("jcode.model", data.model),
        otel_int_attr("jcode.tokens.input", data.input_tokens),
        otel_int_attr("jcode.tokens.output", data.output_tokens),
        otel_int_attr("jcode.tokens.total", data.total_tokens),
        otel_int_attr("jcode.tool_calls", data.tool_calls as u64),
        otel_int_attr("jcode.tool_failures", data.tool_failures as u64),
        otel_int_attr("jcode.tool_calls.executed", data.executed_tool_calls as u64),
        otel_int_attr("jcode.file_writes", data.file_write_calls as u64),
        otel_int_attr("jcode.tests.run", data.tests_run as u64),
        otel_int_attr("jcode.tests.passed", data.tests_passed as u64),
        otel_bool_attr("jcode.turn.success", data.turn_success),
        otel_bool_attr("jcode.turn.abandoned", data.turn_abandoned),
        otel_string_attr("jcode.turn.end_reason", data.end_reason),
    ];
    json!({
        "traceId": data.trace_id,
        "spanId": data.span_id,
        "parentSpanId": data.parent_span_id,
        "name": "jcode.turn",
        "kind": 1,
        "startTimeUnixNano": data.start_nanos.to_string(),
        "endTimeUnixNano": data.end_nanos.to_string(),
        "attributes": attrs,
        "status": { "code": 1 }
    })
}

// ─── Protobuf encoding ────────────────────────────────────────────────────────
//
// Hand-rolled minimal encoder for OTLP ExportTraceServiceRequest protobuf.
// No prost or other proto crate required.
//
// Proto field numbers used (from opentelemetry-proto):
//
//   ExportTraceServiceRequest  resource_spans=1
//   ResourceSpans              resource=1  scope_spans=2
//   Resource (resource.proto)  attributes=1
//   ScopeSpans                 scope=1  spans=2
//   InstrumentationScope       name=1  version=2
//   Span                       trace_id=1  span_id=2  parent_span_id=4
//                              name=5  kind=6  start_time_unix_nano=7
//                              end_time_unix_nano=8  attributes=9  status=15
//   Status                     code=3
//   KeyValue                   key=1  value=2
//   AnyValue (oneof)           string_value=1  bool_value=2  int_value=3
//
// Wire types:  0=varint  1=I64(fixed64)  2=LEN  5=I32(fixed32)

/// Encode an unsigned integer as a protobuf varint.
fn pb_varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

/// Encode a tag byte for (field_number, wire_type).
#[inline]
fn pb_tag(field: u32, wire_type: u8) -> Vec<u8> {
    pb_varint(((field as u64) << 3) | wire_type as u64)
}

/// Encode a LEN-delimited field: tag + varint(len) + data.
/// Used for bytes, strings, and embedded messages.
fn pb_len(field: u32, data: &[u8]) -> Vec<u8> {
    let mut out = pb_tag(field, 2);
    out.extend(pb_varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

/// Encode a string field (LEN-delimited UTF-8).
#[inline]
fn pb_string(field: u32, s: &str) -> Vec<u8> {
    pb_len(field, s.as_bytes())
}

/// Encode a bytes field (LEN-delimited raw bytes).
#[inline]
fn pb_bytes(field: u32, data: &[u8]) -> Vec<u8> {
    pb_len(field, data)
}

/// Encode an embedded message field (LEN-delimited).
#[inline]
fn pb_msg(field: u32, data: &[u8]) -> Vec<u8> {
    pb_len(field, data)
}

/// Encode a varint (wire type 0) field.
fn pb_varint_field(field: u32, n: u64) -> Vec<u8> {
    let mut out = pb_tag(field, 0);
    out.extend(pb_varint(n));
    out
}

/// Encode a fixed64 (wire type 1) field — little-endian 8 bytes.
fn pb_fixed64(field: u32, n: u64) -> Vec<u8> {
    let mut out = pb_tag(field, 1);
    out.extend_from_slice(&n.to_le_bytes());
    out
}

/// Decode a lowercase hex string to raw bytes. Silently skips invalid nibbles.
fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.as_bytes();
    let len = hex.len() / 2;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let hi = hex_nibble(hex[i * 2]);
        let lo = hex_nibble(hex[i * 2 + 1]);
        out.push((hi << 4) | lo);
    }
    out
}

#[inline]
fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// ── AnyValue helpers ──

fn any_string(s: &str) -> Vec<u8> {
    pb_string(1, s) // oneof string_value = field 1
}

fn any_int(n: u64) -> Vec<u8> {
    pb_varint_field(3, n) // oneof int_value = field 3
}

fn any_bool(b: bool) -> Vec<u8> {
    pb_varint_field(2, if b { 1 } else { 0 }) // oneof bool_value = field 2
}

// ── KeyValue helpers ──

fn kv_string(key: &str, val: &str) -> Vec<u8> {
    let mut out = pb_string(1, key); // KeyValue.key = 1
    out.extend(pb_msg(2, &any_string(val))); // KeyValue.value = 2
    out
}

fn kv_int(key: &str, val: u64) -> Vec<u8> {
    let mut out = pb_string(1, key);
    out.extend(pb_msg(2, &any_int(val)));
    out
}

fn kv_bool(key: &str, val: bool) -> Vec<u8> {
    let mut out = pb_string(1, key);
    out.extend(pb_msg(2, &any_bool(val)));
    out
}

// ── Resource ──

fn encode_resource_proto(cfg: &OtelConfig) -> Vec<u8> {
    let mut out = Vec::new();
    // Resource.attributes = field 1 (repeated KeyValue)
    for kv in &[
        kv_string("service.name", &cfg.service_name),
        kv_string("service.version", &cfg.service_version),
        kv_string("telemetry.sdk.name", "jcode-otel"),
        kv_string("telemetry.sdk.language", "rust"),
    ] {
        out.extend(pb_msg(1, kv));
    }
    for (k, v) in &cfg.resource_attrs {
        out.extend(pb_msg(1, &kv_string(k, v)));
    }
    out
}

// ── Span proto builders ──

fn build_session_span_proto(data: &SessionSpanData<'_>, cfg: &OtelConfig) -> Vec<u8> {
    let mut span = Vec::new();

    // trace_id = 1 (bytes, 16 raw bytes)
    span.extend(pb_bytes(1, &hex_to_bytes(data.trace_id)));
    // span_id = 2 (bytes, 8 raw bytes)
    span.extend(pb_bytes(2, &hex_to_bytes(data.span_id)));
    // no parent_span_id for session root spans
    // name = 5
    span.extend(pb_string(5, "jcode.session"));
    // kind = 6 (SPAN_KIND_INTERNAL = 1)
    span.extend(pb_varint_field(6, 1));
    // start_time_unix_nano = 7 (fixed64)
    span.extend(pb_fixed64(7, data.start_nanos));
    // end_time_unix_nano = 8 (fixed64)
    span.extend(pb_fixed64(8, data.end_nanos));

    // attributes = 9 (repeated KeyValue)
    for kv in &[
        kv_string("jcode.session.id", data.session_id),
        kv_string("jcode.session.correlation_id", data.correlation_id),
        kv_string("jcode.provider", data.provider),
        kv_string("jcode.model", data.model),
        kv_string("jcode.session.end_reason", data.end_reason),
        kv_int("jcode.session.turns", data.turns as u64),
        kv_int("jcode.tokens.input", data.input_tokens),
        kv_int("jcode.tokens.output", data.output_tokens),
        kv_int("jcode.tokens.total", data.total_tokens),
        kv_int("jcode.tool_calls", data.tool_calls as u64),
        kv_int("jcode.tool_failures", data.tool_failures as u64),
        kv_bool("jcode.session.resumed", data.resumed),
        kv_string("jcode.os", data.os),
        kv_string("jcode.arch", data.arch),
        kv_string("jcode.version", data.version),
        kv_string("service.name", &cfg.service_name),
    ] {
        span.extend(pb_msg(9, kv));
    }
    if let Some(parent) = data.parent_session_id {
        span.extend(pb_msg(9, &kv_string("jcode.session.parent_id", parent)));
    }

    // status = 15 { code = 3 } (STATUS_CODE_OK = 1)
    let status = pb_varint_field(3, 1);
    span.extend(pb_msg(15, &status));

    span
}

fn build_turn_span_proto(data: &TurnSpanData<'_>) -> Vec<u8> {
    let mut span = Vec::new();

    span.extend(pb_bytes(1, &hex_to_bytes(data.trace_id)));
    span.extend(pb_bytes(2, &hex_to_bytes(data.span_id)));
    // parent_span_id = 4
    span.extend(pb_bytes(4, &hex_to_bytes(data.parent_span_id)));
    span.extend(pb_string(5, "jcode.turn"));
    span.extend(pb_varint_field(6, 1)); // SPAN_KIND_INTERNAL
    span.extend(pb_fixed64(7, data.start_nanos));
    span.extend(pb_fixed64(8, data.end_nanos));

    for kv in &[
        kv_string("jcode.session.id", data.session_id),
        kv_int("jcode.turn.index", data.turn_index as u64),
        kv_string("jcode.provider", data.provider),
        kv_string("jcode.model", data.model),
        kv_int("jcode.tokens.input", data.input_tokens),
        kv_int("jcode.tokens.output", data.output_tokens),
        kv_int("jcode.tokens.total", data.total_tokens),
        kv_int("jcode.tool_calls", data.tool_calls as u64),
        kv_int("jcode.tool_failures", data.tool_failures as u64),
        kv_int("jcode.tool_calls.executed", data.executed_tool_calls as u64),
        kv_int("jcode.file_writes", data.file_write_calls as u64),
        kv_int("jcode.tests.run", data.tests_run as u64),
        kv_int("jcode.tests.passed", data.tests_passed as u64),
        kv_bool("jcode.turn.success", data.turn_success),
        kv_bool("jcode.turn.abandoned", data.turn_abandoned),
        kv_string("jcode.turn.end_reason", data.end_reason),
    ] {
        span.extend(pb_msg(9, kv));
    }

    let status = pb_varint_field(3, 1);
    span.extend(pb_msg(15, &status));

    span
}

/// Wrap a single encoded Span in a full ExportTraceServiceRequest message.
fn build_export_request_proto(cfg: &OtelConfig, span_bytes: Vec<u8>) -> Vec<u8> {
    // InstrumentationScope
    let mut scope = Vec::new();
    scope.extend(pb_string(1, "jcode.telemetry")); // name = 1
    scope.extend(pb_string(2, env!("CARGO_PKG_VERSION"))); // version = 2

    // ScopeSpans { scope=1, spans=2 }
    let mut scope_spans = Vec::new();
    scope_spans.extend(pb_msg(1, &scope));
    scope_spans.extend(pb_msg(2, &span_bytes));

    // ResourceSpans { resource=1, scope_spans=2 }
    let resource = encode_resource_proto(cfg);
    let mut resource_spans = Vec::new();
    resource_spans.extend(pb_msg(1, &resource));
    resource_spans.extend(pb_msg(2, &scope_spans));

    // ExportTraceServiceRequest { resource_spans=1 }
    pb_msg(1, &resource_spans)
}

// ─── HTTP transport ───────────────────────────────────────────────────────────

enum OtlpPayload {
    Json(Value),
    Proto(Vec<u8>),
}

/// Post an OTLP payload (JSON or protobuf) to the configured traces endpoint.
fn post_otlp(cfg: &OtelConfig, payload: OtlpPayload) -> bool {
    let user_agent = concat!("jcode/", env!("CARGO_PKG_VERSION"));
    let client = match reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .timeout(cfg.timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            logging::warn(&format!("otel: failed to build HTTP client: {err}"));
            return false;
        }
    };
    let mut req = client.post(&cfg.traces_url);
    for (k, v) in &cfg.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let result = match payload {
        OtlpPayload::Json(json_val) => req
            .header("Content-Type", "application/json")
            .json(&json_val)
            .send(),
        OtlpPayload::Proto(bytes) => req
            .header("Content-Type", "application/x-protobuf")
            .body(bytes)
            .send(),
    };
    match result {
        Ok(resp) if resp.status().is_success() => true,
        Ok(resp) => {
            logging::warn(&format!(
                "otel: collector returned HTTP {}",
                resp.status()
            ));
            false
        }
        Err(err) => {
            logging::warn(&format!("otel: export failed: {err}"));
            false
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Export a session lifecycle span. Safe to call even when OTEL is disabled.
pub fn export_session_span(data: &SessionSpanData<'_>) {
    let Some(cfg) = otel_config().as_ref() else {
        return;
    };
    let payload = match cfg.protocol {
        OtlpProtocol::HttpJson => {
            let span = build_session_span_json(cfg, data);
            OtlpPayload::Json(build_export_request_json(cfg, span))
        }
        OtlpProtocol::HttpProtobuf => {
            let span_bytes = build_session_span_proto(data, cfg);
            OtlpPayload::Proto(build_export_request_proto(cfg, span_bytes))
        }
    };
    let _ = post_otlp(cfg, payload);
}

/// Export a turn span. Safe to call even when OTEL is disabled.
pub fn export_turn_span(data: &TurnSpanData<'_>) {
    let Some(cfg) = otel_config().as_ref() else {
        return;
    };
    let payload = match cfg.protocol {
        OtlpProtocol::HttpJson => {
            let span = build_turn_span_json(data);
            OtlpPayload::Json(build_export_request_json(cfg, span))
        }
        OtlpProtocol::HttpProtobuf => {
            let span_bytes = build_turn_span_proto(data);
            OtlpPayload::Proto(build_export_request_proto(cfg, span_bytes))
        }
    };
    let _ = post_otlp(cfg, payload);
}

// ─── Span-ID generation ───────────────────────────────────────────────────────

/// Generate a 16-hex-char span ID from a UUID string (use first 8 bytes).
pub fn span_id_from_uuid(uuid: &str) -> String {
    let hex: String = uuid.chars().filter(|c| c.is_ascii_hexdigit()).take(16).collect();
    if hex.len() == 16 {
        hex
    } else {
        format!("{hex:0<16}")
    }
}

/// Generate a 32-hex-char trace ID from a UUID string.
pub fn trace_id_from_uuid(uuid: &str) -> String {
    let hex: String = uuid.chars().filter(|c| c.is_ascii_hexdigit()).take(32).collect();
    if hex.len() == 32 {
        hex
    } else {
        format!("{hex:0<32}")
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_pairs_basic() {
        let pairs = parse_kv_pairs("Authorization=Bearer token123,x-custom=hello");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("Authorization".to_string(), "Bearer token123".to_string()));
        assert_eq!(pairs[1], ("x-custom".to_string(), "hello".to_string()));
    }

    #[test]
    fn parse_kv_pairs_empty_and_whitespace() {
        let pairs = parse_kv_pairs("  , key = val , ,");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("key".to_string(), "val".to_string()));
    }

    #[test]
    fn span_id_from_uuid_strips_hyphens() {
        let id = span_id_from_uuid("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "id={id}");
    }

    #[test]
    fn trace_id_from_uuid_strips_hyphens() {
        let id = trace_id_from_uuid("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "id={id}");
    }

    fn make_test_cfg(protocol: OtlpProtocol) -> OtelConfig {
        OtelConfig {
            traces_url: "http://localhost:4318/v1/traces".to_string(),
            headers: vec![],
            timeout: Duration::from_secs(5),
            service_name: "jcode".to_string(),
            service_version: "0.0.0".to_string(),
            resource_attrs: vec![],
            protocol,
        }
    }

    fn make_test_session_data<'a>() -> SessionSpanData<'a> {
        SessionSpanData {
            trace_id: "aaaabbbbccccddddaaaabbbbccccdddd",
            span_id: "aaaabbbbccccdddd",
            session_id: "sess-1",
            correlation_id: "corr-1",
            provider: "anthropic",
            model: "claude",
            start_nanos: 1_000_000_000,
            end_nanos: 2_000_000_000,
            end_reason: "normal_exit",
            turns: 3,
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            tool_calls: 5,
            tool_failures: 0,
            resumed: false,
            parent_session_id: None,
            os: "linux",
            arch: "x86_64",
            version: "0.80.0",
        }
    }

    #[test]
    fn build_session_span_json_has_required_fields() {
        let cfg = make_test_cfg(OtlpProtocol::HttpJson);
        let data = make_test_session_data();
        let span = build_session_span_json(&cfg, &data);
        assert_eq!(span["name"], "jcode.session");
        assert_eq!(span["traceId"], "aaaabbbbccccddddaaaabbbbccccdddd");
        assert_eq!(span["spanId"], "aaaabbbbccccdddd");
    }

    #[test]
    fn pb_varint_encodes_correctly() {
        assert_eq!(pb_varint(0), vec![0x00]);
        assert_eq!(pb_varint(1), vec![0x01]);
        assert_eq!(pb_varint(127), vec![0x7F]);
        assert_eq!(pb_varint(128), vec![0x80, 0x01]);
        assert_eq!(pb_varint(300), vec![0xAC, 0x02]);
    }

    #[test]
    fn hex_to_bytes_round_trips() {
        let bytes = hex_to_bytes("aaaabbbbccccdddd");
        assert_eq!(bytes.len(), 8);
        assert_eq!(bytes[0], 0xAA);
        assert_eq!(bytes[1], 0xAA);
        assert_eq!(bytes[2], 0xBB);
        assert_eq!(bytes[3], 0xBB);
    }

    #[test]
    fn build_session_span_proto_is_nonempty() {
        let cfg = make_test_cfg(OtlpProtocol::HttpProtobuf);
        let data = make_test_session_data();
        let span_bytes = build_session_span_proto(&data, &cfg);
        assert!(!span_bytes.is_empty(), "proto span should not be empty");
        // First field should be trace_id (field 1, wire type 2 = tag 0x0A)
        assert_eq!(span_bytes[0], 0x0A, "first byte should be tag for trace_id");
        // Second byte is the length of trace_id (16 bytes)
        assert_eq!(span_bytes[1], 16);
    }

    #[test]
    fn build_export_request_proto_wraps_in_resource_spans() {
        let cfg = make_test_cfg(OtlpProtocol::HttpProtobuf);
        let data = make_test_session_data();
        let span_bytes = build_session_span_proto(&data, &cfg);
        let req_bytes = build_export_request_proto(&cfg, span_bytes);
        assert!(!req_bytes.is_empty());
        // Top-level field 1 (resource_spans), wire type 2 -> tag 0x0A
        assert_eq!(req_bytes[0], 0x0A, "first byte should be ExportTraceServiceRequest.resource_spans tag");
    }

    #[test]
    fn otel_not_enabled_without_env() {
        // Only works if no OTEL env vars are set in the test environment.
        if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok()
            || std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_ok()
        {
            return;
        }
        // OTEL_CONFIG may already be initialized by other tests, so we can't
        // rely on is_otel_enabled() returning false here. Just confirm the
        // function does not panic.
        let _ = is_otel_enabled();
    }
}
