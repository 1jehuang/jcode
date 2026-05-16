//! Rust 最佳实践知识库 (Best Practices Knowledge Base)
//!
//! 本模块包含从实际开发经验中总结的Rust编程最佳实践，
//! 确保CarpAI生成的所有代码都遵循业界标准。
//!
//! ## 核心原则
//!
//! ### 模块系统规则 (Module System Rules)
//!
//! **规则1: 一个目录 = 一个入口文件**
//! ```text
//! ✅ 正确:
//! src/
//! +-- my_module/
//!     +-- mod.rs          <- 唯一入口
//!
//! ❌ 错误:
//! src/
//! +-- my_module.rs        <- 多余的外部文件
//! +-- my_module/
//!     +-- mod.rs          <- 与上面冲突
//!     +-- my_module.rs    <- 冗余的内部文件
//! ```
//!
//! **规则2: mod.rs 是目录模块的标准命名约定**
//! - 使用 `mod.rs` 作为目录内的入口文件
//! - 不要使用其他名称（如 `lib.rs`, `main.rs` 等）
//! - 保持命名一致性，便于团队协作
//!
//! **规则3: 不要创建冗余的中间层 .rs 文件**
//! - 避免创建只做 re-export 的占位符文件
//! - 如果需要重新导出，直接在 `mod.rs` 中使用 `pub use`
//! - 减少不必要的抽象层
//!
//! **规则4: 保持简单 - 代码组织策略**
//! ```rust
//! 策略A: 小模块 (<500行)
//! // 所有代码放在 mod.rs 中
//! pub fn foo() {}
//! pub struct Bar {}
//!
//! 策略B: 大模块 (>500行)
//! // 拆分为多个子模块
//! pub mod sub_module_a;
//! pub mod sub_module_b;
//! pub use sub_module_a::*;  // 可选：重新导出
//!
//! 策略C: 超大模块 (>2000行)
//! // 使用独立文件 + mod.rs 作为入口
//! pub mod types;      // types.rs
//! pub mod impls;      // impls.rs
//! pub mod tests;      // tests.rs
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Types ---------------------------------

/// Rust最佳实践规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustBestPractice {
    /// 规则ID
    pub id: String,
    
    /// 规则类别
    pub category: PracticeCategory,
    
    /// 规则名称
    pub name: String,
    
    /// 描述
    pub description: String,
    
    /// 正确示例
    pub correct_example: String,
    
    /// 错误示例
    pub incorrect_example: String,
    
    /// 严重程度
    pub severity: SeverityLevel,
    
    /// 自动修复能力
    pub auto_fixable: bool,
    
    /// 相关规则ID列表
    pub related_rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PracticeCategory {
    ModuleSystem,
    NamingConventions,
    ErrorHandling,
    Performance,
    Safety,
    IdiomaticPatterns,
    Testing,
    Documentation,
    ProjectStructure,
    Dependencies,
    AsyncRuntime,
}

