# CarpAI 生产就绪开发计划 v1.0

> 基于 2026-05-23 代码审计结果制定的分阶段生产就绪路线图。
> 总目标：消除所有编译警告、消除运行时 panic 风险、完成 todo!() 功能、建立 CI/CD。

---

## 阶段 0 — 编译稳固化（1-2 天）

### P0.1 修复 blocking 编译错误

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 1 | `src/lib_minimal.rs:82` | `pub sub_agents;` 缺少 `mod` | 改为 `pub mod sub_agents;` |
| 2 | `src/lib.rs` | 所有 `pub mod` 引用的模块确保存在文件 | 添加缺失的模块文件 |

### P0.2 消除全部 `todo!()` 运行时 panic（10 处）

| # | 文件 | 函数 | 策略 |
|---|------|------|------|
| 1-10 | `src/cli/expanded_cmds.rs` | run_clear_command 等 10 个函数 | 逐个实现或添加 `panik::todo_deprecated!()` 并标记废弃。高优先级实现：`run_cost_command`（成本查询）、`run_rate_limit_command`（速率限制） |

---

## 阶段 1 — 安全加固（3-5 天）

### P1.1 消除生产路径 `unwrap()`（401 处，按优先级）

```
优先级分层：
  Fatal (立即修复):  调度器核心路径 (~80处)
  Critical (本周):   TUI 核心 (~100处)
  High (本月):       Server 模块 (~40处)
  Normal (后续):     工具模块、crates
```

**立即修复清单**：

| # | 文件 | unwrap 数 | 修复模式 |
|---|------|-----------|---------|
| 1 | `crates/jcode-unified-scheduler/src/unified_queue.rs` | 28 | 全部改为 `?` 或 `expect("context")` |
| 2 | `crates/jcode-unified-scheduler/src/lib.rs` | 13 | RwLock/DashMap 操作加错误处理 |
| 3 | `crates/jcode-unified-scheduler/src/resource_node.rs` | 8 | 节点管理操作加 fallback |
| 4 | `crates/jcode-unified-scheduler/src/goap_planner.rs` | 8 | A* 搜索边界检查 |
| 5 | `src/tui/app/remote.rs` | 5 | TUI 远程连接 unwrap → error 传播 |

**操作指南**：
```rust
// 修复前
let x = map.get(&key).unwrap();

// 修复后
let x = map.get(&key).ok_or_else(|| anyhow!("key {} not found", key))?;
```

### P1.2 所有 unsafe 块添加 `// SAFETY:` 注释（40 处，30 处缺失）

| # | 热点文件 | unsafe 类型 |
|---|---------|------------|
| 1 | `src/ssh/pty.rs` | libc PTY 操作 — 需文档化文件描述符生命周期 |
| 2 | `src/transport/windows.rs` | Windows API 原始指针 |
| 3 | `src/perf.rs` | 系统级性能指标采集 |
| 4 | `src/process_memory.rs` | jemalloc 内部指标读取 |
| 5 | `src/platform.rs` | 平台检测系统调用 |

### P1.3 修复生产路径 `panic!()`（7 处）

| # | 文件 | 行 | 修复 |
|---|------|----|------|
| 1 | `src/token_budget.rs` | 303 | `panic!` → `bail!()` 返回错误 |
| 2-4 | `src/scheduler.rs` | 671,687,699 | `panic!("select_X called with empty")` → 返回 `None` 或 `Err` |
| 5 | `src/completion/bash/parser.rs` | 595 | 改为返回 `Result` |

---

## 阶段 2 — 代码完成（5-7 天）

### P2.1 实现或移除占位代码

| # | 文件 | 操作 |
|---|------|------|
| 1 | `src/engine.rs` | **移除** — 整个文件只有占位符，无人使用。删除文件并去除 `src/lib.rs` 中的 `pub mod engine` |
| 2 | `src/cli/expanded_cmds.rs` | **实现** 10 个 empty command。至少实现 cost / rate-limit / env，其余可标记 deprecated |
| 3 | `src/cli/completion_gen.rs` | **实现** completion 命令或删除路由 |
| 4 | `src/cli/code_nav.rs` | **实现** code-nav 命令或删除路由 |
| 5 | `src/cli/build_cmd.rs` | **实现** build 命令或删除路由 |
| 6 | `crates/jcode-provider-qwen/src/lib.rs` | **实现** Qwen provider（或标记 `publish = false` + 文档说明"待实现"） |

### P2.2 清理 stub crate 和聚合 crate

| # | crate | 操作 |
|---|-------|------|
| 1 | `crates/jcode-pdf` | **合并** 到主 crate 的 `src/pdf.rs`（~6 行代码不值得独立 crate） |
| 2 | `crates/jcode-gateway-types` | **合并** 到 `src/gateway_types.rs` |
| 3 | `crates/jcode-batch-types` | 继续保留，但验证是否被使用 |
| 4 | `crates/jcode-azure-auth` | **合并** 到 `src/auth/azure.rs` |
| 5 | `crates/jcode-code-value` | **合并** 到主 crate（~146B 只有 re-export） |
| 6 | `jcode-tui-*` (12 crates) | **评估** 是否可合并为 3-4 个较大 crate（可选） |

