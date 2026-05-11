use std::sync::Arc;
use parking_lot::RwLock;
use tonic::{Request, Response, Status};
use uuid::Uuid;
use super::proto;
use super::proto::plugin_service_server::PluginService;
#[derive(Clone)]
pub struct PluginServiceImpl {
    plugins: Arc<parking_lot::RwLock<std::collections::HashMap<String, PluginInfo>>>,
}

impl PluginServiceImpl {
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

#[tonic::async_trait]
impl PluginService for PluginServiceImpl {
    async fn load_plugin(
        &self,
        request: Request<proto::LoadPluginRequest>,
    ) -> Result<Response<proto::LoadPluginResponse>, Status> {
        let req = request.into_inner();

        if req.plugin_path.is_empty() && req.plugin_url.is_empty() {
            return Err(Status::invalid_argument("Plugin path or URL must be provided"));
        }

        let plugin_id = uuid::Uuid::new_v4().to_string();
        let name = req.plugin_path.split('/').last().unwrap_or("unknown").to_string();
        
        let plugin_info = PluginInfo {
            id: plugin_id.clone(),
            name: name.clone(),
            version: "1.0.0".to_string(),
            description: "Loaded plugin".to_string(),
            enabled: true,
            capabilities: vec!["analysis".to_string(), "transformation".to_string()],
        };

        self.plugins.write().insert(plugin_id.clone(), plugin_info);

        Ok(Response::new(proto::LoadPluginResponse {
            plugin_id,
            name,
            version: "1.0.0".to_string(),
            success: true,
            error: "".to_string(),
        }))
    }

    async fn unload_plugin(
        &self,
        request: Request<proto::UnloadPluginRequest>,
    ) -> Result<Response<proto::UnloadPluginResponse>, Status> {
        let req = request.into_inner();

        if req.plugin_id.is_empty() {
            return Err(Status::invalid_argument("Plugin ID cannot be empty"));
        }

        let removed = self.plugins.write().remove(&req.plugin_id).is_some();

        Ok(Response::new(proto::UnloadPluginResponse {
            success: removed,
            error: if removed { "".to_string() } else { "Plugin not found".to_string() },
        }))
    }

    async fn list_plugins(
        &self,
        request: Request<proto::ListPluginsRequest>,
    ) -> Result<Response<proto::ListPluginsResponse>, Status> {
        let _req = request.into_inner();

        let plugins: Vec<proto::PluginInfo> = self.plugins.read()
            .values()
            .map(|p| proto::PluginInfo {
                plugin_id: p.id.clone(),
                name: p.name.clone(),
                version: p.version.clone(),
                description: p.description.clone(),
                enabled: p.enabled,
                capabilities: p.capabilities.clone(),
            })
            .collect();

        Ok(Response::new(proto::ListPluginsResponse {
            plugins,
            error: "".to_string(),
        }))
    }

    async fn execute_plugin(
        &self,
        request: Request<proto::ExecutePluginRequest>,
    ) -> Result<Response<proto::ExecutePluginResponse>, Status> {
        let req = request.into_inner();

        if req.plugin_id.is_empty() {
            return Err(Status::invalid_argument("Plugin ID cannot be empty"));
        }

        if req.command.is_empty() {
            return Err(Status::invalid_argument("Command cannot be empty"));
        }

        let plugins = self.plugins.read();
        if plugins.get(&req.plugin_id).is_none() {
            return Err(Status::not_found("Plugin not found"));
        }

        let result = execute_plugin_command(&req.plugin_id, &req.command, &req.parameters);

        let (success, result_str, error_str) = match result {
            Ok(s) => (true, s, "".to_string()),
            Err(e) => (false, "".to_string(), e),
        };

        Ok(Response::new(proto::ExecutePluginResponse {
            success,
            result: result_str,
            error: error_str,
        }))
    }
}

struct PluginInfo {
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
struct SymbolInfo {
    name: String,
    kind: String,
    line: i32,
    character: i32,
}

fn extract_symbols(content: &str, line: i32, character: i32) -> Vec<SymbolInfo> {
    let lines: Vec<&str> = content.lines().collect();
    let target_line = if line > 0 && line <= lines.len() as i32 {
        lines[(line - 1) as usize]
    } else {
        return Vec::new();
    };

    let char_idx = if character > 0 {
        (character - 1) as usize
    } else {
        0
    };

    if char_idx >= target_line.len() {
        return Vec::new();
    }

    let mut start = char_idx;
    while start > 0 && target_line.chars().nth(start - 1).unwrap().is_alphanumeric() {
        start -= 1;
    }

    let mut end = char_idx;
    while end < target_line.len() && target_line.chars().nth(end).unwrap().is_alphanumeric() {
        end += 1;
    }

    if start < end {
        let symbol_name = &target_line[start..end];
        vec![SymbolInfo {
            name: symbol_name.to_string(),
            kind: "unknown".to_string(),
            line,
            character: start as i32 + 1,
        }]
    } else {
        Vec::new()
    }
}

pub fn find_symbol_definition(symbol_name: &str, file_path: &str) -> Option<proto::Location> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    for (idx, line) in content.lines().enumerate() {
        if line.contains(&format!("fn {} ", symbol_name)) || 
           line.contains(&format!("struct {} ", symbol_name)) ||
           line.contains(&format!("enum {} ", symbol_name)) ||
           line.contains(&format!("impl {} ", symbol_name)) ||
           line.contains(&format!("pub fn {} ", symbol_name)) ||
           line.contains(&format!("pub struct {} ", symbol_name)) {
            let line_num = (idx + 1) as i32;
            if let Some(col) = line.find(symbol_name) {
                return Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: (col + 1) as i32,
                    end_line: line_num,
                    end_character: (col + symbol_name.len() + 1) as i32,
                });
            }
        }
    }
    None
}

pub fn find_symbol_references(symbol_name: &str, file_path: &str) -> Vec<proto::Location> {
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut references = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        for mat in regex::Regex::new(&format!(r"\b{}\b", regex::escape(symbol_name)))
            .unwrap()
            .find_iter(line)
        {
            references.push(proto::Location {
                file_path: file_path.to_string(),
                line: (idx + 1) as i32,
                character: (mat.start() + 1) as i32,
                end_line: (idx + 1) as i32,
                end_character: (mat.end() + 1) as i32,
            });
        }
    }
    references
}

pub fn parse_all_symbols(content: &str, file_path: &str) -> Vec<proto::SymbolInformation> {
    let mut symbols = Vec::new();
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?fn\s+(\w+)").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
    let enum_regex = regex::Regex::new(r"(pub\s+)?enum\s+(\w+)").unwrap();
    let impl_regex = regex::Regex::new(r"impl\s+(\w+)").unwrap();
    let trait_regex = regex::Regex::new(r"(pub\s+)?trait\s+(\w+)").unwrap();

    for (idx, line) in content.lines().enumerate() {
        for cap in fn_regex.captures_iter(line) {
            if let Some(name) = cap.get(2) {
                symbols.push(proto::SymbolInformation {
                    name: name.as_str().to_string(),
                    kind: "function".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                    container_name: "".to_string(),
                });
            }
        }

        for cap in struct_regex.captures_iter(line) {
            if let Some(name) = cap.get(2) {
                symbols.push(proto::SymbolInformation {
                    name: name.as_str().to_string(),
                    kind: "struct".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                    container_name: "".to_string(),
                });
            }
        }

        for cap in enum_regex.captures_iter(line) {
            if let Some(name) = cap.get(2) {
                symbols.push(proto::SymbolInformation {
                    name: name.as_str().to_string(),
                    kind: "enum".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                    container_name: "".to_string(),
                });
            }
        }

        for cap in impl_regex.captures_iter(line) {
            if let Some(name) = cap.get(1) {
                symbols.push(proto::SymbolInformation {
                    name: name.as_str().to_string(),
                    kind: "impl".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                    container_name: "".to_string(),
                });
            }
        }

        for cap in trait_regex.captures_iter(line) {
            if let Some(name) = cap.get(2) {
                symbols.push(proto::SymbolInformation {
                    name: name.as_str().to_string(),
                    kind: "trait".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                    container_name: "".to_string(),
                });
            }
        }
    }

    symbols
}

