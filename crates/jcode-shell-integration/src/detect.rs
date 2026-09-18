//! Shell identification from environment and process info.

use std::fmt;

/// Supported shell types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)]
pub enum Shell {
    Fish,
    Zsh,
    Bash,
    PowerShell,
    Nushell,
    Elvish,
    Unknown(String),
}

impl Shell {
    /// Detect the running shell from environment variables.
    pub fn detect() -> Self {
        // 1. Shell-specific version env vars
        if std::env::var("FISH_VERSION").is_ok() {
            return Self::Fish;
        }
        if std::env::var("ZSH_VERSION").is_ok() {
            return Self::Zsh;
        }
        if std::env::var("BASH_VERSION").is_ok() {
            return Self::Bash;
        }
        if std::env::var("NU_VERSION").is_ok() {
            return Self::Nushell;
        }
        if std::env::var("PSModulePath").is_ok() {
            return Self::PowerShell;
        }

        // 2. SHELL env var (bash/zsh/fish path)
        if let Ok(shell_path) = std::env::var("SHELL") {
            let name = shell_path.rsplit('/').next().unwrap_or(&shell_path);
            return Self::from_name(name);
        }

        // 3. Pwsh sets PSModulePath but also check parent process
        Self::Unknown("unknown".into())
    }

    /// Identify a shell from its name (basename of the executable).
    pub fn from_name(name: &str) -> Self {
        match name {
            "fish" => Self::Fish,
            "zsh" | "zsh-beta" => Self::Zsh,
            "bash" | "bash5" | "bash5.1" | "sh" => Self::Bash,
            "pwsh" | "powershell" | "powershell.exe" => Self::PowerShell,
            "nu" | "nushell" => Self::Nushell,
            "elvish" => Self::Elvish,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns true if the shell is known.
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }

    /// File extension for shell scripts (without dot).
    pub fn extension(&self) -> &str {
        match self {
            Self::Fish => "fish",
            Self::Zsh => "zsh",
            Self::Bash => "sh",
            Self::PowerShell => "ps1",
            Self::Nushell => "nu",
            Self::Elvish => "elv",
            Self::Unknown(_) => "sh",
        }
    }

    /// Human-readable name.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Fish => "fish",
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::PowerShell => "PowerShell",
            Self::Nushell => "nushell",
            Self::Elvish => "elvish",
            Self::Unknown(name) => name,
        }
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fish_detected_from_version() {
        unsafe {
            std::env::set_var("FISH_VERSION", "3.6.0");
        }
        assert_eq!(Shell::detect(), Shell::Fish);
        unsafe {
            std::env::remove_var("FISH_VERSION");
        }
    }

    #[test]
    fn zsh_detected_from_version() {
        unsafe {
            std::env::set_var("ZSH_VERSION", "5.9");
        }
        assert_eq!(Shell::detect(), Shell::Zsh);
        unsafe {
            std::env::remove_var("ZSH_VERSION");
        }
    }

    #[test]
    fn bash_detected_from_version() {
        unsafe {
            std::env::set_var("BASH_VERSION", "5.2.21");
        }
        assert_eq!(Shell::detect(), Shell::Bash);
        unsafe {
            std::env::remove_var("BASH_VERSION");
        }
    }

    #[test]
    fn pwsh_detected() {
        unsafe {
            std::env::set_var("PSModulePath", "/usr/local/share/powershell/Modules");
        }
        assert_eq!(Shell::detect(), Shell::PowerShell);
        unsafe {
            std::env::remove_var("PSModulePath");
        }
    }

    #[test]
    fn from_name_recognizes_shells() {
        assert_eq!(Shell::from_name("fish"), Shell::Fish);
        assert_eq!(Shell::from_name("zsh"), Shell::Zsh);
        assert_eq!(Shell::from_name("bash"), Shell::Bash);
        assert_eq!(Shell::from_name("sh"), Shell::Bash);
        assert_eq!(Shell::from_name("pwsh"), Shell::PowerShell);
        assert_eq!(Shell::from_name("nu"), Shell::Nushell);
        assert_eq!(Shell::from_name("elvish"), Shell::Elvish);
        assert!(matches!(Shell::from_name("dash"), Shell::Unknown(_)));
    }

    #[test]
    fn extension_is_correct() {
        assert_eq!(Shell::Fish.extension(), "fish");
        assert_eq!(Shell::Zsh.extension(), "zsh");
        assert_eq!(Shell::Bash.extension(), "sh");
        assert_eq!(Shell::PowerShell.extension(), "ps1");
        assert_eq!(Shell::Nushell.extension(), "nu");
        assert_eq!(Shell::Elvish.extension(), "elv");
    }

    #[test]
    fn display_names() {
        assert_eq!(Shell::Fish.display_name(), "fish");
        assert_eq!(Shell::PowerShell.display_name(), "PowerShell");
        assert_eq!(Shell::Unknown("test".into()).display_name(), "test");
    }

    #[test]
    fn is_known() {
        assert!(Shell::Fish.is_known());
        assert!(Shell::Bash.is_known());
        assert!(!Shell::Unknown("x".into()).is_known());
    }
}
