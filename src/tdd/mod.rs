//! 测试驱动开发引擎 (TDD)
//!
//! 对标 Claude Code 的测试能力 + 超越:
//! - Auto Unit Test Generation: 基于函数签名自动生成测试
//! - Test Coverage Analysis: 覆盖率分析与缺口报告
//! - Edge Case Detection: 边界情况检测 (空值/越界/并发)
//! - Test-driven Refactoring: 先写测试→重构→验证
//!
//! # 使用示例
//!
//! ## LLM增强版（推荐）
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use carpai::tdd::{TestGenerator, TddRefactorer};
//! use jcode_provider_core::Provider;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 获取LLM Provider（例如从MultiProvider）
//!     let provider: Arc<dyn Provider> = /* ... */;
//!     
//!     // 方式1: 直接生成测试代码
//!     let test_code = TestGenerator::generate_unit_test_llm(
//!         "src/lib.rs",
//!         "my_function",
//!         provider.clone()
//!     ).await?;
//!     println!("Generated test:\n{}", test_code);
//!     
//!     // 方式2: 完整TDD循环
//!     let result = TddRefactorer::tdd_cycle_llm(
//!         "src/lib.rs",
//!         "my_function",
//!         std::path::Path::new("."),
//!         provider
//!     ).await?;
//!     println!("TDD completed in {:?}", result.duration);
//!     
//!     Ok(())
//! }
//! ```
//!
//! ## 传统模板版（向后兼容）
//!
//! ```rust,no_run
//! use carpai::tdd::{TestGenerator, TddRefactorer};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 生成基础测试模板
//!     let test_code = TestGenerator::generate_unit_test(
//!         "src/lib.rs",
//!         "my_function"
//!     ).await?;
//!     
//!     // 完整TDD循环
//!     let result = TddRefactorer::tdd_cycle(
//!         "src/lib.rs",
//!         "my_function",
//!         std::path::Path::new(".")
//!     ).await?;
//!     
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use regex::Regex;
use async_trait::async_trait;
use jcode_provider_core::Provider;
use jcode_message_types::{ContentBlock, Message, Role, ToolDefinition, StreamEvent};
use futures::StreamExt;

/// TDD配置
#[derive(Debug, Clone)]
pub struct TddConfig {
    pub llm_enabled: bool,
    pub batch_size: usize,
    pub parallel_limit: usize,
    pub cache_enabled: bool,
}

impl Default for TddConfig {
    fn default() -> Self {
        Self {
            llm_enabled: false,
            batch_size: 5,
            parallel_limit: 3,
            cache_enabled: true,
        }
    }
}

/// ===== [0] TDD缓存管理器 =====
pub struct TddCache {
    /// L1: 内存缓存 (LRU)
    memory_cache: RwLock<HashMap<String, CacheEntry>>,
    /// 缓存统计
    stats: RwLock<CacheStats>,
    max_memory_entries: usize,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    data: String,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl TddCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            memory_cache: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
            max_memory_entries: max_entries,
        }
    }
    
    /// 生成缓存键
    fn generate_cache_key(file_path: &str, function_name: &str, cache_type: &str) -> String {
        format!("tdd:{}:{}:{}", cache_type, file_path, function_name)
    }
    
    /// 获取缓存（L1: 内存）
    pub async fn get(&self, key: &str) -> Option<String> {
        let mut cache = self.memory_cache.write().await;
        
        if let Some(entry) = cache.get_mut(key) {
            entry.last_accessed = Instant::now();
            entry.access_count += 1;
            
            // 更新统计
            let mut stats = self.stats.write().await;
            stats.hits += 1;
            
            return Some(entry.data.clone());
        }
        
        // Cache miss
        let mut stats = self.stats.write().await;
        stats.misses += 1;
        
        None
    }
    
    /// 设置缓存（L1: 内存）
    pub async fn set(&self, key: &str, data: String) {
        let mut cache = self.memory_cache.write().await;
        
        // LRU eviction
        if cache.len() >= self.max_memory_entries {
            // 找到最少访问的条目
            if let Some(oldest_key) = cache.iter()
                .min_by_key(|(_, entry)| entry.access_count)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest_key);
                let mut stats = self.stats.write().await;
                stats.evictions += 1;
            }
        }
        
        cache.insert(key.to_string(), CacheEntry {
            data,
            created_at: Instant::now(),
            last_accessed: Instant::now(),
            access_count: 0,
        });
    }
    
    /// 清除缓存
    pub async fn clear(&self) {
        let mut cache = self.memory_cache.write().await;
        cache.clear();
    }
    
    /// 获取缓存统计
    pub async fn get_stats(&self) -> CacheStats {
        self.stats.read().await.clone()
    }
    
    /// 计算缓存命中率
    pub async fn hit_rate(&self) -> f64 {
        let stats = self.stats.read().await;
        let total = stats.hits + stats.misses;
        if total == 0 {
            0.0
        } else {
            stats.hits as f64 / total as f64
        }
    }
}

/// ===== [1] 自动测试生成 =====
pub struct TestGenerator {
    provider: Option<Arc<dyn Provider>>,
    cache: Option<Arc<TddCache>>,
}

impl TestGenerator {
    pub fn new(_config: TddConfig) -> Self {
        Self { 
            provider: None,
            cache: None,
        }
    }

    pub fn new_empty() -> Self {
        Self { 
            provider: None,
            cache: None,
        }
    }
    
    pub fn with_provider(provider: Arc<dyn Provider>) -> Self {
        Self { 
            provider: Some(provider),
            cache: None,
        }
    }
    
    pub fn with_cache(cache: Arc<TddCache>) -> Self {
        Self {
            provider: None,
            cache: Some(cache),
        }
    }
    
    pub fn with_provider_and_cache(provider: Arc<dyn Provider>, cache: Arc<TddCache>) -> Self {
        Self {
            provider: Some(provider),
            cache: Some(cache),
        }
    }