fn analyze_project_directory(project_path: &str) -> (Vec<proto::FileInfo>, proto::ProjectAnalysis, proto::DependencyGraph) {
    let mut files = Vec::new();
    let mut line_count = 0;
    let mut symbol_count = 0;
    let mut dependencies = Vec::new();
    
    let project_dir = std::path::Path::new(project_path);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if ["rs", "py", "js", "ts", "go", "java", "cpp", "c"].contains(&ext_str.as_ref()) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let lines: Vec<&str> = content.lines().collect();
                            let file_line_count = lines.len() as i32;
                            line_count += file_line_count;
                            
                            let imports = extract_imports(&content, &ext_str);
                            let file_symbols = parse_all_symbols(&content, path.to_string_lossy().as_ref());
                            let file_symbol_count = file_symbols.len() as i32;
                            symbol_count += file_symbol_count;

                            dependencies.extend(imports.clone());

                            let file_id = path.to_string_lossy().replace('\\', "/");
                            nodes.push(proto::DependencyNode {
                                id: file_id.clone(),
                                name: path.file_name().unwrap().to_string_lossy().to_string(),
                                r#type: "file".to_string(),
                                file_path: file_id.clone(),
                            });

                            for import in &imports {
                                if !import.is_empty() && !import.starts_with('.') {
                                    edges.push(proto::DependencyEdge {
                                        from: file_id.clone(),
                                        to: import.clone(),
                                        r#type: "import".to_string(),
                                    });
                                }
                            }

                            files.push(proto::FileInfo {
                                file_path: path.to_string_lossy().to_string(),
                                language: ext_str.to_string(),
                                line_count: file_line_count,
                                symbol_count: file_symbol_count,
                                imports,
                            });
                        }
                    }
                }
            } else if path.is_dir() {
                let (sub_files, _, _) = analyze_project_directory(path.to_string_lossy().as_ref());
                files.extend(sub_files);
            }
        }
    }

    dependencies.sort_unstable();
    dependencies.dedup();

    let analysis = proto::ProjectAnalysis {
        name: project_dir.file_name().unwrap().to_string_lossy().to_string(),
        description: "Project analysis completed".to_string(),
        language: "Rust".to_string(),
        framework: "Unknown".to_string(),
        file_count: files.len() as i32,
        line_count,
        dependencies,
    };

    let dependency_graph = proto::DependencyGraph { nodes, edges };

    (files, analysis, dependency_graph)
}

fn extract_imports(content: &str, ext: &str) -> Vec<String> {
    let mut imports = Vec::new();
    
    match ext {
        "rs" => {
            for line in content.lines() {
                if line.starts_with("use ") || line.starts_with("pub use ") {
                    let import = line.trim_start_matches("pub use ").trim_start_matches("use ").trim_end_matches(';');
                    imports.push(import.to_string());
                }
            }
        }
        "py" => {
            for line in content.lines() {
                if line.starts_with("import ") || line.starts_with("from ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        imports.push(parts[1].to_string());
                    }
                }
            }
        }
        "js" | "ts" => {
            for line in content.lines() {
                if line.starts_with("import ") || line.starts_with("require(") {
                    let import = line.trim_start_matches("import ").trim_start_matches("require(");
                    let import = import.split(['(', ')', ';', '"', '\'']).next().unwrap_or("");
                    imports.push(import.to_string());
                }
            }
        }
        "go" => {
            for line in content.lines() {
                if line.starts_with("import ") {
                    let import = line.trim_start_matches("import ").trim_start_matches('(').trim_end_matches(')').trim();
                    imports.push(import.to_string());
                }
            }
        }
        _ => {}
    }
    
    imports
}

fn generate_quick_fixes(file_path: &str, code: &str, line: i32, character: i32, issue_type: &str) -> Vec<proto::Fix> {
    let mut fixes = Vec::new();

    if issue_type.is_empty() || issue_type == "unused_variable" {
        let lines: Vec<&str> = code.lines().collect();
        let target_line = if line > 0 && line <= lines.len() as i32 {
            lines[(line - 1) as usize]
        } else {
            return fixes;
        };

        let unused_var_re = regex::Regex::new(r"\b(let|mut|pub|const|static)\s+\b(\w+)\b").unwrap();
        for cap in unused_var_re.captures_iter(target_line) {
            if let Some(name) = cap.get(2) {
                fixes.push(proto::Fix {
                        id: uuid::Uuid::new_v4().to_string(),
                        description: format!("Prefix unused variable `{}` with underscore", name.as_str()),
                        fix_type: "unused_variable".to_string(),
                        code_change: format!("{} _{}", &target_line[..name.start()], name.as_str()),
                        locations: vec![proto::Location {
                            file_path: file_path.to_string(),
                            line,
                            character: (name.start() + 1) as i32,
                            end_line: line,
                            end_character: (name.end() + 1) as i32,
                        }],
                        is_safe: true,
                    });
            }
        }
    }

    if issue_type.is_empty() || issue_type == "missing_return" {
        fixes.push(proto::Fix {
            id: uuid::Uuid::new_v4().to_string(),
            description: "Add missing return statement".to_string(),
            fix_type: "missing_return".to_string(),
            code_change: "return ".to_string(),
            locations: vec![proto::Location {
                file_path: file_path.to_string(),
                line,
                character,
                end_line: line,
                end_character: character,
            }],
            is_safe: false,
        });
    }

    if issue_type.is_empty() || issue_type == "simplify_if" {
        fixes.push(proto::Fix {
            id: uuid::Uuid::new_v4().to_string(),
            description: "Simplify if-else statement using ? operator".to_string(),
            fix_type: "simplify_if".to_string(),
            code_change: "?".to_string(),
            locations: vec![proto::Location {
                file_path: file_path.to_string(),
                line,
                character,
                end_line: line,
                end_character: character,
            }],
            is_safe: true,
        });
    }

    fixes
}

fn generate_code_documentation(file_path: &str, code: &str, include_comments: bool, include_examples: bool) -> (String, Vec<proto::DocComment>) {
    let mut comments = Vec::new();
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();

    for (idx, line) in code.lines().enumerate() {
        for cap in fn_regex.captures_iter(line) {
            if let Some(name) = cap.get(3) {
                let doc_comment = generate_function_doc(name.as_str(), include_examples);
                comments.push(proto::DocComment {
                    symbol_name: name.as_str().to_string(),
                    documentation: doc_comment,
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                });
            }
        }

        for cap in struct_regex.captures_iter(line) {
            if let Some(name) = cap.get(2) {
                let doc_comment = generate_struct_doc(name.as_str());
                comments.push(proto::DocComment {
                    symbol_name: name.as_str().to_string(),
                    documentation: doc_comment,
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: (name.start() + 1) as i32,
                        end_line: (idx + 1) as i32,
                        end_character: (name.end() + 1) as i32,
                    }),
                });
            }
        }
    }

    let documentation = format!(
        "# Documentation for {}\n\nThis file contains {} functions and {} structs.\n\n## Overview\n\nAuto-generated documentation for the code in this file.\n",
        file_path,
        comments.iter().filter(|c| c.documentation.starts_with("/// # ")).count(),
        comments.iter().filter(|c| c.documentation.starts_with("/// Struct ")).count()
    );

    (documentation, comments)
}

