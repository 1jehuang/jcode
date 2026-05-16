
/// 编程语言
#[derive(Debug, Clone, PartialEq)]
pub enum Language {
    Rust, TypeScript, JavaScript, Python, Java, Kotlin, Go, Ruby, CSharp, Swift, Generic(String),
}

impl Language {
    pub fn as_str(&self) -> &str {
        match self { Self::Rust => "rust", Self::TypeScript => "typescript", Self::JavaScript => "javascript",
            Self::Python => "python", Self::Java => "java", Self::Kotlin => "kotlin",
            Self::Go => "go", Self::Ruby => "ruby", Self::CSharp => "csharp",
            Self::Swift => "swift", Self::Generic(s) => s, }
    }
}

/// 框架
#[derive(Debug, Clone, PartialEq)]
pub enum Framework {
    // Rust
    Axum, Actix, Rocket, Leptos, Yew,
    // JVM
    SpringBoot, Quarkus, Micronaut,
    // Node
    Express, NestJs, NextJs, Nuxt,
    // Python
    Django, FastApi, Flask,
    // Go
    Gin, Echo, Fiber,
    // Mobile
    Flutter, ReactNative,
    // Generic
    None,
}

/// 构建工具
#[derive(Debug, Clone, PartialEq)]
pub enum BuildTool {
    Cargo, Maven, Gradle, Npm, Yarn, Pnpm, Pipenv, Poetry, GoMod, Bundler, DotNet, Generic(String),
}

impl BuildTool {
    pub fn as_str(&self) -> &str {
        match self { Self::Cargo => "cargo", Self::Maven => "maven", Self::Gradle => "gradle",
            Self::Npm => "npm", Self::Yarn => "yarn", Self::Pnpm => "pnpm",
            Self::Pipenv => "pipenv", Self::Poetry => "poetry", Self::GoMod => "go",
            Self::Bundler => "bundler", Self::DotNet => "dotnet", Self::Generic(s) => s, }
    }
}

/// 检测到的技术栈
#[derive(Debug, Clone)]
pub struct TechStack {
    pub language: Language,
    pub framework: Framework,
    pub build_tool: BuildTool,
    pub test_framework: String,
    pub linter: String,
    pub has_dockerfile: bool,
    pub has_swagger: bool,
    pub docker_registry: Option<String>,
}

/// 技术栈检测器
pub struct StackDetector;

impl StackDetector {
    pub fn new() -> Self { Self }

    /// 扫描项目目录，检测技术栈
    pub fn detect(&self, root: &str) -> anyhow::Result<TechStack> {
        let root = std::path::Path::new(root);
        let files = self.list_files(root);

        let language = self.detect_language(&files);
        let framework = self.detect_framework(&files, &language);
        let build_tool = self.detect_build_tool(&files, &language);

        Ok(TechStack {
            language: language.clone(),
            framework,
            build_tool,
            test_framework: self.detect_test_framework(&files, &language),
            linter: self.detect_linter(&files, &language),
            has_dockerfile: files.contains(&"Dockerfile".to_string()),
            has_swagger: files.iter().any(|f| f.contains("swagger") || f.contains("openapi")),
            docker_registry: None,
        })
    }

    fn list_files(&self, root: &std::path::Path) -> Vec<String> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.path().is_file() { files.push(name); }
            }
        }
        files
    }

    fn detect_language(&self, files: &[String]) -> Language {
        for f in files {
            return match f.as_str() {
                "Cargo.toml" => Language::Rust,
                "package.json" if f.contains("package.json") => {
                    if std::fs::read_to_string("package.json").ok().map_or(false, |c| c.contains("\"typescript\"")) 
                    { Language::TypeScript } else { Language::JavaScript }
                }
                "pom.xml" => Language::Java,
                "build.gradle" | "build.gradle.kts" => Language::Kotlin,
                "go.mod" => Language::Go,
                "Gemfile" => Language::Ruby,
                "requirements.txt" | "Pipfile" | "pyproject.toml" => Language::Python,
                "Podfile" | "Cartfile" => Language::Swift,
                _ if f.starts_with("package.json") => {
                    if std::fs::read_to_string("package.json").ok().map_or(false, |c| c.contains("\"typescript\"")) 
                    { Language::TypeScript } else { Language::JavaScript }
                }
                _ => continue,
            };
        }
        Language::Generic("unknown".into())
    }

    fn detect_framework(&self, files: &[String], lang: &Language) -> Framework {
        let content = |name: &str| std::fs::read_to_string(name).ok();
        match lang {
            Language::Rust => {
                if let Some(c) = content("Cargo.toml") {
                    if c.contains("axum") { return Framework::Axum; }
                    if c.contains("actix") { return Framework::Actix; }
                    if c.contains("rocket") { return Framework::Rocket; }
                    if c.contains("leptos") { return Framework::Leptos; }
                    if c.contains("yew") { return Framework::Yew; }
                }
                Framework::None
            }
            Language::Java | Language::Kotlin => {
                if files.iter().any(|f| f.contains("Application.java") || f.contains("Application.kt")) {
                    return Framework::SpringBoot;
                }
                Framework::None
            }
            Language::TypeScript | Language::JavaScript => {
                if let Some(c) = content("package.json") {
                    if c.contains("\"@nestjs") { return Framework::NestJs; }
                    if c.contains("\"express") { return Framework::Express; }
                    if c.contains("next") { return Framework::NextJs; }
                    if c.contains("nuxt") { return Framework::Nuxt; }
                }
                Framework::None
            }
            Language::Python => {
                if let Some(c) = content("requirements.txt") {
                    if c.contains("django") { return Framework::Django; }
                    if c.contains("fastapi") { return Framework::FastApi; }
                    if c.contains("flask") { return Framework::Flask; }
                }
                Framework::None
            }
            Language::Go => Framework::None,
            _ => Framework::None,
        }
    }

    fn detect_build_tool(&self, files: &[String], _lang: &Language) -> BuildTool {
        for f in files {
            match f.as_str() {
                "Cargo.toml" => return BuildTool::Cargo,
                "pom.xml" => return BuildTool::Maven,
                "build.gradle.kts" | "build.gradle" => return BuildTool::Gradle,
                "yarn.lock" => return BuildTool::Yarn,
                "pnpm-lock.yaml" => return BuildTool::Pnpm,
                "package-lock.json" => return BuildTool::Npm,
                "go.mod" => return BuildTool::GoMod,
                "Gemfile.lock" => return BuildTool::Bundler,
                "Pipfile" => return BuildTool::Pipenv,
                "poetry.lock" => return BuildTool::Poetry,
                _ => continue,
            }
        }
        BuildTool::Generic("make".into())
    }

    fn detect_test_framework(&self, _files: &[String], lang: &Language) -> String {
        match lang {
            Language::Rust => "cargo test".to_string(),
            Language::TypeScript | Language::JavaScript => "jest".to_string(),
            Language::Python => "pytest".to_string(),
            Language::Java | Language::Kotlin => "junit".to_string(),
            Language::Go => "go test".to_string(),
            Language::Ruby => "rspec".to_string(),
            _ => "unknown".into(),
        }
    }

    fn detect_linter(&self, _files: &[String], lang: &Language) -> String {
        match lang {
            Language::Rust => "clippy".to_string(),
            Language::TypeScript | Language::JavaScript => "eslint".to_string(),
            Language::Python => "ruff".to_string(),
            Language::Java => "checkstyle".to_string(),
            Language::Go => "golangci-lint".to_string(),
            _ => "unknown".into(),
        }
    }
}
