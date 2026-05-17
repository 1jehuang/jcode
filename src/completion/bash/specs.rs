//! # 命令规格系统
//!
//! 提供命令规格的定义、验证和序列化功能：
//! - **规格定义** - 命令、子命令、参数的完整结构
//! - **规格验证** - 确保规格的完整性和一致性
//! - **规格序列化** - 支持多种格式的导入导出

use crate::completion::bash::{
    CommandSpec, SubcommandSpec, OptionSpec, CommandCategory,
};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SpecError {
    #[error("Missing required field: {field}")]
    MissingField { field: String },

    #[error("Invalid argument type: {reason}")]
    InvalidArgType { reason: String },

    #[error("Duplicate subcommand: {name}")]
    DuplicateSubcommand { name: String },
}

/// 命令规格构建器
#[derive(Debug, Clone)]
pub struct SpecBuilder {
    name: String,
    description: String,
    long_description: Option<String>,
    subcommands: HashMap<String, SubcommandSpec>,
    global_options: Vec<OptionSpec>,
    category: CommandCategory,
    popularity_weight: u8,
}

impl SpecBuilder {
    fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            long_description: None,
            subcommands: HashMap::new(),
            global_options: Vec::new(),
            category: CommandCategory::Other,
            popularity_weight: 50,
        }
    }

    fn long_description(mut self, desc: impl Into<String>) -> Self {
        self.long_description = Some(desc.into());
        self
    }

    fn category(mut self, cat: CommandCategory) -> Self {
        self.category = cat;
        self
    }

    fn popularity(mut self, weight: u8) -> Self {
        self.popularity_weight = weight;
        self
    }

    fn add_subcommand(mut self, spec: SubcommandSpec) -> Result<Self, SpecError> {
        if self.subcommands.contains_key(&spec.name) {
            return Err(SpecError::DuplicateSubcommand { name: spec.name });
        }
        self.subcommands.insert(spec.name.clone(), spec);
        Ok(self)
    }

    fn add_option(mut self, opt: OptionSpec) -> Self {
        self.global_options.push(opt);
        self
    }

    fn build(self) -> CommandSpec {
        let subcmds = if self.subcommands.is_empty() {
            None
        } else {
            Some(self.subcommands)
        };
        CommandSpec {
            name: self.name,
            description: self.description,
            long_description: self.long_description,
            subcommands: subcmds,
            global_options: self.global_options,
            category: self.category,
            popularity_weight: self.popularity_weight,
        }
    }
}

/// 规格验证器
pub struct SpecValidator;

impl SpecValidator {
    fn validate(spec: &CommandSpec) -> Result<(), Vec<SpecError>> {
        let mut errors = Vec::new();

        if spec.name.is_empty() {
            errors.push(SpecError::MissingField { field: "name".into() });
        }
        if spec.description.is_empty() {
            errors.push(SpecError::MissingField { field: "description".into() });
        }

        if let Some(ref subs) = spec.subcommands {
            for (name, sub) in subs {
                if sub.name != *name {
                    errors.push(SpecError::InvalidArgType {
                        reason: format!("subcommand key '{}' doesn't match name '{}'", name, sub.name),
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_builder_basic() {
        let spec = SpecBuilder::new("test-cmd", "A test command")
            .category(CommandCategory::DevelopmentTools)
            .popularity(80)
            .build();

        assert_eq!(spec.name, "test-cmd");
        assert_eq!(spec.popularity_weight, 80);
    }

    #[test]
    fn test_spec_validation_empty_name() {
        let spec = CommandSpec {
            name: String::new(),
            description: "test".into(),
            long_description: None,
            subcommands: None,
            global_options: vec![],
            category: CommandCategory::Other,
            popularity_weight: 50,
        };

        let result = SpecValidator::validate(&spec);
        assert!(result.is_err());
    }
}
