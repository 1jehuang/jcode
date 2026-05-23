# CarpAI 六环交付流水线 — 主计划

> 从 Plan 到 功能交付 全自动

---

## 六环总览

```
                          ┌──────────┐
                          │  需求输入  │
                          └────┬─────┘
                               ▼
                    ┌──────────────────┐
          ┌────────→│  ① Plan 自主规划  │←────────┐
          │         │  AutonomousAgent  │          │
          │         └────────┬─────────┘          │
          │                  ▼                     │
          │         ┌──────────────────┐          │
          │         │  ② 代码生成 + 修改  │          │
          │         │  LLM → Refactor   │          │
          │         └────────┬─────────┘          │
          │                  ▼                     │
          │         ┌──────────────────┐          │
          │         │  ③ 编译验证 + 修复  │          │
          │         │  CompileEngine    │          │
          │         │  FixEngine ×3     │          │
          │         └────────┬─────────┘          │
          │                  ▼                     │
          │         ┌──────────────────┐          │
          │         │  ④ 测试环         │  ← 新增    │
          │         │  TestRing ×3     │          │
          │         └────────┬─────────┘          │
          │                  ▼                     │
          │         ┌──────────────────┐          │
          │         │  ⑤ 审查环         │  ← 新增    │
          │         │  ReviewRing       │          │
          │         └────────┬─────────┘          │
          │                  ▼                     │
          │         ┌──────────────────┐          │
          │         │  ⑥ 交付环         │  ← 新增    │
          │         │  GitRing          │          │
          │         │  commit → PR      │          │
          │         └────────┬─────────┘          │
          │       ┌──────────┘                     │
          │       ▼                                │
          │  ┌──────────┐                          │
          │  │  交付完成  │                          │
          │  └──────────┘                          │
          │                                        │
          └────────────────────────────────────────┘
                    失败则回退修复
```

## 环状态

| # | 环 | 模块 | 状态 | 文件 |
|:-:|----|------|:----:|------|
| 1 | **Plan 自主规划** | `AutonomousAgent` | ✅ | `src/agent_runtime.rs` |
| 2 | **代码生成/修改** | `execute_edits()` + `AstRenamer` | ✅ | `src/agent_runtime.rs` + `src/refactor/semantic.rs` |
| 3 | **编译验证+修复** | `CompilationEngine` + `FixEngine` ×3 | ✅ | `src/compilation_engine.rs` |
| 4 | **测试验证+修复** | `TestRing` ×3 | 🆕 已创建 | `src/delivery_pipeline.rs` |
| 5 | **代码审查** | `ReviewRing` (风格/安全/复杂度) | 🆕 已创建 | `src/delivery_pipeline.rs` |
| 6 | **Git 交付** | `GitRing` (commit → PR) | 🆕 已创建 | `src/delivery_pipeline.rs` |

## 入口

```rust
// 完整六环流水线:
use delivery_pipeline::DeliveryPipeline;
let pipeline = DeliveryPipeline::new(Path::new("."));
pipeline.deliver("添加用户认证模块").await?;
```

## 前提条件

- ✅ 各环代码已就位
- ❌ **~244 个编译错误阻止任何环实际运行**（正在修复中）
- ❌ 需要 `gh` CLI 才能自动创建 PR (GitRing)

## 修复编译错误后的计划

```
1. 修复 244 个编译错误  ← 正在进行
2. cargo check --all     ← 验证
3. cargo test --all      ← 验证测试环
4. 端到端测试 DeliverPipeline  ← 验证全链路
5. 接入主 CLI 命令       ← jcode deliver "goal"
```
