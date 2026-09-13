//! Low-level protobuf wire codecs, bounded google.protobuf.Value encoders/decoders,
//! tool schema serialization, and server-native execution rejection encoders for Cursor.
//!
//! Conforms to OpenCodex `agent_pb.ts` and `native-exec.ts` specifications.

use anyhow::{Context, Result, bail};
use jcode_message_types::ToolDefinition;
use serde_json::{Map, Value};

/// Maximum recursion depth allowed during google.protobuf.Value encoding and decoding.
pub const MAX_VALUE_DEPTH: usize = 32;

/// Maximum payload size allowed for google.protobuf.Value bytes (10 MiB).
pub const MAX_VALUE_BYTES: usize = 10 * 1024 * 1024;

/// Canonical provider identifier for jcode tools advertised to Cursor.
pub const JCODE_TOOL_PROVIDER: &str = "jcode";

// --------------------------------------------------------------------------
// Protobuf Wire Primitives
// --------------------------------------------------------------------------

/// Encode an unsigned 64-bit integer into variable-length protobuf varint bytes.
pub fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Encode a length-delimited protobuf field (wire type 2).
pub fn field_ld(field: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    encode_varint((field << 3) | 2, &mut out);
    encode_varint(data.len() as u64, &mut out);
    out.extend_from_slice(data);
    out
}

/// Encode a varint protobuf field (wire type 0).
pub fn field_varint(field: u64, value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    encode_varint(field << 3, &mut out);
    encode_varint(value, &mut out);
    out
}

/// Encode a UTF-8 string field (wire type 2).
pub fn field_str(field: u64, s: &str) -> Vec<u8> {
    field_ld(field, s.as_bytes())
}

/// Encode a 64-bit fixed float/double field (wire type 1).
pub fn field_fixed64(field: u64, value: f64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    encode_varint((field << 3) | 1, &mut out);
    out.extend_from_slice(&value.to_le_bytes());
    out
}

/// Encode a boolean field (wire type 0).
pub fn field_bool(field: u64, value: bool) -> Vec<u8> {
    field_varint(field, if value { 1 } else { 0 })
}

/// Read a varint from a byte slice.
pub fn read_varint(buf: &[u8]) -> Option<(u64, &[u8])> {
    let mut result = 0u64;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, &buf[i + 1..]));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// A parsed protobuf field header and data slice.
#[derive(Debug, Clone, Copy)]
pub struct PbWireField<'a> {
    pub field: u64,
    pub wire: u8,
    pub varint: u64,
    pub data: &'a [u8],
}

/// Iterate over top-level protobuf fields in a byte buffer.
pub fn iter_fields(mut buf: &[u8]) -> impl Iterator<Item = PbWireField<'_>> {
    std::iter::from_fn(move || {
        if buf.is_empty() {
            return None;
        }
        let (tag, rest) = read_varint(buf)?;
        let field = tag >> 3;
        let wire = (tag & 7) as u8;
        buf = rest;
        match wire {
            0 => {
                let (val, rest) = read_varint(buf)?;
                buf = rest;
                Some(PbWireField {
                    field,
                    wire,
                    varint: val,
                    data: &[],
                })
            }
            1 => {
                if buf.len() < 8 {
                    return None;
                }
                let data = &buf[..8];
                buf = &buf[8..];
                Some(PbWireField {
                    field,
                    wire,
                    varint: 0,
                    data,
                })
            }
            2 => {
                let (len, rest) = read_varint(buf)?;
                let len = len as usize;
                if rest.len() < len {
                    return None;
                }
                let data = &rest[..len];
                buf = &rest[len..];
                Some(PbWireField {
                    field,
                    wire,
                    varint: 0,
                    data,
                })
            }
            5 => {
                if buf.len() < 4 {
                    return None;
                }
                let data = &buf[..4];
                buf = &buf[4..];
                Some(PbWireField {
                    field,
                    wire,
                    varint: 0,
                    data,
                })
            }
            _ => None,
        }
    })
}

// --------------------------------------------------------------------------
// google.protobuf.Value Encoding & Decoding
// --------------------------------------------------------------------------

