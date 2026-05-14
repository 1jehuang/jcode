# Browser Provider Protocol

Status: draft
Owner: jcode
Audience: jcode core, browser bridge authors, adapter authors

## Why this exists

jcode should expose a single first-class `browser` tool while remaining compatible with multiple browser automation backends:

- Firefox Agent Bridge
- CloakBrowser Playwright fallback
- Chrome Agent Bridge
- Chrome remote debugging / CDP adapters
- WebDriver / BiDi adapters
- Safari automation adapters
- other third-party browser control systems

The protocol in this document defines the **normalized contract** between jcode and a browser provider.

This is intentionally **not** a demand that every bridge speak exactly the same native command language. Instead:

- jcode defines a **core semantic layer** it can rely on
- providers declare the capabilities and commands they support
- providers may expose **provider-specific commands** beyond the core
- adapters can translate a provider's native model into this protocol

That gives us both consistency and room for bridge-specific power features.

---

## Design goals

1. **One first-class tool in jcode**
   - The model should use a single `browser` tool.

2. **Multiple provider implementations**
   - Firefox, Chrome, Safari, Edge, WebDriver, and other systems should fit.

3. **Capability negotiation**
   - jcode should know what each provider can and cannot do.

4. **Extensibility without fragmentation**
   - We need a standard core, but providers must have room for browser-specific features.

5. **Stable session and element references**
   - The model should be able to snapshot a page, then act on returned references.

6. **Transport-neutral semantics**
   - The semantic protocol should be the same whether the provider is in-process, over stdio, over a socket, or wrapped through another adapter.

---

## Non-goals

1. Standardizing every low-level browser primitive.
2. Forcing all providers to support deep DOM, network, or JS introspection.
3. Requiring all providers to attach to the user's existing browser profile.
4. Making provider-specific commands part of the required core.

---

## Terminology

- **browser tool**: the user/model-facing jcode tool.
- **provider**: a backend implementation that satisfies this protocol.
- **bridge**: an external browser integration such as Firefox Agent Bridge.
- **adapter**: glue code that translates a bridge's native API into this protocol.
- **browser session**: the provider's isolated session or attachment scope for a jcode session.
- **page**: a tab, target, or browsing surface under a session.
- **element ref**: an opaque provider-issued handle for an actionable element.

---

## Conformance model

Providers do not need to implement everything.

### Core required for certification

A provider should support these normalized operations to be considered `certified`:

- `provider.describe`
- `provider.status`
- `session.ensure`
- `session.close`
- `page.open`
- `page.snapshot`
- `page.click`
- `page.type`
- `page.wait`
- `page.screenshot`

### Optional but recommended

- `page.go_back`
- `page.go_forward`
- `page.reload`
- `tab.list`
- `tab.activate`
- `tab.close`
- `page.eval`
- `page.press`
- `page.scroll`
- `page.select`
- `download.list`

### Provider-specific extensions

Providers may expose additional commands such as:

- `firefox.install_extension`
- `chrome.attach_debug_target`
- `cdp.send`
- `webdriver.perform_actions`

These are allowed, but they are not part of the required core.

---

## Transport model

This protocol defines **message semantics**, not one required wire format.

Supported implementation styles may include:

- direct Rust trait calls inside jcode
- stdio JSON request/response
- local socket RPC
- wrapped remote API

For external-process integrations, the recommended envelope is a JSON-RPC-like shape.

---

## Message envelope

For external providers, requests and responses should use a stable envelope.

### Request

```json
{
  "protocol_version": "0.1",
  "id": "req_123",
  "method": "page.open",
  "params": {
    "session_id": "sess_abc",
    "url": "https://example.com"
  }
}
```

### Success response

```json
{
  "protocol_version": "0.1",
  "id": "req_123",
  "ok": true,
  "result": {
    "page_id": "page_1",
    "url": "https://example.com",
    "title": "Example Domain"
  },
  "warnings": []
}
```

### Error response

```json
{
  "protocol_version": "0.1",
  "id": "req_123",
  "ok": false,
  "error": {
    "code": "unsupported_method",
    "message": "This provider does not implement page.eval",
    "retryable": false,
    "details": {}
  }
}
```

### Event envelope

If a provider emits async events, use:

```json
{
  "protocol_version": "0.1",
  "event": "page.navigated",
  "payload": {
    "session_id": "sess_abc",
    "page_id": "page_1",
    "url": "https://example.com/next"
  }
}
```