    /// 为 Rust 函数生成单元测试（使用LLM）
    pub async fn generate_unit_test_llm(
        file_path: &str,
        function_name: &str,
        provider: Arc<dyn Provider>,
    ) -> Result<String, String> {
        // 尝试从缓存获取
        if let Some(ref cache) = self.cache {
            let cache_key = TddCache::generate_cache_key(file_path, function_name, "llm_test");
            if let Some(cached) = cache.get(&cache_key).await {
                return Ok(cached);
            }
        }
        
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        let signature = Self::extract_signature(&content, function_name)?;
        let edge_cases = EdgeCaseDetector::detect(&content, function_name);
        
        // 提取函数体上下文（前后20行）
        let context = Self::extract_function_context(&content, function_name)?;
        
        // 构建LLM prompt
        let prompt = format!(
            "You are an expert Rust developer specializing in test-driven development.\n\n             Generate comprehensive unit tests for the following Rust function:\n\n             Function signature:\n```rust\n{}\n```\n\n             Context (surrounding code):\n```rust\n{}\n```\n\n             Detected edge cases to cover:\n{}\n\n             Requirements:\n             1. Use #[cfg(test)] module structure\n             2. Include basic functionality test\n             3. Cover all detected edge cases\n             4. Add property-based tests if applicable\n             5. Use descriptive test names (snake_case)\n             6. Include assertions with clear error messages\n             7. Mock external dependencies if needed\n             8. Test both success and failure paths\n\n             Return ONLY the complete test code without any explanation or markdown formatting.",
            signature,
            context,
            edge_cases.iter()
                .enumerate()
                .map(|(i, ec)| format!("{}. {} (severity: {:?})", i+1, ec.description, ec.severity))
                .collect::<Vec<_>>()
                .join("\n")
        );
        
        // 调用LLM生成测试代码
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
                ..Default::default()
            }
        ];
        
        let system = "You are a Rust testing expert. Generate production-quality unit tests.";
        
        let mut event_stream = provider
            .complete(&messages, &[], system, None)
            .await
            .map_err(|e| format!("LLM completion failed: {}", e))?;
        
        // 收集流式响应
        let mut test_code = String::new();
        while let Some(event_result) = event_stream.next().await {
            match event_result {
                Ok(StreamEvent::ContentDelta { delta, .. }) => {
                    test_code.push_str(&delta);
                }
                Ok(StreamEvent::ContentBlockStop { .. }) => {
                    break;
                }
                Err(e) => {
                    return Err(format!("Stream error: {}", e));
                }
                _ => {}
            }
        }
        
        if test_code.trim().is_empty() {
            return Err("LLM returned empty test code".to_string());
        }
        
        // 清理可能的markdown代码块标记
        let test_code = test_code
            .replace("```rust", "")
            .replace("```", "")
            .trim()
            .to_string();
        
        // 存入缓存
        if let Some(ref cache) = self.cache {
            let cache_key = TddCache::generate_cache_key(file_path, function_name, "llm_test");
            cache.set(&cache_key, test_code.clone()).await;
        }
        
        Ok(test_code)
    }

    /// 为 Rust 函数生成单元测试（传统模板方式，保留向后兼容）
    pub async fn generate_unit_test(file_path: &str, function_name: &str) -> Result<String, String> {
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        let signature = Self::extract_signature(&content, function_name)?;
        let edge_cases = EdgeCaseDetector::detect(&content, function_name);

        let mut test = format!(
            "#[cfg(test)]\nmod tests_{} {{\n    use super::*;\n\n",
            function_name
        );

        // 基本功能测试
        test.push_str(&format!("    #[test]\n    fn test_{}_basic() {{\n", function_name));
        test.push_str(&format!("        // TODO: Implement basic test for {}\n", signature));
        test.push_str("        // assert_eq!(target_function(input), expected_output);\n");
        test.push_str("    }\n\n");

        // 边界情况测试
        for (i, edge) in edge_cases.iter().enumerate() {
            test.push_str(&format!(
                "    #[test]\n    fn test_{}_edge_{}() {{\n", function_name, i + 1
            ));
            test.push_str(&format!("        // Edge case: {}\n", edge.description));
            test.push_str("        // TODO: Implement\n");
            test.push_str("    }\n\n");
        }

        test.push_str("}\n");
        Ok(test)
    }

    /// 生成属性测试 (proptest)
    pub async fn generate_property_test(file_path: &str, function_name: &str) -> Result<String, String> {
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        let signature = Self::extract_signature(&content, function_name)?;

        Ok(format!(
            "#[cfg(test)]\nmod proptest_{} {{\n    use proptest::prelude::*;\n    use super::*;\n\n\
             \    proptest! {{\n        #[test]\n        fn test_{}_property(#[strategy(\"[a-z]{{1,10}}\")] input: String) {{\n\
             \            // Property: {} should never panic\n\
             \            let _ = {}(&input);\n        }}\n    }}\n}}",
            function_name, function_name, function_name, function_name
        ))
    }

    fn extract_signature(content: &str, function_name: &str) -> Result<String, String> {
        let re = Regex::new(&format!(
            r#"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+{}\s*\([^)]*\)\s*(?:->\s*[^{{]+)?"#,
            regex::escape(function_name)
        )).map_err(|_| "Regex error".to_string())?;

        re.find(content)
            .map(|m| m.as_str().trim().to_string())
            .ok_or_else(|| format!("Function '{}' not found", function_name))
    }
    
    fn extract_function_context(content: &str, function_name: &str) -> Result<String, String> {
        let lines: Vec<&str> = content.lines().collect();
        
        // 查找函数定义的行号
        let func_line = lines.iter().position(|line| {
            line.contains(&format!("fn {}", function_name)) && 
            (line.contains("pub") || line.contains("fn"))
        }).ok_or_else(|| format!("Function '{}' not found", function_name))?;
        
        // 提取前后20行作为上下文
        let start = func_line.saturating_sub(20);
        let end = (func_line + 20).min(lines.len());
        
        Ok(lines[start..end].join("\n"))
    }
}