impl std::fmt::Display for PracticeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModuleSystem => write!(f, "📦 模块系统"),
            Self::NamingConventions => write!(f, "🏷️ 命名规范"),
            Self::ErrorHandling => write!(f, "⚠️ 错误处理"),
            Self::Performance => write!(f, "⚡ 性能优化"),
            Self::Safety => write!(f, "🔒 安全性"),
            Self::IdiomaticPatterns => write!(f, "✨ 惯用模式"),
            Self::Testing => write!(f, "🧪 测试"),
            Self::Documentation => write!(f, "📖 文档"),
            Self::ProjectStructure => write!(f, "🏗️ 项目结构"),
            Self::Dependencies => write!(f, "📦 依赖管理"),
            Self::AsyncRuntime => write!(f, "🔄 异步运行时"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SeverityLevel {
    Critical,  // 必须遵守，否则编译失败或严重bug
    Major,     // 强烈建议，影响代码质量
    Minor,     // 建议遵循，提升可维护性
    Info,      // 可选，风格偏好
}

// --- Core Knowledge Base ----------------------

/// Rust最佳实践知识库
pub struct RustKnowledgeBase {
    rules: HashMap<String, RustBestPractice>,
    category_index: HashMap<PracticeCategory, Vec<String>>,
}

impl Default for RustKnowledgeBase {
    fn default() -> Self {
        let mut kb = Self {
            rules: HashMap::new(),
            category_index: HashMap::new(),
        };
        
        kb.initialize_builtin_rules();
        kb
    }
}

impl RustKnowledgeBase {
    /// 创建新的知识库实例
    pub fn new() -> Self {
        Self::default()
    }
    
    /// 初始化内置规则
    fn initialize_builtin_rules(&mut self) {
        // === 模块系统规则 ===
        
        self.add_rule(RustBestPractice {
            id: "MOD-001".to_string(),
            category: PracticeCategory::ModuleSystem,
            name: "单一入口文件原则".to_string(),
            description: "每个目录应该只有一个模块入口文件（mod.rs），避免创建冗余的中间层".to_string(),
            correct_example: r#"
// ✅ 正确结构
src/
+-- my_module/
    +-- mod.rs           // 唯一入口，包含所有公开API
        pub struct MyStruct;
        pub fn my_function() {}
"#.to_string(),
            incorrect_example: r#"
// ❌ 错误结构（冗余）
src/
+-- my_module.rs       // 多余！与下面的mod.rs冲突
+-- my_module/
    +-- mod.rs         // 实际代码在这里
    +-- my_module.rs   // 又一个多余的文件
"#.to_string(),
            severity: SeverityLevel::Critical,
            auto_fixable: true,
            related_rules: vec!["MOD-002".to_string(), "MOD-003".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "MOD-002".to_string(),
            category: PracticeCategory::ModuleSystem,
            name: "mod.rs 命名约定".to_string(),
            description: "使用 mod.rs 作为目录模块的标准入口文件名，这是Rust社区的广泛共识".to_string(),
            correct_example: r#"// ✅ 标准做法
// 文件: src/utils/mod.rs
pub mod string_utils;
pub mod file_utils;

pub use string::*;  // 方便外部使用
"#.to_string(),
            incorrect_example: r#"// ❌ 不推荐
// 文件: src/utils/lib.rs 或 src/utils/main.rs
// 虽然能工作，但不符合社区惯例
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: false,
            related_rules: vec!["MOD-001".to_string(), "MOD-003".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "MOD-003".to_string(),
            category: PracticeCategory::ModuleSystem,
            name: "避免冗余re-export文件".to_string(),
            description: "不要创建只做 re-export 的中间层 .rs 文件，直接在 mod.rs 中使用 pub use".to_string(),
            correct_example: r#"// ✅ 正确：直接在mod.rs中导出
// 文件: src/my_crate/mod.rs
pub mod internal;

// 直接导出需要的项
pub use internal::{PublicStruct, public_function};
"#.to_string(),
            incorrect_example: r#"// ❌ 错误：创建额外的导出文件
// 文件: src/my_crate/api.rs (多余!)
pub mod internal;
pub use internal::*;

// 文件: src/my_crate/mod.rs
pub mod api;  // 只是为了转发api.rs
pub use api::*;
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: true,
            related_rules: vec!["MOD-001".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "MOD-004".to_string(),
            category: PracticeCategory::ModuleSystem,
            name: "合理的模块拆分粒度".to_string(),
            description: "根据代码规模选择合适的模块组织策略：<500行放mod.rs，>500行拆分子模块".to_string(),
            correct_example: r#"// ✅ 小模块 (<500行)
// src/parser/mod.rs
pub struct Parser { /* ... */ }
impl Parser { /* ... */ }

// 大模块 (>500行)
// src/database/
// +-- mod.rs              // 入口 + 公共类型
// |   pub mod connection;
// |   pub mod query;
// |   pub mod migration;
// |   
// |   // 重新导出常用项
// |   pub use connection::Connection;
// |   pub use query::QueryBuilder;
// |
// +-- connection.rs       // 连接管理
// +-- query.rs            // 查询构建
// +-- migration.rs       // 数据库迁移
"#.to_string(),
            incorrect_example: r#"// ❌ 过度拆分
// src/parser/
// +-- mod.rs             // 空
// +-- parser.rs          // 只有struct定义
// +-- parse_impl.rs      // 实现部分
// +-- parse_utils.rs     // 工具函数
// 结果：文件太多，导航困难
"#.to_string(),
            severity: SeverityLevel::Minor,
            auto_fixable: false,
            related_rules: vec!["MOD-001".to_string(), "MOD-003".to_string()],
        });
        
        // === 命名规范规则 ===
        
        self.add_rule(RustBestPractice {
            id: "NAME-001".to_string(),
            category: PracticeCategory::NamingConventions,
            name: "snake_case 命名".to_string(),
            description: "所有变量、函数、模块名使用 snake_case，类型和trait使用 PascalCase".to_string(),
            correct_example: r#"// ✅ 正确命名
let user_name: String = "Alice";
fn calculate_total(items: &[Item]) -> f64 { /* ... */ }
struct UserProfile { /* ... */ }
trait AsyncProcessor { /* ... */ }
mod file_system_utils { /* ... */ }
"#.to_string(),
            incorrect_example: r#"// ❌ 错误命名
let userName: String = "Alice";  // 应该是 user_name
fn CalculateTotal() {}            // 应该是 calculate_total
struct user_profile {}           // 应该是 UserProfile
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: true,
            related_rules: vec!["NAME-002".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "NAME-002".to_string(),
            category: PracticeCategory::NamingConventions,
            name: "有意义的命名".to_string(),
            description: "变量和函数名应清晰表达意图，避免缩写和单字母命名（循环变量除外）".to_string(),
            correct_example: r#"// ✅ 有意义的命名
let mut active_user_sessions: HashMap<UserId, Session> = HashMap::new();
fn validate_email_format(email: &str) -> Result<(), EmailError> { /* ... */ }
const MAX_RETRY_ATTEMPTS: u32 = 3;
"#.to_string(),
            incorrect_example: r#"// ❌ 含糊的命名
let mut m: HashMap<U, S> = HashMap::new();  // m 是什么？U, S?
fn check(e: &str) -> Result<(), E> {}      // check什么？
const N: u32 = 3;                           // N代表什么？
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: false,
            related_rules: vec!["NAME-001".to_string()],
        });
        
        // === 错误处理规则 ===
        
        self.add_rule(RustBestPractice {
            id: "ERR-001".to_string(),
            category: PracticeCategory::ErrorHandling,
            name: "使用Result而非panic/unwrap".to_string(),
            description: "优先使用Result返回错误，避免在生产代码中使用unwrap()或expect()".to_string(),
            correct_example: r#"// ✅ 正确的错误处理
fn read_config(path: &Path) -> Result<Config, IoError> {
    let content = std::fs::read_to_string(path)?;  // 使用?操作符
    
    match toml::from_str(&content) {
        Ok(config) => Ok(config),
        Err(e) => Err(IoError::ParseError(e)),
    }
}

// 在应用层处理错误
match read_config("config.toml") {
    Ok(config) => start_server(config),
    Err(e) => {
        eprintln!("配置加载失败: {}", e);
        std::process::exit(1);
    }
}
"#.to_string(),
            incorrect_example: r#"// ❌ 危险的错误处理
fn read_config_bad(path: &Path) -> Config {
    let content = std::fs::read_to_string(path)
        .expect("无法读取配置");  // panic! 生产环境崩溃!
    
    toml::from_str(&content).unwrap()  // 可能panic!
}
"#.to_string(),
            severity: SeverityLevel::Critical,
            auto_fixable: true,
            related_rules: vec!["ERR-002".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "ERR-002".to_string(),
            category: PracticeCategory::ErrorHandling,
            name: "自定义Error类型".to_string(),
            description: "为特定领域定义专门的Error枚举或struct，提供丰富的上下文信息".to_string(),
            correct_example: r#"// ✅ 自定义错误类型
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库连接失败: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("配置错误: {message}")]
    Config { message: String },
    
    #[error("认证失败: {reason}")]
    Auth { reason: String },
    
    #[error("IO错误: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("业务逻辑错误: {code} - {message}")]
    Business { code: u16, message: String },
}

// 使用自定义错误
async fn get_user(id: UserId) -> Result<User, AppError> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&db)
        .await
        .map_err(AppError::Database)?;
    
    if user.is_banned {
        return Err(AppError::Auth {
            reason: format!("用户{}已被封禁", id),
        });
    }
    
    Ok(user)
}
"#.to_string(),
            incorrect_example: r#"// ❌ 使用通用错误类型
fn get_user_bad(id: i32) -> Result<User, Box<dyn std::error::Error>> {
    // 错误信息丢失上下文
    let user = db.query(id).map_err(|e| e.into())?;
    Ok(user)
}
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: false,
            related_rules: vec!["ERR-001".to_string()],
        });
        
        // === 性能优化规则 ===
        
        self.add_rule(RustBestPractice {
            id: "PERF-001".to_string(),
            category: PracticeCategory::Performance,
            name: "避免不必要的克隆".to_string(),
            description: "优先使用引用(&)传递数据，只在确实需要所有权时进行克隆".to_string(),
            correct_example: r#"// ✅ 使用引用避免克隆
fn process_users(users: &[User]) -> Vec<UserSummary> {
    users.iter()
        .map(|user| UserSummary {
            id: user.id,                    // Copy类型，自动复制
            name: user.name.clone(),        // String需要显式克隆
            email_count: user.emails.len(), // 无需克隆
        })
        .collect()
}

// 或者接受迭代器
fn process_users_iter<I>(users: I) -> Vec<UserSummary>
where
    I: IntoIterator<Item = User>,
{
    users.into_iter().map(|user| UserSummary { /* ... */ }).collect()
}
"#.to_string(),
            incorrect_example: r#"// ❌ 过度克隆
fn process_users_bad(users: Vec<User>) -> Vec<UserSummary> {
    // 整个Vec被克隆了！
    let cloned = users.clone();
    cloned.iter().map(|user| { /* ... */ }).collect()
}
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: true,
            related_rules: vec!["PERF-002".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "PERF-002".to_string(),
            category: PracticeCategory::Performance,
            name: "合理使用String vs &str".to_string(),
            description: "函数参数优先使用 &str（字符串切片），减少内存分配；返回值根据需求选择".to_string(),
            correct_example: r#"// ✅ 参数使用&str
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

// 返回String如果需要拥有所有权
fn generate_report(data: &ReportData) -> String {
    // 复杂构建过程...
    format!("Report: {}", data.title)
}

// 如果调用者只需要借用，返回引用
fn get_title(report: &Report) -> &str {
    &report.title
}
"#.to_string(),
            incorrect_example: r#"// ❌ 参数使用String导致不必要的分配
fn greet_bad(name: String) -> String {  // 调用者必须.to_string()
    format!("Hello, {}!", name)
}

greet_bad(name.to_string());  // 每次调用都分配新String
"#.to_string(),
            severity: SeverityLevel::Minor,
            auto_fixable: true,
            related_rules: vec!["PERF-001".to_string()],
        });
        
        // === 异步运行时规则 ===
        
        self.add_rule(RustBestPractice {
            id: "ASYNC-001".to_string(),
            category: PracticeCategory::AsyncRuntime,
            name: "正确使用async/await".to_string(),
            description: "只在真正需要等待I/O时使用async，不要在计算密集型任务中使用".to_string(),
            correct_example: r#"// ✅ 正确的async使用
// I/O密集型 - 适合async
async fn fetch_user(id: u64) -> Result<User, reqwest::Error> {
    reqwest::get(&format!("https://api.example.com/users/{}", id))
        .send()
        .await?
        .json()
        .await
}

// CPU密集型 - 不要使用async
fn compute_hash(data: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(data)  // 同步执行
}

// 混合场景：异步包装CPU任务
async fn process_with_io(input: &Input) -> Result<Output, Error> {
    let data = fetch_data(input.id).await?;  // async I/O
    
    // CPU密集型任务放到线程池
    let result = tokio::task::spawn_blocking(move || {
        heavy_computation(&data)
    }).await?;
    
    save_result(&result).await?;  // async I/O
    Ok(result)
}
"#.to_string(),
            incorrect_example: r#"// ❌ 错误的async使用
// CPU密集型任务不应该async
async fn compute_bad(data: &[u8]) -> [u8; 32] {
    // 这会阻塞async运行时！
    heavy_computation(data)
}

// 纯计算不需要async
async fn add_numbers(a: i32, b: i32) -> i32 {
    a + b  // 没有任何I/O，为什么要async?
}
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: true,
            related_rules: vec!["ASYNC-002".to_string()],
        });
        
        self.add_rule(RustBestPractice {
            id: "ASYNC-002".to_string(),
            category: PracticeCategory::AsyncRuntime,
            name: "使用spawn_blocking处理CPU密集型任务".to_string(),
            description: "在Tokio运行时中使用tokio::task::spawn_blocking执行阻塞或CPU密集型操作".to_string(),
            correct_example: r#"// ✅ 正确使用spawn_blocking
use tokio::task::spawn_blocking;

async fn generate_pdf_report(report: &Report) -> Result<Vec<u8>, Error> {
    // PDF生成是CPU密集型的
    let pdf_bytes = spawn_blocking(move || {
        pdf_generator::generate(report)  // 可能耗时几百毫秒
    }).await?;

    // 保存文件（I/O操作）
    tokio::fs::write("report.pdf", &pdf_bytes).await?;
    
    Ok(pdf_bytes)
}

// 批量处理
async fn process_batch(items: Vec<Item>) -> Vec<Result<Output, Error>> {
    let futures = items.into_iter().map(|item| {
        spawn_blocking(move || cpu_intensive_processing(item))
    });
    
    futures::future::join_all(futures).await
}
"#.to_string(),
            incorrect_example: r#"// ❌ 直接在async函数中进行CPU密集型操作
async fn generate_pdf_bad(report: &Report) -> Result<Vec<u8>, Error> {
    // 这会阻塞整个Tokio运行时！
    let pdf_bytes = pdf_generator::generate(report); 
    
    tokio::fs::write("report.pdf", &pdf_bytes).await?;
    Ok(pdf_bytes)
}
"#.to_string(),
            severity: SeverityLevel::Critical,
            auto_fixable: true,
            related_rules: vec!["ASYNC-001".to_string()],
        });
        
        // === 测试规则 ===
        
        self.add_rule(RustBestPractice {
            id: "TEST-001".to_string(),
            category: PracticeCategory::Testing,
            name: "单元测试命名和组织".to_string(),
            description: "测试文件放在同名目录下的tests子目录，测试函数以test_为前缀并描述测试场景".to_string(),
            correct_example: r#"// ✅ 测试文件结构
// src/
// +-- utils/
//     +-- mod.rs
//     +-- tests/
//         +-- mod.rs           // 对utils模块的测试

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_valid_email_returns_success() {
        let email = "user@example.com";
        let result = parse_email(email);
        assert!(result.is_ok());
    }
    
    #[test]
    #[should_panic(expected = "Invalid email format")]
    fn test_parse_empty_email_panics() {
        parse_email("");
    }
    
    #[tokio::test]
    async fn test_fetch_user_returns_correct_data() {
        let user = fetch_user(1).await.unwrap();
        assert_eq!(user.name, "Test User");
    }
}
"#.to_string(),
            incorrect_example: r#"// ❌ 测试组织混乱
// 测试散落在各处，没有统一结构
// 测试函数命名不清晰

#[test]
fn utils_test1() {}  // test_前缀但无意义
#[test]
fn check_email() {}   // 缺少test_前缀
"#.to_string(),
            severity: SeverityLevel::Minor,
            auto_fixable: false,
            related_rules: vec!["TEST-002".to_string()],
        });
        
        // === 项目结构规则 ===
        
        self.add_rule(RustBestPractice {
            id: "PROJ-001".to_string(),
            category: PracticeCategory::ProjectStructure,
            name: "标准的Cargo项目布局".to_string(),
            description: "遵循Rust社区公认的项目结构，便于维护和协作".to_string(),
            correct_example: r#"# ✅ 标准项目结构
my-project/
+-- Cargo.toml              # 项目元数据和依赖
+-- Cargo.lock             # 依赖锁定文件（提交到VCS）
+-- src/
|   +-- lib.rs              # 库入口（如果是库）
|   +-- main.rs             # 二进制入口（如果是应用）
|   |
|   +-- bin/                # 多个二进制目标
|   |   +-- cli-tool.rs
|   |   +-- gui-app.rs
|   |
|   +-- lib/                # 库代码的主要模块
|   |   +-- mod.rs
|   |   +-- core.rs
|   |   +-- utils.rs
|   |
|   +-- tests/             # 集成测试
|   |   +-- integration_tests.rs
|   |
|   +-- examples/          # 示例代码
|       +-- basic_usage.rs
|
+-- benches/               # 性能基准测试
|   +-- my_benchmark.rs
|
+-- docs/                  # 文档
|   +-- README.md
|   +-- API.md
|
+-- tests/                 # 单元测试（根级别）
    +-- test_main.rs
"#.to_string(),
            incorrect_example: r#"# ❌ 混乱的项目结构
my-project-bad/
+-- src/
|   +-- all_code_in_one_file.rs  # 所有代码混在一起
|   +-- helper.rs               // 杂乱的工具函数
|   +-- test_stuff.rs            // 测试和生产代码混合
|   +-- temp.rs                 // 临时文件未清理
+-- code/                       # 自定义的非标准目录
+-- lib.rs                      # 放错位置了
"#.to_string(),
            severity: SeverityLevel::Major,
            auto_fixable: false,
            related_rules: vec!["MOD-001".to_string(), "MOD-004".to_string()],
        });
    }
    
    /// 添加规则到知识库
    fn add_rule(&mut self, rule: RustBestPractice) {
        let id = rule.id.clone();
        let category = rule.category;
        
        self.rules.insert(id, rule);
        
        self.category_index
            .entry(category)
            .or_insert_with(Vec::new)
            .push(id);
    }
    
    /// 获取所有规则
    pub fn get_all_rules(&self) -> Vec<&RustBestPractice> {
        self.rules.values().collect()
    }
    
    /// 按类别获取规则
    pub fn get_rules_by_category(&self, category: PracticeCategory) -> Vec<&RustBestPractice> {
        let rules: Vec<&RustBestPractice> = self.category_index
            .get(&category)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.rules.get(id))
                    .collect()
            })
            .unwrap_or_default();
        rules
    }
    
    /// 获取规则详情
    pub fn get_rule(&self, id: &str) -> Option<&RustBestPractice> {
        self.rules.get(id)
    }
    
    /// 验证代码是否符合最佳实践
    pub fn validate_code(&self, code: &str) -> ValidationReport {
        let mut violations = Vec::new();
        let mut suggestions = Vec::new();
        
        for rule in self.rules.values() {
            if let Some(violation) = self.check_rule(code, &rule) {
                if violation.is_violation {
                    violations.push(violation);
                } else {
                    suggestions.push(rule);
                }
            }
        }
        
        ValidationReport {
            total_rules_checked: self.rules.len(),
            violations,
            suggestions,
            score: self.calculate_score(&violations),
        }
    }
    
    fn check_rule(&self, code: &str, rule: &RustBestPractice) -> Option<RuleViolation> {
        // 简化的规则检查（实际实现会更复杂）
        let is_violation = match rule.id.as_str() {
            "MOD-001" => {
                // 检测是否有冗余的模块文件模式
                code.contains("pub mod ") && 
                (code.contains(".rs") && code.contains("pub use"))
            },
            "NAME-001" => {
                // 检测是否有非snake_case的标识符
                let has_uppercase_var = regex::Regex::new(r"let [A-Z]").is_ok()
                    && code.contains("let ");
                has_uppercase_var || code.contains("fn [A-Z]")
            },
            "ERR-001" => {
                // 检测是否有unwrap()或expect()
                code.contains(".unwrap()") || code.contains(".expect(")
            },
            _ => false,
        };
        
        if is_violation {
            Some(RuleViolation {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                severity: rule.severity,
                description: rule.description.clone(),
                suggestion: rule.correct_example.clone(),
                location: None, // 需要AST分析才能定位
                auto_fix_available: rule.auto_fixable,
            })
        } else {
            None
        }
    }
    
    fn calculate_score(&self, violations: &[RuleViolation]) -> f64 {
        if self.rules.is_empty() {
            return 100.0;
        }
        
        let total_weight: f64 = violations.iter()
            .map(|v| match v.severity {
                SeverityLevel::Critical => 10.0,
                SeverityLevel::Major => 5.0,
                SeverityLevel::Minor => 2.0,
                SeverityLevel::Info => 0.5,
            })
            .sum();
        
        let max_possible_weight: f64 = self.rules.len() as f64 * 5.0; // 假设平均权重为5
        
        let score = ((max_possible_weight - total_weight) / max_possible_weight * 100.0).max(0.0);
        
        score.round()
    }
    
    /// 生成改进建议报告
    pub fn generate_improvement_report(&self, code: &str) -> ImprovementReport {
        let validation = self.validate_code(code);
        
        let critical_issues: Vec<_> = validation.violations.iter()
            .filter(|v| matches!(v.severity, SeverityLevel::Critical))
            .collect();
        
        let major_issues: Vec<_> = validation.violations.iter()
            .filter(|v| matches!(v.severity, SeverityLevel::Major))
            .collect();
        
        ImprovementReport {
            overall_score: validation.score,
            total_issues: validation.violations.len(),
            critical_issues: critical_issues.len(),
            major_issues: major_issues.len(),
            top_recommendations: validation.violations.iter()
                .take(5)
                .map(|v| Recommendation {
                    title: format!("修复: {}", v.rule_name),
                    priority: match v.severity {
                        SeverityLevel::Critical => "立即修复".to_string(),
                        SeverityLevel::Major => "尽快修复".to_string(),
                        SeverityLevel::Minor => "计划修复".to_string(),
                        SeverityLevel::Info => "可选改进".to_string(),
                    },
                    effort: match v.auto_fix_available {
                        true => "低 (可能自动修复)".to_string(),
                        false => "中等 (需手动修改)".to_string(),
                    },
                    impact: format!("遵循规则 {}, 提升代码质量", v.rule_id),
                    details: v.suggestion.clone(),
                })
                .collect(),
            best_practices_to_adopt: validation.suggestions.iter()
                .take(5)
                .map(|r| BestPracticeTip {
                    category: r.category,
                    practice: r.name.clone(),
                    benefit: r.description.clone(),
                    example: r.correct_example.chars().take(100).collect(),
                })
                .collect(),
        }
    }
}

