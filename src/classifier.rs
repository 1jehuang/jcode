use anyhow::{anyhow, Result};
use futures::StreamExt;
use jcode_message_types::Message;
use jcode_provider_core::{EventStream, Provider};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClassificationResult {
    Approved,
    Denied,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationRequest {
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub context: serde_json::Value,
    pub user_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResponse {
    pub result: ClassificationResult,
    pub confidence: f64,
    pub reason: String,
    pub rule_matched: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: RuleCategory,
    pub pattern: String,
    pub action: RuleAction,
    pub priority: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleCategory {
    Allow,
    SoftDeny,
    Environment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleAction {
    Approve,
    Deny,
    RequireConfirmation,
}

#[derive(Clone)]
pub struct LlmClassifier {
    rules: Arc<RwLock<Vec<ClassifierRule>>>,
    allowlisted_tools: Arc<RwLock<HashSet<String>>>,
    provider: Option<Arc<dyn Provider>>,
    cache: Arc<RwLock<HashMap<String, ClassificationResponse>>>,
    cache_ttl_seconds: u64,
}

impl LlmClassifier {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Self::default_rules())),
            allowlisted_tools: Arc::new(RwLock::new(Self::default_allowlisted_tools())),
            provider: None,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_seconds: 300,
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    fn default_rules() -> Vec<ClassifierRule> {
        vec![
            ClassifierRule {
                id: "allow_read_only".to_string(),
                name: "Allow Read-Only Operations".to_string(),
                description: "Allow all read-only file operations".to_string(),
                category: RuleCategory::Allow,
                pattern: r"(?i)(file_read|read_file|grep|glob)".to_string(),
                action: RuleAction::Approve,
                priority: 100,
                enabled: true,
            },
            ClassifierRule {
                id: "deny_network".to_string(),
                name: "Deny Network Access".to_string(),
                description: "Block network requests in auto mode".to_string(),
                category: RuleCategory::SoftDeny,
                pattern: r"(?i)(http|https|fetch|request)".to_string(),
                action: RuleAction::RequireConfirmation,
                priority: 90,
                enabled: true,
            },
            ClassifierRule {
                id: "deny_sensitive".to_string(),
                name: "Deny Sensitive Operations".to_string(),
                description: "Block sensitive system operations".to_string(),
                category: RuleCategory::SoftDeny,
                pattern: r"(?i)(rm|delete|format|shutdown|reboot)".to_string(),
                action: RuleAction::Deny,
                priority: 80,
                enabled: true,
            },
        ]
    }

    fn default_allowlisted_tools() -> HashSet<String> {
        [
            "file_read",
            "read_file",
            "grep",
            "glob",
            "list_files",
            "todo_write",
            "task_list",
            "sleep",
            "tool_search",
            "ask_user",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub async fn classify(&self, request: ClassificationRequest) -> Result<ClassificationResponse> {
        if self.is_allowlisted(&request.tool_name).await {
            return Ok(ClassificationResponse {
                result: ClassificationResult::Approved,
                confidence: 1.0,
                reason: "Tool is allowlisted".to_string(),
                rule_matched: None,
            });
        }

        let cache_key = self.generate_cache_key(&request);
        if let Some(cached) = self.get_cached_response(&cache_key).await {
            return Ok(cached);
        }

        let rule_result = self.match_rules(&request).await;
        if let Some(result) = rule_result {
            self.cache_response(&cache_key, &result).await;
            return Ok(result);
        }

        if let Some(provider) = &self.provider {
            let llm_result = self.classify_with_llm(provider.as_ref(), &request).await?;
            self.cache_response(&cache_key, &llm_result).await;
            return Ok(llm_result);
        }

        Ok(ClassificationResponse {
            result: ClassificationResult::Pending,
            confidence: 0.5,
            reason: "No rules matched and no LLM available".to_string(),
            rule_matched: None,
        })
    }

    async fn is_allowlisted(&self, tool_name: &str) -> bool {
        self.allowlisted_tools.read().await.contains(tool_name)
    }

    async fn match_rules(&self, request: &ClassificationRequest) -> Option<ClassificationResponse> {
        let rules = self.rules.read().await;
        let mut enabled_rules: Vec<_> = rules.iter().filter(|r| r.enabled).collect();
        enabled_rules.sort_by_key(|r| std::cmp::Reverse(r.priority));

        for rule in enabled_rules {
            if self.matches_rule(&rule, request) {
                let result = match rule.action {
                    RuleAction::Approve => ClassificationResult::Approved,
                    RuleAction::Deny => ClassificationResult::Denied,
                    RuleAction::RequireConfirmation => ClassificationResult::Pending,
                };

                return Some(ClassificationResponse {
                    result,
                    confidence: 0.95,
                    reason: rule.description.clone(),
                    rule_matched: Some(rule.id.clone()),
                });
            }
        }

        None
    }

    fn matches_rule(&self, rule: &ClassifierRule, request: &ClassificationRequest) -> bool {
        let pattern = match regex::Regex::new(&rule.pattern) {
            Ok(p) => p,
            Err(_) => return false,
        };

        pattern.is_match(&request.tool_name)
            || pattern.is_match(&request.tool_args.to_string())
            || pattern.is_match(&request.context.to_string())
    }

    async fn classify_with_llm(
        &self,
        provider: &dyn Provider,
        request: &ClassificationRequest,
    ) -> Result<ClassificationResponse> {
        use jcode_message_types::StreamEvent;
        
        let system_prompt = self.build_classifier_system_prompt().await;
        let user_prompt = self.build_classifier_user_prompt(request);

        let messages = vec![Message::user(&user_prompt)];
        let mut stream: EventStream = provider.complete(&messages, &[], &system_prompt, None).await?;
        
        let mut response = String::new();
        while let Some(event) = stream.next().await {
            if let Ok(event) = event {
                match event {
                    StreamEvent::TextDelta(text) => response.push_str(&text),
                    _ => {}
                }
            }
        }

        self.parse_llm_response(&response)
    }

    async fn build_classifier_system_prompt(&self) -> String {
        let rules = self.rules.read().await;
        let allow_rules: Vec<_> = rules
            .iter()
            .filter(|r| r.category == RuleCategory::Allow && r.enabled)
            .collect();
        let deny_rules: Vec<_> = rules
            .iter()
            .filter(|r| r.category != RuleCategory::Allow && r.enabled)
            .collect();

        format!(
            r#"You are an expert classifier for AI agent tool operations.

Your task is to determine whether an action should be:
- APPROVED: Safe to execute automatically
- DENIED: Too dangerous, should be blocked
- PENDING: Needs user confirmation

Rules to consider:

ALLOW RULES:
{}

SOFT DENY RULES:
{}

Format your response as JSON:
{{
  "result": "APPROVED" | "DENIED" | "PENDING",
  "confidence": 0.0-1.0,
  "reason": "brief explanation"
}}"#,
            self.format_rules(allow_rules),
            self.format_rules(deny_rules)
        )
    }

    fn format_rules(&self, rules: Vec<&ClassifierRule>) -> String {
        if rules.is_empty() {
            return "None".to_string();
        }
        rules
            .iter()
            .map(|r| format!("- {}: {}", r.name, r.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn build_classifier_user_prompt(&self, request: &ClassificationRequest) -> String {
        format!(
            r#"Classify the following tool operation:

Tool: {}
Arguments: {}
Context: {}
User Prompt: {}

Provide your classification:"#,
            request.tool_name,
            request.tool_args,
            request.context,
            request.user_prompt.as_deref().unwrap_or("N/A")
        )
    }

    fn parse_llm_response(&self, response: &str) -> Result<ClassificationResponse> {
        let trimmed = response.trim();
        let json_str = if trimmed.starts_with('{') {
            trimmed
        } else if let Some(start) = trimmed.find('{') {
            &trimmed[start..]
        } else {
            return Ok(self.fallback_classification(response));
        };

        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(result) => {
                let result_str = result["result"]
                    .as_str()
                    .unwrap_or("PENDING")
                    .to_uppercase();

                let classification_result = match result_str.as_str() {
                    "APPROVED" => ClassificationResult::Approved,
                    "DENIED" => ClassificationResult::Denied,
                    _ => ClassificationResult::Pending,
                };

                Ok(ClassificationResponse {
                    result: classification_result,
                    confidence: result["confidence"].as_f64().unwrap_or(0.7),
                    reason: result["reason"]
                        .as_str()
                        .unwrap_or("LLM classification")
                        .to_string(),
                    rule_matched: None,
                })
            }
            Err(_) => Ok(self.fallback_classification(response)),
        }
    }

    fn fallback_classification(&self, response: &str) -> ClassificationResponse {
        let upper = response.to_uppercase();
        let result = if upper.contains("APPROVE") || upper.contains("SAFE") {
            ClassificationResult::Approved
        } else if upper.contains("DENY") || upper.contains("BLOCK") || upper.contains("DANGER") {
            ClassificationResult::Denied
        } else {
            ClassificationResult::Pending
        };

        ClassificationResponse {
            result,
            confidence: 0.6,
            reason: format!("Fallback classification: {}", response),
            rule_matched: None,
        }
    }

    fn generate_cache_key(&self, request: &ClassificationRequest) -> String {
        format!(
            "{}-{}-{}",
            request.tool_name,
            request.tool_args,
            request.context
        )
    }

    async fn get_cached_response(&self, key: &str) -> Option<ClassificationResponse> {
        self.cache.read().await.get(key).cloned()
    }

    async fn cache_response(&self, key: &str, response: &ClassificationResponse) {
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), response.clone());
    }

    pub async fn add_rule(&self, rule: ClassifierRule) {
        let mut rules = self.rules.write().await;
        rules.push(rule);
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        self.invalidate_cache().await;
    }

    pub async fn remove_rule(&self, rule_id: &str) -> Result<()> {
        let mut rules = self.rules.write().await;
        let index = rules.iter().position(|r| r.id == rule_id);
        if let Some(index) = index {
            rules.remove(index);
            self.invalidate_cache().await;
            Ok(())
        } else {
            Err(anyhow!("Rule not found"))
        }
    }

    pub async fn add_allowlisted_tool(&self, tool_name: &str) {
        let mut tools = self.allowlisted_tools.write().await;
        tools.insert(tool_name.to_string());
    }

    pub async fn remove_allowlisted_tool(&self, tool_name: &str) {
        let mut tools = self.allowlisted_tools.write().await;
        tools.remove(tool_name);
    }

    async fn invalidate_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    pub async fn get_rules(&self) -> Vec<ClassifierRule> {
        self.rules.read().await.clone()
    }

    pub async fn get_allowlisted_tools(&self) -> HashSet<String> {
        self.allowlisted_tools.read().await.clone()
    }
}