### P2.3 实现 WS 处理器（WebSocket 核心功能）

| # | 文件 | 缺失功能 |
|---|------|---------|
| 1 | `src/ws/handlers/terminal.rs` | 终端数据发送/接收 |
| 2 | `src/ws/handlers/collab.rs` | OT 算法、实时广播 |
| 3 | `src/ws/handlers/fs.rs` | 文件变更监控 |
| 4 | `src/ws/handlers/project.rs` | 项目构建信息 |

---

## 阶段 3 — 架构优化（5-7 天）

### P3.1 模块精简

| # | 动作 | 影响 |
|---|------|------|
| 1 | 合并 `jcode-core-types` + `jcode-runtime-types` + `jcode-ui-types` → `jcode-types` | 减少 3 个 crate |
| 2 | 合并 12 个 `jcode-tui-*` crates → 3-4 个 | 减少 8-9 个 crate |
| 3 | 移除 `src/lib_minimal.rs` 中注释掉的 ~35 个模块（或不作为编译目标） | 减少编译时间 |
| 4 | 将条件编译 `#[cfg(feature = "...")]` 改为 Cargo feature 分组 | 简化 feature 管理 |

### P3.2 依赖优化

| # | 依赖 | 问题 | 修复 |
|---|------|------|------|
| 1 | `lazy_static` | 已过时（替代: `once_cell` / `std::sync::LazyLock`） | 迁移到 `std::sync::LazyLock`（Rust 1.80+） |
| 2 | `unwrap()` 连锁 | `.lock().unwrap()` 模式 20+ 处 | 添加 Mutex 中毒恢复 |
| 3 | 重复依赖 | 根 Cargo.toml 和 sub-crate Cargo.toml 中版本不一致 | `cargo deny` 检查 |

---

## 阶段 4 — 质量门禁（2-3 天）

### P4.1 编译警告清零

| # | 警告类型 | 当前数量 | 目标 |
|---|---------|---------|------|
| 1 | `unused_imports` | ~8 | 0 |
| 2 | `unused_mut` | ~4 | 0 |
| 3 | `unused_variables` | ~5 | 0 |
| 4 | `dead_code` | ~113 | 0 |
| 5 | `unused_macros` | ~3 | 0 |

### P4.2 CI/CD 设置

```yaml
# .github/workflows/ci.yml (核心)
jobs:
  check:
    - cargo check --all-features          # 全量编译
    - cargo clippy -- -D warnings          # lint 检查
    - cargo test --all                     # 全部测试
    - cargo deny check                     # 安全审计
  
  security:
    - cargo audit                          # 依赖漏洞扫描
    - trivy filesystem .                   # 文件系统扫描
```

### P4.3 文档完成度

| # | 文档 | 当前 | 目标 |
|---|------|------|------|
| 1 | `// SAFETY` on unsafe | 10/40 (25%) | 40/40 (100%) |
| 2 | 公开 API docs | ~60% | 100% |
| 3 | Error 类型文档 | ~40% | 100% |

---

## 阶段 5 — 性能与测试（3-5 天）

### P5.1 热路径优化

| # | 热点 | 问题 | 优化 |
|---|------|------|------|
| 1 | `.clone()` 调用 4924+ 处 | 大量不必要的克隆 | 热路径中改用引用 + 生命周期标注 |
| 2 | `src/agent/turn_loops.rs` | 大型函数，可能有 Clone 瓶颈 | profile + 重构 |
| 3 | `crates/jcode-unified-scheduler/` | 调度器核心性能 | 基准测试 + 优化 |

### P5.2 测试覆盖率提升

| # | 模块 | 当前覆盖 | 目标 |
|---|------|---------|------|
| 1 | `crates/jcode-llm` | ~30% | 80% |
| 2 | `crates/jcode-unified-scheduler` | ~40% | 80% |
| 3 | `src/provider/` | ~50% | 80% |
| 4 | `src/agent/` | ~60% | 80% |

---

## 总结：时间线与里程碑

```
周1-2:  阶段0 + 阶段1 → 编译通过、无 panic 风险
周3-4:  阶段2 → 所有功能完整、无 stub 代码  
周5:    阶段3 → 架构精简、编译时间减半
周6:    阶段4 → 零警告、零 dead_code、CI 就绪
周7-8:  阶段5 → 性能达标、测试覆盖 80%
        🎉 生产就绪
```

## 关键指标追踪

| 指标 | 当前值 | 目标值 |
|------|-------|-------|
| 编译警告数 | ~15 | **0** |
| `unwrap()` 在生产代码 | 401 | **<50** |
| 缺少 SAFETY 注释的 unsafe | 30 | **0** |
| `todo!()` 运行时 panic | 10 | **0** |
| `#[allow(dead_code)]` | 113 | **0** |
| workspace crates | 70+ | **<40** |
| 测试覆盖率 | ~40% | **>80%** |
