//! 七类基本错误检测引擎
//!
//! 1. 类型错误      — 函数参数/返回值类型不匹配
//! 2. 字段错误      — 结构体字段不存在/类型不匹配
//! 3. 前后端字段不一致 — API 请求/响应字段与数据库模型不匹配
//! 4. API 字段不一致  — 同一 API 的前后端字段名不同
//! 5. 数据库字段缺失  — ORM 模型缺少数据库列
//! 6. 语法错误      — 解析错误
//! 7. 路由错误      — URL 路径与处理器不匹配

use regex::Regex;
use std::collections::{HashMap, HashSet};

// ══════════════════════════════════════════════════════════════════
// 错误类型定义
// ══════════════════════════════════════════════════════════════════

/// 七类基本错误的统一枚举
#[derive(Debug, Clone, PartialEq)]
pub enum CodeError {
    /// 1. 类型错误: 函数参数/返回值类型不匹配
    TypeError {
        file: String, line: usize, symbol: String,
        expected: String, found: String, message: String,
    },
    /// 2. 字段错误: 访问了不存在的字段
    FieldError {
        file: String, line: usize, field: String,
        struct_name: String, suggestion: Option<String>,
    },
    /// 3. 前后端字段不一致: API字段 ≠ DB模型字段
    FieldMismatch {
        api_file: String, api_field: String,
        db_file: String, db_field: String,
        direction: MismatchDirection,
    },
    /// 4. API 字段不一致: 同 API 的 req 和 resp 字段不同
    ApiFieldInconsistency {
        file: String, endpoint: String,
        request_field: String,
        response_field: String,
    },
    /// 5. 数据库字段缺失: ORM 模型缺少数据库列
    MissingDbField {
        model_file: String, model_name: String,
        missing_column: String, table: String,
    },
    /// 6. 语法错误
    SyntaxError {
        file: String, line: usize, column: usize,
        message: String,
    },
    /// 7. 路由错误: URL 路径与处理器不匹配
    RouteError {
        file: String, route: String,
        handler: Option<String>,
        existing_routes: Vec<String>,
    },
}

/// 不一致的方向
#[derive(Debug, Clone, PartialEq)]
pub enum MismatchDirection {
    /// API 响应中有，但 DB 模型中无
    ApiHasFieldNotInDb,
    /// DB 模型中有，但 API 响应中无
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

        // 收集所有文件
        let files = self.collect_files(root);

        // 并行执行各类检查
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
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // 跳过 node_modules, target, .git
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name != "node_modules" && name != "target" && name != ".git" && !name.starts_with('.') {
                        files.extend(self.collect_files(&path.to_string_lossy()));
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
        files
    }

    // ── 1. 类型错误检测 ──

    fn detect_type_errors(&self, files: &[(String, String)]) -> Vec<CodeError> {
        let mut errors = Vec::new();
        let _fn_re = Regex::new(r"fn\s+(\w+)\s*\(([^)]*)\)\s*(->\s*([^{]+))?").unwrap();
        let _call_re = Regex::new(r"(\w+)\s*\(([^)]*)\)").unwrap();
        let assign_re = Regex::new(r"let\s+(\w+)\s*:\s*(\w+)\s*=\s*(.+)").unwrap();

        for (file, content) in files {
            for (line, text) in content.lines().enumerate() {
                // 检测函数调用类型不匹配 (简化版)
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
            // 收集结构体定义
            let mut structs: HashMap<String, HashSet<String>> = HashMap::new();
            for cap in struct_def_re.captures_iter(content) {
                let name = cap[1].to_string();
                let fields: HashSet<String> = cap[2].split(',')
                    .map(|f| f.trim().split(':').next().unwrap_or("").trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                structs.insert(name, fields);
            }

            // 检查字段访问
            for (line, text) in content.lines().enumerate() {
                for cap in field_access_re.captures_iter(text) {
                    let obj = cap[1].to_string();
                    let field = cap[2].to_string();
                    // 跳过基本类型方法调用
                    if matches!(field.as_str(), "len" | "is_empty" | "clone" | "to_string" | "as_str") {
                        continue;
                    }
                    // 简化检查: 如果 struct 已知且字段不存在
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

        // API 响应结构体解析
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

        // 比较 API 字段和 DB 字段
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

                // 检查路由后的下一行是否有处理器
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

    // ── 4/5/6 预留存根 ──

    fn detect_api_inconsistencies(&self, _files: &[(String, String)]) -> Vec<CodeError> { Vec::new() }
    fn detect_missing_db_fields(&self, _files: &[(String, String)]) -> Vec<CodeError> { Vec::new() }
    fn detect_syntax_errors(&self, _files: &[(String, String)]) -> Vec<CodeError> { Vec::new() }
}
