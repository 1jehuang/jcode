// ════════════════════════════════════════════════════════════════
// 交互式配置向导 — 引导用户完成初始化配置
//
// 步骤:
//   1. 选择 LLM Provider (OpenAI/Anthropic/Gemini/Qwen/Local)
//   2. 配置 API Key / Endpoint
//   3. 选择默认模型
//   4. 设置权限模式
//   5. 配置工作目录
//   6. (可选) MCP Server 配置
//
// 支持回退、跳过、保存预设
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// 向导步骤 ID
pub type StepId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardStep {
    pub id: StepId,
    pub title: String,
    pub description: String,
    pub step_type: StepType,
    
    /// 用户输入的值
    pub value: Option<WizardValue>,
    
    /// 是否必填
    pub required: bool,
    
    /// 验证函数名 (内置或自定义)
    pub validator: Option<String>,
    
    /// 上一步 ID
    pub prev_step: Option<StepId>,
    
    /// 下一步 ID (条件分支)
    pub next_steps: Vec<(String, StepId)>, // (condition_label, step_id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    SelectOne { options: Vec<SelectOption> },
    TextInput { placeholder: Option<String>, password: bool },
    MultiSelect { options: Vec<SelectOption> },
    Toggle,
    FilePath,
    ConfirmSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WizardValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<String>),
    Null,
}

impl WizardValue {
    pub fn as_str(&self) -> &str {
        match self {
            Self::String(s) => s,
            _ => "",
        }
    }

    pub fn is_set(&self) -> bool {
        !matches!(self, Self::Null)
    }
}

/// 向导执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardResult {
    pub completed: bool,
    pub steps: Vec<WizardStepAnswer>,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardStepAnswer {
    pub step_id: StepId,
    pub value: WizardValue,
    pub skipped: bool,
}

/// 配置向导
pub struct ConfigWizard {
    steps: Vec<WizardStep>,
    current_index: usize,
    answers: HashMap<StepId, WizardValue>,
}

impl Default for ConfigWizard {
    fn default() -> Self { Self::new() }
}

impl ConfigWizard {
    /// 创建标准的 JCode 初始化向导
    pub fn new() -> Self {
        let mut wizard = Self {
            steps: vec![],
            current_index: 0,
            answers: HashMap::new(),
        };

        // Step 1: 欢迎 + 选择 Provider
        wizard.add_select_one(
            "选择 LLM 提供商",
            "选择您的主要 AI 模型提供商。后续可随时在设置中更改。",
            "provider",
            true,
            vec![
                SelectOption { label: "Anthropic (Claude)".into(), value: "anthropic".into(), description: Some("推荐: Claude Sonnet/Opus".into()), default: true },
                SelectOption { label: "OpenAI (GPT-4o)".into(), value: "openai".into(), description: Some("GPT-4o / o1 / o3 系列".into()), default: false },
                SelectOption { label: "Google Gemini".into(), value: "gemini".into(), description: Some("Gemini 2.0 Pro/Flash".into()), default: false },
                SelectOption { label: "阿里通义千问".into(), value: "qwen".into(), description: Some("Qwen-Max/Coder".into()), default: false },
                SelectOption { label: "本地 Ollama".into(), value: "local".into(), description: Some("离线运行, 无需 API Key".into()), default: false },
            ],
        );

        // Step 2: API Key
        wizard.add_text_input(
            "API Key",
            "输入您的 API 密钥（将安全存储，仅本机使用）。",
            "api_key",
            true,
            Some("sk-...".into()),
            false,
        );

        // Step 3: 默认模型
        wizard.add_text_input(
            "默认模型",
            "指定默认使用的模型名称（如 claude-sonnet-4-20250514）。",
            "model",
            true,
            Some("claude-sonnet-4-20250514".into()),
            false,
        );

        // Step 4: 权限模式
        wizard.add_select_one(
            "默认权限模式",
            "选择工具调用的默认审批策略。",
            "permission_mode",
            true,
            vec![
                SelectOption { label: "Default (推荐)".into(), value: "default".into(), description: Some("写操作需要确认".into()), default: true },
                SelectOption { label: "Auto (YOLO)".into(), value: "auto".into(), description: Some("AI 自动判断安全性".into()), default: false },
                SelectOption { label: "Bypass".into(), value: "bypass".into(), description: Some("跳过所有确认（仅限可信环境）".into()), default: false },
            ],
        );

        // Step 5: 工作目录
        wizard.add_text_input(
            "工作目录",
            "项目根目录路径（留空则使用当前目录）。",
            "workspace_path",
            false,
            Some(".".into()),
            false,
        );

        // Step 6: 确认摘要
        wizard.steps.push(WizardStep {
            id: Uuid::new_v4(),
            title: "配置确认".into(),
            description: "请确认以下配置信息。".into(),
            step_type: StepType::ConfirmSummary,
            value: None,
            required: false,
            validator: None,
            prev_step: None,
            next_steps: vec![],
        });

        wizard
    }