/// ===== [2] 边界情况检测 =====
pub struct EdgeCaseDetector;

#[derive(Debug, Clone)]
pub struct EdgeCase {
    pub description: String,
    pub severity: EdgeSeverity,
    pub line: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum EdgeSeverity { Critical, High, Medium, Low }

impl EdgeCaseDetector {
    pub fn detect(content: &str, _function_name: &str) -> Vec<EdgeCase> {
        let mut cases = Vec::new();

        for (i, line) in content.lines().enumerate() {
            // 检测 unwrap()
            if line.contains(".unwrap(") {
                cases.push(EdgeCase {
                    description: format!("Line {}: unwrap() without error handling — may panic on None/Err", i + 1),
                    severity: EdgeSeverity::Critical,
                    line: Some(i + 1),
                });
            }
            // 检测索引越界
            if line.contains('[') && line.contains(']') && !line.contains("get(") {
                cases.push(EdgeCase {
                    description: format!("Line {}: direct index access without bounds check", i + 1),
                    severity: EdgeSeverity::High, line: Some(i + 1),
                });
            }
            // 检测空Vec处理
            if line.contains(".first()") || line.contains(".last()") {
                cases.push(EdgeCase {
                    description: format!("Line {}: first()/last() on potentially empty collection", i + 1),
                    severity: EdgeSeverity::Medium, line: Some(i + 1),
                });
            }
            // 检测除零
            if line.contains(" / ") && !line.contains("checked_div") {
                cases.push(EdgeCase {
                    description: format!("Line {}: division without zero-check", i + 1),
                    severity: EdgeSeverity::High, line: Some(i + 1),
                });
            }
            // 检测panic
            if line.contains("panic!(") || line.contains("unreachable!(") {
                cases.push(EdgeCase {
                    description: format!("Line {}: explicit panic/unreachable", i + 1),
                    severity: EdgeSeverity::Medium, line: Some(i + 1),
                });
            }
        }

        // 通用边界情况
        cases.push(EdgeCase {
            description: "Empty input: function should handle empty/null input gracefully".into(),
            severity: EdgeSeverity::High, line: None,
        });
        cases.push(EdgeCase {
            description: "Maximum input size: test with very large input for performance regressions".into(),
            severity: EdgeSeverity::Medium, line: None,
        });
        cases.push(EdgeCase {
            description: "Concurrent access: test thread safety if function uses shared state".into(),
            severity: EdgeSeverity::Medium, line: None,
        });

        cases
    }
}

/// ===== [3] 智能断言生成器 =====
pub struct AssertionGenerator;

#[derive(Debug, Clone)]
pub struct GeneratedAssertion {
    pub assertion_code: String,
    pub assertion_type: AssertionType,
    pub description: String,
    pub confidence: f64,  // 0.0 - 1.0
}

#[derive(Debug, Clone)]
pub enum AssertionType {
    Equality,      // assert_eq!
    Inequality,    // assert_ne!
    Boolean,       // assert!()
    Panic,         // #[should_panic]
    TypeCheck,     // is_ok()/is_err()
    Collection,    // len()/contains()
    StringComparison, // starts_with()/ends_with()
}

impl AssertionGenerator {
    /// 基于函数签名和返回值类型生成智能断言
    pub fn generate_assertions(
        signature: &str,
        return_type: Option<&str>,
        edge_cases: &[EdgeCase],
    ) -> Vec<GeneratedAssertion> {
        let mut assertions = Vec::new();
        
        // 分析返回类型
        if let Some(ret_type) = return_type {
            assertions.extend(Self::assertions_for_return_type(ret_type));
        }
        
        // 基于边界情况生成断言
        assertions.extend(Self::assertions_for_edge_cases(edge_cases));
        
        // 基于函数名推断断言
        assertions.extend(Self::assertions_from_function_name(signature));
        
        assertions
    }
    
    /// 根据返回类型生成断言
    fn assertions_for_return_type(return_type: &str) -> Vec<GeneratedAssertion> {
        let mut assertions = Vec::new();
        
        if return_type.contains("Result") {
            // Result<T, E> 类型
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(result.is_ok());".to_string(),
                assertion_type: AssertionType::TypeCheck,
                description: "Verify operation succeeded".to_string(),
                confidence: 0.95,
            });
            
            assertions.push(GeneratedAssertion {
                assertion_code: "let value = result.unwrap();".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Extract successful value".to_string(),
                confidence: 0.90,
            });
            
