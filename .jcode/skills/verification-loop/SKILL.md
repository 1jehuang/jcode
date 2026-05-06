---
name: verification-loop
description: Deliberate verification workflow. Use this skill after meaningful changes, before preparing a PR, after refactoring, or whenever the user needs a reliable build-test-lint-review loop, even if they do not explicitly ask for verification.
allowed-tools: read, grep, bash
---

# Verification Loop

Use this workflow to verify changes before handing work back to the user or preparing a PR.

## When To Use

- After completing a feature or bug fix.
- Before preparing a PR or handoff.
- After refactoring shared code, protocols, tools, prompts, skills, docs, providers, memory behavior, or safety behavior.
- After editing configuration, command surfaces, wrappers, or integration docs.

## Verification Order

1. Run the narrowest checks that directly cover the changed area.
2. Run broader validation if the change affects shared behavior.
3. Review the diff for accidental scope growth.
4. Report anything not verified and why.

## Common jcode Checks

Prefer the narrowest relevant subset first:

```bash
cargo test <module_or_test_name>
cargo test
cargo clippy --all-targets --all-features
cargo fmt --check
```

For documentation-only or skill-only changes, run structural checks that prove the files are parseable and placed where jcode loads them:

```bash
find .jcode/skills -name SKILL.md -print
rg -n "^name:|^description:" .jcode/skills
```

If a full Rust build is too expensive or unrelated, say that explicitly and run the strongest cheap checks available.

## Diff Review

Before closing the task, inspect:

```bash
git diff --stat
git diff
git status --short
```

Check for:

- Unrelated file churn.
- Generated or cached files.
- Formatting-only changes mixed with behavior changes.
- Missing tests for changed behavior.
- Documentation that no longer matches implementation.

## Stop Conditions

Stop and fix before closing the task if:

- Syntax, parser, formatting, or lint checks fail.
- Relevant tests fail.
- The change introduces an undocumented command, config, protocol field, or user-facing behavior.
- Skill or prompt files are missing required frontmatter.

## Output Expectations

Close with:

- Context read.
- Files changed.
- Tests or checks run.
- Result.
- Relevant logs or command output.
- Risks.
- Next steps.
