//! Parity between the Rust and TypeScript SDKs.
//!
//! Second-order dogfooding only works if the two SDKs stay the same shape. If
//! the Rust one drifts into "whatever desktop2 happened to need", desktop2
//! stops telling us anything about the TypeScript one and we are back to
//! validating the TS SDK with examples written to make it look good.
//!
//! So the capability list below is the contract, and both SDKs are checked
//! against it: a method added to one and not the other fails here. Naming is
//! the only translation (`snake_case` in Rust, `camelCase` in TS); semantics
//! and arity are expected to match.
//!
//! What is deliberately *not* checked: error handling, streaming style, and
//! launch strategy. `Result` versus `throw`, channels versus `EventEmitter`,
//! and "attach to the user's jcode" versus "provision a private instance" are
//! places where forcing symmetry would produce an un-idiomatic SDK in one
//! language to flatter the other.

/// One capability, named in both SDKs' conventions.
struct Capability {
    /// Rust method on `JcodeClient`.
    rust: &'static str,
    /// TypeScript method on `JcodeClient`.
    ts: &'static str,
}

/// The shared surface. Adding a capability means adding it here first.
const CAPABILITIES: &[Capability] = &[
    cap("connect", "connect"),
    cap("list_sessions", "listSessions"),
    cap("create_session", "createSession"),
    cap("attach_session", "attachSession"),
    cap("detach_session", "detachSession"),
    cap("send_message", "sendMessage"),
    cap("cancel", "cancel"),
    cap("soft_interrupt", "softInterrupt"),
    cap("get_history", "getHistory"),
    cap("peek_session", "peekSession"),
    cap("clear", "clear"),
    cap("rewind", "rewind"),
    cap("rewind_undo", "rewindUndo"),
    cap("respond_to_permission", "respondToPermission"),
    cap("list_models", "listModels"),
    cap("set_model", "setModel"),
    cap("set_reasoning_effort", "setReasoningEffort"),
    cap("compact", "compact"),
    cap("rename_session", "renameSession"),
    cap("cancel_soft_interrupts", "cancelSoftInterrupts"),
    cap("ping", "ping"),
    cap("run", "run"),
    cap("events", "events"),
    cap("request", "request"),
    cap("notify", "notify"),
    cap("supports", "supports"),
];

const fn cap(rust: &'static str, ts: &'static str) -> Capability {
    Capability { rust, ts }
}

/// Every shared capability exists in the Rust SDK.
#[test]
fn the_rust_sdk_implements_every_shared_capability() {
    let source = rust_client_source();
    let missing: Vec<&str> = CAPABILITIES
        .iter()
        .map(|c| c.rust)
        .filter(|name| !source.contains(&format!("pub fn {name}(")))
        .collect();
    assert!(
        missing.is_empty(),
        "the shared SDK surface names capabilities the Rust SDK does not have: \
         {missing:?}. Implement them in client.rs, or remove them from \
         CAPABILITIES if the capability is being dropped from both SDKs."
    );
}