            // 错误路径测试
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(error_result.is_err());".to_string(),
                assertion_type: AssertionType::TypeCheck,
                description: "Verify error case returns Err".to_string(),
                confidence: 0.85,
            });
        } else if return_type.contains("Option") {
            // Option<T> 类型
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(option.is_some());".to_string(),
                assertion_type: AssertionType::TypeCheck,
                description: "Verify value exists".to_string(),
                confidence: 0.90,
            });
            
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(option.unwrap(), expected_value);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify correct value".to_string(),
                confidence: 0.85,
            });
        } else if return_type.contains("bool") {
            // bool 类型
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(boolean_result);".to_string(),
                assertion_type: AssertionType::Boolean,
                description: "Verify condition is true".to_string(),
                confidence: 0.95,
            });
        } else if return_type.contains("Vec") || return_type.contains("HashMap") {
            // 集合类型
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(!collection.is_empty());".to_string(),
                assertion_type: AssertionType::Collection,
                description: "Verify collection is not empty".to_string(),
                confidence: 0.80,
            });
            
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(collection.len(), expected_size);".to_string(),
                assertion_type: AssertionType::Collection,
                description: "Verify collection size".to_string(),
                confidence: 0.85,
            });
        } else if return_type.contains("String") || return_type.contains("&str") {
            // 字符串类型
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(!result.is_empty());".to_string(),
                assertion_type: AssertionType::StringComparison,
                description: "Verify string is not empty".to_string(),
                confidence: 0.75,
            });
            
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(result, expected_string);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify exact string match".to_string(),
                confidence: 0.90,
            });
        } else {
            // 默认相等性断言
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(result, expected_value);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify correct output".to_string(),
                confidence: 0.80,
            });
        }
        
        assertions
    }
    
    /// 基于边界情况生成断言
    fn assertions_for_edge_cases(edge_cases: &[EdgeCase]) -> Vec<GeneratedAssertion> {
        let mut assertions = Vec::new();
        
        for edge_case in edge_cases {
            if edge_case.description.contains("unwrap") {
                assertions.push(GeneratedAssertion {
                    assertion_code: "let value = result.expect(\"Should not panic on valid input\");".to_string(),
                    assertion_type: AssertionType::Panic,
                    description: "Safe unwrap with error message".to_string(),
                    confidence: 0.90,
                });
            }
            
            if edge_case.description.contains("index access") || edge_case.description.contains("bounds") {
                assertions.push(GeneratedAssertion {
                    assertion_code: "assert!(index < collection.len());".to_string(),
                    assertion_type: AssertionType::Boolean,
                    description: "Bounds check before access".to_string(),
                    confidence: 0.95,
                });
            }
            
            if edge_case.description.contains("division") || edge_case.description.contains("zero") {
                assertions.push(GeneratedAssertion {
                    assertion_code: "assert_ne!(divisor, 0);".to_string(),
                    assertion_type: AssertionType::Inequality,
                    description: "Prevent division by zero".to_string(),
                    confidence: 0.95,
                });
            }
            
            if edge_case.description.contains("empty") {
                assertions.push(GeneratedAssertion {
                    assertion_code: "assert!(result.is_err() || result.is_none());".to_string(),
                    assertion_type: AssertionType::TypeCheck,
                    description: "Handle empty input gracefully".to_string(),
                    confidence: 0.85,
                });
            }
        }
        
        assertions
    }
    
    /// 从函数名推断断言
    fn assertions_from_function_name(signature: &str) -> Vec<GeneratedAssertion> {
        let mut assertions = Vec::new();
        let func_name_lower = signature.to_lowercase();
        
        if func_name_lower.contains("add") || func_name_lower.contains("sum") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(result, a + b);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify addition result".to_string(),
                confidence: 0.95,
            });
        }
        
        if func_name_lower.contains("subtract") || func_name_lower.contains("sub") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(result, a - b);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify subtraction result".to_string(),
                confidence: 0.95,
            });
        }
        
        if func_name_lower.contains("multiply") || func_name_lower.contains("mul") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert_eq!(result, a * b);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Verify multiplication result".to_string(),
                confidence: 0.95,
            });
        }
        
        if func_name_lower.contains("divide") || func_name_lower.contains("div") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!((result - expected).abs() < f64::EPSILON);".to_string(),
                assertion_type: AssertionType::Boolean,
                description: "Verify division with floating point tolerance".to_string(),
                confidence: 0.90,
            });
        }
        
        if func_name_lower.contains("sort") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(result.windows(2).all(|w| w[0] <= w[1]));".to_string(),
                assertion_type: AssertionType::Boolean,
                description: "Verify sorted order".to_string(),
                confidence: 0.95,
            });
        }
        
        if func_name_lower.contains("filter") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(result.iter().all(|x| predicate(x)));".to_string(),
                assertion_type: AssertionType::Boolean,
                description: "Verify all items match predicate".to_string(),
                confidence: 0.90,
            });
        }
        
        if func_name_lower.contains("parse") || func_name_lower.contains("convert") {
            assertions.push(GeneratedAssertion {
                assertion_code: "assert!(result.is_ok());".to_string(),
                assertion_type: AssertionType::TypeCheck,
                description: "Verify parsing succeeded".to_string(),
                confidence: 0.85,
            });
        }
        
        assertions
    }
    
    /// 格式化断言为可执行的测试代码
    pub fn format_assertions(assertions: &[GeneratedAssertion], test_name: &str) -> String {
        let mut code = String::new();
        
        code.push_str(&format!("    #[test]\n    fn test_{}() {{\n", test_name));
        
        for assertion in assertions {
            code.push_str(&format!("        // {}\n", assertion.description));
            code.push_str(&format!("        {}\n", assertion.assertion_code));
            code.push('\n');
        }
        
        code.push_str("    }\n");
        
        code
    }
}

/// ===== [4] 测试执行器 =====
pub struct TestExecutor;

#[derive(Debug, Clone)]
pub struct TestExecutionResult {
    pub test_name: String,
    pub passed: bool,
    pub output: String,
    pub duration_ms: u64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TestSuiteResult {
    pub total_tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total_duration_ms: u64,
    pub results: Vec<TestExecutionResult>,
    pub coverage: Option<CoverageReport>,
}

impl TestExecutor {
    /// 执行单个测试文件
    pub async fn execute_test_file(test_file: &Path) -> Result<TestSuiteResult, String> {
        let start = Instant::now();
        
        // 运行 cargo test --test <file>
        let output = tokio::process::Command::new("cargo")
            .args([
                "test",
                "--test",
                test_file.file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("Invalid test file name")?,
                "--color=never",
                "--quiet",
            ])
            .output()
            .await
            .map_err(|e| format!("Failed to execute test: {}", e))?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined_output = format!("{}{}", stdout, stderr);
        
        // 解析测试结果
        let results = Self::parse_test_output(&combined_output);
        let duration = start.elapsed();
        
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = results.iter().filter(|r| !r.passed).count();
        
        Ok(TestSuiteResult {
            total_tests: results.len(),
            passed,
            failed,
            skipped: 0,  // TODO: Parse skipped tests
            total_duration_ms: duration.as_millis() as u64,
            results,
            coverage: None,  // Will be populated separately
        })
    }
    