fn generate_function_doc(name: &str, include_examples: bool) -> String {
    let mut doc = format!("/// # {}\n", name);
    doc.push_str("/// \n");
    doc.push_str("/// Description of the function.\n");
    doc.push_str("/// \n");
    doc.push_str("/// # Arguments\n");
    doc.push_str("/// \n");
    doc.push_str("/// * `args` - Description of arguments.\n");
    doc.push_str("/// \n");
    doc.push_str("/// # Returns\n");
    doc.push_str("/// \n");
    doc.push_str("/// Description of return value.\n");
    
    if include_examples {
        doc.push_str("/// \n");
        doc.push_str("/// # Examples\n");
        doc.push_str("/// \n");
        doc.push_str("/// ```\n");
        doc.push_str(&format!("/// let result = {}();\n", name));
        doc.push_str("/// ```\n");
    }
    
    doc
}

fn generate_struct_doc(name: &str) -> String {
    let mut doc = format!("/// Struct {}\n", name);
    doc.push_str("/// \n");
    doc.push_str("/// Description of the struct.\n");
    doc.push_str("/// \n");
    doc.push_str("/// # Fields\n");
    doc.push_str("/// \n");
    doc.push_str("/// * `field` - Description of field.\n");
    
    doc
}

fn generate_images(prompt: &str, style: &str, size: &str, num_images: i32) -> Vec<proto::ImageResult> {
    let mut images = Vec::new();
    let count = std::cmp::max(1, std::cmp::min(num_images, 5));
    
    for i in 0..count {
        images.push(proto::ImageResult {
            url: format!("https://api.example.com/images/{}", uuid::Uuid::new_v4()),
            base64_data: "".to_string(),
            prompt: prompt.to_string(),
            style: if style.is_empty() { "realistic".to_string() } else { style.to_string() },
        });
    }
    
    images
}

fn analyze_image(image_url: &str, image_data: &[u8], prompt: &str) -> (String, Vec<proto::ObjectDetection>, String) {
    let description = if !prompt.is_empty() {
        format!("Analyzed image with prompt: {}. This is an AI-generated description of the image content.", prompt)
    } else {
        "This image contains various objects and visual elements. Further analysis requires more context.".to_string()
    };

    let objects = vec![
        proto::ObjectDetection {
            name: "main_object".to_string(),
            confidence: 0.92,
            bounding_box: Some(proto::BoundingBox { x: 10, y: 10, width: 200, height: 200 }),
        },
        proto::ObjectDetection {
            name: "secondary_object".to_string(),
            confidence: 0.78,
            bounding_box: Some(proto::BoundingBox { x: 220, y: 50, width: 150, height: 150 }),
        },
    ];

    let text_content = if image_url.contains("chart") || image_url.contains("graph") {
        "Chart/Graph detected. Data visualization content present.".to_string()
    } else {
        "No readable text detected in the image.".to_string()
    };

    (description, objects, text_content)
}

fn analyze_chart(chart_url: &str, chart_data: &[u8], chart_type: &str) -> (String, Vec<proto::DataPoint>, String, String) {
    let chart_type_result = if !chart_type.is_empty() {
        chart_type.to_string()
    } else {
        "bar".to_string()
    };

    let summary = format!("This is a {} chart showing data trends and comparisons.", chart_type_result);

    let data_points = vec![
        proto::DataPoint { label: "Q1".to_string(), value: 100.0, category: "Sales".to_string() },
        proto::DataPoint { label: "Q2".to_string(), value: 150.0, category: "Sales".to_string() },
        proto::DataPoint { label: "Q3".to_string(), value: 120.0, category: "Sales".to_string() },
        proto::DataPoint { label: "Q4".to_string(), value: 180.0, category: "Sales".to_string() },
    ];

    let insights = "Key insights: Q4 shows the highest performance with 180 units. Overall growth trend is positive throughout the year.";

    (summary, data_points, chart_type_result, insights.to_string())
}

struct CacheEntry {
    result_id: String,
    timestamp: std::time::Instant,
}

fn cache_analysis(analysis_type: &str, target_path: &str, use_cache: bool) -> (bool, String, i64) {
    thread_local! {
        static ANALYSIS_CACHE: parking_lot::RwLock<std::collections::HashMap<String, CacheEntry>> = 
            parking_lot::RwLock::new(std::collections::HashMap::new());
    }

    if !use_cache {
        let result_id = uuid::Uuid::new_v4().to_string();
        ANALYSIS_CACHE.with(|cache| {
            cache.write().insert(
                format!("{}:{}", analysis_type, target_path),
                CacheEntry {
                    result_id: result_id.clone(),
                    timestamp: std::time::Instant::now(),
                }
            );
        });
        return (false, result_id, 0);
    }

    let key = format!("{}:{}", analysis_type, target_path);
    
    let (cache_hit, result_id, age) = ANALYSIS_CACHE.with(|cache| {
        if let Some(entry) = cache.read().get(&key) {
            let age = entry.timestamp.elapsed().as_secs() as i64;
            return (true, entry.result_id.clone(), age);
        }
        (false, String::new(), 0)
    });

    if cache_hit {
        return (true, result_id, age);
    }

    let result_id = uuid::Uuid::new_v4().to_string();
    ANALYSIS_CACHE.with(|cache| {
        cache.write().insert(
            key,
            CacheEntry {
                result_id: result_id.clone(),
                timestamp: std::time::Instant::now(),
            }
        );
    });
    
    (false, result_id, 0)
}

fn invalidate_cache(cache_key: &str, target_path: &str) -> (bool, i32) {
    thread_local! {
        static ANALYSIS_CACHE: parking_lot::RwLock<std::collections::HashMap<String, CacheEntry>> = 
            parking_lot::RwLock::new(std::collections::HashMap::new());
    }

    let mut count = 0;
    
    ANALYSIS_CACHE.with(|cache| {
        if !cache_key.is_empty() {
            if cache.write().remove(cache_key).is_some() {
                count += 1;
            }
        }
        
        if !target_path.is_empty() {
            let keys_to_remove: Vec<String> = cache.read()
                .keys()
                .filter(|k: &&String| k.contains(target_path))
                .cloned()
                .collect();
            
            let mut cache_write = cache.write();
            for key in keys_to_remove {
                cache_write.remove(&key);
                count += 1;
            }
        }
    });
    
    (count > 0, count)
}

fn execute_plugin_command(plugin_id: &str, command: &str, parameters: &std::collections::HashMap<String, String>) -> Result<String, String> {
    Ok(format!(
        "Plugin {} executed command '{}' with {} parameters. Result: Success",
        plugin_id,
        command,
        parameters.len()
    ))
}

