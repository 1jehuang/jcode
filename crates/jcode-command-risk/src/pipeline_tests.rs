//! Unit tests for the security pipeline.

use super::*;
use std::path::PathBuf;

fn ctx() -> RiskContext {
    RiskContext {
        working_dir: Some(PathBuf::from("/home/u/proj")),
        home_dir: Some(PathBuf::from("/home/u")),
        scratch_dir: None,
    }
}

fn pipe(cmd: &str) -> PipelineResult {
    run_pipeline(cmd, &ctx())
}

// -- Empty / incomplete -----------------------------------------------------

#[test]
fn empty_command_is_ask() {
    assert_eq!(
        pipe(""),
        PipelineResult::Ask {
            reason: "Empty command".into()
        }
    );
}

#[test]
fn whitespace_only_is_ask() {
    assert_eq!(
        pipe("   "),
        PipelineResult::Ask {
            reason: "Empty command".into()
        }
    );
}

#[test]
fn trailing_and_is_ask() {
    assert!(matches!(pipe("echo hi &&"), PipelineResult::Ask { .. }));
}

#[test]
fn trailing_pipe_is_ask() {
    assert!(matches!(pipe("cat foo |"), PipelineResult::Ask { .. }));
}

#[test]
fn trailing_or_is_ask() {
    assert!(matches!(pipe("echo hi ||"), PipelineResult::Ask { .. }));
}

#[test]
fn trailing_semicolon_is_ask() {
    assert!(matches!(pipe("echo hi;"), PipelineResult::Ask { .. }));
}

// -- Control characters -----------------------------------------------------

#[test]
fn null_byte_is_ask() {
    assert!(matches!(pipe("echo\x00hi"), PipelineResult::Ask { .. }));
}

#[test]
fn bell_char_is_ask() {
    assert!(matches!(pipe("echo\x07hi"), PipelineResult::Ask { .. }));
}

// -- Carriage return --------------------------------------------------------

#[test]
fn cr_is_ask() {
    assert!(matches!(
        pipe("echo hi\r\nrm -rf ~"),
        PipelineResult::Ask { .. }
    ));
}

// -- Newline + destructive --------------------------------------------------

#[test]
fn multiline_destructive_is_ask() {
    assert!(matches!(
        pipe("echo hi\nrm -rf /tmp/foo"),
        PipelineResult::Ask { ref reason } if reason.contains("Multiline")
    ));
}

#[test]
fn multiline_safe_no_flag() {
    assert_eq!(pipe("echo hi\necho bye"), PipelineResult::Continue,);
}

// -- Unicode whitespace -----------------------------------------------------