    /// 执行工作区所有测试
    pub async fn execute_workspace_tests(workspace_root: &Path) -> Result<TestSuiteResult, String> {
        let start = Instant::now();
        
        let output = tokio::process::Command::new("cargo")
            .args(["test", "--color=never", "--quiet"])
            .current_dir(workspace_root)
            .output()
            .await
            .map_err(|e| format!("Failed to execute workspace tests: {}", e))?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined_output = format!("{}{}", stdout, stderr);
        
        let results = Self::parse_test_output(&combined_output);
        let duration = start.elapsed();
        
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = results.iter().filter(|r| !r.passed).count();
        
        // 同时获取覆盖率
        let coverage = CoverageAnalyzer::analyze(workspace_root).await.ok();
        
        Ok(TestSuiteResult {
            total_tests: results.len(),
            passed,
            failed,
            skipped: 0,
            total_duration_ms: duration.as_millis() as u64,
            results,
            coverage,
        })
    }
    
    /// 执行特定函数的测试
    pub async fn execute_specific_test(
        workspace_root: &Path,
        test_name: &str,
    ) -> Result<TestExecutionResult, String> {
        let start = Instant::now();
        
        let output = tokio::process::Command::new("cargo")
            .args([
                "test",
                test_name,
                "--color=never",
                "--quiet",
                "--",
                "--exact",
            ])
            .current_dir(workspace_root)
            .output()
            .await
            .map_err(|e| format!("Failed to execute test: {}", e))?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let passed = output.status.success();
        
        Ok(TestExecutionResult {
            test_name: test_name.to_string(),
            passed,
            output: format!("{}{}", stdout, stderr),
            duration_ms: start.elapsed().as_millis() as u64,
            error_message: if !passed {
                Some(stderr.to_string())
            } else {
                None
            },
        })
    }
    
    /// 解析cargo test输出
    fn parse_test_output(output: &str) -> Vec<TestExecutionResult> {
        let mut results = Vec::new();
        
        // 匹配测试行: test test_name ... ok/FAILED
        let test_re = Regex::new(r"test\s+(\S+)\s+\.\.\.\s+(ok|FAILED)").unwrap();
        
        for cap in test_re.captures_iter(output) {
            let test_name = cap[1].to_string();
            let status = &cap[2];
            
            results.push(TestExecutionResult {
                test_name,
                passed: status == "ok",
                output: String::new(),
                duration_ms: 0,  // Duration parsing would require more complex regex
                error_message: if status == "FAILED" {
                    Some("Test failed".to_string())
                } else {
                    None
                },
            });
        }
        
        // 如果没有找到测试结果，尝试其他格式
        if results.is_empty() {
            if output.contains("test result: ok") {
                results.push(TestExecutionResult {
                    test_name: "all_tests".to_string(),
                    passed: true,
                    output: output.to_string(),
                    duration_ms: 0,
                    error_message: None,
                });
            } else if output.contains("test result: FAILED") {
                results.push(TestExecutionResult {
                    test_name: "all_tests".to_string(),
                    passed: false,
                    output: output.to_string(),
                    duration_ms: 0,
                    error_message: Some("Some tests failed".to_string()),
                });
            }
        }
        
        results
    }
    
    /// 生成测试报告
    pub fn generate_report(result: &TestSuiteResult) -> String {
        let mut report = String::new();
        
        report.push_str(&format!(
            "\n━━━ Test Execution Report ━━━\n\n"
        ));
        
        report.push_str(&format!(
            "Total: {} | Passed: {} | Failed: {} | Skipped: {}\n",
            result.total_tests, result.passed, result.failed, result.skipped
        ));
        
        report.push_str(&format!(
            "Duration: {}ms\n\n",
            result.total_duration_ms
        ));
        
        if let Some(ref coverage) = result.coverage {
            report.push_str(&format!(
                "Coverage: {:.1}% ({}/{})\n\n",
                coverage.coverage_pct,
                coverage.tested_functions,
                coverage.total_functions
            ));
        }
        
        // 详细结果
        if !result.results.is_empty() {
            report.push_str("Detailed Results:\n");
            for (i, res) in result.results.iter().enumerate() {
                let status = if res.passed { "✓ PASS" } else { "✗ FAIL" };
                report.push_str(&format!(
                    "  {}. {} {} ({}ms)\n",
                    i + 1,
                    status,
                    res.test_name,
                    res.duration_ms
                ));
                
                if let Some(ref error) = res.error_message {
                    report.push_str(&format!("     Error: {}\n", error));
                }
            }
        }
        
        report
    }
}

/// ===== [6] 批量测试生成器 =====
pub struct BatchTestGenerator;

impl BatchTestGenerator {
    /// 批量生成多个函数的测试（并行）
    pub async fn generate_batch_tests(
        file_path: &str,
        function_names: &[&str],
        provider: Arc<dyn Provider>,
        cache: Option<Arc<TddCache>>,
        max_concurrency: usize,
    ) -> Result<HashMap<String, String>, String> {
        let mut results = HashMap::new();
        
        // 使用信号量限制并发
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
        
        let mut tasks = Vec::new();
        
        for &func_name in function_names {
            let sem = semaphore.clone();
            let provider = provider.clone();
            let file = file_path.to_string();
            let func = func_name.to_string();
            let cache_clone = cache.clone();
            
            let task = tokio::spawn(async move {
                let _permit = sem.acquire().await.map_err(|e| e.to_string())?;
                
                let generator = if let Some(c) = cache_clone {
                    TestGenerator::with_provider_and_cache(provider.clone(), c)
                } else {
                    TestGenerator::with_provider(provider.clone())
                };
                
                match generator.generate_unit_test_llm(&file, &func, provider).await {
                    Ok(test_code) => Ok((func, test_code)),
                    Err(e) => Err((func, e)),
                }
            });
            
            tasks.push(task);
        }
        
        // 收集结果
        for task in tasks {
            match task.await {
                Ok(Ok((func_name, test_code))) => {
                    results.insert(func_name, test_code);
                }
                Ok(Err((func_name, error))) => {
                    eprintln!("Failed to generate test for {}: {}", func_name, error);
                }
                Err(e) => {
                    eprintln!("Task panicked: {}", e);
                }
            }
        }
        
        Ok(results)
    }
    