    fn add_select_one(&mut self, title: &str, desc: &str, key: &str, required: bool, options: Vec<SelectOption>) {
        self.steps.push(WizardStep {
            id: Uuid::new_v4(),
            title: title.into(),
            description: desc.into(),
            step_type: StepType::SelectOne { options },
            value: None,
            required,
            validator: None,
            prev_step: if self.steps.is_empty() { None } else { Some(self.steps.last().unwrap().id) },
            next_steps: vec![],
        });
    }

    fn add_text_input(&mut self, title: &str, desc: &str, key: &str, required: bool, placeholder: Option<String>, is_password: bool) {
        self.steps.push(WizardStep {
            id: Uuid::new_v4(),
            title: title.into(),
            description: desc.into(),
            step_type: StepType::TextInput { placeholder, password: is_password },
            value: None,
            required,
            validator: None,
            prev_step: if self.steps.is_empty() { None } else { Some(self.steps.last().unwrap().id) },
            next_steps: vec![],
        });
    }

    /// 获取当前步骤
    pub fn current_step(&self) -> Option<&WizardStep> {
        self.steps.get(self.current_index)
    }

    /// 回答当前步骤
    pub fn answer_current(&mut self, value: WizardValue) -> Result<(), String> {
        let step = self.current_step().ok_or("No current step")?;
        
        // 验证必填字段
        if step.required && !value.is_set() {
            return Err(format!("'{}' 是必填项", step.title));
        }

        self.answers.insert(step.id, value);
        Ok(())
    }

    /// 下一步
    pub fn next(&mut self) -> Option<&WizardStep> {
        if self.current_index < self.steps.len().saturating_sub(1) {
            self.current_index += 1;
            self.current_step()
        } else {
            None
        }
    }

    /// 上一步
    pub fn prev(&mut self) -> Option<&WizardStep> {
        if self.current_index > 0 {
            self.current_index -= 1;
            self.current_step()
        } else {
            None
        }
    }

    /// 跳过当前步骤 (非必填时允许)
    pub fn skip_current(&mut self) -> Result<(), String> {
        let step = self.current_step().ok_or("No current step")?;
        if step.required {
            return Err(format!("不能跳过必填步骤 '{}'", step.title));
        }
        self.answers.insert(step.id, WizardValue::Null);
        self.next();
        Ok(())
    }

    /// 完成向导 — 收集所有答案并生成配置 JSON
    pub fn finish(self) -> WizardResult {
        let mut config = serde_json::Map::new();
        let mut answers_vec = Vec::new();

        for step in &self.steps {
            let value = self.answers.get(&step.id)
                .cloned()
                .unwrap_or(WizardValue::Null);

            // 从 step_type 推断配置键名 (简化版)
            let key = self.infer_config_key(step);
            
            match &value {
                WizardValue::String(s) => { config.insert(key.clone(), serde_json::json!(s)); }
                WizardValue::Int(n) => { config.insert(key.clone(), serde_json::json!(n)); }
                WizardValue::Float(f) => { config.insert(key.clone(), serde_json::json!(f)); }
                WizardValue::Bool(b) => { config.insert(key.clone(), serde_json::json!(b)); }
                WizardValue::List(lst) => { config.insert(key.clone(), serde_json::Value::Array(lst.iter().map(|s| serde_json::json!(s)).collect())); }
                WizardValue::Null => { continue; }
            }

            answers_vec.push(WizardStepAnswer {
                step_id: step.id,
                value,
                skipped: matches!(self.answers.get(&step.id), Some(WizardValue::Null)) || !self.answers.contains_key(&step.id),
            });
        }

        WizardResult {
            completed: true,
            steps: answers_vec,
            config: serde_json::Value::Object(config),
        }
    }

    fn infer_config_key(&self, step: &WizardStep) -> String {
        // 简化: 使用步骤标题的小写+下划线格式
        step.title.to_lowercase().replace(' ', "_")
    }

    /// 进度百分比 (0-100)
    pub fn progress_percent(&self) -> f32 {
        if self.steps.is_empty() { return 100.0; }
        ((self.answers.len() as f32 / self.steps.len() as f32) * 100.0).min(100.0)
    }
}
