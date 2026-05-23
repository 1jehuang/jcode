# Phase 2: 类型系统问题修复任务清单

## 问题分类总览

| 类别 | 错误代码 | 数量 | 优先级 |
|------|---------|------|--------|
| 模块/类型解析 | E0761, E0433, E0425 | ~20 | P0 |
| 生命周期参数 | E0106, E0195 | ~6 | P0 |
| 类型不匹配 | E0308, E0424, E0282 | ~60 | P1 |
| 缺少字段/方法 | E0609, E0599 | ~25 | P1 |
| 借用检查器 | E0502, E0382 | ~6 | P1 |
| Async/Future | E0728, E0277 | ~10 | P2 |
| 模式匹配 | E0004 | 1 | P2 |
| Trait兼容 | E0038 | 2 | P2 |
| 常量计算 | E0010, E0015 | ~50 | P2 |
| 其他 | E0560, E0618, E0521 | ~5 | P3 |

---

## P0: 阻塞性问题 (必须首先修复)

### Task 2.1: 修复 logging 模块冲突 [P0]
**错误**: `E0761: file for module logging found at both src\logging.rs and src\logging\mod.rs`

**问题**: 存在两个 logging 模块文件
**影响**: 阻止所有编译

**修复步骤**:
1. 检查 `src/logging.rs` 是否是旧文件
2. 如果 `src/logging.rs` 是 stub，删除它
3. 确保 `src/logging/mod.rs` 是正确的模块实现
4. 验证 `info` 函数在模块中正确导出

---

### Task 2.2: 修复 `tc` 和 `mgr` 变量未定义 [P0]
**错误**: `E0425: cannot find value tc/mgr in this scope` (8处)

**问题**: 代码引用了未定义的变量

**修复步骤**:
1. 搜索 `tc` 的所有出现位置
2. 追踪它应该在何处定义
3. 如果是重构产物，添加适当的定义
4. 如果是遗留代码，更新引用或删除

---

### Task 2.3: 修复 `providers` 模块 [P0]
**错误**: `E0433: failed to resolve: use of unresolved module or unlinked crate providers` (4处)

**问题**: completion_engine 下的 providers 模块引用失败

**修复步骤**:
1. 检查 `src/completion_engine/providers` 目录
2. 确认模块结构完整
3. 修复 providers.rs 中的模块声明

---

### Task 2.4: 修复 `CompletionPrefetchState` 类型 [P0]
**错误**: `E0433: cannot find CompletionPrefetchState` (1处)
**错误**: `E0425: cannot find type CompletionPrefetchState` (1处)

**修复步骤**:
1. 在 `src/tui/completion_helper.rs` 中添加 `CompletionPrefetchState` 类型定义
2. 或找到正确的定义位置并导入

---

### Task 2.5: 修复 `ContentBlock` 类型 [P0]
**错误**: `E0433: failed to resolve: use of undeclared type ContentBlock` (1处)
**错误**: `E0424: expected value, found module self` (2处)

**修复步骤**:
1. 检查 `jcode-message-types` crate 中是否有 ContentBlock
2. 如果没有，在 `src/types/` 中定义
3. 更新所有引用位置

---

### Task 2.6: 修复 `RedisCache` 类型 [P0]
**错误**: `E0425: cannot find type RedisCache in this scope` (2处)

**修复步骤**:
1. 在 `src/cache/` 中创建 `redis_cache.rs`
2. 实现简单的 RedisCache 结构体（可使用 HashMap 作为临时替代）
3. 或删除对 RedisCache 的依赖

---

### Task 2.7: 修复 `num_cpus` crate [P0]
**错误**: `E0433: failed to resolve: use of unresolved module or unlinked crate num_cpus` (4处)

**修复步骤**:
1. 在 `Cargo.toml` 中添加 `num_cpus` 依赖
2. 或替换为 `std::thread::available_parallelism()`

---

### Task 2.8: 修复 `provide_completions` 生命周期 [P0]
**错误**: `E0195: lifetime parameters or bounds on method provide_completions do not match the trait declaration` (4处)

**修复步骤**:
1. 检查 trait 定义中的生命周期参数
2. 确保实现中的签名与 trait 声明完全匹配
3. 可能需要添加 `#[async_trait]` 宏

---

### Task 2.9: 修复 `missing lifetime specifier` [P0]
**错误**: `E0106: missing lifetime specifier` (1处)