/// Encode a JSON value into `google.protobuf.Value` binary wire bytes with recursion depth check.
pub fn encode_google_protobuf_value(val: &Value, depth: usize) -> Result<Vec<u8>> {
    if depth > MAX_VALUE_DEPTH {
        bail!("google.protobuf.Value recursion depth limit ({MAX_VALUE_DEPTH}) exceeded");
    }

    match val {
        Value::Null => Ok(field_varint(1, 0)),
        Value::Number(num) => {
            let f = num.as_f64().unwrap_or(0.0);
            Ok(field_fixed64(2, f))
        }
        Value::String(s) => Ok(field_str(3, s)),
        Value::Bool(b) => Ok(field_bool(4, *b)),
        Value::Object(map) => {
            let mut struct_bytes = Vec::new();
            for (k, v) in map {
                let v_bytes = encode_google_protobuf_value(v, depth + 1)?;
                let mut entry = field_str(1, k);
                entry.extend(field_ld(2, &v_bytes));
                struct_bytes.extend(field_ld(1, &entry));
            }
            Ok(field_ld(5, &struct_bytes))
        }
        Value::Array(arr) => {
            let mut list_bytes = Vec::new();
            for item in arr {
                let item_bytes = encode_google_protobuf_value(item, depth + 1)?;
                list_bytes.extend(field_ld(1, &item_bytes));
            }
            Ok(field_ld(6, &list_bytes))
        }
    }
}

/// Decode `google.protobuf.Value` binary wire bytes into a JSON value with depth and size bounds.
pub fn decode_google_protobuf_value(bytes: &[u8], depth: usize) -> Result<Value> {
    if depth > MAX_VALUE_DEPTH {
        bail!("google.protobuf.Value decoding depth limit ({MAX_VALUE_DEPTH}) exceeded");
    }
    if bytes.len() > MAX_VALUE_BYTES {
        bail!("google.protobuf.Value byte size limit ({MAX_VALUE_BYTES}) exceeded");
    }

    for f in iter_fields(bytes) {
        match f.field {
            1 => return Ok(Value::Null),
            2 => {
                if f.data.len() >= 8 {
                    let num_val = f64::from_le_bytes(f.data[..8].try_into().unwrap());
                    if num_val.fract() == 0.0
                        && num_val >= (i64::MIN as f64)
                        && num_val <= (i64::MAX as f64)
                    {
                        return Ok(Value::Number(serde_json::Number::from(num_val as i64)));
                    }
                    if let Some(num) = serde_json::Number::from_f64(num_val) {
                        return Ok(Value::Number(num));
                    }
                    return Ok(Value::Number(serde_json::Number::from(0)));
                }
            }
            3 => {
                let s = std::str::from_utf8(f.data).context("Invalid UTF-8 in StringValue")?;
                return Ok(Value::String(s.to_string()));
            }
            4 => {
                return Ok(Value::Bool(f.varint != 0));
            }
            5 => {
                let mut map = Map::new();
                for entry_field in iter_fields(f.data) {
                    if entry_field.field == 1 && entry_field.wire == 2 {
                        let mut key = None;
                        let mut val = None;
                        for kv in iter_fields(entry_field.data) {
                            if kv.field == 1 && kv.wire == 2 {
                                key = std::str::from_utf8(kv.data).ok().map(|s| s.to_string());
                            } else if kv.field == 2 && kv.wire == 2 {
                                val = decode_google_protobuf_value(kv.data, depth + 1).ok();
                            }
                        }
                        if let (Some(k), Some(v)) = (key, val) {
                            if k != "__proto__" && k != "constructor" && k != "prototype" {
                                map.insert(k, v);
                            }
                        }
                    }
                }
                return Ok(Value::Object(map));
            }
            6 => {
                let mut arr = Vec::new();
                for item_field in iter_fields(f.data) {
                    if item_field.field == 1 && item_field.wire == 2 {
                        let item_val = decode_google_protobuf_value(item_field.data, depth + 1)?;
                        arr.push(item_val);
                    }
                }
                return Ok(Value::Array(arr));
            }
            _ => continue,
        }
    }

    bail!("Empty or unrecognized google.protobuf.Value payload");
}

// --------------------------------------------------------------------------
// McpToolDefinition & McpTools
// --------------------------------------------------------------------------