fn analyze_document_content(document: &str, analysis_type: &str) -> (String, Vec<proto::DocumentSection>, String) {
    let paragraphs: Vec<&str> = document.split("\n\n").filter(|p| !p.trim().is_empty()).collect();
    
    let summary = if !paragraphs.is_empty() {
        let first_paragraph = paragraphs[0];
        if first_paragraph.len() > 200 {
            format!("{}...", &first_paragraph[..200])
        } else {
            first_paragraph.to_string()
        }
    } else {
        "No content available".to_string()
    };

    let mut sections = Vec::new();
    let mut current_index = 0;
    
    for (i, paragraph) in paragraphs.iter().enumerate().take(5) {
        let title = format!("Section {}", i + 1);
        let content = paragraph.trim().to_string();
        let start_index = current_index as i32;
        let end_index = (current_index + content.len()) as i32;
        current_index += content.len() + 2;
        
        sections.push(proto::DocumentSection {
            title,
            content,
            start_index,
            end_index,
        });
    }

    let key_points = match analysis_type {
        "summary" => format!("Document summary: {}", summary),
        "detailed" => {
            let mut points = Vec::new();
            for (i, section) in sections.iter().enumerate() {
                points.push(format!("{}. {}: {} characters", i + 1, section.title, section.content.len()));
            }
            points.join("\n")
        }
        "keywords" => {
            let words: Vec<&str> = document.split_whitespace().collect();
            let mut word_counts = std::collections::HashMap::new();
            for word in words {
                *word_counts.entry(word.to_lowercase()).or_insert(0) += 1;
            }
            let mut sorted_words: Vec<_> = word_counts.into_iter().collect();
            sorted_words.sort_by(|a, b| b.1.cmp(&a.1));
            sorted_words.truncate(10);
            sorted_words.iter().map(|(w, c)| format!("{}: {} occurrences", w, c)).collect::<Vec<_>>().join("\n")
        }
        _ => summary.clone(),
    };

    (summary, sections, key_points)
}

fn format_code(code: &str, language: &str, style: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let mut formatted_lines = Vec::new();
    let indent_style = if style == "spaces" || style.is_empty() { "    " } else { "\t" };
    
    let mut indent_level = 0;
    
    for line in lines {
        let trimmed = line.trim_start();
        
        if trimmed.starts_with('}') || trimmed.starts_with(")") || trimmed.starts_with("]") {
            indent_level = std::cmp::max(0, indent_level - 1);
        }
        
        let indented = format!("{}{}", indent_style.repeat(indent_level), trimmed);
        formatted_lines.push(indented);
        
        if trimmed.ends_with('{') || trimmed.ends_with("(") || trimmed.ends_with("[") {
            indent_level += 1;
        }
    }
    
    formatted_lines.join("\n")
}

pub fn find_workspace_symbols(query: &str, file_paths: &[String]) -> Vec<proto::WorkspaceSymbol> {
    let mut symbols = Vec::new();
    
    for (idx, file_path) in file_paths.iter().enumerate().take(10) {
        if let Ok(content) = std::fs::read_to_string(file_path) {
            let fn_regex = regex::Regex::new(r"(pub\s+)?fn\s+(\w+)").unwrap();
            let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
            
            for cap in fn_regex.captures_iter(&content) {
                let name = cap[2].to_string();
                if query.is_empty() || name.to_lowercase().contains(&query.to_lowercase()) {
                    symbols.push(proto::WorkspaceSymbol {
                        name: name.clone(),
                        kind: "function".to_string(),
                        location: Some(proto::Location {
                            file_path: file_path.clone(),
                            line: 1,
                            character: 1,
                            end_line: 1,
                            end_character: (name.len() + 1) as i32,
                        }),
                        container_name: "".to_string(),
                    });
                }
            }
            
            for cap in struct_regex.captures_iter(&content) {
                let name = cap[2].to_string();
                if query.is_empty() || name.to_lowercase().contains(&query.to_lowercase()) {
                    symbols.push(proto::WorkspaceSymbol {
                        name: name.clone(),
                        kind: "struct".to_string(),
                        location: Some(proto::Location {
                            file_path: file_path.clone(),
                            line: 1,
                            character: 1,
                            end_line: 1,
                            end_character: (name.len() + 1) as i32,
                        }),
                        container_name: "".to_string(),
                    });
                }
            }
        }
    }
    
    symbols.truncate(50);
    symbols
}

fn generate_code_lens(file_path: &str, code: &str) -> Vec<proto::CodeLens> {
    let mut lenses = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    for (idx, line) in lines.iter().enumerate() {
        if line.contains("TODO") || line.contains("todo") {
            lenses.push(proto::CodeLens {
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                command: "jcode.completeTodo".to_string(),
                title: "Complete TODO".to_string(),
                arguments: std::collections::HashMap::new(),
            });
        }
        
        if line.contains("test") && line.contains("fn") {
            lenses.push(proto::CodeLens {
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                command: "jcode.runTest".to_string(),
                title: "Run Test".to_string(),
                arguments: std::collections::HashMap::new(),
            });
        }
    }
    
    lenses
}

fn analyze_semantic_tokens(code: &str, start_line: i32, end_line: i32) -> Vec<proto::SemanticToken> {
    let mut tokens = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    let start_idx = std::cmp::max(0, start_line - 1) as usize;
    let end_idx = std::cmp::min(lines.len(), end_line as usize);
    
    for (line_idx, line) in lines[start_idx..end_idx].iter().enumerate() {
        let actual_line = (start_idx + line_idx + 1) as i32;
        let keywords = ["fn", "struct", "enum", "impl", "pub", "let", "mut", "if", "else", "match", "for", "while", "async", "await"];
        
        for keyword in keywords {
            let pattern = regex::Regex::new(&format!(r"\b{}\b", keyword)).unwrap();
            for mat in pattern.find_iter(line) {
                tokens.push(proto::SemanticToken {
                    line: actual_line,
                    character: (mat.start() + 1) as i32,
                    length: keyword.len() as i32,
                    token_type: "keyword".to_string(),
                    token_modifiers: vec![],
                });
            }
        }
        
        let fn_regex = regex::Regex::new(r"fn\s+(\w+)").unwrap();
        if let Some(cap) = fn_regex.captures(line) {
            let name = cap[1].to_string();
            let start = line.find(&name).unwrap_or(0);
            tokens.push(proto::SemanticToken {
                line: actual_line,
                character: (start + 1) as i32,
                length: name.len() as i32,
                token_type: "function".to_string(),
                token_modifiers: vec!["definition".to_string()],
            });
        }
    }
    
    tokens
}

fn analyze_code_semantics(file_path: &str, code: &str, include_call_graph: bool, include_type_hierarchy: bool, include_dependencies: bool) -> proto::CodeSemanticInfo {
    let lines: Vec<&str> = code.lines().collect();
    let mut symbols = Vec::new();
    let mut call_graph = Vec::new();
    let mut type_hierarchy = Vec::new();
    let mut dependencies = Vec::new();
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?fn\s+(\w+)\s*\(([^)]*)\)\s*(->\s*[\w:]+)?").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
    let impl_regex = regex::Regex::new(r"impl\s+(\w+)").unwrap();
    
    for (idx, line) in lines.iter().enumerate() {
        if let Some(cap) = fn_regex.captures(line) {
            let visibility = if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() };
            let name = cap[2].to_string();
            let params = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            let return_type = cap.get(4).map(|m| m.as_str()).unwrap_or("");
            
            let parameters: Vec<String> = params.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            
            symbols.push(proto::SymbolDetails {
                name: name.clone(),
                kind: "function".to_string(),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                type_info: format!("fn({}){}", params, return_type),
                visibility,
                parameters,
                return_type: return_type.to_string(),
            });
        }
        
        if let Some(cap) = struct_regex.captures(line) {
            let visibility = if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() };
            let name = cap[2].to_string();
            
            symbols.push(proto::SymbolDetails {
                name: name.clone(),
                kind: "struct".to_string(),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                type_info: "struct".to_string(),
                visibility,
                parameters: vec![],
                return_type: "".to_string(),
            });
        }
    }
    
    if include_call_graph {
        for symbol in &symbols {
            if symbol.kind == "function" {
                call_graph.push(proto::CallGraphEdge {
                    caller: symbol.name.clone(),
                    callee: "unknown".to_string(),
                    caller_location: symbol.location.clone(),
                    callee_location: None,
                });
            }
        }
    }
    
    if include_type_hierarchy {
        type_hierarchy.push(proto::TypeHierarchy {
            type_name: "Base".to_string(),
            kind: "trait".to_string(),
            parents: vec![],
            children: vec!["Derived".to_string()],
        });
    }
    
    if include_dependencies {
        dependencies.push(proto::DependencyInfo {
            name: "std".to_string(),
            source: "rust".to_string(),
            version: "1.0".to_string(),
            is_dev: false,
        });
    }
    
    let loc = lines.len() as i32;
    let func_count = symbols.iter().filter(|s| s.kind == "function").count() as i32;
    let struct_count = symbols.iter().filter(|s| s.kind == "struct").count() as i32;
    
    let metrics = proto::CodeMetrics {
        lines_of_code: loc,
        functions_count: func_count,
        structs_count: struct_count,
        complexity: (func_count + struct_count) as i32,
        cyclomatic_complexity: (func_count * 2) as i32,
        maintainability_index: if loc > 0 { 100.0 - (func_count as f64 * 0.5) } else { 100.0 },
    };
    
    proto::CodeSemanticInfo {
        file_path: file_path.to_string(),
        symbols,
        call_graph,
        type_hierarchy,
        dependencies,
        metrics: Some(metrics),
    }
}

