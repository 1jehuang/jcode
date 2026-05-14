//! 七类基本错误检测引擎
//!
//! 1. 类型错误      — 函数参数/返回值类型不匹配
//! 2. 字段错误      — 结构体字段不存在/类型不匹配
//! 3. 前后端字段不一致 — API 请求/响应字段与数据库模型不匹配
//! 4. API 字段不一致  — 同一 API 的前后端字段名不同
//! 5. 数据库字段缺失  — ORM 模型缺少数据库列
//! 6. 语法错误      — tree-sitter 解析错误
//! 7. 路由错误      — URL 路径与处理器不匹配

use regex::Regex;
use std::collections::{HashMap, HashSet};

// ══════════════════════════════════════════════════════════════════
// 错误类型定义
// ══════════════════════════════════════════════════════════════════

/// 七类基本错误的统一枚举
#[derive(Debug, Clone, PartialEq)]
pub enum CodeError {
    TypeError {
        file: String, line: usize, symbol: String,
        expected: String, found: String, message: String,
    },
    FieldError {
        file: String, line: usize, field: String,
        struct_name: String, suggestion: Option<String>,
    },
    FieldMismatch {
        api_file: String, api_field: String,
        db_file: String, db_field: String,
        direction: MismatchDirection,
    },
    ApiFieldInconsistency {
        file: String, endpoint: String,
        request_field: String,
        response_field: String,
    },
    MissingDbField {
        model_file: String, model_name: String,
        missing_column: String, table: String,
    },
    SyntaxError {
        file: String, line: usize, column: usize,
        message: String,
    },
    RouteError {
        file: String, route: String,
        handler: Option<String>,
        existing_routes: Vec<String>,
    },
}

/// 不一致的方向
#[derive(Debug, Clone, PartialEq)]
pub enum MismatchDirection {
    ApiHasFieldNotInDb,
    DbHasFieldNotInApi,
}