/// Derive standard wire name for an advertised tool (e.g. `mcp_jcode__bash`).
pub fn mcp_wire_name(tool_name: &str) -> String {
    if tool_name.starts_with("mcp_") {
        tool_name.to_string()
    } else {
        format!("mcp_{JCODE_TOOL_PROVIDER}__{tool_name}")
    }
}

/// Extract bare tool name from an advertised or inbound wire name.
pub fn mcp_bare_name(wire_name: &str) -> &str {
    if let Some(rest) = wire_name.strip_prefix(&format!("mcp_{JCODE_TOOL_PROVIDER}__")) {
        rest
    } else if let Some((_, rest)) = wire_name.split_once("__") {
        rest
    } else {
        wire_name
    }
}

/// Encode an `McpToolDefinition` message:
/// - field 1: name (string)
/// - field 2: description (string)
/// - field 3: input_schema (bytes, serialized google.protobuf.Value)
/// - field 4: provider_identifier (string, "jcode")
/// - field 5: tool_name (string, bare name)
pub fn encode_mcp_tool_definition(def: &ToolDefinition) -> Result<Vec<u8>> {
    let wire_name = mcp_wire_name(&def.name);
    let bare_name = mcp_bare_name(&def.name);
    let schema_bytes = encode_google_protobuf_value(&def.input_schema, 0)
        .context("Failed to encode tool input_schema to google.protobuf.Value")?;

    let mut out = field_str(1, &wire_name);
    out.extend(field_str(2, &def.description));
    out.extend(field_ld(3, &schema_bytes));
    out.extend(field_str(4, JCODE_TOOL_PROVIDER));
    out.extend(field_str(5, bare_name));
    Ok(out)
}

/// Encode an `McpTools` message containing repeated `mcp_tools` (field 1).
pub fn encode_mcp_tools(tools: &[ToolDefinition]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for tool in tools {
        let def_bytes = encode_mcp_tool_definition(tool)?;
        out.extend(field_ld(1, &def_bytes));
    }
    Ok(out)
}

// --------------------------------------------------------------------------
// McpArgs & Args Map Decoding
// --------------------------------------------------------------------------

/// Decode an inbound Cursor McpArgs `args` map (map<string, bytes>) into a JSON Object.
/// Falls back to UTF-8 JSON text decoding if canonical protobuf Value decoding fails.
pub fn decode_cursor_args_map(args_data: &[u8]) -> Result<Value> {
    let mut map = Map::new();

    for field in iter_fields(args_data) {
        if field.field == 2 && field.wire == 2 {
            // Entry in repeated map<string, bytes>
            let mut key = None;
            let mut val_bytes: Option<&[u8]> = None;
            for kv in iter_fields(field.data) {
                if kv.field == 1 && kv.wire == 2 {
                    key = std::str::from_utf8(kv.data).ok().map(|s| s.to_string());
                } else if kv.field == 2 && kv.wire == 2 {
                    val_bytes = Some(kv.data);
                }
            }

            if let (Some(k), Some(bytes)) = (key, val_bytes) {
                if k == "__proto__" || k == "constructor" || k == "prototype" {
                    continue;
                }
                // Try canonical protobuf Value first
                let decoded = match decode_google_protobuf_value(bytes, 0) {
                    Ok(val) => val,
                    Err(_) => {
                        // Fallback: UTF-8 JSON text parse or string
                        match std::str::from_utf8(bytes) {
                            Ok(text) => match serde_json::from_str::<Value>(text) {
                                Ok(json_val) => json_val,
                                Err(_) => Value::String(text.to_string()),
                            },
                            Err(_) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
                        }
                    }
                };
                map.insert(k, decoded);
            }
        }
    }

    Ok(Value::Object(map))
}

