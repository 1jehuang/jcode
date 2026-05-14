use super::registry::SkillRegistry;

pub struct SkillCommand;

impl SkillCommand {
    pub fn execute(args: &[String], registry: &SkillRegistry) -> String {
        if args.is_empty() {
            return Self::usage().to_string();
        }

        match args[0].as_str() {
            "list" | "ls" => Self::list_skills(registry),
            "search" | "find" => {
                if args.len() < 2 {
                    return "Usage: skills search <query>".to_string();
                }
                Self::search_skills(registry, &args[1])
            }
            "info" | "show" => {
                if args.len() < 2 {
                    return "Usage: skills info <name>".to_string();
                }
                Self::show_skill(registry, &args[1])
            }
            "categories" | "cats" => Self::list_categories(registry),
            "count" => format!("Total skills: {}", registry.count_sync()),
            _ => format!("Unknown subcommand: {}. {}", args[0], Self::usage()),
        }
    }

    fn usage() -> &'static str {
        "Usage: skills <list|search|info|categories|count>"
    }

    fn list_skills(registry: &SkillRegistry) -> String {
        let skills = registry.list_sync();
        if skills.is_empty() {
            return "No skills registered.".to_string();
        }
        let mut output = String::from("Skills:\n");
        for skill in &skills {
            let builtin = if skill.definition.is_builtin { "[builtin]" } else { "[loaded]" };
            output.push_str(&format!("  {} {} - {} ({})\n",
                builtin, skill.definition.name, skill.definition.description,
                skill.definition.category.label()));
        }
        output
    }

    fn search_skills(registry: &SkillRegistry, query: &str) -> String {
        let skills = registry.search_sync(query);
        if skills.is_empty() {
            return format!("No skills found matching '{}'", query);
        }
        let mut output = format!("Skills matching '{}':\n", query);
        for skill in &skills {
            output.push_str(&format!("  {} - {}\n", skill.definition.name, skill.definition.description));
        }
        output
    }

    fn show_skill(registry: &SkillRegistry, name: &str) -> String {
        match registry.get_sync(name) {
            Some(skill) => {
                let mut output = String::new();
                output.push_str(&format!("Skill: {} ({})\n", skill.definition.display_name, skill.definition.name));
                output.push_str(&format!("  Description: {}\n", skill.definition.description));
                output.push_str(&format!("  Category: {}\n", skill.definition.category.label()));
                output.push_str(&format!("  Built-in: {}\n", skill.definition.is_builtin));
                if !skill.definition.tags.is_empty() {
                    output.push_str(&format!("  Tags: {}\n", skill.definition.tags.join(", ")));
                }
                if !skill.definition.params.is_empty() {
                    output.push_str("  Parameters:\n");
                    for param in &skill.definition.params {
                        let req = if param.required { "(required)" } else { "(optional)" };
                        output.push_str(&format!("    - {}: {} {}\n", param.name, param.description, req));
                    }
                }
                if !skill.definition.required_mcp_plugins.is_empty() {
                    output.push_str(&format!("  Requires MCP plugins: {}\n", skill.definition.required_mcp_plugins.join(", ")));
                }
                output
            }
            None => format!("Skill '{}' not found", name),
        }
    }

    fn list_categories(registry: &SkillRegistry) -> String {
        let skills = registry.list_sync();
        let mut categories: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for skill in &skills {
            let cat = skill.definition.category.label().to_string();
            *categories.entry(cat).or_insert(0) += 1;
        }
        let mut output = String::from("Skill Categories:\n");
        for (cat, count) in &categories {
            output.push_str(&format!("  {}: {} skills\n", cat, count));
        }
        output
    }
}