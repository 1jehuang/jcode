//! Global push-to-talk: the Copilot-key listener and the session check.
//!
//! Shared by Jcode Desktop and `jcode companion`, so voice works whether or
//! not Desktop is open. Both are GPUI-free and never run in the server.

#[cfg(target_os = "linux")]
pub mod hotkey;
pub mod session;