**修复步骤**:
1. 找到缺少生命周期说明符的函数/方法
2. 添加适当的 `'a` 或其他生命周期参数

---

## P1: 高优先级问题

### Task 2.10: 修复类型注解缺失 [P1]
**错误**: `E0282: type annotations needed` (~30处)

**修复步骤**:
1. 为每个需要类型注解的位置添加明确的类型
2. 可能需要使用 `.collect::<Vec<_>>()` 等形式

---

### Task 2.11: 修复类型不匹配 [P1]
**错误**: `E0308: mismatched types` (~35处)

**修复步骤**:
1. 检查每个不匹配的类型的实际类型和期望类型
2. 添加类型转换（如 `.to_string()`, `.as_str()` 等）
3. 或修改函数签名以接受正确类型

---

### Task 2.12: 修复 tree_sitter::SymbolInfo 字段 [P1]
**错误**: `E0609: no field signature/range/kind on type &tree_sitter::SymbolInfo` (12处)

**问题**: tree-sitter API 变更，SymbolInfo 结构体字段名不同

**修复步骤**:
1. 检查 tree-sitter 版本的 SymbolInfo 实际字段
2. 使用正确的字段名（如 `name` 替代 `signature`）
3. 或使用兼容层

---

### Task 2.13: 修复缺少的方法 [P1]
**错误**: `E0599: no method named list_all_tools/unwrap_or/success/finish` 等

**修复步骤**:
1. `list_all_tools`: 在 DynamicToolRegistry 实现该方法
2. `unwrap_or`: 检查返回类型是否正确
3. `success`: 在 ToolOutput 实现该方法
4. `finish`: 检查 CoreWrapper 的正确方法名

---

### Task 2.14: 修复借用检查器错误 [P1]
**错误**: `E0502: cannot borrow as immutable because it is also borrowed as mutable` (4处)

**修复步骤**:
1. 分离借用范围
2. 在使用可变借用后，显式 drop 或使用代码块限制作用域

---

### Task 2.15: 修复 `expected value, found module self` [P1]
**错误**: `E0424: expected value, found module self` (2处)

**修复步骤**:
1. 检查 self 的使用是否正确
2. 可能需要使用 `*self` 或 `self.as_ref()`

---

## P2: 中优先级问题

### Task 2.16: 修复 Async/Await 问题 [P2]
**错误**: `E0728: await is only allowed inside async functions` (2处)
**错误**: `E0277: F is not a future, () is not a future` (8处)

**修复步骤**:
1. 将同步函数改为 async
2. 或重构代码以避免在同步上下文中使用 await
3. 检查泛型约束 `F: Future`

---

### Task 2.17: 修复 LspOperation 模式匹配 [P2]
**错误**: `E0004: non-exhaustive patterns` (1处)

**修复步骤**:
1. 添加 `CodeAction` 和 `Rename` 分支
2. 使用 `todo!()` 作为临时实现

---

### Task 2.18: 修复 VectorDatabase trait [P2]
**错误**: `E0038: the trait VectorDatabase is not dyn compatible` (2处)

**修复步骤**:
1. 为 trait 添加 `dyn` 兼容
2. 或使用类型参数而非 trait 对象

---

### Task 2.19: 修复常量中的分配 [P2]
**错误**: `E0010: allocations are not allowed in constants` (50处)
**错误**: `E0015: cannot call non-const method in constants` (50处)

**修复步骤**:
1. 将 const 改为 static
2. 或将初始化移至运行时

---

### Task 2.20: 修复 `Bm25Scorer` 和 `VectorSearchEngine` [P2]
**错误**: `E0433: failed to resolve: use of undeclared type` (2处)

**修复步骤**:
1. 在 `src/knowledge/` 中创建类型定义
2. 或使用桩类型作为临时替代

---

### Task 2.21: 修复 StreamEvent 变体 [P2]
**错误**: `E0599: no variant named ContentDelta/ContentBlockStop` (2处)

**修复步骤**:
1. 检查 jcode-message-types 中 StreamEvent 的实际变体
2. 使用正确的变体名称

---

### Task 2.22: 修复 `TestGenerator` 方法 [P2]
**错误**: `E0599: no method named generate_unit_test_llm` (2处)

**修复步骤**:
1. 在 `src/tdd/mod.rs` 的 TestGenerator 中实现该方法
2. 或删除对该方法的调用

---

