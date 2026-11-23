use super::App;

/// Build the `/skills` report: currently loaded skills (marking the active one)
/// plus the curated list of jcode-endorsed skills (marking which are installed).
pub(super) fn build_skills_report(app: &App) -> String {
    let mut out = String::new();

    let active = app.active_skill().map(|s| s.to_string());

    // Loaded skills. In remote mode we only have names; locally we have full
    // skill metadata (description + path).
    out.push_str("Loaded skills\n");
    if app.is_remote && !app.remote_skills.is_empty() {
        let mut names = app.remote_skills.clone();
        names.sort();
        for name in &names {
            let marker = if active.as_deref() == Some(name.as_str()) {
                " (active)"
            } else {
                ""
            };
            out.push_str(&format!("- /{}{}\n", name, marker));
        }
    } else {
        let snapshot = app.current_skills_snapshot();
        let mut skills = snapshot.list();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        if skills.is_empty() {
            out.push_str(
                "- none loaded\n  Add skills under ~/.jcode/skills/<name>/SKILL.md or ./.jcode/skills/<name>/SKILL.md\n",
            );
        } else {
            for skill in skills {
                let marker = if active.as_deref() == Some(skill.name.as_str()) {
                    " (active)"
                } else {
                    ""
                };
                out.push_str(&format!("- /{}{}\n", skill.name, marker));
                out.push_str(&format!("    {}\n", skill.description));
                out.push_str(&format!("    path: {}\n", skill.path.display()));
            }
        }
    }

    // Endorsed skills, marking which are installed. Build the installed set in a
    // remote-aware way (the inherent `available_skills()` ignores remote skills).
    let installed: std::collections::HashSet<String> =
        if app.is_remote && !app.remote_skills.is_empty() {
            app.remote_skills.iter().cloned().collect()
        } else {
            app.current_skills_snapshot()
                .list()
                .iter()
                .map(|s| s.name.clone())
                .collect()
        };
    out.push_str("\nEndorsed skills (recommended by jcode)\n");
    // Group by category, preserving first-seen category order.
    let mut category_order: Vec<&str> = Vec::new();
    for endorsed in crate::skill::endorsed_skills() {
        if !category_order.contains(&endorsed.category) {
            category_order.push(endorsed.category);
        }
    }
    for category in category_order {
        let installed_in_category = crate::skill::endorsed_skills()
            .iter()
            .filter(|e| e.category == category && installed.contains(e.name))
            .count();
        let total_in_category = crate::skill::endorsed_skills()
            .iter()
            .filter(|e| e.category == category)
            .count();
        out.push_str(&format!(
            "\n  {} ({}/{} installed)\n",
            category, installed_in_category, total_in_category
        ));
        for endorsed in crate::skill::endorsed_skills()
            .iter()
            .filter(|e| e.category == category)
        {
            let is_installed = installed.contains(endorsed.name);
            let status = if is_installed {
                "installed"
            } else {
                "not installed"
            };
            out.push_str(&format!("  - /{} [{}]\n", endorsed.name, status));
            out.push_str(&format!("      {}\n", endorsed.description));
            out.push_str(&format!("      source: {}\n", endorsed.source));
            if !is_installed && let Some(install) = endorsed.install {
                out.push_str(&format!("      install: {}\n", install));
            }
        }
    }

    out.push_str("\nActivate a skill by typing its slash command (e.g. /optimization).\n");
    out.push_str("Manage skills with the skill_manage tool (list/load/read/reload).\n");
    out.push_str(
        "NVIDIA CUDA-X skills come from the official catalog at https://github.com/NVIDIA/skills.\n",
    );

    out.trim_end().to_string()
}