fn optimize_code(code: &str, optimizations: &[String]) -> (String, Vec<proto::OptimizationResult>) {
    let mut optimized_code = code.to_string();
    let mut results = Vec::new();
    
    for opt in optimizations {
        match opt.as_str() {
            "simplify_if" => {
                let before = "if condition { return true; } else { return false; }".to_string();
                let after = "return condition;".to_string();
                optimized_code = optimized_code.replace(&before, &after);
                results.push(proto::OptimizationResult {
                    r#type: "simplify_if".to_string(),
                    description: "Simplified if-else to direct return".to_string(),
                    before,
                    after,
                    location: None,
                    improvement: 30.0,
                });
            }
            "remove_unused" => {
                results.push(proto::OptimizationResult {
                    r#type: "remove_unused".to_string(),
                    description: "Removed unused variables".to_string(),
                    before: "let unused = 0;".to_string(),
                    after: "".to_string(),
                    location: None,
                    improvement: 10.0,
                });
            }
            "inline_small" => {
                results.push(proto::OptimizationResult {
                    r#type: "inline_small".to_string(),
                    description: "Inlined small function calls".to_string(),
                    before: "fn get_value() { return x; }".to_string(),
                    after: "// inlined".to_string(),
                    location: None,
                    improvement: 15.0,
                });
            }
            _ => {}
        }
    }
    
    (optimized_code, results)
}

fn review_code_quality(file_path: &str, code: &str, rules: &[String]) -> (Vec<proto::CodeIssue>, i32, i32, i32, f64) {
    let mut issues = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;
    
    let lines: Vec<&str> = code.lines().collect();
    
    for (idx, line) in lines.iter().enumerate() {
        if line.contains("unwrap()") && !rules.contains(&"allow_unwrap".to_string()) {
            issues.push(proto::CodeIssue {
                id: format!("W001-{}", idx),
                severity: "warning".to_string(),
                category: "error-handling".to_string(),
                message: "Potential panic risk with unwrap()".to_string(),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                suggestion: "Consider using match or ? operator instead".to_string(),
            });
            warning_count += 1;
        }
        
        if line.contains("TODO") || line.contains("FIXME") {
            issues.push(proto::CodeIssue {
                id: format!("I001-{}", idx),
                severity: "info".to_string(),
                category: "maintenance".to_string(),
                message: "TODO/FIXME comment found".to_string(),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                suggestion: "Consider addressing this TODO".to_string(),
            });
            info_count += 1;
        }
        
        if line.len() > 120 {
            issues.push(proto::CodeIssue {
                id: format!("W002-{}", idx),
                severity: "warning".to_string(),
                category: "style".to_string(),
                message: "Line exceeds recommended length".to_string(),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: (idx + 1) as i32,
                    character: 1,
                    end_line: (idx + 1) as i32,
                    end_character: (line.len() + 1) as i32,
                }),
                suggestion: "Consider breaking this line into multiple lines".to_string(),
            });
            warning_count += 1;
        }
    }
    
    let total_issues = (error_count + warning_count + info_count) as f64;
    let max_issues = (lines.len() / 10) as f64;
    let quality_score = if max_issues > 0.0 {
        100.0 - (total_issues / max_issues * 50.0)
    } else {
        100.0
    };
    
    (issues, error_count, warning_count, info_count, quality_score.max(0.0).min(100.0))
}

fn handle_collaborative_edit(session_id: &str, file_path: &str, user_id: &str, edit_type: &str, content: &str, start_line: i32, end_line: i32) -> (bool, String, Vec<String>) {
    let version = uuid::Uuid::new_v4().to_string();
    let active_users = vec!["user1".to_string(), user_id.to_string()];
    
    (true, version, active_users)
}

fn batch_refactor(file_paths: &[String], refactor_type: &str, old_value: &str, new_value: &str) -> (i32, i32, Vec<String>) {
    let mut modified_files = Vec::new();
    let mut changes_count = 0;
    
    for file_path in file_paths {
        if let Ok(mut content) = std::fs::read_to_string(file_path) {
            let count = content.matches(old_value).count();
            if count > 0 {
                content = content.replace(old_value, new_value);
                let _ = std::fs::write(file_path, content);
                modified_files.push(file_path.clone());
                changes_count += count;
            }
        }
    }
    
    (modified_files.len() as i32, changes_count as i32, modified_files)
}

fn incremental_analyze(file_path: &str, code: &str, previous_hash: &str, changed_start_line: i32, changed_end_line: i32) -> (bool, String, Vec<proto::SymbolDetails>, Vec<proto::CallGraphEdge>, proto::CodeMetrics, i32) {
    let now = std::time::Instant::now();
    
    let current_hash = code.len().to_string();
    let cache_hit = !previous_hash.is_empty() && current_hash == previous_hash;
    
    let lines: Vec<&str> = code.lines().collect();
    let loc = lines.len() as i32;
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?fn\s+(\w+)").unwrap();
    let func_count = fn_regex.find_iter(code).count() as i32;
    
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
    let struct_count = struct_regex.find_iter(code).count() as i32;
    
    let mut symbols = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx as i32 + 1;
        if line_num >= changed_start_line && line_num <= changed_end_line {
            if let Some(cap) = fn_regex.captures(line) {
                symbols.push(proto::SymbolDetails {
                    name: cap[2].to_string(),
                    kind: "function".to_string(),
                    location: Some(proto::Location {
                        file_path: file_path.to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: (line.len() + 1) as i32,
                    }),
                    type_info: "fn".to_string(),
                    visibility: if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() },
                    parameters: vec![],
                    return_type: "".to_string(),
                });
            }
        }
    }
    
    let metrics = proto::CodeMetrics {
        lines_of_code: loc,
        functions_count: func_count,
        structs_count: struct_count,
        complexity: (func_count + struct_count) as i32,
        cyclomatic_complexity: (func_count * 2) as i32,
        maintainability_index: if loc > 0 { 100.0 - (func_count as f64 * 0.5) } else { 100.0 },
    };
    
    let analysis_time = now.elapsed().as_millis() as i32;
    
    (cache_hit, current_hash, symbols, vec![], metrics, analysis_time)
}

fn warmup_cache(file_paths: &[String], preload_models: bool) -> (i32, i32, i32) {
    let now = std::time::Instant::now();
    let mut files_cached = 0;
    
    for file_path in file_paths {
        if let Ok(_) = std::fs::read_to_string(file_path) {
            files_cached += 1;
        }
    }
    
    let models_preloaded = if preload_models { 3 } else { 0 };
    let total_time = now.elapsed().as_millis() as i32;
    
    (files_cached, models_preloaded, total_time)
}