### Task 2.23: 修复 `VectorDatabase` trait 方法 [P2]
**错误**: `E0061: this function takes 0 arguments but 1 argument was supplied` (2处)

**修复步骤**:
1. 检查函数签名
2. 提供正确的参数数量

---

## P3: 低优先级问题

### Task 2.24: 修复 AstEdit 结构体 [P3]
**错误**: `E0560: struct AstEdit has no field named operation/start_line/end_line/content` (8处)

**修复步骤**:
1. 检查正确的 AstEdit 字段名
2. 使用正确的字段名

---

### Task 2.25: 修复 `expected function, found F` [P3]
**错误**: `E0618: expected function, found F`

**修复步骤**:
1. 检查泛型参数的使用
2. 可能需要调用 `func()` 而非传递 `func`

---

### Task 2.26: 修复 `borrowed data escapes` [P3]
**错误**: `E0521: borrowed data escapes outside of method`

**修复步骤**:
1. 调整生命周期参数
2. 确保借用数据在方法返回前不被释放

---

### Task 2.27: 修复 `borrow of moved value` [P3]
**错误**: `E0382: borrow of moved value: persona`

**修复步骤**:
1. 在移动前克隆值
2. 或使用引用而非移动

---

### Task 2.28: 修复 `binary operation == cannot be applied` [P3]
**错误**: `E0369: binary operation == cannot be applied to type ActionType/LogSeverity`

**修复步骤**:
1. 实现 `PartialEq` trait
2. 或使用 `matches!` 宏替代

---

### Task 2.29: 修复 `KvCacheManager` 私有字段 [P3]
**错误**: `E0616: field block_size of struct KvCacheManager is private`

**修复步骤**:
1. 将字段改为 `pub` 或添加 getter 方法
2. 或使用已有的公共 API

---

### Task 2.30: 修复 `no method named finish on CoreWrapper` [P3]
**错误**: `E0599: no method named finish found for CoreWrapper`

**修复步骤**:
1. 检查正确的 finish 方法名（可能是 `finalize` 或其他）
2. 或使用正确的 trait 方法

---

## 任务执行顺序建议

### Day 1 (P0 阻塞性问题)
- [ ] Task 2.1: 修复 logging 模块冲突
- [ ] Task 2.2: 修复 tc/mgr 变量
- [ ] Task 2.3: 修复 providers 模块
- [ ] Task 2.4: 修复 CompletionPrefetchState
- [ ] Task 2.5: 修复 ContentBlock
- [ ] Task 2.6: 修复 RedisCache
- [ ] Task 2.7: 修复 num_cpus
- [ ] Task 2.8: 修复 provide_completions 生命周期
- [ ] Task 2.9: 修复 lifetime specifier

### Day 2-3 (P1 高优先级)
- [ ] Task 2.10: 修复类型注解缺失
- [ ] Task 2.11: 修复类型不匹配
- [ ] Task 2.12: 修复 tree_sitter 字段
- [ ] Task 2.13: 修复缺少的方法
- [ ] Task 2.14: 修复借用检查器
- [ ] Task 2.15: 修复 module self 错误

### Day 4-5 (P2 中优先级)
- [ ] Task 2.16: 修复 Async/Await
- [ ] Task 2.17: 修复 LspOperation
- [ ] Task 2.18: 修复 VectorDatabase
- [ ] Task 2.19: 修复常量分配
- [ ] Task 2.20: 修复 Bm25Scorer
- [ ] Task 2.21: 修复 StreamEvent
- [ ] Task 2.22: 修复 TestGenerator
- [ ] Task 2.23: 修复 VectorDatabase 方法

### Day 6 (P3 低优先级)
- [ ] Task 2.24: 修复 AstEdit
- [ ] Task 2.25-2.30: 其他问题

---

## 验证命令

每个任务完成后验证:
```bash
cargo check --lib 2>&1 | Select-String -Pattern "error\[E" | Measure-Object -Line
```

预期: 错误数量应逐步减少

---

## 依赖关系图

```
Task 2.1 (logging) ──┬── Task 2.2 (tc/mgr)
                     │
Task 2.3 (providers)┼── Task 2.4 (CompletionPrefetchState)
                     │
Task 2.5 (ContentBlock) ─── Task 2.6 (RedisCache)
                     │
Task 2.7 (num_cpus) ────── Task 2.8 (lifetime)
                     │
Task 2.9 (lifetime) ─────── Task 2.10-2.30
```

---

最后更新: 2026-05-23
