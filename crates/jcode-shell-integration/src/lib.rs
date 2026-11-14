//! Shell detection, hook generation, completions, and prompt functions
//! for jcode.
//!
//! Detects which shell is running and generates shell-specific
//! integration code for prompt marking (OSC 133), CWD reporting,
//! command timing, and completion scripts.

mod completions;
mod detect;
mod hooks;

pub use completions::{generate_completions, CommandDef};
pub use detect::Shell;
pub use hooks::{FeatureFlags, HookConfig, generate_hooks};
