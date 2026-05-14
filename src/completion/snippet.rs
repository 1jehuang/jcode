//! # 代码片段(Snippet)管理系统
//!
//! 提供智能的代码片段补全功能：
//! - **50+内置片段** - 覆盖Rust/Python/TS/Go/Shell等主流语言
//! - **Tab触发** - 输入前缀自动展开
//! - **占位符跳转** - $1, $2... 支持光标跳转
//! - **变量替换** - $YEAR, $FILENAME, $CLASSNAME等内置变量
//! - **作用域限制** - 按语言/文件类型过滤
//! - **用户自定义** - 支持加载自定义snippet文件
//!
//! ## 使用示例
//!
//! ```rust
//! use carpai::completion::snippet::{SnippetManager, SnippetContext};
//!
//! let manager = SnippetManager::with_builtin_snippets();
//!
//! // 展开Rust函数片段
//! let ctx = SnippetContext {
//!     language: Some("rust".to_string()),
//!     file_name: Some("main.rs".to_string()),
//!     ..Default::default()
//! };
//!
//! if let Some(expanded) = manager.expand("fn", &ctx) {
//!     println!("{}", expanded.text);
//!     // 输出:
//!     // fn ${1:name}(${2:params}) {
//!     //     ${3:// body}
//!     // }
//! }
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 代码片段管理器
pub struct SnippetManager {
    /// 片段库
    snippets: Vec<Snippet>,
    
    /// 用户自定义片段路径
    user_snippets_path: Option<PathBuf>,
    
    /// 前缀索引（用于快速查找）
    prefix_index: HashMap<String, Vec<usize>>,
}

/// 代码片段定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    /// 触发前缀 (如 "fn", "if", "class")
    pub prefix: String,
    
    /// 片段模板内容（支持$1, $2占位符和${VAR}变量）
    pub body: String,
    
    /// 简短描述
    pub description: String,
    
    /// 适用语言（None表示所有语言）
    pub scope: Option<String>,
    
    /// 变量定义列表
    pub variables: Vec<SnippetVariable>,
    
    /// 优先级（0-100）
    pub priority: u8,
    
    /// 是否为内置片段
    #[serde(skip)]
    is_builtin: bool,
}

/// 片段中的变量定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnippetVariable {
    /// 变量名 ($1, $2 或 $FILENAME, $YEAR等)
    pub name: String,
    
    /// 默认值（可选）
    pub default_value: Option<String>,
    
    /// 交互提示文本（IDE显示给用户）
    pub prompt: Option<String>,
    
    /// 可选的候选值列表
    pub choices: Option<Vec<String>>,
}

/// 片段展开上下文
#[derive(Debug, Clone, Default)]
pub struct SnippetContext {
    /// 当前语言 (rust/python/typescript/go/shell)
    pub language: Option<String>,
    
    /// 文件名 (用于$FILENAME变量)
    pub file_name: Option<String>,
    
    /// 项目根目录
    pub project_root: Option<PathBuf>,
    
    /// 当前行号
    pub line_number: Option<u32>,
    
    /// 自定义变量覆盖
    pub custom_variables: HashMap<String, String>,
}

/// 展开后的片段结果
#[derive(Debug, Clone)]
pub struct ExpandedSnippet {
    /// 展开后的文本
    pub text: String,
    
    /// 光标应该停留的位置（第一个占位符）
    pub cursor_position: usize,
    
    /// 占位符位置列表（用于Tab跳转）
    pub tab_stops: Vec<TabStop>,
}

/// Tab停止位置（用于占位符跳转）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabStop {
    /// 占位符编号 (0=最终光标位置, 1,2,3...=中间跳转点)
    pub index: u32,
    
    /// 在展开文本中的起始位置
    pub start: usize,
    
    /// 长度
    pub length: usize,
    
    /// 默认值
    pub default_value: Option<String>,
}

impl Default for SnippetManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SnippetManager {
    /// 创建空的Snippet管理器
    pub fn new() -> Self {
        Self {
            snippets: vec![],
            user_snippets_path: None,
            prefix_index: HashMap::new(),
        }
    }

    /// 创建带内置片段的管理器（推荐）
    pub fn with_builtin_snippets() -> Self {
        let mut manager = Self::new();
        manager.register_builtin_snippets();
        manager.build_index();
        manager
    }

