//! Public-API regressions for the harness refinement review findings.
use jcode_plan::refine::surface::{CODEX_SURFACE, OPENCODE_SURFACE, Tree, validate};

fn tree(pairs: &[(&str, &str)]) -> Tree {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn review_regression_unicode_cursor() {
    for surface in [&OPENCODE_SURFACE, &CODEX_SURFACE] {
        for argument in ["\"中\"", "'😀'", "(中)", "中", "\"\"", ""] {
            let prose = format!("python {argument} then python scripts/missing.py.");
            let t = tree(&[(surface.always_on[0], &prose)]);
            let err = validate(surface, &t, false).unwrap_err();
            assert!(err.contains("invokes `scripts/missing.py`"), "{err}");
        }
        for prose in ["python \"中\"", "python \"中", "python '😀", "python (中)"] {
            let t = tree(&[(surface.always_on[0], prose)]);
            validate(surface, &t, false).expect("Unicode argument must not panic");
        }
    }
}

#[test]
fn review_regression_codex_explicit_helper() {
    for command in [
        "python .agents/skills/s/scripts/helper.py",
        "python './.agents/skills/s/scripts/helper.py'",
        "python \".agents/skills/s/scripts/helper.py\".",
    ] {
        let mut t = tree(&[
            ("AGENTS.md", command),
            (".agents/skills/s/SKILL.md", "description: s skill"),
            (".agents/skills/s/scripts/helper.py", "print(1)"),
        ]);
        validate(&CODEX_SURFACE, &t, false).expect("shipped Codex helper passes");
        t.remove(".agents/skills/s/scripts/helper.py");
        let err = validate(&CODEX_SURFACE, &t, false).unwrap_err();
        assert!(
            err.contains("invokes `.agents/skills/s/scripts/helper.py`"),
            "{err}"
        );
    }
}

#[test]
fn review_regression_helper_fallback_is_exact() {
    for surface in [&OPENCODE_SURFACE, &CODEX_SURFACE] {
        for reference in [
            format!("{}/b/scripts/helper.py", surface.skill_dir),
            "b/scripts/helper.py".to_string(),
            "other/scripts/helper.py".to_string(),
            "scripts/nested/helper.py".to_string(),
        ] {
            let skill = format!("{}/a/SKILL.md", surface.skill_dir);
            let helper = format!("{}/a/scripts/helper.py", surface.skill_dir);
            let prose = format!("name: a\ndescription: review\npython {reference}");
            let mut t = tree(&[
                (surface.always_on[0], "load a"),
                (&skill, &prose),
                (&helper, "print(1)"),
            ]);
            let err = validate(surface, &t, false).unwrap_err();
            assert!(err.contains(&format!("invokes `{reference}`")), "{err}");
            for relative in ["scripts/helper.py", "a/scripts/helper.py"] {
                t.insert(
                    skill.clone(),
                    format!("name: a\ndescription: review\npython {relative}"),
                );
                validate(surface, &t, false).expect("supported skill-relative helper passes");
            }
        }
    }
}
