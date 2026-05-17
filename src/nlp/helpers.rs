use crate::nlp::{FuncInfo, ClassInfo, ClassType, CommentInfo, CommentType, ComplexityMetrics};

pub fn count_functions(code: &str) -> usize {
    let mut count = 0;
    for line in code.lines() {
        if line.trim().starts_with("fn ") 
            || line.trim().starts_with("def ")
            || line.trim().starts_with("public ")
            || line.trim().starts_with("private ")
            || line.contains("function ")
            || line.contains("func ")
        {
            count += 1;
        }
    }
    count
}

pub fn count_classes(code: &str) -> usize {
    let mut count = 0;
    for line in code.lines() {
        if line.contains("class ") 
            || line.contains("struct ")
            || line.contains("interface ")
            || line.contains("enum ")
            || line.contains("type ")
        {
            count += 1;
        }
    }
    count
}

pub fn extract_function_signatures(code: &str) -> Vec<FuncInfo> {
    let mut funcs = Vec::new();
    
    // Simple regex-free extraction (real impl would use proper parsing)
    for line in code.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ") {
            let sig = trimmed.to_string();
            funcs.push(FuncInfo {
                name: extract_func_name(&sig).to_string(),
                signature: sig,
                description: None,
                parameters: None,
            });
        }
    }
    
    funcs.truncate(20); // Limit for brevity
    funcs
}

pub fn extract_class_definitions(code: &str) -> Vec<ClassInfo> {
    let mut classes = Vec::new();
    
    for line in code.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("pub class ") 
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("struct ")
        {
            let name = trimmed.split_whitespace()
                .nth(1)
                .unwrap_or("Unknown")
                .split('{')
                .next()
                .unwrap_or("Unknown")
                .trim();
            
            let class_type = if trimmed.contains("class ") {
                ClassType::Class
            } else if trimmed.contains("interface ") {
                ClassType::Interface
            } else if trimmed.contains("enum ") {
                ClassType::Enum
            } else if trimmed.contains("trait ") {
                ClassType::Trait
            } else {
                ClassType::Struct
            };
            
            classes.push(ClassInfo {
                name: name.to_string(),
                class_type,
                methods: Vec::new(), // Would need full parsing
                properties: Vec::new(),
            });
        }
    }
    
    classes.truncate(15);
    classes
}

pub fn extract_imports(code: &str) -> Vec<String> {
    code.lines()
        .filter(|l| l.starts_with("use ") || l.starts_with("import ") || l.starts_with("#include "))
        .map(|l| l.trim().to_string())
        .collect()
}

pub fn extract_exports(code: &str) -> Vec<String> {
    code.lines()
        .filter(|l| l.starts_with("pub ") || l.starts_with("export "))
        .map(|l| l.trim().to_string())
        .collect()
}

pub fn extract_comments(code: &str) -> Vec<CommentInfo> {
    let mut comments = Vec::new();
    
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("//") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed[2..].trim().to_string(),
                comment_type: CommentType::Line,
            });
        } else if trimmed.starts_with("/*") || trimmed.starts_with("*") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed.trim_matches('/').trim_matches('*').trim().to_string(),
                comment_type: CommentType::Block,
            });
        } else if trimmed.starts_with("///") || trimmed.starts_with("/**") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed.trim_start_matches("///").trim_start_matches("/**")
                    .trim_end_matches("*/").trim().to_string(),
                comment_type: CommentType::Doc,
            });
        }
    }
    
    comments.truncate(20);
    comments
}

pub fn calculate_code_complexity(code: &str) -> ComplexityMetrics {
    let lines = code.lines().count();
    let funcs = count_functions(code);
    
    // Simplified cyclomatic complexity estimation
    let cyclomatic = code.matches("if ").count() as f64 * 0.5
        + code.matches("for ").count() as f64 * 0.5
        + code.matches("while ").count() as f64 * 1.0
        + code.matches("match ").count() as f64 * 1.5;
    
    // Cognitive complexity (rough approximation)
    let cognitive = cyclomatic * 1.2;
    
    // Lines of code per function
    let loc_per_func = if funcs > 0 { lines as f64 / funcs as f64 } else { 0.0 };
    
    ComplexityMetrics {
        cyclomatic,
        cognitive,
        loc_per_function: loc_per_func,
    }
}

fn extract_func_name(signature: &str) -> String {
    signature
        .replace("pub ", "")
        .replace("async ", "")
        .split('(')
        .next()
        .unwrap_or("unknown")
        .split_whitespace()
        .rev()
        .next()
        .unwrap_or("unknown")
        .to_string()
}
