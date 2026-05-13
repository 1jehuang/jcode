/// Category classification for skills
#[derive(Debug, Clone, PartialEq)]
pub enum SkillCategory {
    Development,
    Testing,
    Debugging,
    Review,
    Build,
    Deploy,
    Git,
    Database,
    Security,
    Documentation,
    Productivity,
    Custom(String),
}

impl SkillCategory {
    pub fn label(&self) -> &str {
        match self {
            SkillCategory::Development => "development",
            SkillCategory::Testing => "testing",
            SkillCategory::Debugging => "debugging",
            SkillCategory::Review => "review",
            SkillCategory::Build => "build",
            SkillCategory::Deploy => "deploy",
            SkillCategory::Git => "git",
            SkillCategory::Database => "database",
            SkillCategory::Security => "security",
            SkillCategory::Documentation => "documentation",
            SkillCategory::Productivity => "productivity",
            SkillCategory::Custom(s) => s.as_str(),
        }
    }
}

/// A parameter that a skill accepts
#[derive(Debug, Clone)]
pub struct SkillParam {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub param_type: SkillParamType,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SkillParamType {
    String,
    Number,
    Boolean,
    Path,
    Choice(Vec<String>),
}

/// Result from executing a skill
#[derive(Debug, Clone)]
pub struct SkillResult {
    pub success: bool,
    pub message: String,
    pub output: Option<String>,
    pub artifacts: Vec<String>,
    pub warnings: Vec<String>,
}

impl SkillResult {
    pub fn ok(msg: &str) -> Self {
        SkillResult {
            success: true,
            message: msg.to_string(),
            output: None,
            artifacts: vec![],
            warnings: vec![],
        }
    }

    pub fn with_output(mut self, output: &str) -> Self {
        self.output = Some(output.to_string());
        self
    }

    pub fn err(msg: &str) -> Self {
        SkillResult {
            success: false,
            message: msg.to_string(),
            output: None,
            artifacts: vec![],
            warnings: vec![],
        }
    }
}

/// Full skill definition from a SKILL.md file or built-in
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: SkillCategory,
    pub params: Vec<SkillParam>,
    pub prompt_template: Option<String>,
    pub source_path: Option<String>,
    pub is_builtin: bool,
    pub required_mcp_plugins: Vec<String>,
    pub tags: Vec<String>,
    pub executor: Option<String>,
}

impl SkillDefinition {
    pub fn new(name: &str, desc: &str, category: SkillCategory) -> Self {
        SkillDefinition {
            name: name.to_string(),
            display_name: name.to_string(),
            description: desc.to_string(),
            category,
            params: vec![],
            prompt_template: None,
            source_path: None,
            is_builtin: false,
            required_mcp_plugins: vec![],
            tags: vec![],
            executor: None,
        }
    }

    pub fn with_param(mut self, name: &str, desc: &str, required: bool) -> Self {
        self.params.push(SkillParam {
            name: name.to_string(),
            description: desc.to_string(),
            required,
            param_type: SkillParamType::String,
            default_value: None,
        });
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }

    pub fn with_mcp_plugin(mut self, plugin: &str) -> Self {
        self.required_mcp_plugins.push(plugin.to_string());
        self
    }
}