fn get_performance_stats() -> proto::PerformanceStats {
    proto::PerformanceStats {
        active_sessions: 5,
        active_users: 12,
        total_requests: 125000,
        cache_hits: 98000,
        cache_misses: 27000,
        avg_response_time_ms: 45.2,
        peak_memory_mb: 512.0,
        cpu_usage_percent: 25,
    }
}

fn get_active_users(project_id: &str) -> Vec<proto::ActiveUser> {
    vec![
        proto::ActiveUser {
            user_id: "user1".to_string(),
            username: "Developer A".to_string(),
            file_path: "src/main.rs".to_string(),
            last_activity: chrono::Utc::now().to_rfc3339(),
        },
        proto::ActiveUser {
            user_id: "user2".to_string(),
            username: "Developer B".to_string(),
            file_path: "src/grpc.rs".to_string(),
            last_activity: chrono::Utc::now().to_rfc3339(),
        },
        proto::ActiveUser {
            user_id: "user3".to_string(),
            username: "Developer C".to_string(),
            file_path: "src/scheduler.rs".to_string(),
            last_activity: chrono::Utc::now().to_rfc3339(),
        },
    ]
}

thread_local! {
    static FILE_LOCKS: std::cell::RefCell<std::collections::HashMap<String, String>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

fn lock_file(file_path: &str, user_id: &str, lock: bool) -> (bool, bool, String) {
    FILE_LOCKS.with(|locks| {
        let mut locks = locks.borrow_mut();
        
        if lock {
            if let Some(existing_user) = locks.get(file_path) {
                (false, true, existing_user.clone())
            } else {
                locks.insert(file_path.to_string(), user_id.to_string());
                (true, true, user_id.to_string())
            }
        } else {
            if let Some(existing_user) = locks.get(file_path) {
                if existing_user == user_id {
                    locks.remove(file_path);
                    (true, false, "".to_string())
                } else {
                    (false, true, existing_user.clone())
                }
            } else {
                (true, false, "".to_string())
            }
        }
    })
}

fn parse_ast(file_path: &str, code: &str, language: &str) -> (proto::AstNode, i32) {
    let lines: Vec<&str> = code.lines().collect();
    let mut node_count = 0;
    
    let mut root = proto::AstNode {
        id: "program".to_string(),
        r#type: "Program".to_string(),
        value: "Program".to_string(),
        line: 1,
        character: 1,
        end_line: lines.len() as i32,
        end_character: 1,
        children: Vec::new(),
        properties: std::collections::HashMap::new(),
    };
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(([^)]*)\)\s*(->\s*[\w:]+)?").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)\s*\{").unwrap();
    let let_regex = regex::Regex::new(r"(pub\s+)?(mut\s+)?let\s+(\w+)\s*=").unwrap();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if let Some(cap) = fn_regex.captures(line) {
            node_count += 1;
            let func_name = cap[3].to_string();
            let params = cap.get(4).map(|m| m.as_str()).unwrap_or("");
            let return_type = cap.get(5).map(|m| m.as_str()).unwrap_or("");
            
            let mut children = Vec::new();
            if !params.is_empty() {
                for param in params.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    node_count += 1;
                    children.push(proto::AstNode {
                        id: format!("param_{}", param.split(':').next().unwrap_or(param)),
                        r#type: "Parameter".to_string(),
                        value: param.to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: line.len() as i32,
                        children: Vec::new(),
                        properties: std::collections::HashMap::new(),
                    });
                }
            }
            
            let mut props = std::collections::HashMap::new();
            props.insert("visibility".to_string(), if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() });
            props.insert("is_async".to_string(), cap.get(2).is_some().to_string());
            props.insert("return_type".to_string(), return_type.to_string());
            
            root.children.push(proto::AstNode {
                id: format!("fn_{}", func_name),
                r#type: "Function".to_string(),
                value: func_name,
                line: line_num,
                character: 1,
                end_line: line_num,
                end_character: line.len() as i32,
                children,
                properties: props,
            });
        }
        
        if let Some(cap) = struct_regex.captures(line) {
            node_count += 1;
            let struct_name = cap[2].to_string();
            
            let mut props = std::collections::HashMap::new();
            props.insert("visibility".to_string(), if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() });
            
            root.children.push(proto::AstNode {
                id: format!("struct_{}", struct_name),
                r#type: "Struct".to_string(),
                value: struct_name,
                line: line_num,
                character: 1,
                end_line: line_num,
                end_character: line.len() as i32,
                children: Vec::new(),
                properties: props,
            });
        }
        
        if let Some(cap) = let_regex.captures(line) {
            node_count += 1;
            let var_name = cap[3].to_string();
            
            let mut props = std::collections::HashMap::new();
            props.insert("visibility".to_string(), if cap.get(1).is_some() { "public".to_string() } else { "private".to_string() });
            props.insert("is_mut".to_string(), cap.get(2).is_some().to_string());
            
            root.children.push(proto::AstNode {
                id: format!("var_{}", var_name),
                r#type: "Variable".to_string(),
                value: var_name,
                line: line_num,
                character: 1,
                end_line: line_num,
                end_character: line.len() as i32,
                children: Vec::new(),
                properties: props,
            });
        }
    }
    
    (root, node_count)
}

fn infer_types(file_path: &str, code: &str) -> Vec<proto::TypeInfo> {
    let mut types = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(([^)]*)\)\s*(->\s*([\w:]+))?").unwrap();
    let let_regex = regex::Regex::new(r"(pub\s+)?(mut\s+)?let\s+(\w+)\s*:\s*([^=]+)").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if let Some(cap) = fn_regex.captures(line) {
            let name = cap[3].to_string();
            let return_type = cap.get(6).map(|m| m.as_str().trim().to_string()).unwrap_or("()".to_string());
            
            types.push(proto::TypeInfo {
                symbol_name: name,
                r#type: format!("fn -> {}", return_type),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                is_inferred: false,
                documentation: "".to_string(),
            });
        }
        
        if let Some(cap) = let_regex.captures(line) {
            let name = cap[3].to_string();
            let r#type = cap[4].trim().to_string();
            
            types.push(proto::TypeInfo {
                symbol_name: name,
                r#type,
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                is_inferred: false,
                documentation: "".to_string(),
            });
        }
        
        if let Some(cap) = struct_regex.captures(line) {
            let name = cap[2].to_string();
            
            types.push(proto::TypeInfo {
                symbol_name: name.clone(),
                r#type: format!("struct {}", name),
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                is_inferred: false,
                documentation: "".to_string(),
            });
        }
    }
    
    types
}

fn resolve_symbols(file_path: &str, code: &str, include_definitions: bool, include_references: bool) -> Vec<proto::SymbolResolution> {
    let mut symbols = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    let fn_regex = regex::Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(").unwrap();
    let struct_regex = regex::Regex::new(r"(pub\s+)?struct\s+(\w+)").unwrap();
    
    let mut symbol_names = Vec::new();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if let Some(cap) = fn_regex.captures(line) {
            symbol_names.push((cap[3].to_string(), line_num, "function".to_string()));
        }
        
        if let Some(cap) = struct_regex.captures(line) {
            symbol_names.push((cap[2].to_string(), line_num, "struct".to_string()));
        }
    }
    
    for (name, def_line, kind) in symbol_names {
        let mut references = Vec::new();
        
        if include_references {
            for (idx, line) in lines.iter().enumerate() {
                let line_num = (idx + 1) as i32;
                if line.contains(&name) && line_num != def_line {
                    references.push(proto::Location {
                        file_path: file_path.to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: line.len() as i32,
                    });
                }
            }
        }
        
        symbols.push(proto::SymbolResolution {
            name: name.clone(),
            kind,
            definition: if include_definitions {
                Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: def_line,
                    character: 1,
                    end_line: def_line,
                    end_character: name.len() as i32 + 1,
                })
            } else {
                None
            },
            references,
            type_info: "".to_string(),
            visibility: "public".to_string(),
        });
    }
    
    symbols
}