// --------------------------------------------------------------------------
// Inbound ExecServerMessage Types & Decoder
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ShellArgs {
    pub command: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteArgs {
    pub path: String,
    pub file_text: String,
    pub file_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteArgs {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: String,
    pub case_insensitive: bool,
    pub glob: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadArgs {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LsArgs {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticsArgs {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestContextArgs {}

#[derive(Debug, Clone, PartialEq)]
pub struct McpArgs {
    pub name: String,
    pub args: Value,
    pub tool_call_id: String,
    pub provider_identifier: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellStreamArgs {
    pub command: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundShellSpawnArgs {
    pub command: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListMcpResourcesExecArgs {}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadMcpResourceExecArgs {
    pub server: String,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchArgs {
    pub url: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordScreenArgs {}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputerUseArgs {}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteShellStdinArgs {
    pub shell_id: u32,
    pub stdin: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecServerMessageVariant {
    Shell(ShellArgs),
    Write(WriteArgs),
    Delete(DeleteArgs),
    Grep(GrepArgs),
    Read(ReadArgs),
    Ls(LsArgs),
    Diagnostics(DiagnosticsArgs),
    RequestContext(RequestContextArgs),
    Mcp(McpArgs),
    ShellStream(ShellStreamArgs),
    BackgroundShellSpawn(BackgroundShellSpawnArgs),
    ListMcpResources(ListMcpResourcesExecArgs),
    ReadMcpResource(ReadMcpResourceExecArgs),
    Fetch(FetchArgs),
    RecordScreen(RecordScreenArgs),
    ComputerUse(ComputerUseArgs),
    WriteShellStdin(WriteShellStdinArgs),
    Unknown(u64, Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecServerMessage {
    pub id: u32,
    pub exec_id: String,
    pub variant: ExecServerMessageVariant,
}

/// Decode an `ExecServerMessage` from protobuf wire bytes.
pub fn decode_exec_server_message(bytes: &[u8]) -> Result<ExecServerMessage> {
    let mut id = 0u32;
    let mut exec_id = String::new();
    let mut variant = None;

    for field in iter_fields(bytes) {
        match field.field {
            1 => id = field.varint as u32,
            15 => {
                if let Ok(s) = std::str::from_utf8(field.data) {
                    exec_id = s.to_string();
                }
            }
            19 => {
                // span_context: tracing metadata, ignored
            }
            2 => {
                // ShellArgs (command=1, working_directory=2)
                let mut cmd = String::new();
                let mut cwd = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cmd = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cwd = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Shell(ShellArgs {
                    command: cmd,
                    working_directory: cwd,
                }));
            }
            3 => {
                // WriteArgs (path=1, file_text=2, file_bytes=3)
                let mut path = String::new();
                let mut file_text = String::new();
                let mut file_bytes = Vec::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        file_text = s.to_string();
                    } else if f.field == 3 {
                        file_bytes = f.data.to_vec();
                    }
                }
                variant = Some(ExecServerMessageVariant::Write(WriteArgs {
                    path,
                    file_text,
                    file_bytes,
                }));
            }
            4 => {
                // DeleteArgs (path=1)
                let mut path = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Delete(DeleteArgs { path }));
            }
            5 => {
                // GrepArgs (pattern=1, path=2, case_insensitive=3, glob=4)
                let mut pattern = String::new();
                let mut path = String::new();
                let mut case_insensitive = false;
                let mut glob = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        pattern = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    } else if f.field == 3 {
                        case_insensitive = f.varint != 0;
                    } else if f.field == 4
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        glob = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Grep(GrepArgs {
                    pattern,
                    path,
                    case_insensitive,
                    glob,
                }));
            }
            7 => {
                // ReadArgs (path=1)
                let mut path = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Read(ReadArgs { path }));
            }
            8 => {
                // LsArgs (path=1)
                let mut path = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Ls(LsArgs { path }));
            }
            9 => {
                // DiagnosticsArgs (path=1)
                let mut path = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        path = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Diagnostics(DiagnosticsArgs {
                    path,
                }));
            }
            10 => {
                variant = Some(ExecServerMessageVariant::RequestContext(
                    RequestContextArgs {},
                ));
            }
            11 => {
                // McpArgs (name=1, args=2 map, tool_call_id=3, provider_identifier=4, tool_name=5)
                let mut name = String::new();
                let mut tool_call_id = String::new();
                let mut provider_identifier = String::new();
                let mut tool_name = String::new();
                let args = decode_cursor_args_map(field.data).unwrap_or(Value::Object(Map::new()));

                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        name = s.to_string();
                    } else if f.field == 3
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        tool_call_id = s.to_string();
                    } else if f.field == 4
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        provider_identifier = s.to_string();
                    } else if f.field == 5
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        tool_name = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Mcp(McpArgs {
                    name,
                    args,
                    tool_call_id,
                    provider_identifier,
                    tool_name,
                }));
            }
            14 => {
                let mut cmd = String::new();
                let mut cwd = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cmd = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cwd = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::ShellStream(ShellStreamArgs {
                    command: cmd,
                    working_directory: cwd,
                }));
            }
            16 => {
                let mut cmd = String::new();
                let mut cwd = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cmd = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        cwd = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::BackgroundShellSpawn(
                    BackgroundShellSpawnArgs {
                        command: cmd,
                        working_directory: cwd,
                    },
                ));
            }
            17 => {
                variant = Some(ExecServerMessageVariant::ListMcpResources(
                    ListMcpResourcesExecArgs {},
                ));
            }
            18 => {
                let mut server = String::new();
                let mut uri = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        server = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        uri = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::ReadMcpResource(
                    ReadMcpResourceExecArgs { server, uri },
                ));
            }
            20 => {
                let mut url = String::new();
                let mut tool_call_id = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        url = s.to_string();
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        tool_call_id = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::Fetch(FetchArgs {
                    url,
                    tool_call_id,
                }));
            }
            21 => variant = Some(ExecServerMessageVariant::RecordScreen(RecordScreenArgs {})),
            22 => variant = Some(ExecServerMessageVariant::ComputerUse(ComputerUseArgs {})),
            23 => {
                let mut shell_id = 0u32;
                let mut stdin = String::new();
                for f in iter_fields(field.data) {
                    if f.field == 1 {
                        shell_id = f.varint as u32;
                    } else if f.field == 2
                        && let Ok(s) = std::str::from_utf8(f.data)
                    {
                        stdin = s.to_string();
                    }
                }
                variant = Some(ExecServerMessageVariant::WriteShellStdin(
                    WriteShellStdinArgs { shell_id, stdin },
                ));
            }
            other => {
                if variant.is_none() {
                    variant = Some(ExecServerMessageVariant::Unknown(
                        other,
                        field.data.to_vec(),
                    ));
                }
            }
        }
    }

    let variant = variant.context("ExecServerMessage contained no execution oneof payload")?;
    Ok(ExecServerMessage {
        id,
        exec_id,
        variant,
    })
}