impl CodeError {
    pub fn severity(&self) -> &'static str {
        match self {
            Self::TypeError { .. } => "high",
            Self::FieldError { .. } => "high",
            Self::FieldMismatch { .. } => "medium",
            Self::ApiFieldInconsistency { .. } => "medium",
            Self::MissingDbField { .. } => "high",
            Self::SyntaxError { .. } => "high",
            Self::RouteError { .. } => "medium",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::TypeError { .. } => "type",
            Self::FieldError { .. } => "field",
            Self::FieldMismatch { .. } => "mismatch",
            Self::ApiFieldInconsistency { .. } => "api_inconsistency",
            Self::MissingDbField { .. } => "missing_db_field",
            Self::SyntaxError { .. } => "syntax",
            Self::RouteError { .. } => "route",
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 检测引擎
// ══════════════════════════════════════════════════════════════════

/// 七类错误统一检测器
pub struct ErrorDetector;

impl ErrorDetector {
    pub fn new() -> Self { Self }

    /// 对项目执行全部七类检查
    pub fn analyze_project(&self, root: &str) -> Vec<CodeError> {
        let mut all = Vec::new();
        if !std::path::Path::new(root).exists() { return all; }

        let files = self.collect_files(root);

        all.extend(self.detect_type_errors(&files));
        all.extend(self.detect_field_errors(&files));
        all.extend(self.detect_field_mismatches(&files));
        all.extend(self.detect_api_inconsistencies(&files));
        all.extend(self.detect_missing_db_fields(&files));
        all.extend(self.detect_syntax_errors(&files));
        all.extend(self.detect_route_errors(&files));

        all
    }

    fn collect_files(&self, root: &str) -> Vec<(String, String)> {
        let mut files = Vec::new();
        self.collect_files_recursive(root, &mut files);
        files
    }

    fn collect_files_recursive(&self, root: &str, files: &mut Vec<(String, String)>) {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name != "node_modules" && name != "target" && name != ".git" && !name.starts_with('.') {
                        self.collect_files_recursive(&path.to_string_lossy(), files);
                    }
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "rs" | "ts" | "tsx" | "js" | "py" | "go" | "java" | "vue") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            files.push((path.to_string_lossy().to_string(), content));
                        }
                    }
                }
            }
        }
    }

    // ── 1. 类型错误检测 ──

    fn detect_type_errors(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let assign_re = Regex::new(r"let\s+(\w+)\s*:\s*(\w+)\s*=\s*(.+)").unwrap();

        for (file, content) in files {
            for (line, text) in content.lines().enumerate() {
                if let Some(cap) = assign_re.captures(text) {
                    let var_type = cap[2].to_string();
                    let value = cap[3].to_string();
                    if value.contains("String") && var_type == "i32" {
                        errors.push(CodeError::TypeError {
                            file: file.clone(), line: line + 1,
                            symbol: cap[1].to_string(),
                            expected: var_type.clone(), found: "String".into(),
                            message: format!("Variable '{}' assigned String but declared as {}", cap[1].to_string(), var_type),
                        });
                    }
                }
            }
        }
        errors
    }

    // ── 2. 字段错误检测 ──

    fn detect_field_errors(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let field_access_re = Regex::new(r"(\w+)\.(\w+)").unwrap();
        let struct_def_re = Regex::new(r"struct\s+(\w+)\s*\{([^}]*)\}").unwrap();

        for (file, content) in files {
            let mut structs: HashMap<String, HashSet<String>> = HashMap::new();
            for cap in struct_def_re.captures_iter(content) {
                let name = cap[1].to_string();
                let fields: HashSet<String> = cap[2].split(',')
                    .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                structs.insert(name, fields);
            }

            for (line, text) in content.lines().enumerate() {
                for cap in field_access_re.captures_iter(text) {
                    let obj = cap[1].to_string();
                    let field = cap[2].to_string();
                    if matches!(field.as_str(), "len" | "is_empty" | "clone" | "to_string" | "as_str") {
                        continue;
                    }
                    if let Some(fields) = structs.get(&obj) {
                        if !fields.contains(&field) {
                            errors.push(CodeError::FieldError {
                                file: file.clone(), line: line + 1,
                                field, struct_name: obj.clone(),
                                suggestion: None,
                            });
                        }
                    }
                }
            }
        }
        errors
    }

    // ── 3. 前后端字段不一致检测 ──

    fn detect_field_mismatches(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let mut api_fields: HashMap<String, HashSet<String>> = HashMap::new();
        let mut db_fields: HashMap<String, HashSet<String>> = HashMap::new();

        let api_struct_re = Regex::new(r"(Response|Request|Dto|VO|Form)").unwrap();

        for (file, content) in files {
            let is_api = file.contains("api") || file.contains("controller") || file.contains("handler");
            let is_db = file.contains("model") || file.contains("entity") || file.contains("schema") || file.contains("migration");

            let struct_re = Regex::new(r"(?:pub\s+)?(?:struct|type)\s+(\w+)\s*\{([^}]*)\}").unwrap();
            for cap in struct_re.captures_iter(content) {
                let name = cap[1].to_string();
                let fields: HashSet<String> = cap[2].split(',')
                    .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();

                if is_api || api_struct_re.is_match(&name) {
                    api_fields.insert(name.clone(), fields.clone());
                }
                if is_db {
                    db_fields.insert(name, fields);
                }
            }
        }

        for (api_name, api_fs) in &api_fields {
            for (db_name, db_fs) in &db_fields {
                let api_only: Vec<_> = api_fs.difference(db_fs).collect();
                let db_only: Vec<_> = db_fs.difference(api_fs).collect();

                for field in &api_only {
                    errors.push(CodeError::FieldMismatch {
                        api_file: "api".into(), api_field: format!("{}.{}", api_name, field),
                        db_file: "db".into(), db_field: format!("{}.{}", db_name, field),
                        direction: MismatchDirection::ApiHasFieldNotInDb,
                    });
                }
                for field in &db_only {
                    errors.push(CodeError::FieldMismatch {
                        api_file: "api".into(), api_field: format!("{}.{}", api_name, field),
                        db_file: "db".into(), db_field: format!("{}.{}", db_name, field),
                        direction: MismatchDirection::DbHasFieldNotInApi,
                    });
                }
            }
        }
        errors
    }

    // ── 4. API 字段不一致检测 (实现版) ──

    fn detect_api_inconsistencies(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let request_re = Regex::new(r"(?:struct|class|interface)\s+(\w*Request\w*)\s*\{([^}]*)\}").unwrap();
        let response_re = Regex::new(r"(?:struct|class|interface)\s+(\w*Response\w*)\s*\{([^}]*)\}").unwrap();

        for (file, content) in files {
            // Collect Request structs
            let mut request_fields: HashMap<String, HashSet<String>> = HashMap::new();
            for cap in request_re.captures_iter(content) {
                let name = cap[1].to_string();
                let fields: HashSet<String> = cap[2].split(',')
                    .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                request_fields.insert(name, fields);
            }

            // Collect Response structs
            let mut response_fields: HashMap<String, HashSet<String>> = HashMap::new();
            for cap in response_re.captures_iter(content) {
                let name = cap[1].to_string();
                let fields: HashSet<String> = cap[2].split(',')
                    .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                response_fields.insert(name, fields);
            }

            // Match Request/Response pairs by base name
            for (req_name, req_fs) in &request_fields {
                let base_name = req_name.replace("Request", "");
                for (resp_name, resp_fs) in &response_fields {
                    let resp_base = resp_name.replace("Response", "");
                    if base_name == resp_base {
                        // Find fields in request but not response
                        for field in req_fs.difference(resp_fs) {
                            errors.push(CodeError::ApiFieldInconsistency {
                                file: file.clone(),
                                endpoint: base_name.clone(),
                                request_field: format!("{}.{}", req_name, field),
                                response_field: format!("{}.{}", resp_name, field),
                            });
                        }
                    }
                }
            }
        }
        errors
    }

    // ── 5. 数据库字段缺失检测 (实现版) ──

    fn detect_missing_db_fields(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();

        // Look for ORM model definitions and migration/schema files
        let model_re = Regex::new(r"(?:struct|class)\s+(\w+Model|\w+Entity|\w+Table)\s*\{([^}]*)\}").unwrap();
        let schema_re = Regex::new(r#"table_name\s*=\s*["'](\w+)["']"#).unwrap();
        let column_re = Regex::new(r#"column\s*\(\s*["'](\w+)["']"#).unwrap();

        for (file, content) in files {
            let is_model = file.contains("model") || file.contains("entity");
            let is_schema = file.contains("schema") || file.contains("migration");

            if is_model {
                // Extract model fields
                for cap in model_re.captures_iter(content) {
                    let model_name = cap[1].to_string();
                    let model_fields: HashSet<String> = cap[2].split(',')
                        .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                        .filter(|f| !f.is_empty() && !f.starts_with("pub"))
                        .collect();

                    // Look for corresponding schema/migration
                    let table_name = schema_re.captures(&content)
                        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
                        .unwrap_or_else(|| model_name.replace("Model", "").replace("Entity", "").to_lowercase());

                    // If this is also a schema file, check columns
                    if is_schema {
                        let schema_columns: HashSet<String> = column_re.captures_iter(&content)
                            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                            .collect();

                        for col in &schema_columns {
                            if !model_fields.contains(col) {
                                errors.push(CodeError::MissingDbField {
                                    model_file: file.clone(),
                                    model_name: model_name.clone(),
                                    missing_column: col.clone(),
                                    table: table_name.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        errors
    }

    // ── 6. 语法错误检测 (基于 tree-sitter，实现版) ──

    fn detect_syntax_errors(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();

        for (file, content) in files {
            if file.ends_with(".rs") {
                // Use tree-sitter for Rust syntax errors
                let mut parser = tree_sitter::Parser::new();
                if parser.set_language(&tree_sitter_rust::LANGUAGE.into()).is_err() {
                    continue;
                }

                if let Some(tree) = parser.parse(content, None) {
                    let root = tree.root_node();
                    self.collect_error_nodes(&root, content, file, &mut errors);
                }
            } else {
                // For non-Rust: basic brace matching
                let mut brace_depth = 0;
                let mut in_string = false;
                let mut escape = false;

                for (line_idx, line) in content.lines().enumerate() {
                    for ch in line.chars() {
                        if escape { escape = false; continue; }
                        if ch == '\\' && in_string { escape = true; continue; }
                        if ch == '"' { in_string = !in_string; continue; }
                        if in_string { continue; }

                        match ch {
                            '{' | '(' | '[' => brace_depth += 1,
                            '}' | ')' | ']' => {
                                if brace_depth > 0 { brace_depth -= 1; }
                                else {
                                    errors.push(CodeError::SyntaxError {
                                        file: file.clone(),
                                        line: line_idx + 1,
                                        column: 0,
                                        message: "Unmatched closing bracket".to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if brace_depth > 0 {
                    errors.push(CodeError::SyntaxError {
                        file: file.clone(),
                        line: 0,
                        column: 0,
                        message: format!("Unclosed bracket(s): {} remaining", brace_depth),
                    });
                }
            }
        }
        errors
    }

    /// Collect ERROR nodes from tree-sitter parse tree
    fn collect_error_nodes(
        &self,
        node: &tree_sitter::Node,
        _source: &str,
        file: &str,
        errors: &mut Vec<CodeError>,
    ) {
        if node.kind() == "ERROR" {
            let start = node.start_position();
            let end = node.end_position();
            errors.push(CodeError::SyntaxError {
                file: file.to_string(),
                line: start.row + 1,
                column: start.column + 1,
                message: format!(
                    "Syntax error at {}:{}-{}:{}",
                    start.row + 1, start.column + 1,
                    end.row + 1, end.column + 1
                ),
            });
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_error_nodes(&child, _source, file, errors);
        }
    }

    // ── 7. 路由错误检测 ──

    fn detect_route_errors(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let route_re = Regex::new(r#"(get|post|put|delete|patch)\s*[("]\s*([^")\s]+)"#).unwrap();
        let handler_re = Regex::new(r"(async\s+)?fn\s+(\w+)").unwrap();
        let mut routes: Vec<String> = Vec::new();

        for (file, content) in files {
            for cap in route_re.captures_iter(content) {
                let route = cap[2].to_string();
                routes.push(route.clone());

                let after_route = &content[cap.get(0).unwrap().end()..];
                if !handler_re.is_match(after_route.split('\n').next().unwrap_or("")) {
                    if let Some(_line) = content.lines().position(|l| l.contains(&route)) {
                        errors.push(CodeError::RouteError {
                            file: file.clone(), route,
                            handler: None,
                            existing_routes: routes.clone(),
                        });
                    }
                }
            }
        }
        errors
    }
}