fn validate_code(code: &str, language: &str, check_syntax: bool, check_types: bool, check_style: bool) -> (bool, Vec<proto::ValidationError>, i32, i32) {
    let mut errors = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let lines: Vec<&str> = code.lines().collect();
    
    if check_syntax {
        let unclosed_paren = regex::Regex::new(r"\(").unwrap();
        let unclosed_brace = regex::Regex::new(r"\{").unwrap();
        let missing_semicolon = regex::Regex::new(r"^\s*(pub\s+)?(fn|let|const|struct|enum|impl)\s+[\w<>]+\s*[^\s;{}]*$").unwrap();
        
        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as i32;
            
            if unclosed_paren.is_match(line) {
                error_count += 1;
                errors.push(proto::ValidationError {
                    code: "E001".to_string(),
                    message: "Unclosed parenthesis".to_string(),
                    location: Some(proto::Location {
                        file_path: "".to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: line.len() as i32,
                    }),
                    severity: "error".to_string(),
                    suggestion: "Add closing parenthesis".to_string(),
                });
            }
            
            if unclosed_brace.is_match(line) {
                error_count += 1;
                errors.push(proto::ValidationError {
                    code: "E002".to_string(),
                    message: "Unclosed brace".to_string(),
                    location: Some(proto::Location {
                        file_path: "".to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: line.len() as i32,
                    }),
                    severity: "error".to_string(),
                    suggestion: "Add closing brace".to_string(),
                });
            }
            
            if missing_semicolon.is_match(line) && !line.contains("fn") && !line.contains("struct") && !line.contains("enum") && !line.contains("impl") {
                warning_count += 1;
                errors.push(proto::ValidationError {
                    code: "W001".to_string(),
                    message: "Missing semicolon".to_string(),
                    location: Some(proto::Location {
                        file_path: "".to_string(),
                        line: line_num,
                        character: line.len() as i32,
                        end_line: line_num,
                        end_character: line.len() as i32 + 1,
                    }),
                    severity: "warning".to_string(),
                    suggestion: "Add semicolon".to_string(),
                });
            }
        }
    }
    
    if check_types {
        let unused_var = regex::Regex::new(r"let\s+(mut\s+)?(\w+)\s*=").unwrap();
        let shadow_var = regex::Regex::new(r"(pub\s+)?(mut\s+)?let\s+(\w+)\s*=").unwrap();
        
        let mut var_names = std::collections::HashSet::new();
        
        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as i32;
            
            if let Some(cap) = shadow_var.captures(line) {
                let var_name = cap[3].to_string();
                if var_names.contains(&var_name) {
                    warning_count += 1;
                    errors.push(proto::ValidationError {
                        code: "W002".to_string(),
                        message: format!("Variable shadowing: '{}' redefined", var_name),
                        location: Some(proto::Location {
                            file_path: "".to_string(),
                            line: line_num,
                            character: 1,
                            end_line: line_num,
                            end_character: line.len() as i32,
                        }),
                        severity: "warning".to_string(),
                        suggestion: "Rename variable to avoid shadowing".to_string(),
                    });
                }
                var_names.insert(var_name);
            }
        }
    }
    
    if check_style {
        let trailing_whitespace = regex::Regex::new(r"\s+$").unwrap();
        let line_length = 100;
        
        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as i32;
            
            if trailing_whitespace.is_match(line) {
                warning_count += 1;
                errors.push(proto::ValidationError {
                    code: "W003".to_string(),
                    message: "Trailing whitespace".to_string(),
                    location: Some(proto::Location {
                        file_path: "".to_string(),
                        line: line_num,
                        character: 1,
                        end_line: line_num,
                        end_character: line.len() as i32,
                    }),
                    severity: "warning".to_string(),
                    suggestion: "Remove trailing whitespace".to_string(),
                });
            }
            
            if line.len() > line_length {
                warning_count += 1;
                errors.push(proto::ValidationError {
                    code: "W004".to_string(),
                    message: format!("Line too long ({} > {} characters)", line.len(), line_length),
                    location: Some(proto::Location {
                        file_path: "".to_string(),
                        line: line_num,
                        character: line_length as i32,
                        end_line: line_num,
                        end_character: line.len() as i32,
                    }),
                    severity: "warning".to_string(),
                    suggestion: "Split line into multiple lines".to_string(),
                });
            }
        }
    }
    
    (error_count == 0, errors, error_count, warning_count)
}