// --------------------------------------------------------------------------
// Outbound ExecClientMessage Codecs
// --------------------------------------------------------------------------

/// Wrap a raw response oneof into an `ExecClientMessage`:
/// - field 1: id (uint32)
/// - field 15: exec_id (string, if non-empty)
/// - field `response_field`: response_payload (wire type 2)
pub fn encode_exec_client_message(
    id: u32,
    exec_id: &str,
    response_field: u64,
    response_payload: &[u8],
) -> Vec<u8> {
    let mut out = field_varint(1, id as u64);
    if !exec_id.is_empty() {
        out.extend(field_str(15, exec_id));
    }
    out.extend(field_ld(response_field, response_payload));
    out
}

/// Wrap an `ExecClientMessage` into an `AgentClientMessage` (field 2).
pub fn encode_agent_client_exec_message(exec_client_bytes: &[u8]) -> Vec<u8> {
    field_ld(2, exec_client_bytes)
}

/// Wrap a stream close control message into an `AgentClientMessage` (field 5).
pub fn encode_agent_client_stream_close(id: u32) -> Vec<u8> {
    // ExecClientControlMessage: field 1 = stream_close (ExecClientStreamClose: id=1)
    let close_bytes = field_varint(1, id as u64);
    let control_bytes = field_ld(1, &close_bytes);
    field_ld(5, &control_bytes)
}

pub fn encode_request_context_result(
    id: u32,
    exec_id: &str,
    request_context_bytes: &[u8],
) -> Vec<u8> {
    let request_context_success = field_ld(1, request_context_bytes);
    let result = field_ld(1, &request_context_success);
    encode_exec_client_message(id, exec_id, 10, &result)
}