Events are optional in v1.

---

## Discovery and handshake

### `provider.describe`

Returns static and semi-static metadata about the provider.

Example:

```json
{
  "provider_id": "firefox_agent_bridge",
  "provider_label": "Firefox Agent Bridge",
  "provider_version": "1.2.3",
  "protocol_version": "0.1",
  "browser_families": ["firefox"],
  "transport": "stdio-json",
  "certification_tier": "candidate",
  "capabilities": {
    "core_methods": [
      "session.ensure",
      "session.close",
      "page.open",
      "page.snapshot",
      "page.click",
      "page.type",
      "page.wait",
      "page.screenshot"
    ],
    "optional_methods": [
      "tab.list",
      "tab.activate",
      "page.eval"
    ],
    "features": [
      "element_refs",
      "a11y_snapshot",
      "attach_existing_browser",
      "persistent_profile"
    ],
    "custom_methods": [
      {
        "name": "firefox.install_extension",
        "stability": "experimental",
        "description": "Install or verify the Firefox extension"
      }
    ]
  }
}
```

### `provider.status`

Returns current availability and setup state.

Example fields:

```json
{
  "availability": "ready",
  "browser_detected": true,
  "browser_running": true,
  "setup_state": "complete",
  "requires_manual_setup": false,
  "recommended_browser": "firefox",
  "manual_steps": [],
  "diagnostics": [
    {
      "level": "info",
      "code": "native_host_detected",
      "message": "Native host manifest found"
    }
  ]
}
```

Suggested enums:

- `availability`: `ready | degraded | unavailable`
- `setup_state`: `complete | partial | required | broken`

---

## Session model

jcode should not care whether a provider uses tabs, contexts, profiles, or remote targets internally.
It only needs a stable handle it can reuse.

### `session.ensure`

Creates or reuses a browser session for a jcode session.

Request:

```json
{
  "client_session_id": "jcode_session_123",
  "browser_preference": "auto",
  "isolation": "per_jcode_session",
  "attach": "prefer",
  "persist": true,
  "metadata": {
    "owner": "agent",
    "purpose": "browser_tool"
  }
}
```

Response:

```json
{
  "session_id": "browser_sess_1",
  "browser_family": "firefox",
  "browser_label": "Firefox",
  "attached_to_existing_browser": true,
  "isolation": "per_jcode_session",
  "default_page_id": "page_1"
}
```

### `session.close`

Closes or detaches the provider session.

Providers may choose whether this closes tabs, detaches from a target, or merely releases provider-side state. The behavior should be documented in `provider.describe` or `provider.status` diagnostics.

---

## Resource identifiers

All resource identifiers are opaque strings.

Examples:

- `session_id`
- `page_id`
- `tab_id`
- `element_ref`
- `download_id`

jcode must not assume identifier shape or encode browser semantics into them.

---

## Normalized core methods

These are the semantics jcode can rely on.

### `page.open`

Open a URL in the current page or a new page.

Request fields:

- `session_id` required
- `url` required
- `page_id` optional
- `new_page` optional
- `foreground` optional
- `wait_until` optional: `load | domcontentloaded | networkidle | provider_default`

Response fields:

- `page_id`
- `url`
- `title` optional
- `navigation_state` optional

### `page.snapshot`

Return a normalized view of the current page for agent reasoning.

This is the most important method for model use.

Request fields:

- `session_id` required
- `page_id` optional
- `include_screenshot` optional
- `include_html` optional
- `include_dom` optional
- `include_a11y` optional
- `include_text` optional
- `max_nodes` optional

Response fields:

- `page_id`
- `url`
- `title`
- `snapshot`
- `elements`
- `text`
- `screenshot_ref` optional
- `provider_data` optional

#### Snapshot shape

Providers may use different internal representations, but `page.snapshot` should normalize into a common minimum format:

```json
{
  "snapshot": {
    "format": "jcode.page_snapshot.v1",
    "root": {
      "node_id": "n1",
      "role": "document",
      "name": "Example Domain",
      "children": ["n2", "n3"]
    },
    "nodes": [
      {
        "node_id": "n2",
        "role": "heading",
        "name": "Example Domain",
        "text": "Example Domain",
        "element_ref": "el_1",
        "actionable": false
      },
      {
        "node_id": "n3",
        "role": "link",
        "name": "More information...",
        "text": "More information...",
        "element_ref": "el_2",
        "actionable": true
      }
    ]
  }
}
```