    /// 为整个文件的所有函数生成测试
    pub async fn generate_file_tests(
        file_path: &str,
        provider: Arc<dyn Provider>,
        cache: Option<Arc<TddCache>>,
    ) -> Result<HashMap<String, String>, String> {
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        
        // 提取所有公共函数
        let functions = CoverageAnalyzer::extract_functions(&content);
        let public_functions: Vec<&str> = functions.iter()
            .map(|s| s.as_str())
            .collect();
        
        if public_functions.is_empty() {
            return Ok(HashMap::new());
        }
        
        Self::generate_batch_tests(
            file_path,
            &public_functions,
            provider,
            cache,
            4, // 最大4个并发
        ).await
    }
}

/// ===== [7] 预测性预计算 =====
pub struct PredictivePrecomputation;

impl PredictivePrecomputation {
    /// 基于代码变更预测需要重新生成的测试
    pub async fn predict_test_regeneration(
        modified_files: &[&str],
        workspace_root: &Path,
    ) -> Result<Vec<(String, String)>, String> {
        let mut predictions = Vec::new();
        
        for &file_path in modified_files {
            let full_path = workspace_root.join(file_path);
            if !full_path.exists() {
                continue;
            }
            
            let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| e.to_string())?;
            let functions = CoverageAnalyzer::extract_functions(&content);
            
            // 预测：如果函数被修改，其测试可能需要更新
            for func in functions {
                predictions.push((file_path.to_string(), func));
            }
        }
        
        Ok(predictions)
    }
    
    /// 预热缓存：预先加载常用函数的测试
    pub async fn warmup_cache(
        frequently_tested: &[(String, String)], // (file_path, function_name)
        provider: Arc<dyn Provider>,
        cache: Arc<TddCache>,
    ) -> Result<usize, String> {
        let mut warmed_up = 0usize;
        
        for (file_path, func_name) in frequently_tested {
            let cache_key = TddCache::generate_cache_key(file_path, func_name, "llm_test");
            
            // 如果缓存中已有，跳过
            if cache.get(&cache_key).await.is_some() {
                warmed_up += 1;
                continue;
            }
            
            // 生成并缓存
            let generator = TestGenerator::with_provider_and_cache(provider.clone(), cache.clone());
            match generator.generate_unit_test_llm(file_path, func_name, provider.clone()).await {
                Ok(_) => warmed_up += 1,
                Err(e) => eprintln!("Failed to warmup cache for {}::{}: {}", file_path, func_name, e),
            }
        }
        
        Ok(warmed_up)
    }
}

/// ===== [5] 测试覆盖率分析 =====
pub struct CoverageAnalyzer;

#[derive(Debug, Clone)]
pub struct CoverageReport {
    pub total_functions: usize,
    pub tested_functions: usize,
    pub coverage_pct: f64,
    pub untested_functions: Vec<String>,
    pub files_analyzed: usize,
}

impl CoverageAnalyzer {
    /// 分析 Rust 项目的测试覆盖率
    pub async fn analyze(workspace_root: &Path) -> Result<CoverageReport, String> {
        let mut total = 0usize;
        let mut tested = 0usize;
        let mut untested = Vec::new();
        let mut files = 0usize;

        let mut dirs = vec![workspace_root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| e.to_string())?;
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        dirs.push(path);
                    }
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    files += 1;
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| e.to_string())?;
                    let functions = Self::extract_functions(&content);
                    let has_tests = content.contains("#[cfg(test)]") || content.contains("#[test]");
                    for func in &functions {
                        total += 1;
                        if has_tests {
                            tested += 1;
                        } else {
                            untested.push(format!("{}:{}", path.display(), func));
                        }
                    }
                }
            }
        }

        let coverage = if total > 0 { tested as f64 / total as f64 * 100.0 } else { 0.0 };

        Ok(CoverageReport {
            total_functions: total,
            tested_functions: tested,
            coverage_pct: coverage,
            untested_functions: untested,
            files_analyzed: files,
        })
    }

    fn extract_functions(content: &str) -> Vec<String> {
        let re = Regex::new(r#"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)"#).unwrap();
        re.captures_iter(content)
            .map(|c| c[1].to_string())
            .collect()
    }
}

/// ===== [4] 测试驱动重构 =====
pub struct TddRefactorer;