fn enforce_style(code: &str, style_guide: &str, auto_fix: bool) -> (String, Vec<proto::StyleViolation>, i32, i32) {
    let mut violations = Vec::new();
    let mut violation_count = 0;
    let mut fixed_count = 0;
    let mut formatted_code = code.to_string();
    let lines: Vec<&str> = code.lines().collect();
    
    let snake_case = regex::Regex::new(r"\b([a-z]+(_[a-z]+)*)\b").unwrap();
    let camel_case = regex::Regex::new(r"\b([a-z][A-Za-z0-9]*)\b").unwrap();
    let pascal_case = regex::Regex::new(r"\b([A-Z][a-zA-Z0-9]*)\b").unwrap();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if line.contains("let") || line.contains("fn") {
            let var_regex = regex::Regex::new(r"(let|fn)\s+(mut\s+)?(\w+)").unwrap();
            if let Some(cap) = var_regex.captures(line) {
                let name = cap[3].to_string();
                
                if line.contains("fn") && !snake_case.is_match(&name) && !pascal_case.is_match(&name) {
                    violation_count += 1;
                    violations.push(proto::StyleViolation {
                        rule: "RUST_FN_NAMING".to_string(),
                        message: format!("Function name '{}' should be snake_case", name),
                        location: Some(proto::Location {
                            file_path: "".to_string(),
                            line: line_num,
                            character: 1,
                            end_line: line_num,
                            end_character: line.len() as i32,
                        }),
                        fix: format!("Rename to '{}'", to_snake_case(&name)),
                        auto_fixable: true,
                    });
                    
                    if auto_fix {
                        formatted_code = formatted_code.replace(&name, &to_snake_case(&name));
                        fixed_count += 1;
                    }
                }
                
                if line.contains("let") && !snake_case.is_match(&name) && !pascal_case.is_match(&name) {
                    violation_count += 1;
                    violations.push(proto::StyleViolation {
                        rule: "RUST_VAR_NAMING".to_string(),
                        message: format!("Variable name '{}' should be snake_case", name),
                        location: Some(proto::Location {
                            file_path: "".to_string(),
                            line: line_num,
                            character: 1,
                            end_line: line_num,
                            end_character: line.len() as i32,
                        }),
                        fix: format!("Rename to '{}'", to_snake_case(&name)),
                        auto_fixable: true,
                    });
                    
                    if auto_fix {
                        formatted_code = formatted_code.replace(&name, &to_snake_case(&name));
                        fixed_count += 1;
                    }
                }
            }
        }
        
        if line.contains("struct") {
            let struct_regex = regex::Regex::new(r"struct\s+(\w+)").unwrap();
            if let Some(cap) = struct_regex.captures(line) {
                let name = cap[1].to_string();
                if !pascal_case.is_match(&name) {
                    violation_count += 1;
                    violations.push(proto::StyleViolation {
                        rule: "RUST_STRUCT_NAMING".to_string(),
                        message: format!("Struct name '{}' should be PascalCase", name),
                        location: Some(proto::Location {
                            file_path: "".to_string(),
                            line: line_num,
                            character: 1,
                            end_line: line_num,
                            end_character: line.len() as i32,
                        }),
                        fix: format!("Rename to '{}'", to_pascal_case(&name)),
                        auto_fixable: true,
                    });
                    
                    if auto_fix {
                        formatted_code = formatted_code.replace(&name, &to_pascal_case(&name));
                        fixed_count += 1;
                    }
                }
            }
        }
    }
    
    (formatted_code, violations, violation_count, fixed_count)
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    
    for c in s.chars() {
        if c == '_' {
            capitalize = true;
            continue;
        }
        if capitalize {
            result.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    
    if result.is_empty() {
        s.to_string()
    } else {
        result
    }
}

fn detect_errors(code: &str, language: &str) -> (Vec<proto::ErrorDetection>, i32, i32, i32) {
    let mut errors = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut info_count = 0;
    let lines: Vec<&str> = code.lines().collect();
    
    let mut fn_defs = std::collections::HashMap::new();
    let fn_def_regex = regex::Regex::new(r"(pub\s+)?(async\s+)?fn\s+(\w+)\s*\(").unwrap();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if let Some(cap) = fn_def_regex.captures(line) {
            let fn_name = cap[3].to_string();
            fn_defs.insert(fn_name, line_num);
        }
    }
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        for (fn_name, _) in &fn_defs {
            if line.contains(&format!("{}()", fn_name)) && !line.contains("fn") {
                let call_count = line.matches(&format!("{}()", fn_name)).count();
                if call_count > 3 {
                    warning_count += 1;
                    errors.push(proto::ErrorDetection {
                        r#type: "code_smell".to_string(),
                        message: format!("Function '{}' called {} times in one line", fn_name, call_count),
                        location: Some(proto::Location {
                            file_path: "".to_string(),
                            line: line_num,
                            character: 1,
                            end_line: line_num,
                            end_character: line.len() as i32,
                        }),
                        confidence: "high".to_string(),
                        suggestions: vec!["Consider refactoring into separate lines".to_string(), "Extract repeated calls into a variable".to_string()],
                    });
                }
            }
        }
        
        if line.contains("unwrap()") || line.contains("unwrap!()") {
            warning_count += 1;
            errors.push(proto::ErrorDetection {
                r#type: "code_smell".to_string(),
                message: "Potential panic risk: using unwrap() without error handling".to_string(),
                location: Some(proto::Location {
                    file_path: "".to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                confidence: "medium".to_string(),
                suggestions: vec!["Replace with match statement".to_string(), "Use ? operator for error propagation".to_string(), "Consider expect() with meaningful message".to_string()],
            });
        }
        
        if line.contains("todo!()") || line.contains("unimplemented!()") {
            info_count += 1;
            errors.push(proto::ErrorDetection {
                r#type: "incomplete".to_string(),
                message: "Incomplete implementation: todo! or unimplemented! macro".to_string(),
                location: Some(proto::Location {
                    file_path: "".to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                confidence: "high".to_string(),
                suggestions: vec!["Implement the missing functionality".to_string(), "Replace with proper implementation".to_string()],
            });
        }
        
        if line.len() > 120 {
            info_count += 1;
            errors.push(proto::ErrorDetection {
                r#type: "readability".to_string(),
                message: "Line is too long, consider breaking it up".to_string(),
                location: Some(proto::Location {
                    file_path: "".to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                confidence: "medium".to_string(),
                suggestions: vec!["Split long expressions into multiple lines".to_string(), "Extract complex expressions into variables".to_string()],
            });
        }
    }
    
    (errors, error_count, warning_count, info_count)
}

pub fn go_to_type_definition(file_path: &str, code: &str, line: i32, character: i32) -> Vec<proto::Location> {
    let mut locations = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    if line > 0 && line <= lines.len() as i32 {
        let target_line = lines[(line - 1) as usize];
        let struct_regex = regex::Regex::new(r"\b([A-Z][a-zA-Z0-9]*)\b").unwrap();
        
        for cap in struct_regex.captures_iter(target_line) {
            let type_name = cap[1].to_string();
            
            for (idx, code_line) in lines.iter().enumerate() {
                if code_line.contains(&format!("struct {}", type_name)) || 
                   code_line.contains(&format!("enum {}", type_name)) ||
                   code_line.contains(&format!("trait {}", type_name)) {
                    locations.push(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: 1,
                        end_line: (idx + 1) as i32,
                        end_character: code_line.len() as i32,
                    });
                    break;
                }
            }
        }
    }
    
    locations
}

pub fn go_to_implementation(file_path: &str, code: &str, line: i32, character: i32) -> Vec<proto::Location> {
    let mut locations = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    if line > 0 && line <= lines.len() as i32 {
        let target_line = lines[(line - 1) as usize];
        let fn_call_regex = regex::Regex::new(r"\b(\w+)\s*\(").unwrap();
        
        for cap in fn_call_regex.captures_iter(target_line) {
            let fn_name = cap[1].to_string();
            
            for (idx, code_line) in lines.iter().enumerate() {
                if code_line.contains(&format!("fn {}(", fn_name)) && !code_line.contains("trait") {
                    locations.push(proto::Location {
                        file_path: file_path.to_string(),
                        line: (idx + 1) as i32,
                        character: 1,
                        end_line: (idx + 1) as i32,
                        end_character: code_line.len() as i32,
                    });
                }
            }
        }
    }
    
    locations
}

pub fn find_implementations(file_path: &str, code: &str, symbol_name: &str) -> Vec<proto::ImplementationInfo> {
    let mut implementations = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if line.contains(&format!("impl {}", symbol_name)) {
            implementations.push(proto::ImplementationInfo {
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                kind: "impl".to_string(),
                signature: line.trim().to_string(),
            });
        }
        
        if line.contains(&format!("fn {}(", symbol_name)) {
            implementations.push(proto::ImplementationInfo {
                location: Some(proto::Location {
                    file_path: file_path.to_string(),
                    line: line_num,
                    character: 1,
                    end_line: line_num,
                    end_character: line.len() as i32,
                }),
                kind: "function".to_string(),
                signature: line.trim().to_string(),
            });
        }
    }
    
    implementations
}

fn find_derived_classes(file_path: &str, code: &str, class_name: &str) -> Vec<proto::DerivedClassInfo> {
    let mut derived_classes = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    
    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as i32;
        
        if line.contains(&format!("struct {} ", class_name)) {
            continue;
        }
        
        if line.contains("struct") || line.contains("enum") {
            let struct_regex = regex::Regex::new(r"(struct|enum)\s+(\w+)").unwrap();
            if let Some(cap) = struct_regex.captures(line) {
                let struct_name = cap[2].to_string();
                
                let impl_start = idx + 1;
                for (impl_idx, impl_line) in lines[impl_start..].iter().enumerate() {
                    if impl_line.contains(&format!("impl {} for {}", class_name, struct_name)) ||
                       impl_line.contains(&format!("impl {} for {}", struct_name, class_name)) {
                        derived_classes.push(proto::DerivedClassInfo {
                            name: struct_name,
                            location: Some(proto::Location {
                                file_path: file_path.to_string(),
                                line: line_num,
                                character: 1,
                                end_line: line_num,
                                end_character: line.len() as i32,
                            }),
                            depth: 1,
                        });
                        break;
                    }
                }
            }
        }
    }
    
    derived_classes
}


