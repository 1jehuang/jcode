---
name: L3-trait-impl-fixes
overview: 修复 src/ 下剩余的所有 L3 Trait 实现相关错误，涵盖 aho_corasick.rs、host_keys.rs、dynamic_registry.rs 等文件的 Ord、Default、方法签名和借用问题
todos:
  - id: fix-aho-trait
    content: "修复 aho_corasick.rs: RiskLevel 添加 Ord/PartialOrd derive，AhoCorasickBuilder 改用 &mut self 链式调用，anyhow::Error 转 Box<dyn Error>，assert! 宏修复，清理警告"
    status: completed
  - id: fix-host-trait
    content: "修复 host_keys.rs: _glob_match 添加 &self 参数，_match_glob_helper 的 char/usize 类型对齐，if/else 返回值类型一致化"
    status: completed
  - id: fix-host-borrow
    content: "修复 host_keys.rs: retain 中 self 借用冲突拆分，existing_entries 移动后借用预收集"
    status: completed
    dependencies:
      - fix-host-trait
  - id: fix-registry-syntax
    content: "修复 registry.rs: Python 替换造成的 3 处 default_value 语法错误，恢复缺失的 ..Default::default()，修复类型推断"
    status: completed
  - id: fix-fish
    content: "修复 fish.rs: match 模式变量绑定、FishCommandNode 类型不匹配、abr 拼写错误"
    status: completed
  - id: fix-editorial
    content: 修复 debug_panel.rs BOM、server_impl.rs tests 模块、cot_engine.rs 借用/String/&str 等剩余小问题
    status: completed
---

## 需求说明

修复 Phase 4 P3 API 适配中剩余的 L3 Trait 实现编译错误，覆盖 6 个关键文件中的约 95 个错误。按复杂度分级，从 Trait 实现问题（Ord、Default、&self）开始，依次处理借用错误、类型不匹配和格式字符串错误。

## 已确认的问题分类

### Level 3a — Trait 实现（核心 L3 问题）

- **aho_corasick.rs**: RiskLevel 缺 `Ord`/`PartialOrd` derive；AhoCorasickBuilder API 返回 `&mut Self` 非 `Self`；`anyhow::Error` 与 `Box<dyn Error>` 不匹配
- **host_keys.rs**: `_glob_match` 是关联函数但调用 `self._match_glob_helper()`（缺 `&self`）；`_match_glob_helper` 中 `&[usize]` 与 `char` 比较

### Level 3b — 借用/类型错误

- **host_keys.rs**: `self.entries.retain()` 中 `&self` 与 `&mut self` 冲突；`existing_entries` 被 for 循环消费后借用；`if/else` 分支返回类型不一致
- **cot_engine.rs**: 多处 `chain`/`final_step`/`synthesis_step` 移动后被借用

### Level 3c — 格式字符串/类型

- **docs.rs**: 35 处 format! 参数计数不匹配
- **skeletons.rs**: 6 处 format! 参数计数不匹配
- **registry.rs**: Python 批量替换引入的语法错误 + 残留的缺失字段错误
- **fish.rs**: match 模式变量绑定不一致、typo、类型不匹配
- **debug_panel.rs**: BOM 字符、format 参数未使用
- **server_impl.rs**: tests 模块文件缺失

## 修复策略

1. 优先修复 Trait 实现（Ord、Default、&self）→ 消除编译阻塞
2. 借用错误通过预克隆 + 作用域拆分解决
3. String/&str 批量加 `.to_string()`
4. format! 字符串按参数补齐 / 移除多余参数
5. 清理未使用导入/变量

## 技术方案

### 1. aho_corasick.rs — Trait 实现修复

**RiskLevel 添加 Ord**:

```rust
// 当前 (L75):
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
// 改为:
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
```

**AhoCorasickBuilder API 适配**:

```rust
// 当前 (L316-322):
let mut builder = aho_corasick::AhoCorasickBuilder::new();
builder = builder.ascii_case_insensitive(true);   // 返回 &mut Self, 不能重赋值
builder = builder.match_kind(...);                 // 同上
// 改为链式调用:
let mut builder = aho_corasick::AhoCorasickBuilder::new();
if config.case_insensitive {
    builder.ascii_case_insensitive(true);
}
builder.match_kind(aho_corasick::MatchKind::LeftmostFirst);
```

**Error 类型转换** (L301-307):

```rust
// 当前: anyhow::bail!("...") 在返回 Result<_, Box<dyn Error>> 的函数中
// 改为:
return Err(anyhow::anyhow!("...").into());
```

**assert! 宏修复** (L860-861):

```rust
// 当前: assert!(stats.hit_rate > 0.9, format!("..."));
// 改为: assert!(stats.hit_rate > 0.9, "Cache hit rate should be >90%, got {:.2}%", ...);
```

### 2. host_keys.rs — 方法签名 + 类型修复

**glob_match 添加 &self** (L752):

```rust
// 当前: fn _glob_match(pattern: &str, text: &str) -> bool
// 改为: fn _glob_match(&self, pattern: &str, text: &str) -> bool
```

**char vs usize 比较** (L758-781):

```rust
// 当前: text: &[usize] 但比较时用 char (c in pattern)
// 改为: text: &[char] — 在 _host_matches_pattern 中转换调用
```

**retain 中的 self 借用冲突** (L487-579):

```rust
// 当前: self.entries.retain(|entry| !self._host_matches_pattern(...))
// 改为: 预计算 matches 结果后再 retain
```

**if/else 类型一致** (L676-684):

```rust
// 当前: (parts[0], 2) 返回 (&str, _) 但 else 分支返回 (String, _)
// 改为: (parts[0].to_string(), 2)
```

### 3. registry.rs — 语法错误修复

Python 批量替换导致 `..Default::default()` 和 `default_value: None` 同时存在（L218-227），需删除 `default_value`/`description` 字段（被 Default 覆盖）。同时修复 L927/938 的类型推断。

### 4. 其他文件

- **fish.rs**: L640 移除未绑定变量 `s`；L799/801 修正 `FishCommandNode` vs `FishAstNode`；L1908 修复 `abr` → `abbr`
- **cot_engine.rs**: 预克隆 `chain.clone()` 避免移动后借用；加 `.to_string()` 修复 String/&str
- **docs.rs/skeletons.rs**: format! 按参数补齐或移除多余参数
- **debug_panel.rs**: BOM 用 Python 脚本移除
- **server_impl.rs**: 创建空 tests.rs 或添加 `#[allow(unused)]` 属性

## 使用的 Agent 扩展

- [subagent:code-explorer]: 用于批量扫描文件结构，定位剩余的编译错误分布