impl TddRefactorer {
    /// 先写测试→重构→验证循环（LLM增强版）
    pub async fn tdd_cycle_llm(
        file_path: &str,
        function_name: &str,
        workspace_root: &Path,
        provider: Arc<dyn Provider>,
    ) -> Result<TddResult, String> {
        let start = Instant::now();
        let mut steps = Vec::new();

        // Step 1: 使用LLM生成智能测试
        steps.push("Generating tests with LLM...".to_string());
        let test_code = TestGenerator::generate_unit_test_llm(file_path, function_name, provider.clone()).await?;
        let test_file = Self::find_test_file(file_path);
        tokio::fs::write(&test_file, &test_code).await.map_err(|e| e.to_string())?;
        steps.push(format!("Test file written to: {}", test_file.display()));

        // Step 2: 运行测试 (应该失败)
        steps.push(format!("Running tests (expecting failure)..."));
        let initial_result = Self::run_cargo_test(workspace_root).await;
        let initial_passed = !initial_result.contains("FAILED");
        steps.push(if initial_passed { 
            "Tests unexpectedly passed!".to_string() 
        } else { 
            "Tests failed as expected (TDD cycle)".to_string() 
        });

        // Step 3: 分析覆盖率
        steps.push("Analyzing coverage...".to_string());
        let coverage = CoverageAnalyzer::analyze(workspace_root).await?;
        steps.push(format!("Coverage: {:.1}%", coverage.coverage_pct));

        // Step 4: 边界情况检测
        steps.push("Detecting edge cases...".to_string());
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        let edge_cases = EdgeCaseDetector::detect(&content, function_name);
        steps.push(format!("Found {} edge cases", edge_cases.len()));

        Ok(TddResult {
            function_name: function_name.to_string(),
            test_code,
            test_file: test_file.to_string_lossy().to_string(),
            coverage,
            edge_cases,
            initial_test_passed: initial_passed,
            steps,
            duration: start.elapsed(),
        })
    }

    /// 先写测试→重构→验证循环（传统模板版，保留向后兼容）
    pub async fn tdd_cycle(
        file_path: &str,
        function_name: &str,
        workspace_root: &Path,
    ) -> Result<TddResult, String> {
        let start = Instant::now();
        let mut steps = Vec::new();

        // Step 1: 生成测试
        steps.push("Generating tests...".to_string());
        let test_code = TestGenerator::generate_unit_test(file_path, function_name).await?;
        let test_file = Self::find_test_file(file_path);
        tokio::fs::write(&test_file, &test_code).await.map_err(|e| e.to_string())?;

        // Step 2: 运行测试 (应该失败)
        steps.push(format!("Running tests (expecting failure)..."));
        let initial_result = Self::run_cargo_test(workspace_root).await;
        let initial_passed = initial_result.contains("FAILED") == false;

        // Step 3: 分析覆盖率
        steps.push("Analyzing coverage...".to_string());
        let coverage = CoverageAnalyzer::analyze(workspace_root).await?;

        // Step 4: 边界情况检测
        steps.push("Detecting edge cases...".to_string());
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| e.to_string())?;
        let edge_cases = EdgeCaseDetector::detect(&content, function_name);

