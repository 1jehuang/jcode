# Clone 审计优化指南

## 优化原则

1. **Arc clone 是廉价的**（仅原子引用计数递增），不需要优化
2. **String/集合 clone 是昂贵的**（深拷贝），需要优化
3. **频繁调用的热路径**优先优化（`run_turn`, `execute`）

## 优先级热点

### P0：必须优化（热路径 + 大对象）

| 位置 | 问题 | 方案 |
|------|------|------|
| `agent/turn_loops.rs:53` | `messages.iter().cloned().collect()` | 用 `messages.clone()` (Vec clone 比逐个 iter clone 更快) |
| `turn_loops.rs` 多处 `tc.input.clone()` | tool_call 输入深拷贝 | 已通过 `&Value` 借用解决 |
| `tool/mod.rs` 多处 `tools.keys().cloned()` | 分配临时 Vec | ✅ 已优化为 drain-filter |

### P1：建议优化（中等频率）

| 位置 | 问题 | 方案 |
|------|------|------|
| `agent.rs` 中 `self.session.id.clone()` | UUID 字符串 clone | 用 `&str` 借用替代 |
| `background.rs` task 回调 | String 参数多余 clone | 用 `Cow<str>` 延迟拷贝 |
| `compaction.rs` `snapshots` 处理 | Vec clone | 用 `swap_remove` 避免拷贝 |

### P2：低优先级（非热路径）

| 位置 | 问题 | 方案 |
|------|------|------|
| 测试代码 | 大量 clone | 无需优化 |
| 错误路径 | 仅在失败时 clone | 无需优化 |

## 实施建议

1. 每次修改使用 `cargo clippy -- -W clippy::clone_on_ref_ptr` 检测不必要的 Arc clone
2. 用 `#[clippy::clone_on_ref_ptr]` lint 逐步淘汰
3. 为热路径添加 `#[inline]` 减少间接调用开销