#### Element list

For agent convenience, providers should also return a flattened actionable list when possible:

```json
{
  "elements": [
    {
      "element_ref": "el_2",
      "role": "link",
      "name": "More information...",
      "text": "More information...",
      "actionable": true,
      "enabled": true,
      "selector_hint": "a"
    }
  ]
}
```

A provider that cannot produce rich DOM/a11y data may still return a weaker snapshot, but it should say so in capabilities.

### `page.click`

Click an element.

Request should support multiple targeting modes:

- `element_ref`
- `selector`
- `text_query`
- `position`

At least one must be provided.

Response fields:

- `page_id`
- `clicked` boolean
- `navigation_occurred` optional
- `url` optional

Providers should prefer `element_ref` when available.

### `page.type`

Type or set text into an input-like target.

Request fields:

- `element_ref` optional
- `selector` optional
- `text` required
- `replace` optional
- `submit` optional

Response fields:

- `page_id`
- `typed` boolean

### `page.wait`

Wait for a condition.

Request fields may include:

- `text_present`
- `text_absent`
- `selector_present`
- `selector_absent`
- `element_ref_present`
- `url_matches`
- `navigation_complete`
- `timeout_ms`

Response fields:

- `satisfied` boolean
- `matched_condition` optional
- `url` optional

### `page.screenshot`

Capture a screenshot.

Request fields:

- `session_id`
- `page_id` optional
- `full_page` optional
- `clip` optional
- `element_ref` optional

Response fields:

- `page_id`
- `image` or `image_ref`
- `media_type`
- `width`
- `height`

Providers may return inline base64 data or a provider-managed image reference depending on transport constraints.

---

## Optional normalized methods

These methods are standardized when present, but not required for certification in the first pass.

### Navigation

- `page.go_back`
- `page.go_forward`
- `page.reload`

### Keyboard and form interaction

- `page.press`
- `page.select`
- `page.hover`
- `page.scroll`

### Tabs and pages

- `tab.list`
- `tab.activate`
- `tab.close`
- `tab.new`

### Introspection and debugging

- `page.eval`
- `network.list`
- `console.list`
- `storage.get`
- `cookie.list`

### Files and downloads

- `download.list`
- `download.wait`
- `upload.set_files`

---

## Extensibility model

This is the key part that allows leeway for provider-specific commands.

### Rule 1: providers may expose custom methods

Custom methods should use a namespaced method name, for example:

- `firefox.install_extension`
- `chrome.attach_debug_target`
- `cdp.send`
- `webdriver.actions`

### Rule 2: providers must advertise custom methods

Every custom method should appear in `provider.describe.capabilities.custom_methods` with:

- `name`
- `description`
- `stability`: `stable | experimental | deprecated`
- optional `input_schema`
- optional `output_schema`

### Rule 3: jcode core should only rely on normalized methods by default

The main `browser` tool should prefer the standard core and optional normalized methods.
Provider-specific methods should only be used when:

- the user explicitly asks for them
- a jcode-side adapter knows how to use them safely
- or a future advanced/debug mode is enabled

### Rule 4: provider-native passthrough is allowed, but should be explicit

If we want an escape hatch, the browser tool can support something like:

```json
{
  "action": "provider_command",
  "provider_method": "cdp.send",
  "params": {
    "method": "Network.enable"
  }
}
```

This should be considered advanced/debug behavior, not the primary UX.

---

## Capability schema

Providers should report both methods and higher-level features.

### Methods

Concrete callable operations:

- `page.open`
- `page.snapshot`
- `tab.list`

### Features

Semantics or qualities that influence jcode behavior:

- `element_refs`
- `a11y_snapshot`
- `dom_snapshot`
- `html_snapshot`
- `full_page_screenshot`
- `attach_existing_browser`
- `persistent_profile`
- `isolated_contexts`
- `js_eval`
- `network_observe`
- `console_observe`
- `file_upload`
- `download_observe`
- `manual_setup_required`
- `extension_required`
- `remote_debugging_required`

---

## Built-in CloakBrowser fallback

jcode includes an optional CloakBrowser-backed provider for Chromium/Playwright automation.
It is intentionally a fallback, not the default setup path:

