//! Template loading and parsing for session memory.

use serde::{Deserialize, Serialize};

/// A parsed session-memory template containing sections with variable placeholders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Raw template text.
    pub raw: String,
    /// Parsed sections (name -> template body).
    pub sections: Vec<TemplateSection>,
}

/// A single section within a template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    /// Section name.
    pub name: String,
    /// Section body with `{{var}}` placeholders.
    pub body: String,
}

/// Loads templates from the filesystem.
#[derive(Debug, Clone)]
pub struct TemplateLoader {
    /// Base directory for template files.
    /// Defaults to `~/.jcode/session-memory/config/`.
    pub base_dir: String,
}

impl Default for TemplateLoader {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            base_dir: format!("{}/.jcode/session-memory/config", home),
        }
    }
}

impl TemplateLoader {
    /// Create a new loader with the default template directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a loader with a custom base directory.
    pub fn with_base_dir(base_dir: impl Into<String>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Load a template from `template.md` in the base directory.
    ///
    /// Returns `None` if the file does not exist.
    pub fn load_template(&self) -> Option<Template> {
        let path = format!("{}/template.md", self.base_dir);
        let raw = std::fs::read_to_string(&path).ok()?;
        Some(Template::parse(&raw))
    }
}

impl Template {
    /// Parse a raw template string into sections.
    ///
    /// Sections are delimited by `## Section Name` headings.
    pub fn parse(raw: &str) -> Self {
        let mut sections = Vec::new();
        let mut current_name: Option<String> = None;
        let mut current_body = String::new();

        for line in raw.lines() {
            if let Some(name) = line.strip_prefix("## ") {
                // Flush previous section.
                if let Some(prev_name) = current_name.take() {
                    sections.push(TemplateSection {
                        name: prev_name,
                        body: current_body.trim().to_string(),
                    });
                    current_body.clear();
                }
                current_name = Some(name.trim().to_string());
            } else if current_name.is_some() {
                current_body.push_str(line);
                current_body.push('\n');
            }
        }

        // Flush last section.
        if let Some(name) = current_name {
            sections.push(TemplateSection {
                name,
                body: current_body.trim().to_string(),
            });
        }

        Self {
            raw: raw.to_string(),
            sections,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_template_with_sections() {
        let raw =
            "## Summary\nThis is a {{project}} session.\n\n## Details\nWorking on {{feature}}.";
        let tmpl = Template::parse(raw);
        assert_eq!(tmpl.sections.len(), 2);
        assert_eq!(tmpl.sections[0].name, "Summary");
        assert!(tmpl.sections[0].body.contains("{{project}}"));
        assert_eq!(tmpl.sections[1].name, "Details");
    }

    #[test]
    fn template_loader_default_dir() {
        let loader = TemplateLoader::new();
        assert!(loader.base_dir.contains(".jcode/session-memory/config"));
    }
}
