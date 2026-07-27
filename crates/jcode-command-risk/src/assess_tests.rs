//! End-to-end assessment tests: real command strings in, verdict out.
//!
//! The two failure modes that matter are opposite and both fatal:
//! under-firing loses a home directory (#604), over-firing trains users to
//! bypass the gate. Both directions are covered.

use super::*;
use std::path::PathBuf;

fn ctx() -> RiskContext {
    RiskContext {
        working_dir: Some(PathBuf::from("/home/u/proj")),
        home_dir: Some(PathBuf::from("/home/u")),
    }
}

fn level(command: &str) -> RiskLevel {
    assess(command, &ctx()).level
}

#[test]
fn the_issue_604_command_is_blocked_outright() {
    // The reported incident: "jcode just deleted everything in my ~".
    for command in [
        "rm -rf ~",
        "rm -rf $HOME",
        "rm -rf \"$HOME\"",
        "rm -rf ${HOME}",
        "rm -rf /home/u",
        "rm -fr ~/",
        "/bin/rm -rf ~",
        "rm -rf ~/*",
    ] {
        assert_eq!(
            level(command),
            RiskLevel::Catastrophic,
            "{command:?} must be denied outright"
        );
    }
}

#[test]
fn root_destruction_is_blocked_outright() {
    for command in [
        "rm -rf /",
        "rm -rf /*",
        "rm -rf /etc",
        "rm --recursive --force /usr",
    ] {
        assert_eq!(level(command), RiskLevel::Catastrophic, "{command:?}");
    }
}

#[test]
fn credential_theft_or_destruction_is_blocked() {
    for command in ["rm -rf ~/.ssh", "shred ~/.gnupg", "rm -rf ~/.aws"] {
        assert_eq!(level(command), RiskLevel::Catastrophic, "{command:?}");
    }
}

#[test]
fn chaining_does_not_bypass_the_gate() {
    // The dangerous half is second; the first half is innocuous.
    assert_eq!(level("echo starting && rm -rf ~"), RiskLevel::Catastrophic);
    assert_eq!(level("cd /tmp; rm -rf $HOME"), RiskLevel::Catastrophic);
    assert_eq!(level("true || rm -rf /"), RiskLevel::Catastrophic);
}

#[test]
fn routine_cleanup_inside_the_project_is_not_blocked() {
    // If these prompt, the gate is useless because everyone learns to dismiss it.
    for command in [
        "rm -rf target",
        "rm -rf ./node_modules",
        "rm -f Cargo.lock",
        "rm -rf /home/u/proj/build",
        "rm -rf /tmp/scratch",
        "cargo clean",
        "ls -la",
        "git status",
        "echo hello > out.txt",
    ] {
        assert!(
            level(command).runs_immediately(),
            "{command:?} should run without a reflection turn, got {:?}",
            level(command)
        );
    }
}

#[test]
fn deleting_outside_the_project_asks_for_justification() {
    for command in ["rm -rf /home/u/other-project", "rm -rf /srv/data"] {
        assert_eq!(level(command), RiskLevel::Confirm, "{command:?}");
    }
}

#[test]
fn non_rm_destructive_tools_are_covered() {
    // A name-based denylist would miss all of these.
    assert_eq!(level("find /home/u -delete"), RiskLevel::Catastrophic);
    assert!(level("dd if=/dev/zero of=/dev/sda").runs_immediately() == false);
    assert!(level("shred /home/u/other/secrets.txt") >= RiskLevel::Confirm);
}

#[test]
fn truncating_redirect_outside_cwd_is_caught() {
    assert!(level("echo '' > /home/u/other/important.conf") >= RiskLevel::Confirm);
}

#[test]
fn appending_is_not_treated_as_destructive() {
    assert!(level("echo line >> /home/u/other/log.txt").runs_immediately());
}

#[test]
fn runtime_computed_targets_require_justification() {
    // We cannot see what $TARGET holds, so we must not assume it is safe.
    assert!(level("rm -rf $TARGET") >= RiskLevel::Confirm);
    assert!(level("rm -rf $(cat list.txt)") >= RiskLevel::Confirm);
}

#[test]
fn destructive_command_without_a_visible_target_escalates() {
    // Unknown blast radius is a reason to ask, not to proceed.
    assert!(level("rm -rf") >= RiskLevel::Confirm);
}

#[test]
fn assessment_is_total_over_garbage_input() {
    let ctx = ctx();
    for command in ["", "   ", "'", "\\", ";;;", "&&", "rm -rf \"unterminated"] {
        let _ = assess(command, &ctx);
    }
}