- `browser="auto"` still prefers the Firefox Agent Bridge when it is ready.
- If Firefox is not ready and the Python `cloakbrowser` module is installed, `auto` falls back to CloakBrowser.
- `browser="chrome"` selects CloakBrowser directly.
- `browser action="setup" browser="chrome"` installs the Python `cloakbrowser` wrapper with `python3 -m pip install cloakbrowser`.
- `JCODE_CLOAKBROWSER_PYTHON` can point at a specific Python interpreter or virtualenv.

The fallback currently supports the core page actions `open`, `snapshot`, `get_content`,
`screenshot`, `eval`, `click`, `type`, and `wait`. It uses a persistent profile under
`~/.jcode/cloakbrowser/<session-id>` and enables CloakBrowser's `humanize` behavior.

Because CloakBrowser downloads and launches a custom Chromium build, users should treat it
as an explicitly trusted optional backend and use it only for legitimate browser automation,
testing, or authorized access.

### Stability

Each feature or method may optionally include a stability tag:

- `stable`
- `experimental`
- `deprecated`

---

## Setup and diagnostics

A browser provider often requires manual setup. The protocol should make that machine-readable.

### Diagnostic item

```json
{
  "level": "warning",
  "code": "extension_missing",
  "message": "Firefox extension is not installed",
  "manual_steps": [
    "Open Firefox",
    "Install the extension from /path/to/bridge.xpi",
    "Restart Firefox if prompted"
  ]
}
```

### Recommended setup-oriented methods

- `provider.status`
- `provider.setup_guide` optional
- `provider.verify` optional

`provider.setup_guide` may return browser-specific instructions, URLs, file paths, permissions, or troubleshooting steps.

---

## Error model

Standard error codes should include:

- `unsupported_method`
- `unsupported_target`
- `invalid_request`
- `invalid_selector`
- `element_not_found`
- `element_not_actionable`
- `navigation_timeout`
- `not_ready`
- `setup_required`
- `permission_denied`
- `browser_not_running`
- `session_not_found`
- `page_not_found`
- `internal_error`

Providers may add provider-specific detail codes in `error.details`.

---

## Versioning

The protocol should be versioned independently from provider versions.

### Rules

- `protocol_version` identifies the semantic protocol version.
- Providers should declare the protocol version they implement.
- Minor additive changes should not break existing certified providers.
- Breaking changes require a new protocol version.

For now use:

- `protocol_version = "0.1"`

---

## Certification guidance

A provider can be classified as:

### Certified

- passes conformance tests for required core methods
- returns stable identifiers and normalized results
- reports setup/diagnostics correctly
- behaves predictably across repeated runs

### Compatible

- supports some or most normalized methods
- may have missing features or partial behavior
- useful, but not yet fully certified

### Experimental

- adapter exists, but semantics are incomplete or unstable

---

## Minimal conformance scenarios

A future conformance suite should verify at least:

1. `provider.describe` succeeds
2. `provider.status` reports a coherent state
3. `session.ensure` creates or reuses a session
4. `page.open` navigates to a test page
5. `page.snapshot` returns usable text and at least one actionable reference when applicable
6. `page.click` can activate a known element
7. `page.type` can fill a known input
8. `page.wait` observes a deterministic page change
9. `page.screenshot` returns an image
10. `session.close` cleans up or detaches cleanly

---

## Recommended jcode integration policy

The jcode `browser` tool should:

1. prefer normalized core methods
2. choose a provider based on user preference, availability, and capability quality
3. expose provider-specific methods only behind an explicit advanced path
4. return setup guidance when no ready provider is available
5. avoid baking Firefox-specific or Chrome-specific assumptions into the core tool API

---

## Open questions

These are intentionally left open for the next iteration.

1. Should screenshots always be inline, or can providers return file/image handles?
2. Should event streaming be required for advanced integrations?
3. How much of raw HTML/DOM should be normalized versus returned as provider data?
4. Should `page.snapshot` support multiple named formats beyond `jcode.page_snapshot.v1`?
5. Should provider-specific methods be invokable through the same `browser` tool or only via debug mode?
6. Should setup/install flows themselves be standardized beyond status and diagnostics?

---

## Proposed next steps

1. Review this document and tighten the core method set.
2. Decide the exact normalized `page.snapshot` format.
3. Define a Rust trait matching this protocol.
4. Implement the first provider adapter for Firefox Agent Bridge.
5. Build a conformance test harness.
6. Add README browser setup and compatibility documentation after the protocol stabilizes.
