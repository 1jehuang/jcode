use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeMetrics {
    pub file_path: String,
    pub lines_of_code: usize,
    pub blank_lines: usize,
    pub comment_lines: usize,
    pub cyclomatic_complexity: usize,
    pub cognitive_complexity: usize,
    pub halstead_volume: f64,
    pub maintainability_index: f64,
    pub functions: Vec<FunctionMetrics>,
    pub classes: Vec<ClassMetrics>,
    pub code_quality: CodeQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionMetrics {
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub lines_of_code: usize,
    pub cyclomatic_complexity: usize,
    pub cognitive_complexity: usize,
    pub parameters: usize,
    pub return_type: Option<String>,
    pub calls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassMetrics {
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub methods: usize,
    pub fields: usize,
    pub inheritance_depth: usize,
    pub cyclomatic_complexity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeQuality {
    pub score: f64,
    pub grade: QualityGrade,
    pub issues: Vec<QualityIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum QualityGrade {
    A,
    B,
    C,
    D,
    F,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityIssue {
    pub code: String,
    pub message: String,
    pub severity: String,
    pub line: Option<usize>,
    pub suggestion: String,
}

#[derive(Debug, Clone)]
pub struct CodeAnalyzer {
    language: String,
    config: AnalysisConfig,
}

#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    pub max_cyclomatic_complexity: usize,
    pub max_cognitive_complexity: usize,
    pub max_function_length: usize,
    pub min_comment_ratio: f64,
    pub max_nesting_depth: usize,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_cyclomatic_complexity: 15,
            max_cognitive_complexity: 20,
            max_function_length: 50,
            min_comment_ratio: 0.1,
            max_nesting_depth: 5,
        }
    }
}

impl CodeAnalyzer {
    pub fn new(language: &str) -> Self {
        Self {
            language: language.to_string(),
            config: AnalysisConfig::default(),
        }
    }

    pub fn with_config(language: &str, config: AnalysisConfig) -> Self {
        Self {
            language: language.to_string(),
            config,
        }
    }

    pub fn analyze(&self, file_path: &str, content: &str) -> Result<CodeMetrics> {
        let lines: Vec<&str> = content.lines().collect();
        
        let lines_of_code = self.count_lines_of_code(&lines);
        let blank_lines = self.count_blank_lines(&lines);
        let comment_lines = self.count_comment_lines(&lines);
        let cyclomatic_complexity = self.calculate_cyclomatic_complexity(content);
        let cognitive_complexity = self.calculate_cognitive_complexity(content);
        let halstead_volume = self.calculate_halstead_volume(content);
        let maintainability_index = self.calculate_maintainability_index(
            lines_of_code,
            comment_lines,
            cyclomatic_complexity,
        );
        
        let functions = self.extract_functions(&lines);
        let classes = self.extract_classes(&lines);
        let code_quality = self.assess_quality(
            lines_of_code,
            comment_lines,
            cyclomatic_complexity,
            cognitive_complexity,
            &functions,
        );

        Ok(CodeMetrics {
            file_path: file_path.to_string(),
            lines_of_code,
            blank_lines,
            comment_lines,
            cyclomatic_complexity,
            cognitive_complexity,
            halstead_volume,
            maintainability_index,
            functions,
            classes,
            code_quality,
        })
    }

    fn count_lines_of_code(&self, lines: &[&str]) -> usize {
        lines
            .iter()
            .filter(|line| !line.trim().is_empty() && !self.is_comment(line))
            .count()
    }

    fn count_blank_lines(&self, lines: &[&str]) -> usize {
        lines.iter().filter(|line| line.trim().is_empty()).count()
    }

    fn count_comment_lines(&self, lines: &[&str]) -> usize {
        lines.iter().filter(|line| self.is_comment(line)).count()
    }

    fn is_comment(&self, line: &str) -> bool {
        let trimmed = line.trim();
        match self.language.to_lowercase().as_str() {
            "rust" => trimmed.starts_with("//"),
            "python" => trimmed.starts_with("#"),
            "javascript" | "typescript" => trimmed.starts_with("//"),
            "go" => trimmed.starts_with("//"),
            "java" | "cpp" | "c" => trimmed.starts_with("//"),
            _ => trimmed.starts_with("//") || trimmed.starts_with("#"),
        }
    }

    fn calculate_cyclomatic_complexity(&self, content: &str) -> usize {
        let mut complexity = 1;
        
        let decision_points = [
            "if", "else", "match", "case", "for", "while", "loop",
            "&&", "||", "==>", "->", "=>",
        ];
        
        for point in decision_points {
            complexity += content.matches(point).count();
        }
        
        complexity
    }

    fn calculate_cognitive_complexity(&self, content: &str) -> usize {
        let mut complexity: usize = 0;
        let mut nesting_depth: usize = 0;
        
        let lines: Vec<&str> = content.lines().collect();
        
        for line in lines {
            let trimmed = line.trim();
            
            if trimmed.starts_with("if") || trimmed.starts_with("while") || 
               trimmed.starts_with("for") || trimmed.starts_with("match") ||
               trimmed.starts_with("case") {
                nesting_depth += 1;
                complexity += 1 + (nesting_depth - 1);
            }
            
            if trimmed.starts_with("}") {
                nesting_depth = nesting_depth.saturating_sub(1);
            }
            
            if trimmed.contains("&&") || trimmed.contains("||") {
                complexity += trimmed.matches("&&").count() + trimmed.matches("||").count();
            }
        }
        
        complexity.max(1)
    }

    fn calculate_halstead_volume(&self, content: &str) -> f64 {
        let operators = [
            "+", "-", "*", "/", "=", "==", "!=", "<", ">", "<=", ">=",
            "&&", "||", "!", "->", "=>", "::", ".", ";", ",",
            "(", ")", "[", "]", "{", "}",
        ];
        
        let mut operator_count = 0;
        for op in operators {
            operator_count += content.matches(op).count();
        }
        
        let words: Vec<&str> = content.split_whitespace().collect();
        let operand_count = words.len() - operator_count;
        
        if operator_count == 0 || operand_count == 0 {
            return 0.0;
        }
        
        let vocabulary = operator_count + operand_count;
        let length = operator_count + operand_count;
        
        if vocabulary == 0 {
            0.0
        } else {
            length as f64 * (vocabulary as f64).log2()
        }
    }

    fn calculate_maintainability_index(&self, loc: usize, comments: usize, complexity: usize) -> f64 {
        if loc == 0 {
            return 100.0;
        }
        
        let comment_ratio = comments as f64 / loc as f64;
        
        171.0
            - 5.2 * (loc as f64).ln()
            - 0.23 * complexity as f64
            + 16.2 * comment_ratio.ln()
    }

    fn extract_functions(&self, lines: &[&str]) -> Vec<FunctionMetrics> {
        let mut functions = Vec::new();
        let mut current_function: Option<FunctionBuilder> = None;
        
        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            
            if let Some(func) = self.parse_function_definition(line, line_num) {
                if let Some(mut current) = current_function.take() {
                    current.line_end = line_num - 1;
                    current.lines_of_code = current.line_end - current.line_start + 1;
                    functions.push(current.build());
                }
                current_function = Some(func);
            } else if let Some(mut current) = current_function.take() {
                if line.trim() == "}" || (line.starts_with("fn ") || line.starts_with("pub fn ")) {
                    current.line_end = line_num - 1;
                    current.lines_of_code = current.line_end - current.line_start + 1;
                    functions.push(current.build());
                    
                    if line.starts_with("fn ") || line.starts_with("pub fn ") {
                        if let Some(func) = self.parse_function_definition(line, line_num) {
                            current_function = Some(func);
                        }
                    }
                }
            }
        }
        
        if let Some(mut current) = current_function.take() {
            current.line_end = lines.len();
            current.lines_of_code = current.line_end - current.line_start + 1;
            functions.push(current.build());
        }
        
        functions
    }

    fn parse_function_definition(&self, line: &str, line_num: usize) -> Option<FunctionBuilder> {
        let trimmed = line.trim();
        
        if self.language.to_lowercase() == "rust" {
            if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") {
                let func_name = trimmed
                    .split_whitespace()
                    .nth(if trimmed.starts_with("pub") { 2 } else { 1 })
                    .and_then(|s| s.split('(').next())
                    .unwrap_or("");
                
                let params = trimmed
                    .split(|c| c == '(' || c == ')')
                    .nth(1)
                    .map(|p| p.split(',').filter(|s| !s.trim().is_empty()).count())
                    .unwrap_or(0);
                
                return Some(FunctionBuilder {
                    name: func_name.to_string(),
                    line_start: line_num,
                    line_end: line_num,
                    lines_of_code: 1,
                    cyclomatic_complexity: 1,
                    cognitive_complexity: 1,
                    parameters: params,
                    return_type: None,
                    calls: Vec::new(),
                });
            }
        }
        
        None
    }

    fn extract_classes(&self, lines: &[&str]) -> Vec<ClassMetrics> {
        let mut classes = Vec::new();
        
        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            
            if self.language.to_lowercase() == "rust" {
                if line.trim().starts_with("struct ") || line.trim().starts_with("pub struct ") {
                    let class_name = line
                        .trim()
                        .split_whitespace()
                        .nth(if line.trim().starts_with("pub") { 2 } else { 1 })
                        .unwrap_or("")
                        .to_string();
                    
                    classes.push(ClassMetrics {
                        name: class_name,
                        line_start: line_num,
                        line_end: line_num,
                        methods: 0,
                        fields: 0,
                        inheritance_depth: 0,
                        cyclomatic_complexity: 1,
                    });
                }
            }
        }
        
        classes
    }

    fn assess_quality(
        &self,
        loc: usize,
        comment_lines: usize,
        cyclomatic_complexity: usize,
        cognitive_complexity: usize,
        functions: &[FunctionMetrics],
    ) -> CodeQuality {
        let mut issues = Vec::new();
        let mut score: f64 = 100.0;
        
        let comment_ratio = if loc > 0 {
            comment_lines as f64 / loc as f64
        } else {
            0.0
        };
        
        if cyclomatic_complexity > self.config.max_cyclomatic_complexity {
            score -= 10.0;
            issues.push(QualityIssue {
                code: "high_cyclomatic_complexity".to_string(),
                message: format!("Cyclomatic complexity {} exceeds threshold of {}", cyclomatic_complexity, self.config.max_cyclomatic_complexity),
                severity: "warning".to_string(),
                line: None,
                suggestion: "Consider refactoring to reduce decision points".to_string(),
            });
        }
        
        if cognitive_complexity > self.config.max_cognitive_complexity {
            score -= 10.0;
            issues.push(QualityIssue {
                code: "high_cognitive_complexity".to_string(),
                message: format!("Cognitive complexity {} exceeds threshold of {}", cognitive_complexity, self.config.max_cognitive_complexity),
                severity: "warning".to_string(),
                line: None,
                suggestion: "Consider simplifying nested control structures".to_string(),
            });
        }
        
        if comment_ratio < self.config.min_comment_ratio && loc > 50 {
            score -= 5.0;
            issues.push(QualityIssue {
                code: "low_comment_ratio".to_string(),
                message: format!("Comment ratio {:.1}% below minimum of {:.1}%", comment_ratio * 100.0, self.config.min_comment_ratio * 100.0),
                severity: "info".to_string(),
                line: None,
                suggestion: "Consider adding more comments for better maintainability".to_string(),
            });
        }
        
        for func in functions {
            if func.lines_of_code > self.config.max_function_length {
                score -= 3.0;
                issues.push(QualityIssue {
                    code: "long_function".to_string(),
                    message: format!("Function '{}' has {} lines, exceeding threshold of {}", func.name, func.lines_of_code, self.config.max_function_length),
                    severity: "warning".to_string(),
                    line: Some(func.line_start),
                    suggestion: "Consider breaking this function into smaller functions".to_string(),
                });
            }
            
            if func.parameters > 5 {
                score -= 2.0;
                issues.push(QualityIssue {
                    code: "many_parameters".to_string(),
                    message: format!("Function '{}' has {} parameters", func.name, func.parameters),
                    severity: "info".to_string(),
                    line: Some(func.line_start),
                    suggestion: "Consider using a struct to group related parameters".to_string(),
                });
            }
        }
        
        score = score.max(0.0);
        
        let grade = match score {
            90.0..=100.0 => QualityGrade::A,
            75.0..=89.9 => QualityGrade::B,
            60.0..=74.9 => QualityGrade::C,
            40.0..=59.9 => QualityGrade::D,
            _ => QualityGrade::F,
        };
        
        CodeQuality { score, grade, issues }
    }

    pub fn analyze_directory(&self, dir_path: &str) -> Result<DirectoryMetrics> {
        let mut all_metrics = Vec::new();
        let mut total_loc = 0;
        let mut total_functions = 0;
        let mut total_classes = 0;
        
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if self.is_supported_language(ext) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let file_path = path.to_string_lossy().to_string();
                            let metrics = self.analyze(&file_path, &content)?;
                            total_loc += metrics.lines_of_code;
                            total_functions += metrics.functions.len();
                            total_classes += metrics.classes.len();
                            all_metrics.push(metrics);
                        }
                    }
                } else if path.is_dir() {
                    let sub_dir_metrics = self.analyze_directory(path.to_string_lossy().as_ref())?;
                    total_loc += sub_dir_metrics.total_lines_of_code;
                    total_functions += sub_dir_metrics.total_functions;
                    total_classes += sub_dir_metrics.total_classes;
                    all_metrics.extend(sub_dir_metrics.file_metrics);
                }
            }
        }
        
        Ok(DirectoryMetrics {
            directory: dir_path.to_string(),
            total_files: all_metrics.len(),
            total_lines_of_code: total_loc,
            total_functions,
            total_classes,
            file_metrics: all_metrics,
        })
    }

    fn is_supported_language(&self, ext: &str) -> bool {
        matches!(
            ext,
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "cpp" | "c" | "rb"
        )
    }
}