pub fn encode_request_context(
    system_prompt: &str,
    tools: &[ToolDefinition],
    cwd: &str,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if !system_prompt.trim().is_empty() {
        let mut rule = field_str(1, "/jcode/system-prompt/0.mdc");
        rule.extend(field_str(2, system_prompt.trim()));
        rule.extend(field_varint(3, 1));
        let rule_type = field_ld(1, &[]);
        rule.extend(field_ld(4, &rule_type));
        out.extend(field_ld(2, &rule));
    }
    let mut env = field_str(1, "linux");
    env.extend(field_str(2, cwd));
    env.extend(field_str(3, "bash"));
    env.extend(field_str(10, "UTC"));
    out.extend(field_ld(4, &env));
    for tool in tools {
        let def_bytes = encode_mcp_tool_definition(tool)?;
        out.extend(field_ld(7, &def_bytes));
    }
    Ok(out)
}

/// Extract cumulative used tokens from a `conversationCheckpointUpdate` payload (field 3 of AgentServerMessage).
/// Path: f3 (conversationCheckpointUpdate) -> f5 (tokenDetails) -> f1 (usedTokens varint).
pub fn extract_checkpoint_used_tokens(payload: &[u8]) -> Option<u64> {
    for f3 in iter_fields(payload) {
        if f3.field == 3 && f3.wire == 2 {
            for f5 in iter_fields(f3.data) {
                if f5.field == 5 && f5.wire == 2 {
                    for f1 in iter_fields(f5.data) {
                        if f1.field == 1 && f1.wire == 0 {
                            return Some(f1.varint);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Wrap an uncompressed payload in a Connect data frame (flag 0).
pub fn connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

// --------------------------------------------------------------------------
// Policy Guidance Messages
// --------------------------------------------------------------------------

pub fn native_local_exec_disabled_message(code_mode: bool) -> &'static str {
    if code_mode {
        r#"Cursor-native local tools are policy-redirected to the jcode bridge for this request; this is not a permissions denial. Use the top-level `exec` tool and call nested helpers inside its JavaScript body as `await tools.<name>(args)` (for example `await tools.exec_command({cmd: "ls"})`, `await tools.apply_patch(input)`, or another helper listed in `ALL_TOOLS`). Do not call `shell_command` or `exec_command` at the top level in code mode."#
    } else {
        "Re-issue this operation NOW through an authorized jcode tool bridge. Do NOT narrate this redirect, comment on tool availability, or re-announce the task — just make the bridge call."
    }
}

pub fn native_shell_disabled_message(code_mode: bool) -> &'static str {
    if code_mode {
        r#"Cursor-native shell is policy-redirected to the jcode bridge for this request; this is not a permissions denial. Use the top-level `exec` tool and call a nested shell helper inside its JavaScript body as `await tools.exec_command({cmd: "..."})` (or another helper listed in `ALL_TOOLS`). Do not call `shell_command` or `exec_command` at the top level in code mode."#
    } else {
        "Re-issue this command NOW through an authorized jcode shell tool bridge. Do NOT narrate this redirect, comment on tool availability, or re-announce the task — just make the bridge call."
    }
}

pub fn native_fetch_disabled_message(code_mode: bool) -> &'static str {
    if code_mode {
        r#"Cursor-native fetch is policy-redirected to the jcode bridge for this request; this is not a permissions denial. Use the top-level `exec` tool and call a nested shell helper inside its JavaScript body as `await tools.exec_command({cmd: "curl ..."})` (or another helper listed in `ALL_TOOLS`). Do not call `shell_command` or `exec_command` at the top level in code mode."#
    } else {
        "Re-issue this fetch NOW through an authorized jcode network tool bridge. Do NOT narrate this redirect or comment on tool availability — just make the bridge call."
    }
}

// --------------------------------------------------------------------------
// Rejection Encoders for Server-Native Exec Variants (CURS-03)
// --------------------------------------------------------------------------

/// Reject `shellArgs` -> returns `shell_result` (field 2) containing `ShellFailure` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_shell_exec(
    id: u32,
    exec_id: &str,
    command: &str,
    cwd: &str,
    reason: &str,
) -> Vec<u8> {
    // ShellFailure: command=1, working_directory=2, exit_code=3 (1), signal=4 (""), stderr=6 (reason), execution_time=7 (0), aborted=11 (true)
    let mut failure = field_str(1, command);
    failure.extend(field_str(2, cwd));
    failure.extend(field_varint(3, 1));
    failure.extend(field_str(4, ""));
    failure.extend(field_str(6, reason));
    failure.extend(field_varint(7, 0));
    failure.extend(field_bool(11, true));

    // ShellResult: failure = 2
    let shell_result = field_ld(2, &failure);
    encode_exec_client_message(id, exec_id, 2, &shell_result)
}

/// Reject `writeArgs` -> returns `write_result` (field 3) containing `WriteRejected` (case 6).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_write_exec(id: u32, exec_id: &str, path: &str, reason: &str) -> Vec<u8> {
    // WriteRejected: path=1, reason=2
    let mut rejected = field_str(1, path);
    rejected.extend(field_str(2, reason));

    // WriteResult: rejected = 6
    let write_result = field_ld(6, &rejected);
    encode_exec_client_message(id, exec_id, 3, &write_result)
}

/// Reject `deleteArgs` -> returns `delete_result` (field 4) containing `DeleteRejected` (case 6).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_delete_exec(id: u32, exec_id: &str, path: &str, reason: &str) -> Vec<u8> {
    // DeleteRejected: path=1, reason=2
    let mut rejected = field_str(1, path);
    rejected.extend(field_str(2, reason));

    // DeleteResult: rejected = 6
    let delete_result = field_ld(6, &rejected);
    encode_exec_client_message(id, exec_id, 4, &delete_result)
}

/// Reject `grepArgs` -> returns `grep_result` (field 5) containing `GrepError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_grep_exec(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // GrepError: error=1
    let grep_err = field_str(1, error);
    // GrepResult: error = 2
    let grep_result = field_ld(2, &grep_err);
    encode_exec_client_message(id, exec_id, 5, &grep_result)
}