// --- Report Types ----------------------------

/// 验证报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// 检查的规则总数
    pub total_rules_checked: usize,
    
    /// 违反的规则
    pub violations: Vec<RuleViolation>,
    
    /// 建议采纳的最佳实践
    pub suggestions: Vec<RustBestPractice>,
    
    /// 符合度评分 (0-100)
    pub score: f64,
}

/// 规则违反记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleViolation {
    pub rule_id: String,
    pub rule_name: String,
    pub severity: SeverityLevel,
    pub description: String,
    pub suggestion: String,
    pub location: Option<String>,
    pub auto_fix_available: bool,
    pub is_violation: bool,
}

/// 改进建议报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementReport {
    /// 总体评分
    pub overall_score: f64,
    
    /// 问题总数
    pub total_issues: usize,
    
    /// 严重问题数
    pub critical_issues: usize,
    
    /// 主要问题数
    pub major_issues: usize,
    
    /// 前5个改进建议
    pub top_recommendations: Vec<Recommendation>,
    
    /// 推荐采纳的最佳实践
    pub best_practices_to_adopt: Vec<BestPracticeTip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub title: String,
    pub priority: String,
    pub effort: String,
    pub impact: String,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestPracticeTip {
    pub category: PracticeCategory,
    pub practice: String,
    pub benefit: String,
    pub example: String,
}

