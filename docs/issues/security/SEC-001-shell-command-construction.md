# [SEC-001] Shell command construction uses `sh -c`/`pwsh -Command` with interpolated strings

**ID:** SEC-001
**Category:** security
**Severity:** high
**File:** crates/kcode-sdk/src/ssh.rs:718,857; crates/kcode-app-core/src/server/client_actions.rs:36; crates/kcode-tui/src/tui/app/input.rs:91
**Status:** open

## Description
Multiple sites build shell commands by passing a caller-provided `command` string to `/bin/sh -c`,
`pwsh -Command`, or `cmd /C` (`build_input_shell_command`). Where the string is derived from
model/agent output or remote input, this is direct command injection into the local shell. The
`!`-prefix input path and SSH `$SSH_ORIGINAL_COMMAND` path are the highest-risk entry points.

## Evidence
```
grep -rInE '"/(bin/)?(sh|bash)"|sh -c|pwsh .-Command|cmd /C' crates src
# crates/kcode-sdk/src/ssh.rs:718: let mut command = Command::new("/bin/sh");
# crates/kcode-sdk/src/ssh.rs:857: let output = Command::new("/bin/sh")
# crates/kcode-app-core/src/server/client_actions.rs:36: cmd.arg("-Command").arg(command);
# crates/kcode-tui/src/tui/app/input.rs:91: ...Command::new("pwsh.exe")... ("-Command").arg(command)
# ssh_integration_tests.rs:135: exec /bin/sh -c "$SSH_ORIGINAL_COMMAND"
```

## Suggested fix
Where a genuine shell (pipes/redirects) is required, this is inherent to the feature, but it should
be (a) explicitly opt-in, (b) gated behind the same confirmation/approval used for other destructive
tools, and (c) never applied to values that are not intended as shell text. For command execution
that does not need a shell, switch to `Command::new(program).args([...])` with no `-c`. Document the
trust boundary in each function's doc comment and add tests asserting that user-supplied *arguments*
are not concatenated into the shell string.

## Effort
- L (half day)
