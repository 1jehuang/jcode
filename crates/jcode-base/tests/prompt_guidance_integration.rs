//! Exercise the public prompt builders with real cwd/HOME resolution, without
//! changing the parent test process's environment or using provider mocks.
#![cfg(unix)]

use jcode_base::prompt::{build_system_prompt_full, build_system_prompt_split};
use std::path::Path;
use std::process::Command;

const AGENTS: &str = "PROMPT_ACCEPTANCE_AGENTS_MARKER\n";
const OVERLAY: &str = "PROMPT_ACCEPTANCE_OVERLAY_MARKER\n";
const TOOLS: &str = "PROMPT_ACCEPTANCE_TOOLS_MARKER\n";

fn write_guidance(dir: &Path) {
    std::fs::create_dir_all(dir.join(".jcode")).unwrap();
    std::fs::write(dir.join("AGENTS.md"), AGENTS).unwrap();
    std::fs::write(dir.join(".jcode/prompt-overlay.md"), OVERLAY).unwrap();
    std::fs::write(dir.join(".jcode/preferred-tools.md"), TOOLS).unwrap();
}

#[test]
fn public_prompt_builders_deduplicate_home_and_aliases_but_preserve_distinct_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let alias = temp.path().join("alias");
    let distinct = temp.path().join("distinct");
    write_guidance(&home);
    write_guidance(&distinct);
    std::fs::create_dir(&alias).unwrap();
    std::os::unix::fs::symlink(home.join(".jcode"), alias.join(".jcode")).unwrap();
    std::os::unix::fs::symlink(home.join("AGENTS.md"), alias.join("AGENTS.md")).unwrap();

    for (case, cwd) in [("home", &home), ("alias", &alias), ("distinct", &distinct)] {
        // Fresh processes also exercise working_dir=None and avoid races with
        // other tests that consult HOME or cache configuration.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "prompt_guidance_child",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(cwd)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env_remove("JCODE_HOME")
            .env("JCODE_PROMPT_ACCEPTANCE_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{case}: full/split prompt contents, headings, and counts passed");
    }
}

#[test]
#[ignore = "invoked in an isolated HOME/cwd by the parent acceptance test"]
fn prompt_guidance_child() {
    let case = std::env::var("JCODE_PROMPT_ACCEPTANCE_CASE").unwrap();
    let copies = if case == "distinct" { 2 } else { 1 };
    let (full, full_info) = build_system_prompt_full(None, &[], false, None, None);
    let (split, split_info) = build_system_prompt_split(None, &[], false, None, None);

    for prompt in [&full, &split.static_part] {
        for marker in [AGENTS, OVERLAY, TOOLS] {
            assert_eq!(prompt.matches(marker.trim()).count(), copies, "{case}");
        }
        for heading in ["Instructions", "Prompt Overlay", "Preferred Tools"] {
            assert!(prompt.contains(&format!("# Project {heading}")));
            assert_eq!(prompt.contains(&format!("# Global {heading}")), copies == 2);
        }
    }
    for info in [full_info, split_info] {
        assert!(info.has_project_agents_md);
        assert_eq!(info.has_global_agents_md, copies == 2);
        assert_eq!(info.project_agents_md_chars, AGENTS.len());
        assert_eq!(info.global_agents_md_chars, (copies - 1) * AGENTS.len());
        assert_eq!(info.prompt_overlay_chars, copies * OVERLAY.len());
        assert_eq!(info.preferred_tools_chars, copies * TOOLS.len());
    }
}
