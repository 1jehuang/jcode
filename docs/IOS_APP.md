# Jcode mobile app

The current iPhone client lives in the separate
[jcode-mobile repository](https://github.com/1jehuang/jcode-mobile).
That repository owns the Rust/GPUI app, iOS platform integrations, mobile tests,
build scripts, and App Store metadata. See its README for development and
platform setup instructions.

The legacy Swift/SwiftUI client formerly in this repository's `ios/` directory
and its `ios-testflight.yml` workflow were retired on November 6, 2026. Their
sources, tests, and release documentation remain available in Git history.
This removal does not assert App Store release readiness for the replacement.

## What stays in this repository

Mobile clients still depend on Jcode's server and shared libraries. Keep the
pairing and gateway endpoints, harness API, wire protocol, cloud session sync,
mobile push support, and `jcode-sdk` here. The separate mobile repository
consumes the SDK from this repository, not the retired Swift client.

Server-side mobile support is not deprecated by this cleanup. Mobile UI and
client release work should happen in `jcode-mobile`, while backend and SDK
changes should continue here.
