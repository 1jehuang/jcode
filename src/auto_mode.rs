use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ActionType {
    CodeGeneration,
    CodeExecution,
    FileWrite,
    FileRead,
    NetworkRequest,
    ToolCall,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    pub action_type: ActionType,
    pub timestamp: u64,
    pub content_hash: String,
    pub result_hash: String,
    pub success: bool,
    pub execution_time_ms: u64,
}

#[derive(Debug, Clone)]
pub struct LoopDetector {
    recent_actions: VecDeque<ActionRecord>,
    max_history_size: usize,
    loop_threshold: usize,
    similarity_threshold: f64,
    action_type_counts: HashMap<ActionType, usize>,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self {
            recent_actions: VecDeque::with_capacity(100),
            max_history_size: 100,
            loop_threshold: 5,
            similarity_threshold: 0.8,
            action_type_counts: HashMap::new(),
        }
    }

    pub fn with_config(max_history: usize, loop_threshold: usize, similarity_threshold: f64) -> Self {
        Self {
            recent_actions: VecDeque::with_capacity(max_history),
            max_history_size: max_history,
            loop_threshold,
            similarity_threshold,
            action_type_counts: HashMap::new(),
        }
    }

    pub fn record_action(&mut self, action: ActionRecord) {
        self.action_type_counts
            .entry(action.action_type.clone())
            .and_modify(|count| *count += 1)
            .or_insert(1);

        if self.recent_actions.len() >= self.max_history_size {
            if let Some(old_action) = self.recent_actions.pop_front() {
                *self.action_type_counts.entry(old_action.action_type).or_insert(0) -= 1;
            }
        }
        self.recent_actions.push_back(action);
    }

    pub fn detect_loop(&self) -> Option<LoopDetectionResult> {
        if self.recent_actions.len() < self.loop_threshold {
            return None;
        }

        let recent_actions: Vec<&ActionRecord> = self.recent_actions.iter().collect();
        
        for window_size in 2..=self.loop_threshold {
            if let Some(result) = self.detect_pattern_loop(&recent_actions, window_size) {
                return Some(result);
            }
        }

        if let Some(result) = self.detect_repeated_failures() {
            return Some(result);
        }

        if let Some(result) = self.detect_action_type_loop() {
            return Some(result);
        }

        None
    }

    fn detect_pattern_loop(&self, actions: &[&ActionRecord], window_size: usize) -> Option<LoopDetectionResult> {
        if actions.len() < window_size * 2 {
            return None;
        }

        let pattern_start = actions.len() - window_size * 2;
        let pattern = &actions[pattern_start..pattern_start + window_size];
        let candidate = &actions[pattern_start + window_size..];

        let mut match_count = 0;
        for (p, c) in pattern.iter().zip(candidate.iter()) {
            if self.are_similar(p, c) {
                match_count += 1;
            }
        }

        let similarity = match_count as f64 / window_size as f64;
        if similarity >= self.similarity_threshold {
            return Some(LoopDetectionResult {
                loop_type: LoopType::PatternRepetition,
                severity: LoopSeverity::Medium,
                pattern_length: window_size,
                similarity,
                suggestion: "Detected repetitive pattern in actions. Consider breaking the cycle or introducing variation.".to_string(),
            });
        }

        None
    }

    fn detect_repeated_failures(&self) -> Option<LoopDetectionResult> {
        let recent_failures: Vec<&ActionRecord> = self.recent_actions
            .iter()
            .filter(|a| !a.success)
            .collect();

        if recent_failures.len() >= self.loop_threshold {
            return Some(LoopDetectionResult {
                loop_type: LoopType::RepeatedFailures,
                severity: LoopSeverity::High,
                pattern_length: recent_failures.len(),
                similarity: 1.0,
                suggestion: format!(
                    "Detected {} consecutive failures. Consider checking input parameters or stopping the operation.",
                    recent_failures.len()
                ),
            });
        }

        None
    }

    fn detect_action_type_loop(&self) -> Option<LoopDetectionResult> {
        for (action_type, count) in &self.action_type_counts {
            if *count >= self.loop_threshold * 2 {
                let total_actions = self.recent_actions.len();
                let proportion = *count as f64 / total_actions as f64;
                
                if proportion > 0.6 {
                    return Some(LoopDetectionResult {
                        loop_type: LoopType::ActionTypeRepetition,
                        severity: LoopSeverity::Low,
                        pattern_length: *count,
                        similarity: proportion,
                        suggestion: format!(
                            "Over-reliance on {} actions detected. Consider diversifying the approach.",
                            self.action_type_to_string(action_type)
                        ),
                    });
                }
            }
        }

        None
    }

    fn are_similar(&self, a: &ActionRecord, b: &ActionRecord) -> bool {
        if a.action_type != b.action_type {
            return false;
        }
        a.content_hash == b.content_hash || a.result_hash == b.result_hash
    }

    fn action_type_to_string(&self, action_type: &ActionType) -> String {
        match action_type {
            ActionType::CodeGeneration => "code generation".to_string(),
            ActionType::CodeExecution => "code execution".to_string(),
            ActionType::FileWrite => "file write".to_string(),
            ActionType::FileRead => "file read".to_string(),
            ActionType::NetworkRequest => "network request".to_string(),
            ActionType::ToolCall => "tool call".to_string(),
            ActionType::Other(s) => s.clone(),
        }
    }

    pub fn get_stats(&self) -> LoopDetectorStats {
        LoopDetectorStats {
            total_actions: self.recent_actions.len(),
            action_type_counts: self.action_type_counts.clone(),
            success_rate: {
                let success_count = self.recent_actions.iter().filter(|a| a.success).count();
                if self.recent_actions.is_empty() {
                    0.0
                } else {
                    success_count as f64 / self.recent_actions.len() as f64
                }
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopDetectionResult {
    pub loop_type: LoopType,
    pub severity: LoopSeverity,
    pub pattern_length: usize,
    pub similarity: f64,
    pub suggestion: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopType {
    PatternRepetition,
    RepeatedFailures,
    ActionTypeRepetition,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoopSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct LoopDetectorStats {
    pub total_actions: usize,
    pub action_type_counts: HashMap<ActionType, usize>,
    pub success_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<String>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub severity: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

pub struct AutoValidator {
    enabled: bool,
    validation_rules: Vec<ValidationRule>,
}

impl AutoValidator {
    pub fn new() -> Self {
        Self {
            enabled: true,
            validation_rules: vec![
                ValidationRule {
                    name: "code_syntax".to_string(),
                    description: "Check code syntax".to_string(),
                    validator: Box::new(code_syntax_validator),
                },
                ValidationRule {
                    name: "security_basic".to_string(),
                    description: "Basic security checks".to_string(),
                    validator: Box::new(basic_security_validator),
                },
                ValidationRule {
                    name: "dependency_check".to_string(),
                    description: "Check for missing dependencies".to_string(),
                    validator: Box::new(dependency_validator),
                },
                ValidationRule {
                    name: "output_sanity".to_string(),
                    description: "Check output sanity".to_string(),
                    validator: Box::new(output_sanity_validator),
                },
            ],
        }
    }

    pub fn validate(&self, code: &str, language: &str) -> ValidationResult {
        if !self.enabled {
            return ValidationResult {
                valid: true,
                errors: Vec::new(),
                warnings: Vec::new(),
                suggestions: Vec::new(),
            };
        }

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut suggestions = Vec::new();

        for rule in &self.validation_rules {
            let result = (rule.validator)(code, language);
            errors.extend(result.errors);
            warnings.extend(result.warnings);
            suggestions.extend(result.suggestions);
        }

        ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings,
            suggestions,
        }
    }

    pub fn validate_execution(&self, output: &str, expected_patterns: &[&str]) -> ValidationResult {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut suggestions = Vec::new();

        if output.contains("error") || output.contains("Error") || output.contains("ERROR") {
            errors.push(ValidationError {
                code: "execution_error".to_string(),
                message: "Execution produced error output".to_string(),
                severity: "high".to_string(),
                line: None,
                column: None,
            });
        }

        if output.is_empty() {
            warnings.push("Execution produced no output".to_string());
            suggestions.push("Consider adding print statements or verifying the code logic".to_string());
        }

        for pattern in expected_patterns {
            if !output.contains(pattern) {
                warnings.push(format!("Expected output pattern not found: {}", pattern));
            }
        }

        ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings,
            suggestions,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn add_rule(&mut self, name: &str, description: &str, validator: ValidatorFn) {
        self.validation_rules.push(ValidationRule {
            name: name.to_string(),
            description: description.to_string(),
            validator,
        });
    }
}

pub type ValidatorFn = Box<dyn Fn(&str, &str) -> ValidationResult + Sync + Send + 'static>;

pub struct ValidationRule {
    pub name: String,
    pub description: String,
    pub validator: ValidatorFn,
}

fn code_syntax_validator(code: &str, language: &str) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();

    let lines: Vec<&str> = code.lines().collect();
    
    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        if language.to_lowercase() == "rust" {
            if line.contains("unsafe ") && !line.starts_with("//") {
                warnings.push(format!("Line {}: Usage of unsafe block detected", line_num));
                suggestions.push("Consider using safe alternatives where possible".to_string());
            }
            
            if line.contains("unwrap()") && !line.starts_with("//") {
                warnings.push(format!("Line {}: Usage of unwrap() detected", line_num));
                suggestions.push("Consider using expect() with meaningful error message or proper error handling".to_string());
            }
        }

        if language.to_lowercase() == "python" {
            if line.contains("eval(") && !line.starts_with("#") {
                errors.push(ValidationError {
                    code: "unsafe_eval".to_string(),
                    message: "Usage of eval() detected which can be unsafe".to_string(),
                    severity: "high".to_string(),
                    line: Some(line_num),
                    column: None,
                });
            }
        }

        if line.trim().starts_with("print(") && language.to_lowercase() != "python" {
            warnings.push(format!("Line {}: print statement may not be valid in this language", line_num));
        }
    }

    if code.len() > 10000 {
        warnings.push("Code is quite large, consider breaking it into smaller functions".to_string());
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        suggestions,
    }
}

fn basic_security_validator(code: &str, _language: &str) -> ValidationResult {
    let errors = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();

    let security_patterns = [
        ("password", "hardcoded_password", "High"),
        ("secret", "hardcoded_secret", "High"),
        ("api_key", "hardcoded_api_key", "High"),
        ("token", "hardcoded_token", "Medium"),
        ("ssh_key", "hardcoded_ssh_key", "High"),
        ("private_key", "hardcoded_private_key", "High"),
    ];

    let lines: Vec<&str> = code.lines().collect();
    
    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        
        for (pattern, _code, _severity) in security_patterns.iter() {
            if line.to_lowercase().contains(pattern) 
                && !line.starts_with("//") 
                && !line.starts_with("#")
                && !line.contains("=") && !line.contains(":") {
                warnings.push(format!("Line {}: Potential {} detected", line_num, pattern));
                suggestions.push("Consider using environment variables or secure vaults for sensitive data".to_string());
            }
        }
    }

    if code.contains("system(") || code.contains("exec(") || code.contains("spawn(") {
        warnings.push("System command execution detected".to_string());
        suggestions.push("Ensure command inputs are properly sanitized".to_string());
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        suggestions,
    }
}

fn dependency_validator(code: &str, language: &str) -> ValidationResult {
    let errors = Vec::new();
    let warnings = Vec::new();
    let mut suggestions = Vec::new();

    let rust_common_deps = ["std", "serde", "tokio", "anyhow", "log"];
    let python_common_deps = ["os", "sys", "json", "requests", "datetime"];

    let detected_deps: HashSet<String> = match language.to_lowercase().as_str() {
        "rust" => code.split_whitespace()
            .filter(|w| w.starts_with("use ") || w.starts_with("extern crate "))
            .map(|w| w.replace("use ", "").replace("extern crate ", "").replace(';', ""))
            .collect(),
        "python" => code.split_whitespace()
            .filter(|w| w.starts_with("import ") || w.starts_with("from "))
            .map(|w| {
                let parts: Vec<&str> = w.split(|c| c == ' ' || c == '.').collect();
                if parts.len() >= 2 { parts[1].to_string() } else { "".to_string() }
            })
            .filter(|s| !s.is_empty())
            .collect(),
        _ => HashSet::new(),
    };

    let common_deps = if language.to_lowercase() == "rust" { rust_common_deps } else { python_common_deps };
    
    for dep in common_deps {
        if !detected_deps.contains(dep) {
            suggestions.push(format!("Consider using {} crate/module", dep));
        }
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        suggestions,
    }
}

fn output_sanity_validator(code: &str, _language: &str) -> ValidationResult {
    let errors = Vec::new();
    let mut warnings = Vec::new();
    let mut suggestions = Vec::new();

    let lines: Vec<&str> = code.lines().collect();
    let line_count = lines.len();

    let empty_lines: usize = lines.iter().filter(|l| l.trim().is_empty()).count();
    if empty_lines > line_count / 2 {
        warnings.push("Code contains many empty lines".to_string());
    }

    let avg_line_length: f64 = lines.iter().map(|l| l.len()).sum::<usize>() as f64 / line_count as f64;
    if avg_line_length > 120.0 {
        warnings.push("Average line length exceeds recommended 120 characters".to_string());
        suggestions.push("Consider breaking long lines for better readability".to_string());
    }

    let comment_ratio = if line_count > 0 {
        lines.iter().filter(|l| l.trim().starts_with("//") || l.trim().starts_with("#")).count() as f64 / line_count as f64
    } else {
        0.0
    };
    
    if comment_ratio == 0.0 && line_count > 50 {
        warnings.push("No comments found in code".to_string());
        suggestions.push("Consider adding comments for better maintainability".to_string());
    }

    if comment_ratio > 0.5 {
        warnings.push("Code contains many comments relative to code".to_string());
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        suggestions,
    }
}

pub struct AutoMode {
    loop_detector: LoopDetector,
    validator: AutoValidator,
    enabled: bool,
    auto_approve_threshold: f64,
}

impl AutoMode {
    pub fn new() -> Self {
        Self {
            loop_detector: LoopDetector::new(),
            validator: AutoValidator::new(),
            enabled: false,
            auto_approve_threshold: 0.9,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
        self.validator.set_enabled(true);
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.validator.set_enabled(false);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn record_action(&mut self, action_type: ActionType, content: &str, result: &str, success: bool, execution_time_ms: u64) {
        let action = ActionRecord {
            action_type,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            content_hash: format!("{:x}", md5::compute(content)),
            result_hash: format!("{:x}", md5::compute(result)),
            success,
            execution_time_ms,
        };
        self.loop_detector.record_action(action);
    }

    pub fn check_loop(&self) -> Option<LoopDetectionResult> {
        if !self.enabled {
            return None;
        }
        self.loop_detector.detect_loop()
    }

    pub fn validate_code(&self, code: &str, language: &str) -> ValidationResult {
        self.validator.validate(code, language)
    }

    pub fn validate_execution(&self, output: &str, expected_patterns: &[&str]) -> ValidationResult {
        self.validator.validate_execution(output, expected_patterns)
    }

    pub fn get_stats(&self) -> AutoModeStats {
        AutoModeStats {
            enabled: self.enabled,
            loop_detector_stats: self.loop_detector.get_stats(),
            auto_approve_threshold: self.auto_approve_threshold,
        }
    }

    pub fn should_auto_approve(&self, validation_result: &ValidationResult) -> bool {
        if !self.enabled {
            return false;
        }
        validation_result.valid && validation_result.warnings.len() < 3
    }
}

#[derive(Debug, Clone)]
pub struct AutoModeStats {
    pub enabled: bool,
    pub loop_detector_stats: LoopDetectorStats,
    pub auto_approve_threshold: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_detection_pattern() {
        let mut detector = LoopDetector::new();
        
        for i in 0..6 {
            detector.record_action(ActionRecord {
                action_type: ActionType::CodeGeneration,
                timestamp: i as u64,
                content_hash: if i % 2 == 0 { "hash_a".to_string() } else { "hash_b".to_string() },
                result_hash: "result".to_string(),
                success: true,
                execution_time_ms: 100,
            });
        }

        let result = detector.detect_loop();
        assert!(result.is_some());
    }

    #[test]
    fn test_loop_detection_failures() {
        let mut detector = LoopDetector::new();
        
        for i in 0..6 {
            detector.record_action(ActionRecord {
                action_type: ActionType::CodeExecution,
                timestamp: i as u64,
                content_hash: "hash".to_string(),
                result_hash: "result".to_string(),
                success: false,
                execution_time_ms: 100,
            });
        }

        let result = detector.detect_loop();
        assert!(result.is_some());
        assert!(matches!(result.unwrap().loop_type, LoopType::RepeatedFailures));
    }

    #[test]
    fn test_validation_code_syntax() {
        let validator = AutoValidator::new();
        let code = "fn main() {\n    let x = 5.unwrap();\n}";
        let result = validator.validate(code, "rust");
        
        assert!(result.valid);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_validation_security() {
        let validator = AutoValidator::new();
        let code = "let password = \"secret123\";";
        let result = validator.validate(code, "rust");
        
        assert!(result.valid);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_auto_mode_should_approve() {
        let mut auto_mode = AutoMode::new();
        auto_mode.enable();
        
        let validation_result = ValidationResult {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            suggestions: Vec::new(),
        };
        
        assert!(auto_mode.should_auto_approve(&validation_result));
    }
}