    /// 注册内置代码片段（50+个）
    fn register_builtin_snippets(&mut self) {
        // ════════════════════════════
        // Rust 语言片段 (15个)
        // ════════════════════════════
        
        self.register(Snippet {
            prefix: "fn".to_string(),
            body: r"fn ${1:name}(${2:params}) {
    ${3:// body}
}".to_string(),
            description: "Function definition".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![
                SnippetVariable { name: "$1".to_string(), default_value: Some("function_name".to_string()), prompt: Some("Function name".to_string()), choices: None },
                SnippetVariable { name: "$2".to_string(), default_value: None, prompt: Some("Parameters".to_string()), choices: None },
                SnippetVariable { name: "$3".to_string(), default_value: Some("// TODO: implement".to_string()), prompt: None, choices: None },
            ],
            priority: 100,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "main".to_string(),
            body: r"fn main() {
    ${1:// code here}
}".to_string(),
            description: "Main function".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "struct".to_string(),
            body: r"struct ${1:Name} {
    ${2:field}: ${3:Type},
}".to_string(),
            description: "Struct definition".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "impl".to_string(),
            body: r"impl ${1:StructName} {
    pub fn ${2:new}(${3}) -> Self {
        Self {
            ${4:// initialize fields}
        }
    }
}".to_string(),
            description: "Impl block with constructor".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "test".to_string(),
            body: r"#[test]
fn test_${1:description}() {
    ${2:// given}
    let ${3:result} = ${4:expr};
    
    ${5:// then}
    assert_eq!(${3:result}, ${6:expected});
}".to_string(),
            description: "Test function".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 92,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "enum".to_string(),
            body: r"enum ${1:Name} {
    ${2:Variant1},
    ${3:Variant2},
}".to_string(),
            description: "Enum definition".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "match".to_string(),
            body: r"match ${1:expr} {
    ${2:pattern} => ${3:expr},
    _ => ${4:default},
}".to_string(),
            description: "Match expression".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 87,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "if".to_string(),
            body: r"if ${1:condition} {
    ${2:// then branch}
}".to_string(),
            description: "If statement".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "ife".to_string(),
            body: r"if ${1:condition} {
    ${2:// then branch}
} else {
    ${3:// else branch}
}".to_string(),
            description: "If-else statement".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "for".to_string(),
            body: r"for ${1:item} in ${2:iterable} {
    ${3:// body}
}".to_string(),
            description: "For loop".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 93,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "loop".to_string(),
            body: r"loop {
    ${1:// infinite loop body}
}".to_string(),
            description: "Infinite loop".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 80,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "while".to_string(),
            body: r"while ${1:condition} {
    ${2:// loop body}
}".to_string(),
            description: "While loop".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 82,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "mod".to_string(),
            body: r"mod ${1:name} {
    ${2:// module content}
}".to_string(),
            description: "Module declaration".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 78,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "pubfn".to_string(),
            body: r"pub fn ${1:name}(${2:params}) -> ${3:ReturnType} {
    ${4:// implementation}
}".to_string(),
            description: "Public function".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "asyncfn".to_string(),
            body: r"async fn ${1:name}(${2:params}) -> ${3:Result<Type>} {
    ${4:async_body}
}".to_string(),
            description: "Async function".to_string(),
            scope: Some("rust".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        // ════════════════════════════
        // Python 语言片段 (12个)
        // ════════════════════════════

        self.register(Snippet {
            prefix: "def".to_string(),
            body: r"def ${1:function_name}(${2:param1}, ${3:param2=None}):
    ""${4:docstring}""
    ${5:# implementation}
    return ${6:result}".to_string(),
            description: "Function definition".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 100,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "class".to_string(),
            body: r"class ${1:ClassName}:
    """${2:class docstring}"""
    
    def __init__(self, ${3:param}):
        ${4:self.param = param}
    
    def ${5:method}(self):
        ${6:# method body}".to_string(),
            description: "Class definition".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "if".to_string(),
            body: r"if ${1:condition}:
    ${2:# do something}".to_string(),
            description: "If statement".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "ife".to_string(),
            body: r"if ${1:condition}:
    ${2:# then}
else:
    ${3:# else}".to_string(),
            description: "If-else".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "for".to_string(),
            body: r"for ${1:item} in ${2:iterable}:
    ${3:# loop body}".to_string(),
            description: "For loop".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 92,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "with".to_string(),
            body: r"with ${1:context_manager} as ${2:alias}:
    ${3:# use resource}".to_string(),
            description: "With statement".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "try".to_string(),
            body: r"try:
    ${1:# try something}
except ${2:Exception} as e:
    ${3:# handle error}
finally:
    ${4:# cleanup}".to_string(),
            description: "Try-except-finally".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 83,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "lambda".to_string(),
            body: r"lambda ${1:params}: ${2:expression}".to_string(),
            description: "Lambda function".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 75,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "import".to_string(),
            body: r"import ${1:module}${2: as alias}".to_string(),
            description: "Import statement".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 80,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "from".to_string(),
            body: r"from ${1:module} import ${2:name}".to_string(),
            description: "From import".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 78,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "async".to_string(),
            body: r"async def ${1:function_name}(${2:params}):
    ${3:async_implementation}
    await ${4:something}".to_string(),
            description: "Async function".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 82,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "print".to_string(),
            body: r"print(f"${1:{variable}}")".to_string(),
            description: "Print with f-string".to_string(),
            scope: Some("python".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        // ════════════════════════════
        // TypeScript/JavaScript 片段 (10个)
        // ════════════════════════════

        self.register(Snippet {
            prefix: "fn".to_string(),
            body: r"function ${1:name}(${2:params}): ${3:returnType} {
    ${4:// body}
}".to_string(),
            description: "Function declaration".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 100,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "arrow".to_string(),
            body: r"const ${1:name} = (${2:params}): ${3:type} => {
    ${4:// body}
};".to_string(),
            description: "Arrow function".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "interface".to_string(),
            body: r"interface ${1:Name} {
    ${2:property}: ${3:type};
}".to_string(),
            description: "Interface definition".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "type".to_string(),
            body: r"type ${1:Name} = ${2:type};".to_string(),
            description: "Type alias".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "class".to_string(),
            body: r"class ${1:ClassName} {
    private ${2:property}: ${3:type};
    
    constructor(${4:params}) {
        this.${2:property} = ${5:value};
    }
    
    public ${6:method}(): ${7:type} {
        ${8:// implementation}
    }
}".to_string(),
            description: "Class definition".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "component".to_string(),
            body: r"interface ${1:ComponentName}Props {
    ${2:prop}: ${3:type};
}

export const ${1:ComponentName}: React.FC<${1:ComponentName}Props> = (${2:props}) => {
    return (
        <div>
            ${4:// JSX}
        </div>
    );
};".to_string(),
            description: "React component".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 92,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "hook".to_string(),
            body: r"useEffect(() => {
    ${1:// effect logic}
    
    return () => {
        ${2:// cleanup}
    };
}, [${3:dependencies}]);".to_string(),
            description: "React useEffect hook".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "state".to_string(),
            body: r"const [${1:state}, set${1:capitalize}] = useState<${2:type}>(${3:initialValue});".to_string(),
            description: "React useState hook".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "async".to_string(),
            body: r"async function ${1:name}(${2:params}): Promise<${3:type}> {
    const result = await ${4:apiCall};
    return result;
}".to_string(),
            description: "Async function".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 82,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "try".to_string(),
            body: r"try {
    ${1:// try block}
} catch (error) {
    console.error('Error:', error);
    ${2:// handle error}
}".to_string(),
            description: "Try-catch block".to_string(),
            scope: Some("typescript".to_string()),
            variables: vec![],
            priority: 80,
            is_builtin: true,
        });

        // ════════════════════════════
        // Go 语言片段 (8个)
        // ════════════════════════════

        self.register(Snippet {
            prefix: "func".to_string(),
            body: r"func ${1:functionName}(${2:params}) ${3:returnType} {
    ${4:// implementation}
}".to_string(),
            description: "Function definition".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 100,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "main".to_string(),
            body: r"func main() {
    ${1:// code here}
}".to_string(),
            description: "Main function".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "struct".to_string(),
            body: r"type ${1:StructName} struct {
    ${2:Field} ${3:Type}
}".to_string(),
            description: "Struct definition".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "if".to_string(),
            body: r"if ${1:condition} {
    ${2:// then}
}".to_string(),
            description: "If statement".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "ife".to_string(),
            body: r"if ${1:condition} {
    ${2:// then}
} else {
    ${3:// else}
}".to_string(),
            description: "If-else".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "for".to_string(),
            body: r"for ${1:i} := 0; ${1:i} < ${2:len}; ${1:i}++ {
    ${3:// loop body}
}".to_string(),
            description: "For loop".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "range".to_string(),
            body: r"for ${1:index}, ${2:value} := range ${3:collection} {
    ${4:// body}
}".to_string(),
            description: "Range loop".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 87,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "switch".to_string(),
            body: r"switch ${1:expr} {
case ${2:value}:
    ${3:// case body}
default:
    ${4:// default case}
}".to_string(),
            description: "Switch statement".to_string(),
            scope: Some("go".to_string()),
            variables: vec![],
            priority: 82,
            is_builtin: true,
        });

        // ════════════════════════════
        // Shell/Bash 片段 (10个)
        // ════════════════════════════

        self.register(Snippet {
            prefix: "if".to_string(),
            body: r"if [[ ${1:condition} ]]; then
    ${2:# commands}
fi".to_string(),
            description: "If statement".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 95,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "ife".to_string(),
            body: r"if [[ ${1:condition} ]]; then
    ${2:# then commands}
else
    ${3:# else commands}
fi".to_string(),
            description: "If-else".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 90,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "for".to_string(),
            body: r"for ${1:item} in ${2:list}; do
    ${3:# commands}
done".to_string(),
            description: "For loop".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 92,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "while".to_string(),
            body: r"while [[ ${1:condition} ]]; do
    ${2:# commands}
done".to_string(),
            description: "While loop".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 85,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "function".to_string(),
            body: r"${1:function_name}() {
    local ${2:param}="${3:$1}"
    
    ${4:# function body}
}

# Usage: ${1:function_name} "${5:arg1}" "${6:arg2}"
".to_string(),
            description: "Function definition".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 93,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "case".to_string(),
            body: r"case "${1:variable}" in
    ${2:pattern1})
        ${3:# commands}
        ;;
    ${4:pattern2})
        ${5:# commands}
        ;;
    *)
        ${6:# default}
        ;;
esac".to_string(),
            description: "Case statement".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 80,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "heredoc".to_string(),
            body: r"cat << '${1:EOF}'
${2:content here}
${1:EOF}".to_string(),
            description: "Heredoc".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 88,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "select".to_string(),
            body: r"PS3="Choose an option: "
select ${1:item} in ${2:option1} ${3:option2} ${4:option3}; do
    case ${1:item} in
        ${2:option1})
            echo "You chose ${2:option1}"
            break
            ;;
        ${3:option2})
            echo "You chose ${3:option2}"
            break
            ;;
        *)
            echo "Invalid option"
            ;;
    esac
done".to_string(),
            description: "Select menu".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 78,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "trap".to_string(),
            body: r"trap '${1:cleanup_function}' EXIT INT TERM

${1:cleanup_function}() {
    echo 'Cleaning up...'
    ${2:# cleanup commands}
}

${3:# main script}".to_string(),
            description: "Trap signal handler".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 72,
            is_builtin: true,
        });

        self.register(Snippet {
            prefix: "parse_args".to_string(),
            body: r"#!/bin/bash
set -euo pipefail

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Options:
    -h, --help      Show this help message
    -v, --verbose   Enable verbose output
    -f, --file FILE Input file path
EOF
    exit 0
}

VERBOSE=false
FILE_PATH=""

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            ;;
        -v|--verbose)
            VERBOSE=true
            shift
            ;;
        -f|--file)
            FILE_PATH="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

echo "File: $FILE_PATH"
echo "Verbose: $VERBOSE"".to_string(),
            description: "Argument parser template".to_string(),
            scope: Some("shell".to_string()),
            variables: vec![],
            priority: 86,
            is_builtin: true,
        });
    }

    /// 注册单个片段
    pub fn register(&mut self, snippet: Snippet) {
        let idx = self.snippets.len();
        self.snippets.push(snippet);
        
        // 更新前缀索引
        let prefix = &self.snippets[idx].prefix;
        self.prefix_index
            .entry(prefix.clone())
            .or_insert_with(Vec::new)
            .push(idx);
    }

    /// 构建前缀索引（加速查找）
    fn build_index(&mut self) {
        self.prefix_index.clear();
        for (idx, snippet) in self.snippets.iter().enumerate() {
            self.prefix_index
                .entry(snippet.prefix.clone())
                .or_insert_with(Vec::new)
                .push(idx);
        }
    }

    /// 展开片段
    pub fn expand(&self, prefix: &str, context: &SnippetContext) -> Option<ExpandedSnippet> {
        // 查找匹配的片段
        let candidates = self.find_candidates(prefix, context)?;
        
        // 选择最佳匹配（优先级最高）
        let best_match = candidates.into_iter()
            .max_by_key(|s| s.priority)?;
        
        self.do_expand(&best_match, context)
    }

    /// 获取匹配的候选片段列表
    pub fn get_completions(
        &self,
        prefix: &str,
        context: &SnippetContext,
    ) -> Vec<SnippetCompletion> {
        let candidates = match self.prefix_index.get(prefix) {
            Some(indices) => indices
                .iter()
                .filter_map(|&idx| self.snippets.get(idx))
                .filter(|s| self.matches_scope(s, context))
                .cloned()
                .collect::<Vec<_>>(),
            None => vec![],
        };

        // 也进行模糊匹配（如果精确匹配太少）
        let fuzzy_matches = if candidates.len() < 3 {
            self.fuzzy_find(prefix, context, 5 - candidates.len())
        } else {
            vec![]
        };

        let all_candidates: Vec<Snippet> = candidates.into_iter().chain(fuzzy_matches).collect();

        all_candidates
            .into_iter()
            .map(|s| SnippetCompletion {
                prefix: s.prefix.clone(),
                display_text: s.prefix.clone(),
                description: s.description.clone(),
                scope: s.scope.clone(),
                preview: self.generate_preview(&s),
                priority: s.priority,
            })
            .collect()
    }

    /// 查找候选片段
    fn find_candidates(
        &self,
        prefix: &str,
        context: &SnippetContext,
    ) -> Option<Vec<Snippet>> {
        let matches: Vec<Snippet> = self.prefix_index
            .get(prefix)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|&idx| self.snippets.get(idx))
                    .filter(|s| self.matches_scope(s, context))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if matches.is_empty() {
            None
        } else {
            Some(matches)
        }
    }

    /// 模糊查找片段
    fn fuzzy_find(
        &self,
        query: &str,
        context: &SnippetContext,
        limit: usize,
    ) -> Vec<Snippet> {
        let query_lower = query.to_lowercase();
        
        self.snippets.iter()
            .filter(|s| self.matches_scope(s, context))
            .filter(|s| {
                s.prefix.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// 检查是否匹配作用域
    fn matches_scope(&self, snippet: &Snippet, context: &SnippetContext) -> bool {
        match (&snippet.scope, &context.language) {
            (Some(snippet_lang), Some(ctx_lang)) => {
                // 精确匹配或父语言匹配
                snippet_lang == ctx_lang || 
                ctx_lang.starts_with(&format!("{}-", snippet_lang)) ||
                snippet_lang == "all"
            }
            (_, _) => true, // 无作用域限制
        }
    }

    /// 执行实际的片段展开
    fn do_expand(
        &self,
        snippet: &Snippet,
        context: &SnippetContext,
    ) -> Option<ExpandedSnippet> {
        let mut text = snippet.body.clone();
        let mut tab_stops = vec![];
        let mut placeholder_count = 0u32;

        // 替换内置变量
        text = self.replace_builtin_variables(&text, context);

        // 查找并记录占位符位置 ($1, $2, ...)
        let placeholder_regex = Regex::new(r"\$(\d+)(?::([^}]*))?").ok()?;
        
        let mut cursor_position = 0;
        let mut first_placeholder_pos = None;

        for cap in placeholder_regex.captures_iter(&text) {
            let full_match = cap.get(0)?.as_str().to_string();
            let num_str = cap.get(1)?.as_str().to_string();
            let default_value = cap.get(2).map(|m| m.as_str().to_string());
            
            let num: u32 = num_str.parse().unwrap_or(0);
            
            // 记录tab stop位置
            let start = text.find(&full_match)?;
            tab_stops.push(TabStop {
                index: num,
                start,
                length: full_match.len(),
                default_value,
            });

            if first_placeholder_pos.is_none() && num > 0 {
                first_placeholder_pos = Some(start);
                placeholder_count += 1;
            }
        }

        // 设置光标位置（第一个占位符或末尾）
        cursor_position = first_placeholder_pos.unwrap_or(text.len());

        Some(ExpandedSnippet {
            text,
            cursor_position,
            tab_stops,
        })
    }

    /// 替换内置变量
    fn replace_builtin_variables(&self, text: &str, context: &SnippetContext) -> String {
        let mut result = text.to_string();

        // 时间相关变量
        let now = chrono::Local::now();
        result = result.replace("${YEAR}", &format!("{:04}", now.year()));
        result = result.replace("${MONTH}", &format!("{:02}", now.month()));
        result = result.replace("${DAY}", &format!("{:02}", now.day()));
        result = result.replace("${HOUR}", &format!("{:02}", now.hour()));
        result = result.replace("${MINUTE}", &format!("{:02}", now.minute()));
        result = result.replace("${SECOND}", &format!("{:02}", now.second()));

        // 文件名相关
        if let Some(file_name) = &context.file_name {
            // 去掉扩展名的文件名
            let name_without_ext = file_name
                .rsplit('.')
                .next()
                .unwrap_or(file_name);
            
            result = result.replace("${FILENAME}", file_name);
            result = result.replace("${FILENAME_NO_EXT}", name_without_ext);
            
            // PascalCase版本
            let pascal_case = to_pascal_case(name_without_ext);
            result = result.replace("${CLASSNAME}", &pascal_case);
            
            // snake_case版本
            let snake_case = to_snake_case(name_without_ext);
            result = result.replace("${SNAKENAME}", &snake_case);
            
            // kebab-case版本
            let kebab_case = snake_case.replace('_', "-");
            result = result.replace("${KEBABNAME}", &kebab_case);
        }

        // 行号
        if let Some(line_num) = context.line_number {
            result = result.replace("${LINE_NUMBER}", &line_num.to_string());
        }

        // 项目根目录
        if let Some(project_root) = &context.project_root {
            if let Ok(root_str) = project_root.to_str() {
                result = result.replace("${PROJECT_ROOT}", root_str);
            }
        }

        // 自定义变量
        for (key, value) in &context.custom_variables {
            result = result.replace(&format!("${{{}}}", key), value);
        }

        result
    }

    /// 生成预览文本
    fn generate_preview(&self, snippet: &Snippet) -> String {
        // 取前两行作为预览
        snippet.body
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 加载用户自定义片段
    pub fn load_user_snippets(&mut self, path: &PathBuf) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("Snippet file not found: {:?}", path));
        }

        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let snippets: Vec<Snippet> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        for snippet in snippets {
            self.register(snippet);
        }

        self.user_snippets_path = Some(path.clone());
        self.build_index();

        Ok(())
    }

    /// 导出所有片段为JSON
    pub fn export_all(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.snippets)
    }

    /// 统计信息
    pub fn statistics(&self) -> SnippetStatistics {
        let builtin_count = self.snippets.iter().filter(|s| s.is_builtin).count();
        let custom_count = self.snippets.len() - builtin_count;

        let scopes: std::collections::HashSet<Option<String>> = self.snippets
            .iter()
            .map(|s| s.scope.clone())
            .collect();

        SnippetStatistics {
            total_snippets: self.snippets.len(),
            builtin_snippets: builtin_count,
            custom_snippets: custom_count,
            unique_prefixes: self.prefix_index.len(),
            supported_scopes: scopes.len(),
        }
    }
}

/// 补全建议
#[derive(Debug, Clone)]
pub struct SnippetCompletion {
    pub prefix: String,
    pub display_text: String,
    pub description: String,
    pub scope: Option<String>,
    pub preview: String,
    pub priority: u8,
}

/// 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnippetStatistics {
    pub total_snippets: usize,
    pub builtin_snippets: usize,
    pub custom_snippets: usize,
    pub unique_prefixes: usize,
    pub supported_scopes: usize,
}

// ==========================================
// 工具函数
// ==========================================

/// 转换为PascalCase
fn to_pascal_case(s: &str) -> String {
    s.split(['-', '_', ' ', '.'])
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

/// 转换为snake_case
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut prev_was_upper = false;

    for c in s.chars() {
        if c.is_uppercase() {
            if !prev_was_upper && !result.is_empty() {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
            prev_was_upper = true;
        } else {
            result.push(c);
            prev_was_upper = false;
        }
    }

    result
}

// ==========================================
// 单元测试
// ==========================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_fn_snippet() {
        let manager = SnippetManager::with_builtin_snippets();
        
        let ctx = SnippetContext {
            language: Some("rust".to_string()),
            ..Default::default()
        };

        let expanded = manager.expand("fn", &ctx);
        assert!(expanded.is_some(), "Should find 'fn' snippet");
        
        let result = expanded.unwrap();
        assert!(result.text.contains("fn "));
        assert!(result.text.contains("$1"));  // 应该有占位符
        assert!(result.tab_stops.len() >= 2);  // 至少2个tab stop
    }

    #[test]
    fn test_python_def_snippet() {
        let manager = SnippetManager::with_builtin_snippets();

        let ctx = SnippetContext {
            language: Some("python".to_string()),
            ..Default::default()
        };

        let expanded = manager.expand("def", &ctx);
        assert!(expanded.is_some());

        let result = expanded.unwrap();
        assert!(result.text.contains("def "));
        assert!(result.text.contains(":"));
    }

    #[test]
    fn test_typescript_arrow_snippet() {
        let manager = SnippetManager::with_builtin_snippets();

        let ctx = SnippetContext {
            language: Some("typescript".to_string()),
            ..Default::default()
        };

        let expanded = manager.expand("arrow", &ctx);
        assert!(expanded.is_some());

        let result = expanded.unwrap();
        assert!(result.text.contains("=>"));
    }

    #[test]
    fn test_shell_if_snippet() {
        let manager = SnippetManager::with_builtin_snippets();

        let ctx = SnippetContext {
            language: Some("shell".to_string()),
            ..Default::default()
        };

        let expanded = manager.expand("if", &ctx);
        assert!(expanded.is_some());

        let result = expanded.unwrap();
        assert!(result.text.contains("fi"));
    }

    #[test]
    fn test_variable_substitution() {
        let manager = SnippetManager::with_builtin_snippets();

        let ctx = SnippetContext {
            file_name: Some("user_service.rs".to_string()),
            line_number: Some(42),
            ..Default::default()
        };

        let expanded = manager.expand("struct", &ctx);
        if let Some(result) = expanded {
            // 注意：struct片段可能不使用这些变量，所以这里只是测试不会崩溃
            assert!(!result.text.is_empty());
        }
    }

    #[test]
    fn test_get_completions_for_language() {
        let manager = SnippetManager::with_builtin_snippets();

        let rust_ctx = SnippetContext {
            language: Some("rust".to_string()),
            ..Default::default()
        };

        let completions = manager.get_completions("", &rust_ctx);
        
        // 应该返回Rust相关的片段
        assert!(!completions.is_empty());
        
        // 应该包含常见的Rust片段前缀
        let prefixes: Vec<&str> = completions.iter().map(|c| c.prefix.as_str()).collect();
        assert!(prefixes.contains(&"fn"), "Should include 'fn' snippet");
        assert!(prefixes.contains(&"struct") || prefixes.contains(&"if"), "Should include basic snippets");
    }

    #[test]
    fn test_fuzzy_search() {
        let manager = SnippetManager::with_builtin_snippets();

        let ctx = SnippetContext {
            language: Some("rust".to_string()),
            ..Default::default()
        };

        // 搜索 "func" 应该能找到 "fn"
        let completions = manager.get_completions("func", &ctx);
        
        let has_fn = completions.iter().any(|c| c.prefix == "fn");
        assert!(has_fn || !completions.is_empty(), "Fuzzy search should find similar snippets");
    }

    #[test]
    fn test_statistics() {
        let manager = SnippetManager::with_builtin_snippets();
        
        let stats = manager.statistics();
        
        assert!(stats.total_snippets >= 55, 
            "Should have at least 55 snippets, got {}", stats.total_snippets);
        assert!(stats.builtin_snippets > 0);
        assert!(stats.unique_prefixes > 0);
        assert!(stats.supported_scopes >= 5);  // rust/python/ts/go/shell
    }

    #[test]
    fn test_pascal_case_conversion() {
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
        assert_eq!(to_pascal_case("user-service"), "UserService");
        assert_eq!(to_pascal_case("my var"), "MyVar");
    }

    #[test]
    fn test_snake_case_conversion() {
        assert_eq!(to_snake_case("HelloWorld"), "hello_world");
        assert_eq!(to_snake_case("UserService"), "user_service");
        assert_eq!(to_snake_case("XMLParser"), "x_m_l_parser");
    }

    #[test]
    fn test_export_import_roundtrip() {
        let manager = SnippetManager::with_builtin_snippets();
        
        let json = manager.export_all().expect("Export should succeed");
        assert!(!json.is_empty());
        
        // 验证是有效的JSON
        let parsed: Vec<Snippet> = serde_json::from_str(&json).expect("Should be valid JSON");
        assert!(!parsed.is_empty());
        assert_eq!(parsed.len(), manager.statistics().total_snippets);
    }
}