/// Reject `readArgs` -> returns `read_result` (field 7) containing `ReadError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_read_exec(id: u32, exec_id: &str, path: &str, error: &str) -> Vec<u8> {
    // ReadError: path=1, error=2
    let mut read_err = field_str(1, path);
    read_err.extend(field_str(2, error));
    // ReadResult: error = 2
    let read_result = field_ld(2, &read_err);
    encode_exec_client_message(id, exec_id, 7, &read_result)
}

/// Reject `lsArgs` -> returns `ls_result` (field 8) containing `LsError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_ls_exec(id: u32, exec_id: &str, path: &str, error: &str) -> Vec<u8> {
    // LsError: path=1, error=2
    let mut ls_err = field_str(1, path);
    ls_err.extend(field_str(2, error));
    // LsResult: error = 2
    let ls_result = field_ld(2, &ls_err);
    encode_exec_client_message(id, exec_id, 8, &ls_result)
}

/// Reject `diagnosticsArgs` -> returns `diagnostics_result` (field 9) containing `DiagnosticsError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_diagnostics_exec(id: u32, exec_id: &str, path: &str, error: &str) -> Vec<u8> {
    // DiagnosticsError: path=1, error=2
    let mut diag_err = field_str(1, path);
    diag_err.extend(field_str(2, error));
    // DiagnosticsResult: error = 2
    let diag_result = field_ld(2, &diag_err);
    encode_exec_client_message(id, exec_id, 9, &diag_result)
}

/// Reject `shellStreamArgs` -> returns `shell_stream` (field 14) containing `ShellStreamStderr` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_shell_stream_exec(id: u32, exec_id: &str, stderr: &str) -> Vec<u8> {
    // ShellStreamStderr: data=1
    let stderr_item = field_str(1, stderr);
    // ShellStream: stderr = 2
    let stream_result = field_ld(2, &stderr_item);
    encode_exec_client_message(id, exec_id, 14, &stream_result)
}

/// Reject `backgroundShellSpawnArgs` -> returns `background_shell_spawn_result` (field 16) containing `BackgroundShellSpawnError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_background_shell_spawn_exec(
    id: u32,
    exec_id: &str,
    command: &str,
    cwd: &str,
    error: &str,
) -> Vec<u8> {
    // BackgroundShellSpawnError: command=1, working_directory=2, error=3
    let mut err = field_str(1, command);
    err.extend(field_str(2, cwd));
    err.extend(field_str(3, error));
    // BackgroundShellSpawnResult: error = 2
    let bg_result = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 16, &bg_result)
}

/// Reject `listMcpResourcesExecArgs` -> returns `list_mcp_resources_exec_result` (field 17) containing `ListMcpResourcesError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_list_mcp_resources_exec(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // ListMcpResourcesError: error=1
    let err = field_str(1, error);
    // ListMcpResourcesExecResult: error = 2
    let res = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 17, &res)
}

