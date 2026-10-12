//! Child-process helpers shared by every SDK spawn.

use std::process::Command;

/// Windows `CREATE_NO_WINDOW`: the child gets a hidden console.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keep a console-subsystem child (the `jcode` CLI, `ssh`, `git`) from opening
/// a console window. GUI hosts such as Jcode Desktop have no console for
/// children to share, so on Windows every spawn would otherwise flash a
/// terminal window. Pipes and inherited handles are unaffected. No-op elsewhere.
pub(crate) fn no_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
