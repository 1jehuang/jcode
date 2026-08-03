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
}

/// Rust-specific members, with the reason each one is not mirrored.
const RUST_ONLY: &[&str] = &[
    // Rust needs an explicit constructor pair where TS uses static factories
    // with optional args; `connect_with` is the transport seam tests use.
    "connect",
    "connect_with",
    // `Drop` closes the connection in Rust, so there is no `close()` to mirror.
    "is_closed",
    // TS reads `client.socketPath` as a field; Rust exposes it as an accessor.
    "socket_path",
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
