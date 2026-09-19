# External provider extension protocol

This repository contains a small, versioned protocol for providers that should
be developed and upgraded independently of the jcode binary. It is intentionally
JSON Lines over stdin/stdout. A provider is a child process, not a Rust dynamic
library, so upgrades do not depend on an unstable Rust ABI.

## Contract

The current wire version is `0.1`, exposed as `jcode_provider_protocol::PROTOCOL_VERSION`.
Each line is one JSON frame and every frame includes `protocol_version`.

1. The client sends `hello` with its identity, supported capabilities, and a
   maximum accepted frame size.
2. The provider responds with `hello_ok`, its identity, and the capabilities it
   supports. The client uses the intersection, in client order.
3. The client sends `request` frames with a unique request ID.
4. The provider may send `event` frames for that request. Event payloads are
   opaque JSON so provider-specific streaming details can evolve without
   changing the transport. The `native_tool_call` and `tool_result` events must
   carry stable call IDs in their payloads.
5. The provider ends the request with a matching `response` frame. Errors are
   structured as `{ code, message, retryable, details }`.
6. The client can send `cancel` with the request ID. A provider should stop work
   and return a cancelled response when it supports the `cancellation`
   capability.

Known capabilities are `streaming`, `native_tools`, and `cancellation`.
Unknown capabilities are ignored for forward compatibility.

## Safety requirements

- Keep request IDs unique for the lifetime of a provider process.
- Enforce the negotiated maximum frame size. The reference adapter defaults to
  16 MiB and bounds reads before allocating an unbounded line.
- Use a handshake deadline and a per-request deadline. The adapter defaults to
  10 seconds for the handshake and 120 seconds for a request.
- Treat a missing newline, malformed JSON, unsupported version, mismatched ID,
  process exit, and timeout as a failed request. Do not silently reuse a
  partially read stream.
- Kill the child when the adapter is dropped or a session is cancelled by the
  application.
- Never put secrets in frame payloads or logs unless the provider contract
  explicitly requires them.

## Reference adapter

`jcode-provider-subprocess::SubprocessProvider` launches a provider, sends the
handshake, reads bounded frames, collects a request stream, sends cancellation,
and cleans up the process. External providers should depend on the published
`jcode-provider-protocol` crate. The subprocess adapter is a workspace reference
implementation and is not packaged as a standalone jcode release until the
protocol crate has been published at the matching version.

The adapter's tests exercise handshake negotiation, streaming events, native tool
correlation, cancellation frames, malformed/oversized input boundaries, request
timeouts, and process cleanup. Providers should add a fixture test that runs
this same exchange against their executable.

## Compatibility policy

Patch releases may add optional fields and capabilities. A provider must ignore
unknown fields and capabilities. A new incompatible wire shape requires a new
protocol version and a separate adapter path. The jcode application version and
`external_provider_protocol_version` reported by `jcode version --json` are
separate values, allowing self-dev binaries and providers to upgrade independently.
