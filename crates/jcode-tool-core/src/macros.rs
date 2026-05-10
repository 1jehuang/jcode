//! # jcode-tool-core macros
//!
//! Macros ported from Claude Code CLI's `buildTool()` pattern (Tool.ts).
//!
//! ## `define_tool!` — 定义工具结构体，用安全默认值填充可选字段
//!
//! 源自 Claude Code 的 `TOOL_DEFAULTS` + `buildTool()` 模式。
//! 每个工具只需提供名称、描述、输入模式和执行函数，其余字段自动使用安全默认值。
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use jcode_tool_core::define_tool;
//!
//! define_tool!(
//!     MyReadTool,
//!     name: "read",
//!     description: "Read file contents",
//!     parameters_schema: serde_json::json!({
//!         "type": "object",
//!         "properties": {
//!             "path": { "type": "string" }
//!         }
//!     }),
//!     is_read_only: true,
//!     execute: |input, ctx| {
//!         Box::pin(async move {
//!             // ... implementation
//!             Ok(ToolOutput::new("content"))
//!         })
//!     }
//! );
//! ```

/// 工具默认值常量 — 译自 Claude Code 的 `TOOL_DEFAULTS`
#[macro_export]
macro_rules! tool_defaults {
    () => {
        fn is_read_only(&self) -> bool { false }
        fn is_destructive(&self) -> bool { false }
        fn max_result_size_chars(&self) -> Option<usize> { None }
        fn mcp_source_info(&self) -> Option<&str> { None }
        fn aliases(&self) -> &[&str] { &[] }
        fn is_concurrency_safe(&self) -> bool { false }
        fn is_enabled(&self) -> bool { true }
    };
}

/// `define_tool!` — 简化工具定义
///
/// 用法:
/// ```rust,ignore
/// define_tool!(
///     MyTool,                          // 结构体名
///     name: "my_tool",                 // 工具名 (必填)
///     description: "Does something",   // 描述 (必填)
///     parameters_schema: json!(...),   // JSON Schema (必填)
///     // 可选字段 (使用默认值):
///     is_read_only: true,              // 默认 false
///     is_destructive: false,           // 默认 false
///     aliases: &["alt_name"],          // 默认 &[]
///     max_result_size_chars: 10000,    // 默认 None
///     execute: |input, ctx| { ... }    // 执行函数 (必填)
/// );
/// ```
#[macro_export]
macro_rules! define_tool {
    (
        $name:ident,
        name: $tool_name:expr,
        description: $desc:expr,
        parameters_schema: $schema:expr,
        $(is_read_only: $read_only:expr,)?
        $(is_destructive: $destructive:expr,)?
        $(aliases: $aliases:expr,)?
        $(max_result_size_chars: $max_chars:expr,)?
        $(mcp_source_info: $mcp_source:expr,)?
        $(is_concurrency_safe: $concurrency_safe:expr,)?
        $(is_enabled: $enabled:expr,)?
        execute: $execute_fn:expr
        $(,)?
    ) => {
        pub struct $name;

        impl $name {
            #[allow(dead_code)]
            pub fn new() -> Self { Self }
        }

        #[async_trait::async_trait]
        impl $crate::Tool for $name {
            fn name(&self) -> &str { $tool_name }

            fn description(&self) -> &str { $desc }

            fn parameters_schema(&self) -> serde_json::Value { $schema }

            $crate::tool_defaults!();

            $(fn is_read_only(&self) -> bool { $read_only })?
            $(fn is_destructive(&self) -> bool { $destructive })?
            $(fn aliases(&self) -> &[&str] { $aliases })?
            $(fn max_result_size_chars(&self) -> Option<usize> { Some($max_chars) })?
            $(fn mcp_source_info(&self) -> Option<&str> { Some($mcp_source) })?
            $(fn is_concurrency_safe(&self) -> bool { $concurrency_safe })?
            $(fn is_enabled(&self) -> bool { $enabled })?

            async fn execute(&self, input: serde_json::Value, ctx: $crate::ToolContext) -> anyhow::Result<$crate::ToolOutput> {
                let f: for<'a> fn(serde_json::Value, $crate::ToolContext) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = anyhow::Result<$crate::ToolOutput>> + Send + 'a>> = $execute_fn;
                f(input, ctx).await
            }
        }

        impl Default for $name {
            fn default() -> Self { Self }
        }
    };
}

/// `build_tool_adapter` — 将闭包转换为 Tool trait 对象 (译自 `buildTool()`)
///
/// 用于需要动态构建工具的场景（如 MCP 工具适配），比 `define_tool!` 更灵活。
#[macro_export]
macro_rules! build_tool_adapter {
    (
        name: $name:expr,
        description: $desc:expr,
        schema: $schema:expr,
        execute: $execute:expr
        $(,)?
    ) => {
        {
            use $crate::Tool;
            use async_trait::async_trait;

            struct Adapter {
                name: String,
                desc: String,
                schema: serde_json::Value,
            }

            #[async_trait]
            impl Tool for Adapter {
                fn name(&self) -> &str { &self.name }
                fn description(&self) -> &str { &self.desc }
                fn parameters_schema(&self) -> serde_json::Value { self.schema.clone() }
                fn is_read_only(&self) -> bool { false }
                fn is_destructive(&self) -> bool { false }

                async fn execute(&self, input: serde_json::Value, ctx: $crate::ToolContext) -> anyhow::Result<$crate::ToolOutput> {
                    ($execute)(input, ctx).await
                }
            }

            std::sync::Arc::new(Adapter {
                name: $name.to_string(),
                desc: $desc.to_string(),
                schema: $schema,
            }) as std::sync::Arc<dyn Tool>
        }
    };
}

/// `tool_matcher!` — 生成工具名称匹配函数（含别名），译自 `toolMatchesName()`
#[macro_export]
macro_rules! tool_matcher {
    () => {
        /// 检查工具名称是否匹配（包括别名）
        pub fn matches_name(tool: &dyn $crate::Tool, name: &str) -> bool {
            if tool.name() == name {
                return true;
            }
            tool.aliases().iter().any(|a| *a == name)
        }

        /// 按名称查找工具（包括别名）
        pub fn find_by_name<'a>(tools: &'a [std::sync::Arc<dyn $crate::Tool>], name: &str) -> Option<&'a std::sync::Arc<dyn $crate::Tool>> {
            tools.iter().find(|t| matches_name(t.as_ref(), name))
        }
    };
}
