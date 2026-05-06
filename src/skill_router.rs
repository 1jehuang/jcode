#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SkillMode {
    Auto,
    Off,
    Always,
}

pub fn select_skills(goal: &str, explicit: &[String], mode: SkillMode) -> Vec<String> {
    if mode == SkillMode::Off {
        return explicit.to_vec();
    }

    let text = goal.to_lowercase();
    let mut selected = Vec::new();
    for name in explicit {
        push_unique(&mut selected, name);
    }

    let coding_terms = [
        "código",
        "codigo",
        "code",
        "bug",
        "teste",
        "test",
        "refator",
        "review",
        "revisar",
        "implementar",
        "implement",
        "corrigir",
        "fix",
        "pr",
        "diff",
    ];
    let perf_terms = [
        "optimization",
        "optimize",
        "otimização",
        "otimizacao",
        "otimizar",
        "performance",
        "latência",
        "latencia",
        "memória",
        "memoria",
        "throughput",
        "eficiência",
        "efficiency",
        "cpu",
        "ram",
    ];

    if mode == SkillMode::Always || coding_terms.iter().any(|term| text.contains(term)) {
        push_unique(&mut selected, "karpathy-guidelines");
        push_unique(&mut selected, "clean-code-guardian");
    }
    if mode == SkillMode::Always || perf_terms.iter().any(|term| text.contains(term)) {
        push_unique(&mut selected, "optimization");
    }

    selected
}

pub fn build_skill_preface(goal: &str, explicit: &[String], mode: SkillMode) -> Option<String> {
    let selected = select_skills(goal, explicit, mode);
    if selected.is_empty() {
        return None;
    }

    let registry = crate::skill::SkillRegistry::load().ok()?;
    let mut out =
        String::from("Use these selected skills for this task. Do not load unrelated skills.\n");
    for name in selected {
        if let Some(skill) = registry.get(&name) {
            out.push_str(&format!(
                "\n## Skill: {}\n{}\n",
                skill.name, skill.description
            ));
            if name == "karpathy-guidelines" {
                out.push_str("Minimal principles: think before coding; make surgical changes; avoid speculative abstractions; state assumptions; define verifiable success criteria.\n");
            } else if name == "clean-code-guardian" {
                out.push_str("Minimal principles: prefer readable, focused, well-tested code; avoid silent errors; improve only touched code; explain trade-offs.\n");
            }
            out.push_str(&skill.content);
            out.push('\n');
        }
    }

    Some(out)
}

fn push_unique(selected: &mut Vec<String>, name: &str) {
    if !selected.iter().any(|existing| existing == name) {
        selected.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(goal: &str, explicit: &[&str], mode: SkillMode) -> Vec<String> {
        let explicit = explicit
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        select_skills(goal, &explicit, mode)
    }

    #[test]
    fn auto_selects_coding_guardrails_for_coding_tasks() {
        assert_eq!(
            names("fix this Rust bug and add a test", &[], SkillMode::Auto),
            vec!["karpathy-guidelines", "clean-code-guardian"]
        );
    }

    #[test]
    fn auto_selects_optimization_for_performance_tasks() {
        assert_eq!(
            names("reduce memory usage and CPU overhead", &[], SkillMode::Auto),
            vec!["optimization"]
        );
    }

    #[test]
    fn auto_combines_coding_and_optimization_when_both_match() {
        assert_eq!(
            names(
                "optimize this code path and review the diff",
                &[],
                SkillMode::Auto
            ),
            vec!["karpathy-guidelines", "clean-code-guardian", "optimization",]
        );
    }

    #[test]
    fn off_preserves_only_explicit_skills() {
        assert_eq!(
            names(
                "fix this bug and reduce memory usage",
                &["optimization"],
                SkillMode::Off,
            ),
            vec!["optimization"]
        );
    }

    #[test]
    fn always_includes_all_builtin_harness_skills_after_explicit() {
        assert_eq!(
            names("write docs", &["custom-skill"], SkillMode::Always),
            vec![
                "custom-skill",
                "karpathy-guidelines",
                "clean-code-guardian",
                "optimization",
            ]
        );
    }

    #[test]
    fn explicit_skill_is_not_duplicated_by_router() {
        assert_eq!(
            names(
                "review this diff",
                &["clean-code-guardian"],
                SkillMode::Auto
            ),
            vec!["clean-code-guardian", "karpathy-guidelines"]
        );
    }
}
