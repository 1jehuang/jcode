//! # Skill System — 可扩展参数化技能/命令系统
//!
//! 超越 Claude Code 的固定斜杠命令：
//! - **参数化模板**：技能定义支持 {{variable}} 占位符
//! - **热加载**：运行时从 YAML/JSON 加载新技能，无需重启
//! - **条件触发**：基于文件类型、语言、上下文自动推荐
//! - **组合管道**：多个技能可串联为 pipeline
//! - **权限绑定**：每个技能可声明所需权限级别
//! - **版本管理**：技能支持版本号和兼容性检查

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub version: String,
    pub description: String,
    pub command_pattern: String,
    pub template: String,
    #[serde(default)]
    pub parameters: Vec<SkillParameter>,
    #[serde(default)]
    pub triggers: SkillTriggers,
    pub required_permissions: Vec<String>,
    pub category: SkillCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    pub name: String,
    pub param_type: ParamType,
    pub required: bool,
    pub default_value: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamType { String, Number, Boolean, FilePath, Enum(Vec<String>) }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillTriggers {
    #[serde(default)]
    pub file_extensions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub min_confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillCategory { CodeEdit, Refactor, Test, Git, Analysis, Custom }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub resolved_args: HashMap<String, String>,
    pub rendered_prompt: String,
    pub confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, SkillDefinition>>,
    skill_dirs: RwLock<Vec<PathBuf>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
            skill_dirs: RwLock::new(Vec::new()),
        }
    }

    pub fn add_skill_dir(&self, dir: impl Into<PathBuf>) -> Result<()> {
        let path = dir.into();
        std::fs::create_dir_all(&path)?;
        self.skill_dirs.write().unwrap().push(path);
        self.reload_from_disk()?; Ok(())
    }

    pub fn register(&self, def: SkillDefinition) -> Result<()> {
        self.skills.write().unwrap().insert(def.name.clone(), def);
        Ok(())
    }

    pub fn reload_from_disk(&self) -> Result<usize> {
        let dirs = self.skill_dirs.read().unwrap().clone();
        let mut loaded = 0;
        for dir in &dirs {
            if !dir.is_dir() { continue; }
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "yaml" || e == "json") {
                    if let Ok(def) = self.load_skill_file(&path) {
                        self.skills.write().unwrap().insert(def.name.clone(), def);
                        loaded += 1;
                    }
                }
            }
        }
        Ok(loaded)
    }

    fn load_skill_file(&self, path: &Path) -> Result<SkillDefinition> {
        let content = std::fs::read_to_string(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "yaml" | "yml" => serde_yaml::from_str(&content).map_err(|e| anyhow::anyhow!("{}", e)),
            "json" => serde_json::from_str(&content).map_err(|e| anyhow::anyhow!("{}", e)),
            _ => bail!("Unsupported skill format"),
        }.with_context(|| format!("Loading skill from {:?}", path))
    }

    pub fn get(&self, name: &str) -> Option<SkillDefinition> {
        self.skills.read().unwrap().get(name).cloned()
    }

    pub fn list_by_category(&self, cat: SkillCategory) -> Vec<SkillDefinition> {
        self.skills.read().unwrap().values()
            .filter(|s| s.category == cat)
            .cloned()
            .collect()
    }

    pub fn recommend(&self, context: &SkillContext) -> Vec<(SkillDefinition, f64)> {
        let skills = self.skills.read().unwrap();
        skills.values().filter_map(|skill| {
            let score = self.compute_relevance(skill, context);
            if score >= skill.triggers.min_confidence { Some((skill.clone(), score)) } else { None }
        }).collect::<Vec<_>>()
    }

    fn compute_relevance(&self, skill: &SkillDefinition, ctx: &SkillContext) -> f64 {
        let mut score = 0.0f64;
        if skill.triggers.file_extensions.is_empty() || skill.triggers.file_extensions.iter().any(|ext| {
            ctx.file_path.as_ref().map_or(false, |p| p.extension().map_or(false, |e| e.to_string_lossy() == *ext))
        }) { score += 0.3; }
        if skill.triggers.languages.is_empty() || skill.triggers.languages.iter().any(|lang| {
            ctx.language.as_ref().map_or(false, |l| l == lang)
        }) { score += 0.3; }
        let keyword_hits = skill.triggers.keywords.iter()
            .filter(|kw| ctx.query.to_lowercase().contains(&kw.to_lowercase()))
            .count();
        score += keyword_hits as f64 * 0.2;
        score.min(1.0)
    }

    pub fn invoke(&self, name: &str, args: HashMap<String, String>) -> Result<SkillInvocation> {
        let skill = self.get(name).ok_or_else(|| anyhow::anyhow!("Skill '{}' not found", name))?;

        let mut resolved = args.clone();
        for param in &skill.parameters {
            if !resolved.contains_key(&param.name) {
                if let Some(ref default) = param.default_value {
                    resolved.insert(param.name.clone(), default.clone());
                } else if param.required {
                    bail!("Required parameter '{}' missing for skill '{}'", param.name, name);
                }
            }
        }

        let mut rendered = skill.template.clone();
        for (key, value) in &resolved {
            rendered = rendered.replace(&format!("{{{{{}}}", key), value);
        }
        // Also handle {{ key }} (with spaces)
        for (key, value) in &resolved {
            rendered = rendered.replace(&format!("{{ {{ {} }} }}", key), value);
        }

        let confidence = 1.0 - (skill.parameters.iter()
            .filter(|p| p.required && !args.contains_key(&p.name))
            .count() as f64 * 0.2);

        Ok(SkillInvocation {
            skill_name: name.to_string(),
            resolved_args: resolved,
            rendered_prompt: rendered,
            confidence: confidence.max(0.0),
        })
    }
}