struct FunctionBuilder {
    name: String,
    line_start: usize,
    line_end: usize,
    lines_of_code: usize,
    cyclomatic_complexity: usize,
    cognitive_complexity: usize,
    parameters: usize,
    return_type: Option<String>,
    calls: Vec<String>,
}

impl FunctionBuilder {
    fn build(self) -> FunctionMetrics {
        FunctionMetrics {
            name: self.name,
            line_start: self.line_start,
            line_end: self.line_end,
            lines_of_code: self.lines_of_code,
            cyclomatic_complexity: self.cyclomatic_complexity,
            cognitive_complexity: self.cognitive_complexity,
            parameters: self.parameters,
            return_type: self.return_type,
            calls: self.calls,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryMetrics {
    pub directory: String,
    pub total_files: usize,
    pub total_lines_of_code: usize,
    pub total_functions: usize,
    pub total_classes: usize,
    pub file_metrics: Vec<CodeMetrics>,
}

impl DirectoryMetrics {
    pub fn to_markdown(&self) -> String {
        let mut markdown = String::from("# Code Quality Report\n\n");
        markdown.push_str(&format!("**Directory:** {}\n\n", self.directory));
        markdown.push_str(&format!("**Total Files:** {}\n", self.total_files));
        markdown.push_str(&format!("**Total Lines of Code:** {}\n", self.total_lines_of_code));
        markdown.push_str(&format!("**Total Functions:** {}\n", self.total_functions));
        markdown.push_str(&format!("**Total Classes:** {}\n\n", self.total_classes));
        
        markdown.push_str("## File Summary\n\n");
        markdown.push_str("| File | LOC | Complexity | Quality |\n");
        markdown.push_str("|------|-----|------------|---------|\n");
        
        for metrics in &self.file_metrics {
            markdown.push_str(&format!(
                "| {} | {} | {} | {:?} |\n",
                metrics.file_path,
                metrics.lines_of_code,
                metrics.cyclomatic_complexity,
                metrics.code_quality.grade
            ));
        }
        
        markdown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_simple_rust() {
        let analyzer = CodeAnalyzer::new("rust");
        let code = r#"
fn main() {
    let x = 5;
    if x > 0 {
        println!("Positive");
    } else {
        println!("Non-positive");
    }
}
"#;
        
        let metrics = analyzer.analyze("test.rs", code).unwrap();
        
        assert!(metrics.lines_of_code > 0);
        assert!(metrics.cyclomatic_complexity >= 1);
        assert!(metrics.code_quality.score >= 0.0);
    }

    #[test]
    fn test_cyclomatic_complexity() {
        let analyzer = CodeAnalyzer::new("rust");
        let code = "if a { if b { if c { } } }";
        
        let metrics = analyzer.analyze("test.rs", code).unwrap();
        assert!(metrics.cyclomatic_complexity > 1);
    }

    #[test]
    fn test_maintainability_index() {
        let analyzer = CodeAnalyzer::new("rust");
        let code = "// This is a comment\nfn main() { let x = 1; }";
        
        let metrics = analyzer.analyze("test.rs", code).unwrap();
        assert!(metrics.maintainability_index > 0.0);
    }

    #[test]
    fn test_quality_grade_a() {
        let analyzer = CodeAnalyzer::new("rust");
        let code = "// Simple function\nfn add(a: i32, b: i32) -> i32 { a + b }";
        
        let metrics = analyzer.analyze("test.rs", code).unwrap();
        assert_eq!(metrics.code_quality.grade, QualityGrade::A);
    }

    #[test]
    fn test_extract_functions() {
        let analyzer = CodeAnalyzer::new("rust");
        let code = "fn foo() {}\npub fn bar() {}";
        
        let metrics = analyzer.analyze("test.rs", code).unwrap();
        assert_eq!(metrics.functions.len(), 2);
    }
}