#[test]
fn nbsp_is_ask() {
    assert!(matches!(
        pipe("echo\u{00A0}hello"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn em_space_is_ask() {
    assert!(matches!(
        pipe("echo\u{2003}hello"),
        PipelineResult::Ask { .. }
    ));
}

// -- Obfuscated flags -------------------------------------------------------

#[test]
fn single_quoted_flag_is_ask() {
    assert!(matches!(pipe("cmd -'v'"), PipelineResult::Ask { .. }));
}

#[test]
fn double_quoted_flag_is_ask() {
    assert!(matches!(pipe("cmd -\"v\""), PipelineResult::Ask { .. }));
}

// -- Backslash escaped whitespace -------------------------------------------

#[test]
fn backslash_space_is_ask() {
    assert!(matches!(pipe("echo\\ test"), PipelineResult::Ask { .. }));
}

#[test]
fn backslash_tab_is_ask() {
    assert!(matches!(pipe("echo\\\ttest"), PipelineResult::Ask { .. }));
}

#[test]
fn escaped_inside_single_quotes_is_ok() {
    // Backslash inside single quotes is literal in bash.
    assert_eq!(pipe("echo '\\ test'"), PipelineResult::Continue);
}

// -- Backslash escaped operators --------------------------------------------

#[test]
fn backslash_semicolon_is_ask() {
    assert!(matches!(pipe("cmd\\; arg"), PipelineResult::Ask { .. }));
}

#[test]
fn backslash_pipe_is_ask() {
    assert!(matches!(pipe("cmd\\| arg"), PipelineResult::Ask { .. }));
}

#[test]
fn backslash_ampersand_is_ask() {
    assert!(matches!(pipe("cmd\\& arg"), PipelineResult::Ask { .. }));
}

// -- Shell substitution -----------------------------------------------------

#[test]
fn backtick_is_ask() {
    assert!(matches!(pipe("echo `whoami`"), PipelineResult::Ask { .. }));
}

#[test]
fn dollar_paren_is_ask() {
    assert!(matches!(pipe("echo $(whoami)"), PipelineResult::Ask { .. }));
}

#[test]
fn param_sub_is_ask() {
    assert!(matches!(pipe("echo ${HOME}"), PipelineResult::Ask { .. }));
}

// -- Zsh dangerous commands -------------------------------------------------

#[test]
fn zmodload_is_ask() {
    assert!(matches!(
        pipe("zmodload zsh/zftp"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn ztcp_is_ask() {
    assert!(matches!(
        pipe("ztcp example.com 80"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn eval_is_ask() {
    assert!(matches!(
        pipe("eval 'rm -rf ~'"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn source_is_ask() {
    assert!(matches!(
        pipe("source /tmp/evil.sh"),
        PipelineResult::Ask { .. }
    ));
}

// -- Heredoc ----------------------------------------------------------------

#[test]
fn safe_heredoc_is_continue() {
    assert_eq!(pipe("cat <<'EOF'\nhello\nEOF"), PipelineResult::Continue);
}

#[test]
fn unsafe_heredoc_is_ask() {
    // Unquoted delimiter means shell expansion inside heredoc body.
    assert!(matches!(
        pipe("echo $(cat <<EOF\nhello\nEOF\n)"),
        PipelineResult::Ask { .. }
    ));
}

// -- IFS injection ----------------------------------------------------------

#[test]
fn ifs_override_is_ask() {
    assert!(matches!(
        pipe("IFS=':' cmd arg"),
        PipelineResult::Ask { ref reason } if reason.contains("IFS")
    ));
}

#[test]
fn ifs_not_on_first_word() {
    // IFS= at the start only.
    assert_eq!(
        pipe("IFS='x' echo hi"),
        PipelineResult::Ask {
            reason: "Sets IFS, altering shell word splitting".into(),
        }
    );
}

// -- Dangerous variables ----------------------------------------------------

#[test]
fn path_override_is_ask() {
    assert!(matches!(
        pipe("PATH=/tmp:$PATH cmd"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn ld_preload_is_ask() {
    assert!(matches!(
        pipe("LD_PRELOAD=/tmp/evil.so cmd"),
        PipelineResult::Ask { .. }
    ));
}

#[test]
fn histfile_is_ask() {
    assert!(matches!(
        pipe("HISTFILE=/dev/null"),
        PipelineResult::Ask { .. }
    ));
}

// -- Brace expansion --------------------------------------------------------

#[test]
fn brace_comma_is_ask() {
    assert!(matches!(pipe("touch {a,b,c}"), PipelineResult::Ask { .. }));
}

#[test]
fn brace_range_is_ask() {
    assert!(matches!(pipe("echo {1..100}"), PipelineResult::Ask { .. }));
}

// -- Blast radius -----------------------------------------------------------

#[test]
fn rm_home_is_block() {
    assert!(matches!(pipe("rm -rf ~"), PipelineResult::Block { .. }));
}

#[test]
fn rm_system_path_is_block() {
    assert!(matches!(pipe("rm -rf /usr"), PipelineResult::Block { .. }));
}

#[test]
fn rm_ssh_dir_is_block() {
    assert!(matches!(
        pipe("rm -rf ~/.ssh"),
        PipelineResult::Block { .. }
    ));
}

// -- Safe commands pass through ---------------------------------------------

#[test]
fn echo_is_continue() {
    assert_eq!(pipe("echo hello"), PipelineResult::Continue);
}

#[test]
fn ls_is_continue() {
    assert_eq!(pipe("ls -la"), PipelineResult::Continue);
}

#[test]
fn git_status_is_continue() {
    assert_eq!(pipe("git status"), PipelineResult::Continue);
}

#[test]
fn cargo_build_is_continue() {
    assert_eq!(pipe("cargo build --release"), PipelineResult::Continue);
}

// -- Quoted content is safe -------------------------------------------------

#[test]
fn single_quoted_backtick_is_continue() {
    assert_eq!(pipe("echo '`whoami`'"), PipelineResult::Continue);
}

#[test]
fn double_quoted_backtick_may_flag() {
    // Double-quoted backtick is expanded by bash, so this gets flagged.
    assert!(matches!(
        pipe("echo \"`whoami`\""),
        PipelineResult::Ask { .. }
    ));
}

// -- Integration: redirection stripping -------------------------------------

#[test]
fn null_redirect_does_not_mask_danger() {
    // `rm ~ 2>/dev/null` should still be blocked.
    assert!(matches!(
        pipe("rm -rf ~ 2>/dev/null"),
        PipelineResult::Block { .. }
    ));
}

#[test]
fn safe_command_with_redirect_is_continue() {
    assert_eq!(pipe("echo hi > /dev/null"), PipelineResult::Continue);
}

// -- run_security_pipeline (default context) --------------------------------

#[test]
fn default_context_rm_home_is_block() {
    // Without explicit context, HOME is from env or absent. If HOME is set
    // in the process environment, rm ~ still blocks.
    let result = run_security_pipeline("rm -rf ~");
    // It either blocks (if HOME is set) or asks (if targets can't be resolved).
    assert!(matches!(
        result,
        PipelineResult::Block { .. } | PipelineResult::Ask { .. }
    ));
}
