use super::*;

pub(super) fn build_input_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(command);
        cmd
    }
}

pub(super) fn combine_input_shell_output(stdout: &[u8], stderr: &[u8]) -> (String, bool) {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut output = String::new();

    if !stdout.is_empty() {
        output.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("[stderr]\n");
        output.push_str(&stderr);
    }

    let truncated = if output.len() > INPUT_SHELL_MAX_OUTPUT_LEN {
        output = truncate_str(&output, INPUT_SHELL_MAX_OUTPUT_LEN).to_string();
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("… output truncated");
        true
    } else {
        false
    };

    (output, truncated)
}
