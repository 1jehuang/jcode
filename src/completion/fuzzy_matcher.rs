//! # 模糊匹配引擎
//!
//! 提供智能的模糊字符串匹配能力：
//! - **编辑距离算法** (Levenshtein) - 衡量字符串相似度
//! - **Jaro-Winkler** - 适合短字符串匹配
//! - **Dice系数** (n-gram重叠) - 适合长文本
//! - **智能排序** - 多维度评分 + 权重调整
//!
//! ## 应用场景
//!
//! ```rust
//! use carpai::completion::fuzzy_matcher::{FuzzyMatcher, MatchAlgorithm, MatchResult};
//!
//! let matcher = FuzzyMatcher::with_algorithm(MatchAlgorithm::JaroWinkler);
//!
//! let results = matcher.match_fuzzy("gut status", &["git status", "get status", "git stash"]);
//!
//! for result in results {
//!     println!("{}: {:.2} ({})", 
//!         result.candidate, 
//!         result.score,
//!         result.match_type);
//! }
//! // 输出:
//! // git status: 0.93 (High)
//! // get status: 0.85 (Medium)
//! ```

use serde::{Deserialize, Serialize};

/// 模糊匹配器
pub struct FuzzyMatcher {
    /// 最小相似度阈值（低于此值的结果将被过滤）
    threshold: f64,
    
    /// 使用的算法
    algorithm: MatchAlgorithm,
    
    /// 是否启用大小写不敏感匹配
    case_insensitive: bool,
    
    /// 最大返回结果数
    max_results: usize,
}

/// 匹配算法枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchAlgorithm {
    /// Levenshtein编辑距离（适合一般场景）
    Levenshtein,
    /// Jaro-Winkler距离（适合短字符串，对前缀更敏感）
    JaroWinkler,
    /// Dice系数基于n-gram（适合长文本）
    DiceCoefficient,
    /// 混合模式（自动选择最佳算法）
    Hybrid,
}

/// 匹配结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    /// 候选字符串
    pub candidate: String,
    
    /// 相似度分数 (0.0-1.0，越高越相似)
    pub score: f64,
    
    /// 匹配类型/质量等级
    pub match_type: MatchQuality,
    
    /// 编辑距离（如果适用）
    pub edit_distance: Option<usize>,
    
    /// 匹配的位置信息（用于高亮显示）
    pub match_ranges: Vec<MatchRange>,
}

/// 匹配质量等级
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchQuality {
    /// 完美匹配 (score >= 0.95)
    Exact,
    /// 高度相似 (0.85 <= score < 0.95)
    High,
    /// 中等相似 (0.7 <= score < 0.85)
    Medium,
    /// 低相似度 (0.5 <= score < 0.7)
    Low,
    /// 不相关 (< 0.5)
    NoMatch,
}

impl std::fmt::Display for MatchQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchQuality::Exact => write!(f, "✅ Exact"),
            MatchQuality::High => write!(f, "🟢 High"),
            MatchQuality::Medium => write!(f, "🟡 Medium"),
            MatchQuality::Low => write!(f, "🔴 Low"),
            MatchQuality::NoMatch => write!(f, "❌ NoMatch"),
        }
    }
}

/// 匹配范围（用于UI高亮）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRange {
    /// 起始位置
    pub start: usize,
    
    /// 结束位置
    pub end: usize,
    
    /// 匹配类型
    pub range_type: RangeType,
}

/// 范围类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RangeType {
    /// 精确字符匹配
    Exact,
    /// 前缀匹配
    Prefix,
    /// 子串匹配
    Substring,
    /// 变形匹配（大小写、拼写错误等）
    Transformed,
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self {
            threshold: 0.6,
            algorithm: MatchAlgorithm::JaroWinkler,
            case_insensitive: true,
            max_results: 20,
        }
    }
}

impl FuzzyMatcher {
    /// 创建使用默认配置的匹配器
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用指定算法创建匹配器
    pub fn with_algorithm(algorithm: MatchAlgorithm) -> Self {
        Self {
            algorithm,
            ..Default::default()
        }
    }