/// Every shared capability exists in the TypeScript SDK.
#[test]
fn the_typescript_sdk_implements_every_shared_capability() {
    let Some(source) = ts_client_source() else {
        // Vendored builds without the sdk/ tree: nothing to compare against.
        return;
    };
    let missing: Vec<&str> = CAPABILITIES
        .iter()
        .map(|c| c.ts)
        .filter(|name| {
            // Methods are declared as `async foo(`, `foo(`, or `static foo(`.
            !source.contains(&format!("{name}("))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "the shared SDK surface names capabilities the TypeScript SDK does not \
         have: {missing:?}. A capability that exists only in Rust means \
         desktop2 is exercising a design the shipped SDK does not have, which \
         is the drift this test exists to prevent."
    );
}

/// Neither SDK has a public capability that is missing from the shared list.
///
/// The direction that actually rots: someone adds a method to the Rust SDK for
/// desktop2, never touches the TS SDK, and the lists silently diverge. Failing
/// here forces the decision to be made rather than deferred.
#[test]
fn neither_sdk_has_an_untriaged_public_capability() {
    let rust = rust_public_methods();
    let known: std::collections::BTreeSet<&str> = CAPABILITIES.iter().map(|c| c.rust).collect();
    let untriaged: Vec<&String> = rust
        .iter()
        .filter(|name| !known.contains(name.as_str()))
        .filter(|name| !RUST_ONLY.contains(&name.as_str()))
        .collect();
    assert!(
        untriaged.is_empty(),
        "these public JcodeClient methods are in the Rust SDK but not in the \
         shared capability list: {untriaged:?}. Add each to CAPABILITIES with \
         its TypeScript counterpart, or to RUST_ONLY with a comment saying why \
         it is Rust-specific."
    );

    let Some(ts) = ts_public_methods() else {
        return;
    };
    let known: std::collections::BTreeSet<&str> = CAPABILITIES.iter().map(|c| c.ts).collect();
    let untriaged: Vec<&String> = ts
        .iter()
        .filter(|name| !known.contains(name.as_str()))
        .filter(|name| !TS_ONLY.iter().any(|(allowed, _)| allowed == &name.as_str()))
        .collect();
    assert!(
        untriaged.is_empty(),
        "these public JcodeClient methods are in the TypeScript SDK but not in the \
         shared capability list: {untriaged:?}. Add each to CAPABILITIES and the \
         Rust SDK, or to TS_ONLY with a reason when it is genuinely language-specific."
    );

    if !TS_ONLY.is_empty() {
        eprintln!(
            "warning: SDK parity has {} explicitly triaged TypeScript-only methods: {}",
            TS_ONLY.len(),
            TS_ONLY
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Rust-specific members, with the reason each one is not mirrored.
const RUST_ONLY: &[&str] = &[
    // `connect_with` is the explicit transport seam Rust tests use; TypeScript
    // accepts its transport through the options passed to `connect`.
    "connect_with",
    // `Drop` closes the connection in Rust, so there is no `close()` to mirror.
    "is_closed",
    // TS reads `client.socketPath` as a field; Rust exposes it as an accessor.
    "socket_path",
];

/// Public TypeScript methods not yet represented by an equivalent Rust method.
///
/// This is intentionally noisy technical debt, not a second capability list.
/// New TS methods fail the parity test unless they are implemented in Rust or
/// added here with a reviewable reason. Removing entries is the parity roadmap.
const TS_ONLY: &[(&str, &str)] = &[
    (
        "launch",
        "TS provisions an isolated child instance; Rust currently only ensures the user runtime",
    ),
    ("close", "Rust closes through Drop"),
    (
        "archiveSession",
        "Rust SDK does not yet expose session archival",
    ),
    (
        "restoreSession",
        "Rust SDK does not yet expose session restoration",
    ),
    (
        "setRetentionPolicy",
        "Rust SDK does not yet expose retention policy",
    ),
    (
        "getRuntimeInfo",
        "Rust SDK does not yet expose runtime introspection",
    ),
    (
        "setApiKey",
        "Rust SDK does not yet expose credential provisioning",
    ),
    (
        "clearApiKey",
        "Rust SDK does not yet expose credential removal",
    ),
    (
        "readFile",
        "Rust SDK does not yet expose session-rooted file reads",
    ),
    (
        "findFiles",
        "Rust SDK does not yet expose session-rooted file discovery",
    ),
    (
        "searchText",
        "Rust SDK does not yet expose session-rooted text search",
    ),
    (
        "fileStatus",
        "Rust SDK does not yet expose session-rooted file status",
    ),
    (
        "globalEvents",
        "Rust has an all-session event filter but not TS reconnecting global events",
    ),
    (
        "runStructured",
        "Rust SDK does not yet expose schema-validated structured output",
    ),
];

fn rust_client_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/client.rs");
    std::fs::read_to_string(path).expect("the Rust SDK client must be readable")
}

fn ts_client_source() -> Option<String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/typescript/src/client.ts");
    std::fs::read_to_string(path).ok()
}

/// Public method names in `impl JcodeClient`.
fn rust_public_methods() -> Vec<String> {
    let source = rust_client_source();
    let start = source
        .find("impl JcodeClient {")
        .expect("client.rs must have an `impl JcodeClient` block");
    // The impl block ends at the first column-zero closing brace after it.
    let body = &source[start..];
    let end = body.find("\n}").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub fn ")?;
            Some(
                rest.chars()
                    .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                    .collect(),
            )
        })
        .collect()
}

/// Public method names in the TypeScript `JcodeClient` class.
///
/// Methods are two-space-indented in this file. Private helpers are explicitly
/// excluded and overloads are deduplicated. Keeping this small parser here makes
/// the guard run under ordinary `cargo test`, without requiring Node or a TS AST.
fn ts_public_methods() -> Option<Vec<String>> {
    let source = ts_client_source()?;
    let start = source.find("export class JcodeClient")?;
    let mut methods = std::collections::BTreeSet::new();
    for line in source[start..].lines() {
        let Some(mut declaration) = line.strip_prefix("  ") else {
            continue;
        };
        if declaration.starts_with(' ') || declaration.starts_with("private ") {
            continue;
        }
        declaration = declaration.strip_prefix("static ").unwrap_or(declaration);
        declaration = declaration.strip_prefix("async ").unwrap_or(declaration);
        let Some(open) = declaration.find('(') else {
            continue;
        };
        let name = &declaration[..open];
        if !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        {
            methods.insert(name.to_string());
        }
    }
    Some(methods.into_iter().collect())
}