#[test]
fn explanation_names_the_offending_target() {
    let assessment = assess("rm -rf ~", &ctx());
    let text = assessment.explanation();
    assert!(
        text.contains("/home/u"),
        "explanation should name the path: {text}"
    );
}

#[test]
fn catastrophic_cannot_be_unlocked_but_confirm_can() {
    assert!(RiskLevel::Catastrophic.is_absolute_deny());
    assert!(!RiskLevel::Confirm.is_absolute_deny());
    assert!(!RiskLevel::Confirm.runs_immediately());
    assert!(RiskLevel::Safe.runs_immediately());
    assert!(RiskLevel::Low.runs_immediately());
}

#[test]
fn severity_ordering_is_monotonic() {
    // from_findings takes the max, so the ordering must be correct.
    assert!(RiskLevel::Safe < RiskLevel::Low);
    assert!(RiskLevel::Low < RiskLevel::Confirm);
    assert!(RiskLevel::Confirm < RiskLevel::Catastrophic);
}

#[test]
fn the_worst_finding_in_a_chain_wins() {
    // A Low finding must not mask a Catastrophic one in the same command.
    let assessment = assess("rm -rf target && rm -rf ~", &ctx());
    assert_eq!(assessment.level, RiskLevel::Catastrophic);
    assert!(assessment.findings.len() >= 2);
}

/// State-space sweep: every combination of a destructive verb, a recursive
/// flag spelling, and a way of naming the home directory must be denied.
///
/// Hand-written cases encode the failures we already thought of. This encodes
/// the ones we did not: 6 verbs x 5 flags x 11 spellings = 330 commands.
#[test]
fn no_combination_of_verb_flag_and_home_spelling_slips_through() {
    let ctx = ctx();
    let verbs = ["rm", "/bin/rm", "shred", "srm", "rmdir", "unlink"];
    let flags = ["-rf", "-fr", "-r -f", "--recursive --force", "-Rf"];
    let homes = [
        "~",
        "~/",
        "$HOME",
        "${HOME}",
        "\"$HOME\"",
        "'/home/u'",
        "/home/u",
        "/home/u/",
        "/home/u/.",
        "/home/u/x/..",
        "/home/./u",
    ];

    let mut escaped = Vec::new();
    for verb in verbs {
        for flag in flags {
            for home in homes {
                let command = format!("{verb} {flag} {home}");
                if assess(&command, &ctx).level != RiskLevel::Catastrophic {
                    escaped.push(command);
                }
            }
        }
    }
    assert!(
        escaped.is_empty(),
        "{} command(s) reached the home directory ungated: {:#?}",
        escaped.len(),
        escaped
    );
}

/// The inverse sweep: ordinary in-project work must never be interrupted.
#[test]
fn no_routine_project_command_triggers_a_prompt() {
    let ctx = ctx();
    let commands = [
        "rm -rf target",
        "rm -rf ./target",
        "rm -rf build dist",
        "rm -f a.txt b.txt",
        "rm -rf src/generated",
        "rm -rf /home/u/proj/.cache",
        "rm -rf /tmp/x",
        "git status",
        "cargo build",
        "npm ci",
        "mkdir -p out",
        "cp a b",
        "mv old new",
        "echo hi > out.txt",
        "cat Cargo.toml",
        "grep -r TODO src",
    ];
    let mut noisy = Vec::new();
    for command in commands {
        let level = assess(command, &ctx).level;
        if !level.runs_immediately() {
            noisy.push(format!("{command} -> {level:?}"));
        }
    }
    assert!(
        noisy.is_empty(),
        "gate fired on routine work, which trains users to bypass it: {noisy:#?}"
    );
}

/// Chaining, in every separator form, must not launder a catastrophic command.
#[test]
fn no_separator_launders_a_catastrophic_command() {
    let ctx = ctx();
    let mut escaped = Vec::new();
    for sep in ["&&", "||", ";", "|", "\n"] {
        for command in [
            format!("echo safe {sep} rm -rf ~"),
            format!("rm -rf ~ {sep} echo done"),
            format!("cd /tmp {sep} rm -rf $HOME"),
        ] {
            if assess(&command, &ctx).level != RiskLevel::Catastrophic {
                escaped.push(command);
            }
        }
    }
    assert!(
        escaped.is_empty(),
        "laundered through a separator: {escaped:#?}"
    );
}