// --- Tests ----------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_knowledge_base_initialization() {
        let kb = RustKnowledgeBase::new();
        
        assert!(!kb.rules.is_empty());
        assert!(kb.rules.len() >= 10); // 至少有10条内置规则
    }
    
    #[test]
    fn test_get_rules_by_category() {
        let kb = RustKnowledgeBase::new();
        
        let module_rules = kb.get_rules_by_category(PracticeCategory::ModuleSystem);
        assert!(!module_rules.is_empty());
        
        for rule in module_rules {
            assert_eq!(rule.category, PracticeCategory::ModuleSystem);
        }
    }
    
    #[test]
    fn test_validate_good_code() {
        let kb = RustKnowledgeBase::new();
        
        let good_code = r#"
// 正确的模块结构
mod utils {
    pub fn helper() -> i32 { 42 }
}

fn main() {
    let result = utils::helper();
    println!("{}", result);
}
"#;
        
        let report = kb.validate_code(good_code);
        assert!(report.score >= 80.0); // 好代码应该得分高
    }
    
    #[test]
    fn test_validate_bad_code() {
        let kb = RustKnowledgeBase::new();
        
        let bad_code = r#"
// 包含问题的代码
fn BadFunction() {
    let X = 5;
    let result = some_operation().unwrap();  // 危险的unwrap
    result
}
"#;
        
        let report = kb.validate_code(bad_code);
        assert!(report.score < 80.0); // 差代码应该得分低
        assert!(!report.violations.is_empty());
    }
    
    #[test]
    fn test_module_system_rules_exist() {
        let kb = RustKnowledgeBase::new();
        
        // 检查关键规则是否存在
        assert!(kb.get_rule("MOD-001").is_some()); // 单一入口文件
        assert!(kb.get_rule("MOD-002").is_some()); // mod.rs命名约定
        assert!(kb.get_rule("MOD-003").is_some()); // 避免冗余文件
        assert!(kb.get_rule("MOD-004").is_some()); // 合理拆分粒度
    }
    
    #[test]
    fn test_improvement_report_generation() {
        let kb = RustKnowledgeBase::new();
        
        let code = r#"
fn BAD_NAMING() {
    let Result = operation().unwrap();
    Result
}
"#;
        
        let report = kb.generate_improvement_report(code);
        
        assert!(report.total_issues > 0);
        assert!(!report.top_recommendations.is_empty());
        assert!(report.overall_score < 100.0);
    }
}
