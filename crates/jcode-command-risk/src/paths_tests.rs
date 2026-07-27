//! Path-layer tests.
//!
//! The catastrophic tier is the only protection that cannot be argued around,
//! so it gets adversarial coverage: every spelling of "the home directory" a
//! model might plausibly emit must resolve to the same verdict.

use super::*;

fn ctx() -> RiskContext {
    RiskContext {
        working_dir: Some(PathBuf::from("/home/u/proj")),
        home_dir: Some(PathBuf::from("/home/u")),
    }
}

#[test]
fn home_directory_is_catastrophic_in_every_spelling() {
    let ctx = ctx();
    // Each of these denotes /home/u. A model under pressure could emit any.
    let spellings = [
        "~",
        "$HOME",
        "${HOME}",
        "/home/u",
        "/home/u/",
        "/home/u/.",
        "/home/u/foo/..",
        "~/",
        "~/.",
        "/home/u/../u",
        "/home/./u",
    ];
    for spelling in spellings {
        let expanded = expand(spelling, &ctx);
        assert!(
            is_catastrophic_target(&expanded, &ctx),
            "{spelling:?} resolved to {expanded:?} and was not caught"
        );
    }
}

#[test]
fn parent_traversal_out_of_home_reaches_root_and_is_caught() {
    let ctx = ctx();
    for spelling in ["~/../..", "/home/u/../..", "~/../../", "/home/u/../../"] {
        let expanded = expand(spelling, &ctx);
        assert_eq!(expanded, PathBuf::from("/"), "{spelling:?}");
        assert!(is_catastrophic_target(&expanded, &ctx), "{spelling:?}");
    }
}

#[test]
fn root_and_system_paths_are_catastrophic() {
    let ctx = ctx();
    for p in ["/", "/etc", "/usr", "/var", "/boot", "/home", "/Users"] {
        assert!(
            is_catastrophic_target(Path::new(p), &ctx),
            "{p} must be protected"
        );
    }
}

#[test]
fn credential_stores_inside_home_are_catastrophic() {
    let ctx = ctx();
    for sub in [".ssh", ".gnupg", ".aws", ".config", ".jcode"] {
        let expanded = expand(&format!("~/{sub}"), &ctx);
        assert!(is_catastrophic_target(&expanded, &ctx), "~/{sub}");
    }
}

#[test]
fn ordinary_project_paths_are_not_catastrophic() {
    let ctx = ctx();
    // The guard must not fire on normal work, or it trains people to bypass it.
    for p in [
        "/home/u/proj/target",
        "/home/u/proj/node_modules",
        "/home/u/proj/src/old.rs",
        "/tmp/scratch",
        "/home/u/scratchpad",
    ] {
        assert!(
            !is_catastrophic_target(Path::new(p), &ctx),
            "{p} should not be treated as catastrophic"
        );
    }
}

#[test]
fn home_subdirectory_is_not_itself_catastrophic() {
    // ~/proj is deletable; ~ is not. The boundary must be exact.
    let ctx = ctx();
    assert!(!is_catastrophic_target(Path::new("/home/u/proj"), &ctx));
    assert!(is_catastrophic_target(Path::new("/home/u"), &ctx));
}

#[test]
fn inside_working_directory_is_low_risk_not_confirm() {
    let ctx = ctx();
    let expanded = expand("target", &ctx);
    let finding = classify_target(&expanded, "target", true, &ctx).expect("finding");
    assert_eq!(finding.level, RiskLevel::Low);
}

#[test]
fn non_recursive_delete_inside_cwd_is_unremarkable() {
    let ctx = ctx();
    let expanded = expand("notes.txt", &ctx);
    assert_eq!(classify_target(&expanded, "notes.txt", false, &ctx), None);
}

#[test]
fn outside_working_directory_requires_confirmation() {
    let ctx = ctx();
    let expanded = expand("/home/u/other-project", &ctx);
    let finding = classify_target(&expanded, "/home/u/other-project", true, &ctx).expect("finding");
    assert_eq!(finding.level, RiskLevel::Confirm);
}

#[test]
fn temp_paths_are_exempt() {
    let ctx = ctx();
    let expanded = expand("/tmp/build-cache", &ctx);
    assert_eq!(
        classify_target(&expanded, "/tmp/build-cache", true, &ctx),
        None
    );
}

#[test]
fn glob_over_a_protected_directory_is_catastrophic() {
    let ctx = ctx();
    for raw in ["~/*", "/*", "$HOME/*"] {
        let expanded = expand(raw, &ctx);
        let finding = classify_target(&expanded, raw, true, &ctx)
            .unwrap_or_else(|| panic!("{raw} produced no finding"));
        assert_eq!(
            finding.level,
            RiskLevel::Catastrophic,
            "{raw} resolved to {expanded:?}"
        );
    }
}

#[test]
fn unresolvable_targets_escalate_rather_than_pass() {
    let ctx = ctx();
    // We deliberately do not expand these, so we cannot vouch for them.
    for raw in ["$SOME_VAR", "`cat paths.txt`", "$(echo /)"] {
        let expanded = expand(raw, &ctx);
        let finding = classify_target(&expanded, raw, true, &ctx)
            .unwrap_or_else(|| panic!("{raw} produced no finding"));
        assert!(
            finding.level >= RiskLevel::Confirm,
            "{raw} must not be silently allowed"
        );
    }
}

#[test]
fn missing_home_context_does_not_panic_or_misfire() {
    // On an exotic host HOME may be unset; the system-path tier must still work.
    let ctx = RiskContext {
        working_dir: Some(PathBuf::from("/srv/app")),
        home_dir: None,
    };
    assert!(is_catastrophic_target(Path::new("/"), &ctx));
    assert!(!is_catastrophic_target(Path::new("/srv/app/tmp"), &ctx));
}

#[test]
fn normalize_never_escapes_above_root() {
    // Pathological input must not underflow into an empty or relative path.
    let ctx = ctx();
    let expanded = expand("/../../../../..", &ctx);
    assert_eq!(expanded, PathBuf::from("/"));
    assert!(is_catastrophic_target(&expanded, &ctx));
}

#[test]
fn individual_credential_files_are_protected_not_just_their_directory() {
    // Exact-match alone left an obvious hole: deleting one private key is as
    // damaging as deleting ~/.ssh wholesale.
    let ctx = ctx();
    for path in [
        "/home/u/.ssh/id_ed25519",
        "/home/u/.ssh/config",
        "/home/u/.gnupg/secring.gpg",
        "/home/u/.aws/credentials",
        "/home/u/.kube/config",
    ] {
        assert!(
            is_catastrophic_target(Path::new(path), &ctx),
            "{path} must be protected"
        );
    }
}

#[test]
fn ordinary_config_files_remain_editable() {
    // The inverse: config directories are protected as a whole, but agents
    // legitimately rewrite individual files inside them all day.
    let ctx = ctx();
    for path in [
        "/home/u/.config/app/settings.toml",
        "/home/u/.jcode/sessions/old.json",
        "/home/u/Documents/notes/draft.md",
    ] {
        assert!(
            !is_catastrophic_target(Path::new(path), &ctx),
            "{path} should stay editable"
        );
    }
    // ...while the roots themselves stay protected.
    for path in ["/home/u/.config", "/home/u/.jcode", "/home/u/Documents"] {
        assert!(is_catastrophic_target(Path::new(path), &ctx), "{path}");
    }
}