/// Reject `readMcpResourceExecArgs` -> returns `read_mcp_resource_exec_result` (field 18) containing `ReadMcpResourceError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_read_mcp_resource_exec(id: u32, exec_id: &str, uri: &str, error: &str) -> Vec<u8> {
    // ReadMcpResourceError: uri=1, error=2
    let mut err = field_str(1, uri);
    err.extend(field_str(2, error));
    // ReadMcpResourceExecResult: error = 2
    let res = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 18, &res)
}

/// Reject `fetchArgs` -> returns `fetch_result` (field 20) containing `FetchError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_fetch_exec(id: u32, exec_id: &str, url: &str, error: &str) -> Vec<u8> {
    // FetchError: url=1, error=2
    let mut err = field_str(1, url);
    err.extend(field_str(2, error));
    // FetchResult: error = 2
    let res = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 20, &res)
}

/// Reject `recordScreenArgs` -> returns `record_screen_result` (field 21) containing `RecordScreenFailure` (case 4).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_record_screen_exec(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // RecordScreenFailure: error=1
    let err = field_str(1, error);
    // RecordScreenResult: failure = 4
    let res = field_ld(4, &err);
    encode_exec_client_message(id, exec_id, 21, &res)
}

/// Reject `computerUseArgs` -> returns `computer_use_result` (field 22) containing `ComputerUseError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_computer_use_exec(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // ComputerUseError: error=1, action_count=2, duration_ms=3
    let mut err = field_str(1, error);
    err.extend(field_varint(2, 0));
    err.extend(field_varint(3, 0));
    // ComputerUseResult: error = 2
    let res = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 22, &res)
}

/// Reject `writeShellStdinArgs` -> returns `write_shell_stdin_result` (field 23) containing `WriteShellStdinError` (case 2).
/// NON-MCP VARIANT: Never returns mcp_result!
pub fn reject_write_shell_stdin_exec(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // WriteShellStdinError: error=1
    let err = field_str(1, error);
    // WriteShellStdinResult: error = 2
    let res = field_ld(2, &err);
    encode_exec_client_message(id, exec_id, 23, &res)
}

// --------------------------------------------------------------------------
// McpResult Encoders (Field 11)
// --------------------------------------------------------------------------

/// Encode a successful McpResult with text output:
/// McpResult -> success (case 1) -> McpSuccess:
/// - content (repeated field 1): McpToolResultContentItem: text (case 1) -> McpTextContent: text=1
/// - is_error (field 2): bool
pub fn encode_mcp_success_result(id: u32, exec_id: &str, text: &str, is_error: bool) -> Vec<u8> {
    // McpTextContent: text = 1
    let text_content = field_str(1, text);
    // McpToolResultContentItem: text = 1
    let content_item = field_ld(1, &text_content);

    // McpSuccess: content = 1 (repeated), is_error = 2
    let mut success = field_ld(1, &content_item);
    success.extend(field_bool(2, is_error));

    // McpResult: success = 1
    let mcp_result = field_ld(1, &success);
    encode_exec_client_message(id, exec_id, 11, &mcp_result)
}

/// Encode an error McpResult:
/// McpResult -> error (case 2) -> McpError: error=1
pub fn encode_mcp_error_result(id: u32, exec_id: &str, error: &str) -> Vec<u8> {
    // McpError: error = 1
    let mcp_err = field_str(1, error);
    // McpResult: error = 2
    let mcp_result = field_ld(2, &mcp_err);
    encode_exec_client_message(id, exec_id, 11, &mcp_result)
}

/// Encode a toolNotFound McpResult:
/// McpResult -> tool_not_found (case 5) -> McpToolNotFound: name=1, available_tools=2 (repeated)
pub fn encode_mcp_not_found_result(
    id: u32,
    exec_id: &str,
    tool_name: &str,
    available_tools: &[String],
) -> Vec<u8> {
    let mut not_found = field_str(1, tool_name);
    for t in available_tools {
        not_found.extend(field_str(2, t));
    }
    // McpResult: tool_not_found = 5
    let mcp_result = field_ld(5, &not_found);
    encode_exec_client_message(id, exec_id, 11, &mcp_result)
}