/// Bypasses found by automated review of the original PR. Each of these was
/// classified `Safe` before the fix, including `sudo rm -rf ~`.
#[test]
fn wrapper_commands_do_not_hide_the_real_program() {
    let ctx = ctx();
    let mut escaped = Vec::new();
    for wrapper in [
        "sudo",
        "doas",
        "env",
        "nice -n 10",
        "ionice -c 3",
        "time",
        "timeout 5",
        "nohup",
        "setsid",
        "stdbuf -o0",
        "command",
        "exec",
        "sudo -u root",
        "env FOO=bar",
        "watch",
    ] {
        for tail in ["rm -rf ~", "rm -rf $HOME", "rm -rf /"] {
            let command = format!("{wrapper} {tail}");
            if assess(&command, &ctx).level != RiskLevel::Catastrophic {
                escaped.push(command);
            }
        }
    }
    assert!(escaped.is_empty(), "wrapper bypass: {escaped:#?}");
}

#[test]
fn nested_wrappers_are_unwrapped_all_the_way_down() {
    let ctx = ctx();
    assert_eq!(
        level("sudo env nice -n 5 rm -rf ~"),
        RiskLevel::Catastrophic
    );
    assert_eq!(level("timeout 5 sudo rm -rf /"), RiskLevel::Catastrophic);
}

#[test]
fn a_wrapper_hiding_an_unparseable_payload_escalates() {
    // We could not see what sudo was about to run, so we must not assume safety.
    let ctx = ctx();
    assert!(assess("sudo", &ctx).level >= RiskLevel::Confirm);
}

#[test]
fn inline_shell_scripts_are_assessed_not_skipped() {
    // `sh -c "..."` is the classic way around a static parser. It cannot be
    // fully solved, but the plain literal case must not be a free pass.
    let ctx = ctx();
    for command in [
        r#"sh -c "rm -rf ~""#,
        r#"bash -c 'rm -rf $HOME'"#,
        r#"sudo sh -c "rm -rf /""#,
    ] {
        assert_eq!(
            assess(command, &ctx).level,
            RiskLevel::Catastrophic,
            "{command:?}"
        );
    }
}

#[test]
fn piped_deletes_cannot_launder_their_targets() {
    // `find ~ -type f | xargs rm -rf` deletes home contents, but neither
    // segment shows it: xargs takes its operands from stdin.
    let ctx = ctx();
    for command in [
        "find ~ -type f | xargs rm -rf",
        "find / -name '*.conf' | xargs rm",
        "cat paths.txt | xargs rm -rf",
    ] {
        assert!(
            !assess(command, &ctx).level.runs_immediately(),
            "{command:?} must not run unchallenged"
        );
    }
}

#[test]
fn files_inside_system_directories_are_protected_too() {
    // Exact-root matching left /etc/passwd merely "Confirm", and apply_patch
    // consults only the catastrophic tier, so it would have deleted it.
    let ctx = ctx();
    for path in [
        "rm -f /etc/passwd",
        "rm -rf /usr/bin/env",
        "rm /boot/vmlinuz",
        "shred /etc/shadow",
    ] {
        assert_eq!(level(path), RiskLevel::Catastrophic, "{path:?}");
    }
}

#[test]
fn user_directories_under_home_root_stay_workable() {
    // /home and /Users must not become recursive, or every project path under
    // them would be blocked.
    let ctx = ctx();
    assert!(level("rm -rf /home/u/proj/target").runs_immediately());
    assert_eq!(level("rm -rf /home"), RiskLevel::Catastrophic);
}

/// The wrapper unwrapping must not make everyday wrapped commands noisy.
#[test]
fn ordinary_wrapped_commands_still_run_immediately() {
    let ctx = ctx();
    let mut noisy = Vec::new();
    for command in [
        "sudo apt update",
        "env RUST_LOG=debug cargo test",
        "timeout 30 cargo build",
        "nice -n 10 make",
        "time ls -la",
        "sudo systemctl status jcode",
        "xargs echo",
        "find . -name '*.rs' | xargs grep TODO",
        "sh -c 'echo hello'",
        "bash -c 'cargo build'",
        "sudo rm -rf /home/u/proj/target",
    ] {
        let level = assess(command, &ctx).level;
        if !level.runs_immediately() {
            noisy.push(format!("{command} -> {level:?}"));
        }
    }
    assert!(
        noisy.is_empty(),
        "gate became noisy on normal work: {noisy:#?}"
    );
}