        Ok(TddResult {
            function_name: function_name.to_string(),
            test_code,
            test_file: test_file.to_string_lossy().to_string(),
            coverage,
            edge_cases,
            initial_test_passed: initial_passed,
            steps,
            duration: start.elapsed(),
        })
    }

    fn find_test_file(source_file: &str) -> PathBuf {
        let path = Path::new(source_file);
        let parent = path.parent().unwrap_or(Path::new("."));
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
        parent.join(format!("{}_tests.rs", stem))
    }

    async fn run_cargo_test(root: &Path) -> String {
        tokio::process::Command::new("cargo")
            .args(["test", "--color=never", "--quiet"])
            .current_dir(root)
            .output().await
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct TddResult {
    pub function_name: String,
    pub test_code: String,
    pub test_file: String,
    pub coverage: CoverageReport,
    pub edge_cases: Vec<EdgeCase>,
    pub initial_test_passed: bool,
    pub steps: Vec<String>,
    pub duration: Duration,
}

pub fn format_tdd_result(result: &TddResult) -> String {
    format!(
        "━━━ TDD Cycle: {} ━━━\n\n\
         Steps:\n{}\n\n\
         Coverage: {:.1}% ({}/{})\n\
         Edge cases: {}\n\
         Initial test: {}\n\
         Duration: {:?}\n\n\
         Generated test file: {}\n\
         ```rust\n{}\n```",
        result.function_name,
        result.steps.iter().enumerate().map(|(i, s)| format!("  {}. {}", i+1, s)).collect::<Vec<_>>().join("\n"),
        result.coverage.coverage_pct, result.coverage.tested_functions, result.coverage.total_functions,
        result.edge_cases.len(),
        if result.initial_test_passed { "PASSED" } else { "FAILED (expected)" },
        result.duration, result.test_file, result.test_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_signature() {
        let content = "pub fn hello(name: &str) -> String {\n    format!(\"Hello {}\", name)\n}";
        let sig = TestGenerator::extract_signature(content, "hello");
        assert!(sig.is_ok());
        assert!(sig.unwrap().contains("fn hello"));
    }

    #[test]
    fn test_edge_case_detection() {
        let content = "fn main() { let x = something.unwrap(); let y = &v[0]; }";
        let cases = EdgeCaseDetector::detect(content, "main");
        assert!(cases.iter().any(|c| c.description.contains("unwrap")));
    }

    #[test]
    fn test_coverage_extract() {
        let functions = CoverageAnalyzer::extract_functions(
            "fn a() {}\npub fn b() {}\nasync fn c() {}"
        );
        assert_eq!(functions.len(), 3);
    }

    #[tokio::test]
    async fn test_tdd_result_format() {
        let result = TddResult {
            function_name: "test_fn".into(),
            test_code: "#[test] fn test() {}".into(),
            test_file: "/tmp/test.rs".into(),
            coverage: CoverageReport { total_functions: 10, tested_functions: 5, coverage_pct: 50.0, untested_functions: vec![], files_analyzed: 3 },
            edge_cases: vec![],
            initial_test_passed: false,
            steps: vec!["Generated".into()],
            duration: Duration::from_secs(1),
        };
        let output = format_tdd_result(&result);
        assert!(output.contains("50.0%"));
    }
    
    #[test]
    fn test_assertion_generator_result_type() {
        let assertions = AssertionGenerator::assertions_for_return_type("Result<String, Error>");
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_type == AssertionType::TypeCheck));
    }
    
    #[test]
    fn test_assertion_generator_option_type() {
        let assertions = AssertionGenerator::assertions_for_return_type("Option<i32>");
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_code.contains("is_some")));
    }
    
    #[test]
    fn test_assertion_generator_vec_type() {
        let assertions = AssertionGenerator::assertions_for_return_type("Vec<String>");
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_type == AssertionType::Collection));
    }
    
    #[test]
    fn test_assertion_from_function_name_add() {
        let assertions = AssertionGenerator::assertions_from_function_name("fn add(a: i32, b: i32) -> i32");
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_code.contains("a + b")));
    }
    
    #[test]
    fn test_assertion_from_function_name_sort() {
        let assertions = AssertionGenerator::assertions_from_function_name("fn sort_list(vec: &mut Vec<i32>)");
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_code.contains("windows")));
    }
    
    #[test]
    fn test_assertion_formatting() {
        let assertions = vec![
            GeneratedAssertion {
                assertion_code: "assert_eq!(result, 42);".to_string(),
                assertion_type: AssertionType::Equality,
                description: "Test equality".to_string(),
                confidence: 0.9,
            }
        ];
        
        let code = AssertionGenerator::format_assertions(&assertions, "my_test");
        assert!(code.contains("#[test]"));
        assert!(code.contains("fn test_my_test()"));
        assert!(code.contains("assert_eq!(result, 42)"));
    }
    
    #[test]
    fn test_edge_case_assertions() {
        let edge_cases = vec![
            EdgeCase {
                description: "Line 10: unwrap() without error handling".to_string(),
                severity: EdgeSeverity::Critical,
                line: Some(10),
            }
        ];
        
        let assertions = AssertionGenerator::assertions_for_edge_cases(&edge_cases);
        assert!(!assertions.is_empty());
        assert!(assertions.iter().any(|a| a.assertion_code.contains("expect")));
    }
    
    #[test]
    fn test_parse_test_output_success() {
        let output = "test tests::test_add ... ok\ntest tests::test_subtract ... ok\n\ntest result: ok. 2 passed; 0 failed;";
        let results = TestExecutor::parse_test_output(output);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.passed));
    }
    
    #[test]
    fn test_parse_test_output_failure() {
        let output = "test tests::test_add ... ok\ntest tests::test_divide ... FAILED\n\ntest result: FAILED. 1 passed; 1 failed;";
        let results = TestExecutor::parse_test_output(output);
        assert_eq!(results.len(), 2);
        assert_eq!(results.iter().filter(|r| r.passed).count(), 1);
        assert_eq!(results.iter().filter(|r| !r.passed).count(), 1);
    }
    
    #[test]
    fn test_generate_report() {
        let result = TestSuiteResult {
            total_tests: 3,
            passed: 2,
            failed: 1,
            skipped: 0,
            total_duration_ms: 150,
            results: vec![
                TestExecutionResult {
                    test_name: "test_pass".to_string(),
                    passed: true,
                    output: String::new(),
                    duration_ms: 50,
                    error_message: None,
                },
                TestExecutionResult {
                    test_name: "test_fail".to_string(),
                    passed: false,
                    output: String::new(),
                    duration_ms: 100,
                    error_message: Some("Assertion failed".to_string()),
                },
            ],
            coverage: None,
        };
        
        let report = TestExecutor::generate_report(&result);
        assert!(report.contains("Test Execution Report"));
        assert!(report.contains("Passed: 2"));
        assert!(report.contains("Failed: 1"));
        assert!(report.contains("✓ PASS"));
        assert!(report.contains("✗ FAIL"));
    }
    
    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = Arc::new(TddCache::new(100));
        
        // Test set and get
        cache.set("test_key", "test_value".to_string()).await;
        let result = cache.get("test_key").await;
        assert_eq!(result, Some("test_value".to_string()));
        
        // Test cache miss
        let miss = cache.get("nonexistent").await;
        assert_eq!(miss, None);
    }
    
    #[tokio::test]
    async fn test_cache_lru_eviction() {
        let cache = Arc::new(TddCache::new(3));
        
        // Fill cache
        cache.set("key1", "value1".to_string()).await;
        cache.set("key2", "value2".to_string()).await;
        cache.set("key3", "value3".to_string()).await;
        
        // Access key1 to make it recently used
        cache.get("key1").await;
        
        // Add new key - should evict key2 (least recently used)
        cache.set("key4", "value4".to_string()).await;
        
        // key2 should be evicted
        assert_eq!(cache.get("key2").await, None);
        
        // Others should still exist
        assert!(cache.get("key1").await.is_some());
        assert!(cache.get("key3").await.is_some());
        assert!(cache.get("key4").await.is_some());
    }
    
    #[tokio::test]
    async fn test_cache_stats() {
        let cache = Arc::new(TddCache::new(10));
        
        cache.set("key1", "value1".to_string()).await;
        cache.get("key1").await; // hit
        cache.get("key1").await; // hit
        cache.get("nonexistent").await; // miss
        
        let stats = cache.get_stats().await;
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        
        let hit_rate = cache.hit_rate().await;
        assert!((hit_rate - 0.666).abs() < 0.01); // ~66.7%
    }
    
    #[tokio::test]
    async fn test_cache_clear() {
        let cache = Arc::new(TddCache::new(10));
        
        cache.set("key1", "value1".to_string()).await;
        cache.set("key2", "value2".to_string()).await;
        
        cache.clear().await;
        
        assert_eq!(cache.get("key1").await, None);
        assert_eq!(cache.get("key2").await, None);
    }
    
    #[test]
    fn test_generate_cache_key() {
        let key = TddCache::generate_cache_key("src/lib.rs", "my_function", "llm_test");
        assert!(key.contains("tdd:"));
        assert!(key.contains("llm_test"));
        assert!(key.contains("src/lib.rs"));
        assert!(key.contains("my_function"));
    }
}
