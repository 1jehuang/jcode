// WASM bindings for carpai-sdk
// Used by VSCode webview and browser-based IDEs

#[cfg(feature = "wasm")]
pub mod bindings;

#[cfg(feature = "wasm")]
pub use bindings::*;