    /// 创建高精度匹配器（适用于命令补全）
    pub fn for_command_completion() -> Self {
        Self {
            threshold: 0.65,
            algorithm: MatchAlgorithm::JaroWinkler,
            case_insensitive: true,
            max_results: 10,
        }
    }

    /// 执行模糊匹配
    pub fn match_fuzzy(
        &self,
        query: &str,
        candidates: &[String],
    ) -> Vec<MatchResult> {
        let normalized_query = if self.case_insensitive {
            query.to_lowercase()
        } else {
            query.to_string()
        };

        let mut results: Vec<MatchResult> = candidates
            .iter()
            .map(|candidate| {
                let normalized_candidate = if self.case_insensitive {
                    candidate.to_lowercase()
                } else {
                    candidate.clone()
                };

                let score = self.calculate_score(&normalized_query, &normalized_candidate);
                
                MatchResult {
                    candidate: candidate.clone(),
                    score,
                    match_type: self.classify_quality(score),
                    edit_distance: Some(self.levenshtein_distance(&normalized_query, &normalized_candidate)),
                    match_ranges: self.find_match_ranges(&normalized_query, &normalized_candidate),
                }
            })
            .filter(|r| r.score >= self.threshold)
            .collect();

        // 按分数降序排序
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // 限制结果数量
        results.truncate(self.max_results);

        results
    }

    /// 计算两个字符串的相似度分数
    fn calculate_score(&self, s1: &str, s2: &str) -> f64 {
        if s1.is_empty() || s2.is_empty() {
            return 0.0;
        }

        // 如果完全相同
        if s1 == s2 {
            return 1.0;
        }

        // 根据选择的算法计算
        let base_score = match self.algorithm {
            MatchAlgorithm::Levenshtein => {
                let dist = self.levenshtein_distance(s1, s2);
                let max_len = s1.len().max(s2.len());
                1.0 - (dist as f64 / max_len as f64)
            }
            MatchAlgorithm::JaroWinkler => {
                self.jaro_winkler_similarity(s1, s2)
            }
            MatchAlgorithm::DiceCoefficient => {
                self.dice_coefficient(s1, s2, 2) // bigram
            }
            MatchAlgorithm::Hybrid => {
                // 对短字符串用Jaro-Winkler，长字符串用Dice
                if s1.len() <= 8 && s2.len() <= 8 {
                    self.jaro_winkler_similarity(s1, s2)
                } else {
                    self.dice_coefficient(s1, s2, 3) // trigram
                }
            }
        };

        // 额外加分项
        let bonus = self.calculate_bonuses(s1, s2);

        (base_score + bonus).min(1.0)
    }

    /// 计算额外加分（前缀匹配、首字母匹配等）
    fn calculate_bonuses(&self, query: &str, candidate: &str) -> f64 {
        let mut bonus = 0.0;

        // 前缀匹配加分
        if candidate.starts_with(query) {
            bonus += 0.15;
        }

        // 首字母缩写匹配加分
        if self.is_acronym_match(query, candidate) {
            bonus += 0.1;
        }

        // 单词边界匹配加分
        if self.has_word_boundary_match(query, candidate) {
            bonus += 0.05;
        }

        bonus
    }

    /// 检查是否是首字母缩写匹配（如 "gs" → "git status"）
    fn is_acronym_match(&self, acronym: &str, text: &str) -> bool {
        if acronym.len() > text.len() || acronym.is_empty() {
            return false;
        }

        let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == '-' || c == '_').collect();
        
        if words.len() < acronym.len() {
            return false;
        }

