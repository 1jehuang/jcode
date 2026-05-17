# CarpAI 语法错误检测与修复策略

## 一、常见语法错误模式库

### 1. 字符串/字符字面量错误 (最高优先级)

```rust
// ❌ 错误模式 A: 缺少引号
("key", value),        // 缺少值的开引号
("key", "value),       // 缺少值的闭引号
('char),               // 缺少闭单引号

// ✅ 修复
("key", "value"),
("key", "value"),
('char'),

// ❌ 错误模式 B: 原始字符串中的引号冲突
let s = r"say \"hello\"";     // r"..." 不能包含未转义的 "
let regex = Regex::new(r"[a-z']+");  // ' 在 r"..." 中有问题

// ✅ 修复
let s = r#"say "hello""#;
let regex = Regex::new(r#"[a-z']+"#);
```

### 2. Unicode 非法字符 (第二优先级)

```rust
// ❌ 裸露的绘图字符 (不在字符串/注释中)
let x = │;      // U+2502
let y = ─;      // U+2500
let z = →;      // U+2192

// ✅ 替换为 ASCII 或放入字符串
let x = "|";
let y = "-";
let z = "->";

// 或者使用常量
const VERTICAL_BAR: &str = "\u{2502}";
```

### 3. Rust Edition 兼容性

```rust
// ❌ Rust 2024 edition 问题
let x = vec.iter().any(|*t| t == target);  // *t 模式解构变化
let gen = ...;  // gen 是保留字

// ✅ 修复
let x = vec.iter().any(|t| t == &target);
let generator = ...;  // 或 r#gen
```

### 4. 模块声明规则

```rust
// ❌ 错误: 在非 mod.rs 文件中声明子模块
// 文件: src/cli/claude_compat.rs
mod sub_module;  // ❌ 这里不能这样写!

// ✅ 正确做法:
// 方案 1: 在 src/cli/mod.rs 中声明
// 方案 2: 直接在 claude_compat.rs 中定义内容
// 方案 3: 使用 inline module
pub mod sub_module { /* ... */ }
```

## 二、诊断策略

### 步骤 1: 快速分类错误

```bash
# 获取错误统计
cargo check --lib -p carpai 2>&1 | Select-String "^error" |
  Group-Object { $_.Line.Substring(0,50) } |
  Sort-Object Count -Descending
```

**按数量排序的优先级：**
1. `unknown start of token` → Unicode/编码问题
2. `unterminated string/char` → 字符串未关闭
3. `prefix X is unknown` → 上游字符串问题的级联效应
4. `expected item, found` → 语法结构破坏
5. `cannot find/import` → 导入或模块问题

### 步骤 2: 定位根因

**关键原则：一个错误可能引发数百个级联错误**

```
示例链:
第398行: ("rebase",变基分支"),  ← 根因: 缺少 "
         ↓
编译器认为 ,变基分支 是标识符
         ↓
后续所有 " 都被解析为 prefix 操作符
         ↓
产生 400+ 个 "unknown prefix" 错误
```

### 步骤 3: 修复顺序

```
P0 - 语法/解析错误 (必须先修)
 ├── 字符串字面量未关闭
 ├── Unicode 非法字符
 ├── 括号/花括号不匹配
 └── 模块声明位置错误

P1 - 类型/导入错误
 ├── unresolved import
 ├── cannot find type
 └── missing fields

P2 - 借用/生命周期
 ├── E0502/E0507 borrow errors
 ├── E0515 lifetime errors
 └── trait bound issues
```

## 三、自动化检测规则

### 可以添加到 CarpAI 的 lint 规则:

```rust
// 1. 检测裸露的 Unicode 绘图字符
// 正则: [^\x00-\x7F] 不在字符串/注释中时报警

// 2. 检测原始字符串中的特殊字符
// r"..." 内部包含 ", ', \ 时警告

// 3. 检测模块声明位置
// 非 mod.rs 文件中的 mod xxx; 声明

// 4. 检测不匹配的括号/引号
// 使用简单的计数器检查平衡性

// 5. 检测 Rust 2024 保留字使用
// gen, try, async 等作为变量名
```

## 四、修复工作流模板

```bash
# 1. 获取基线
cargo check --lib -p carpai 2>&1 > baseline.txt

# 2. 分析并分类
cat baseline.txt | grep "^error" | sort | uniq -c | sort -rn

# 3. 修复 P0 错误
# 编辑文件...

# 4. 验证减少量
cargo check --lib -p carpai 2>&1 | tail -5

# 5. 重复直到完成
```