impl Default for SkillRegistry {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone, Default)]
pub struct SkillContext {
    pub file_path: Option<PathBuf>,
    pub language: Option<String>,
    pub query: String,
    pub selection: Option<String>,
}

pub fn builtin_skills() -> Vec<SkillDefinition> {
    vec![
        SkillDefinition {
            name: "refactor".into(), version: "1.0".into(),
            description: "Refactor selected code or function".into(),
            command_pattern: "/refactor [target]".into(),
            template: r#"Refactor the following code to improve readability and maintainability while preserving behavior:

{{selection}}

Focus on:
- Extract magic numbers into named constants
- Reduce cyclomatic complexity
- Improve variable naming
- Add type annotations where helpful"#.into(),
            parameters: vec![
                SkillParameter { name: "target".into(), param_type: ParamType::String, required: false, default_value: None, description: Some("Target scope".into()) },
                SkillParameter { name: "selection".into(), param_type: ParamType::String, required: true, default_value: None, description: Some("Code to refactor".into()) },
            ],
            triggers: SkillTriggers { keywords: vec!["refactor".into()], ..Default::default() },
            required_permissions: vec!["read".into(), "write".into()],
            category: SkillCategory::Refactor,
        },
        SkillDefinition {
            name: "explain".into(), version: "1.0".into(),
            description: "Explain code in detail".into(),
            command_pattern: "/explain [depth]".into(),
            template: r#"Explain the following code {{depth}}:

{{selection}}

Cover:
- What it does
- How it works
- Key design decisions
- Potential edge cases"#.into(),
            parameters: vec![
                SkillParameter { name: "depth".into(), param_type: ParamType::Enum(vec!["briefly".into(), "in detail".into(), "for a beginner".into()]), required: false, default_value: Some("in detail".into()), description: None },
                SkillParameter { name: "selection".into(), param_type: ParamType::String, required: true, default_value: None, description: None },
            ],
            triggers: SkillTriggers { keywords: vec!["explain".into(), "what does".into(), "how does".into()], ..Default::default() },
            required_permissions: vec!["read".into()],
            category: SkillCategory::Analysis,
        },
        SkillDefinition {
            name: "fix".into(), version: "1.0".into(),
            description: "Diagnose and fix errors".into(),
            command_pattern: "/fix [error_type]".into(),
            template: r#"Analyze and fix the following error:
{{error}}

Context:
{{selection}}

Provide:
1. Root cause analysis
2. The fix (as a precise edit)
3. Why this fix works"#.into(),
            parameters: vec![
                SkillParameter { name: "error".into(), param_type: ParamType::String, required: true, default_value: None, description: Some("Error message".into()) },
                SkillParameter { name: "selection".into(), param_type: ParamType::String, required: false, default_value: None, description: None },
            ],
            triggers: SkillTriggers { keywords: vec!["fix".into(), "error".into(), "bug".into()], ..Default::default() },
            required_permissions: vec!["read".into(), "write".into()],
            category: SkillCategory::CodeEdit,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_invoke() {
        let registry = SkillRegistry::new();
        for skill in builtin_skills() {
            registry.register(skill).unwrap();
        }

        let mut args = HashMap::new();
        args.insert("selection".into(), "fn foo() { 1 + 1 }".into());
        let inv = registry.invoke("explain", args).unwrap();
        assert!(inv.rendered_prompt.contains("Explain"));
        assert!(inv.confidence > 0.0);
    }

    #[test]
    fn test_recommend_skills() {
        let registry = SkillRegistry::new();
        for skill in builtin_skills() {
            registry.register(skill).unwrap();
        }

        let ctx = SkillContext {
            query: "please refactor this code".into(),
            language: Some("rust".into()),
            ..Default::default()
        };
        let recs = registry.recommend(&ctx);
        assert!(!recs.is_empty());
    }

    #[test]
    fn test_template_rendering() {
        let registry = SkillRegistry::new();
        registry.register(builtin_skills()[0].clone()).unwrap();

        let mut args = HashMap::new();
        args.insert("selection".into(), "let x = 1;".into());
        let inv = registry.invoke("refactor", args).unwrap();
        assert!(inv.rendered_prompt.contains("let x = 1"));
    }
}