        acronym.chars().enumerate().all(|(i, ac_char)| {
            words.get(i).map_or(false, |word| {
                !word.is_empty() && word.chars().next().map_or(false, |c| {
                    c.to_ascii_lowercase() == ac_char.to_ascii_lowercase()
                })
            })
        })
    }

    /// 检查是否有单词边界匹配
    fn has_word_boundary_match(&self, query: &str, candidate: &str) -> bool {
        let query_words: Vec<&str> = query.split_whitespace().collect();
        
        query_words.iter().any(|word| {
            candidate.contains(word) || 
            candidate.split_whitespace().any(|cw| cw.starts_with(word))
        })
    }

    /// 分类匹配质量
    fn classify_quality(&self, score: f64) -> MatchQuality {
        if score >= 0.95 {
            MatchQuality::Exact
        } else if score >= 0.85 {
            MatchQuality::High
        } else if score >= 0.7 {
            MatchQuality::Medium
        } else if score >= 0.5 {
            MatchQuality::Low
        } else {
            MatchQuality::NoMatch
        }
    }

    /// 查找匹配位置范围（用于高亮显示）
    fn find_match_ranges(&self, query: &str, candidate: &str) -> Vec<MatchRange> {
        let mut ranges = vec![];

        // 简单实现：查找所有子串出现位置
        let mut search_start = 0;
        while let Some(pos) = candidate[search_start..].find(query) {
            let actual_pos = search_start + pos;
            
            ranges.push(MatchRange {
                start: actual_pos,
                end: actual_pos + query.len(),
                range_type: if actual_pos == 0 { RangeType::Prefix } else { RangeType::Substring },
            });

            search_start = actual_pos + 1;

            // 防止无限循环
            if ranges.len() > 20 {
                break;
            }
        }

        if ranges.is_empty() && !query.is_empty() {
            // 尝试逐字符匹配
            for (i, qc) in query.chars().enumerate() {
                if let Some((pos, _)) = candidate.char_indices().find(|(_, cc)| *cc == qc) {
                    ranges.push(MatchRange {
                        start: pos,
                        end: pos + qc.len_utf8(),
                        range_type: RangeType::Transformed,
                    });
                    
                    if ranges.len() >= 5 {
                        break;
                    }
                }
            }
        }

        ranges
    }

    // ════════════════════════════
    // 距离算法实现
    // ════════════════════════════

    /// Levenshtein编辑距离
    fn levenshtein_distance(&self, s1: &str, s2: &str) -> usize {
        let chars1: Vec<char> = s1.chars().collect();
        let chars2: Vec<char> = s2.chars().collect();
        let len1 = chars1.len();
        let len2 = chars2.len();

        if len1 == 0 { return len2; }
        if len2 == 0 { return len1; }

        // 使用优化的动态规划（只保留两行）
        let mut prev_row: Vec<usize> = (0..=len2).collect();

        for (i, c1) in chars1.iter().enumerate() {
            let mut curr_row = vec![i + 1];

            for (j, c2) in chars2.iter().enumerate() {
                let cost = if c1 == c2 { 0 } else { 1 };
                
                curr_row.push(*[
                    curr_row.last().unwrap(),      // deletion
                    prev_row.get(j + 1).unwrap(),   // insertion
                    prev_row.get(j).unwrap() + cost, // substitution
                ].iter().min().unwrap());
            }

            prev_row = curr_row;
        }

        *prev_row.last().unwrap()
    }

    /// Jaro-Winkler相似度 (0.0-1.0)
    fn jaro_winkler_similarity(&self, s1: &str, s2: &str) -> f64 {
        let jaro = self.jaro_similarity(s1, s2);
        
        // Winkler modification: 给前缀匹配额外权重
        let prefix_len = s1.chars()
            .zip(s2.chars())
            .take(4)
            .take_while(|(a, b)| a == b)
            .count();

        let winkler_bonus = 0.1 * prefix_len as f64 * (1.0 - jaro);

        (jaro + winkler_bonus).min(1.0)
    }

    /// Jaro相似度基础算法
    fn jaro_similarity(&self, s1: &str, s2: &str) -> f64 {
        let chars1: Vec<char> = s1.chars().collect();
        let chars2: Vec<char> = s2.chars().collect();
        let len1 = chars1.len();
        let len2 = chars2.len();

        if len1 == 0 && len2 == 0 { return 1.0; }
        if len1 == 0 || len2 == 0 { return 0.0; }

        let match_distance = (len1.max(len2) / 2).max(1);

        // 找到匹配的字符
        let s1_matches: Vec<bool> = vec![false; len1];
        let s2_matches: Vec<bool> = vec![false; len2];
        let mut matches = 0usize;

        for i in 0..len1 {
            let start = (i as isize - match_distance as isize).max(0) as usize;
            let end = (i + match_distance + 1).min(len2);

            for j in start..end {
                if !s2_matches[j] && chars1[i] == chars2[j] {
                    s1_matches[i] = true;
                    s2_matches[j] = true;
                    matches += 1;
                    break;
                }
            }
        }

        if matches == 0 { return 0.0; }

        // 计算转置数
        let mut transpositions = 0usize;
        let mut k = 0usize;

        for i in 0..len1 {
            if s1_matches[i] {
                while !s2_matches[k] { k += 1; }
                if chars1[i] != chars2[k] {
                    transpositions += 1;
                }
                k += 1;
            }
        }

        let m = matches as f64;
        let t = transpositions as f64 / 2.0;

        (m / len1 as f64 + m / len2 as f64 + (m - t) / m) / 3.0
    }

    /// Dice系数 (基于n-gram)
    fn dice_coefficient(&self, s1: &str, s2: &str, n: usize) -> f64 {
        if n > s1.len() || n > s2.len() {
            return if s1 == s2 { 1.0 } else { 0.0 };
        }

        let ngrams1 = self.get_ngrams(s1, n);
        let ngrams2 = self.get_ngrams(s2, n);

        if ngrams1.is_empty() && ngrams2.is_empty() {
            return 1.0;
        }

        let intersection: usize = ngrams1
            .intersection(&ngrams2)
            .count();

        let union_size = ngrams1.len() + ngrams2.len();

        2.0 * intersection as f64 / union_size as f64
    }

    /// 提取n-gram
    fn get_ngrams(&self, s: &str, n: usize) -> std::collections::HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        let mut ngrams = std::collections::HashSet::new();

        for window in chars.windows(n) {
            ngrams.insert(window.iter().collect());
        }

        ngrams
    }
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let matcher = FuzzyMatcher::for_command_completion();
        let results = matcher.match_fuzzy("git status", &["git status".to_string()]);
        
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 1.0).abs() < f64::EPSILON);
        assert_eq!(results[0].match_type, MatchQuality::Exact);
    }

    #[test]
    fn test_typo_tolerance() {
        let matcher = FuzzyMatcher::for_command_completion();
        let results = matcher.match_fuzzy(
            "gut status", 
            &["git status".to_string(), "get status".to_string()]
        );
        
        assert!(results.len() >= 1);
        
        // "git status" 应该得分更高
        let git_result = results.iter().find(|r| r.candidate == "git status").unwrap();
        assert!(git_result.score > 0.8, "Should tolerate typo with high score");
    }

    #[test]
    fn test_prefix_preference() {
        let matcher = FuzzyMatcher::for_command_completion();
        let results = matcher.match_fuzzy(
            "gi",
            &["git".to_string(), "gift".to_string(), "give".to_string()]
        );
        
        assert!(!results.is_empty());
        
        // "git" 应该排在前面（前缀匹配）
        if results.len() >= 1 {
            assert_eq!(results[0].candidate, "git");
        }
    }

    #[test]
    fn test_acronym_matching() {
        let matcher = FuzzyMatcher::for_command_completion();
        let results = matcher.match_fuzzy(
            "gs",
            &["git status".to_string(), "get status".to_string(), "gsutil".to_string()]
        );
        
        // "git status" 应该被识别为首字母缩写
        let git_status = results.iter().find(|r| r.candidate == "git status");
        assert!(git_status.is_some(), "Should match 'gs' to 'git status' as acronym");
    }

    #[test]
    fn test_levenshtein_algorithm() {
        let matcher = FuzzyMatcher::with_algorithm(MatchAlgorithm::Levenshtein);
        let results = matcher.match_fuzzy(
            "docker ps",
            &["docker ps".to_string(), "docker push".to_string(), "docket ps".to_string()]
        );
        
        assert!(!results.is_empty());
        
        // 精确匹配应该得分最高
        let exact = results.iter().find(|r| r.candidate == "docker ps").unwrap();
        assert!(exact.score > 0.9);
    }

    #[test]
    fn test_jaro_winkler_for_short_strings() {
        let matcher = FuzzyMatcher::with_algorithm(MatchAlgorithm::JaroWinklet);
        
        let results = matcher.match_fuzzy(
            "fn",
            &["fn".to_string(), "function".to_string(), "ifn".to_string()]
        );
        
        assert!(!results.is_empty());
        
        // "fn" 应该完美匹配
        let exact = results.iter().find(|r| r.candidate == "fn").unwrap();
        assert!((exact.score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_dice_for_long_strings() {
        let matcher = FuzzyMatcher::with_algorithm(MatchAlgorithm::DiceCoefficient);
        
        let long_text = "this is a very long string for testing dice coefficient";
        let results = matcher.match_fuzzy(
            "very lng strng",  // 有typo
            &[long_text.to_string(), "short text".to_string()]
        );
        
        // 长文本应该匹配成功
        assert!(!results.is_empty());
        assert!(results[0].candidate.contains("long"));
    }

    #[test]
    fn test_threshold_filtering() {
        let matcher = FuzzyMatcher {
            threshold: 0.9,  // 高阈值
            ..Default::default()
        };
        
        let results = matcher.match_fuzzy(
            "abc",
            &["abc".to_string(), "abd".to_string(), "xyz".to_string()]
        );
        
        // 只有 "abc" 应该通过
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].candidate, "abc");
    }

    #[test]
    fn test_max_results_limiting() {
        let matcher = FuzzyMatcher {
            max_results: 2,
            ..Default::default()
        };
        
        let candidates: Vec<String> = (0..10)
            .map(|i| format!("item_{}", i))
            .collect();
        
        let results = matcher.match_fuzzy("item", &candidates);
        
        assert!(results.len() <= 2);
    }

    #[test]
    fn test_case_insensitive_matching() {
        let matcher = FuzzyMatcher {
            case_insensitive: true,
            ..Default::default()
        };
        
        let results = matcher.match_fuzzy(
            "GIT STATUS",
            &["git status".to_string()]  // 小写
        );
        
        assert_eq!(results.len(), 1);
        assert!(results[0].score > 0.95);  // 大小写不敏感应该几乎完美匹配
    }

    #[test]
    fn test_edit_distance_calculation() {
        let matcher = FuzzyMatcher::new();
        
        // 相同字符串
        assert_eq!(matcher.levenshtein_distance("", ""), 0);
        assert_eq!(matcher.levenshtein_distance("a", "a"), 0);
        
        // 一个操作
        assert_eq!(matcher.levenshtein_distance("a", ""), 1);
        assert_eq!(matcher.levenshtein_distance("", "a"), 1);
        assert_eq!(matcher.levenshtein_distance("ab", "a"), 1);
        
        // 替换
        assert_eq!(matcher.levenshtein_distance("cat", "bat"), 1);
        
        // 复杂示例
        assert_eq!(matcher.levenshtein_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn test_match_ranges_for_highlighting() {
        let matcher = FuzzyMatcher::new();
        
        let results = matcher.match_fuzzy(
            "status",
            &["git status command".to_string()]
        );
        
        assert_eq!(results.len(), 1);
        
        let ranges = &results[0].match_ranges;
        assert!(!ranges.is_empty(), "Should have match ranges for highlighting");
        
        // 第一个range应该在 "status" 的位置
        let first_range = &ranges[0];
        let candidate = &results[0].candidate;
        assert_eq!(
            &candidate[first_range.start..first_range.end],
            "status"
        );
    }

    #[test]
    fn test_empty_query_handling() {
        let matcher = FuzzyMatcher::new();
        let results = matcher.match_fuzzy("", &["a".to_string(), "b".to_string()]);
        
        // 空查询不应该返回结果
        assert!(results.is_empty());
    }

    #[test]
    fn test_special_characters() {
        let matcher = FuzzyMatcher::new();
        
        let results = matcher.match_fuzzy(
            "rm -rf",
            &["rm -rf /tmp".to_string(), "rm-rf data".to_string()]
        );
        
        assert!(!results.is_empty());
        
        // 应该能正确处理特殊字符
        let exact = results.iter().find(|r| r.candidate.starts_with("rm -rf"));
        assert!(exact.is_some());
    }
}
