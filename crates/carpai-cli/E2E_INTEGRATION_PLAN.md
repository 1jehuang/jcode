# Phase 8: 联调配合计划 (Wk9-10)

## 概述

Paw-brave 小组与 solo-Turbo (架构组) 的联调配合计划。目标是确保 carpai-cli 与 carpai-core、carpai-server 的完整集成。

---

## 1. 预检查清单（Wk9 前完成）

### carpai-cli 内部检查
- [ ] `cargo check -p carpai-cli` 通过 (0 errors)
- [ ] `cargo test -p carpai-cli` 全部通过
- [ ] `cargo clippy -p carpai-cli` 主要警告已消除
- [ ] 所有 CLI 子命令调用不 panic

### 接口契约对齐
- [ ] `agent_bridge::execute_turn()` 返回类型与 `carpai_core::execute_agent_turn()` 匹配
- [ ] `AgentTurnOutput` 字段与 core 层一致
- [ ] `BridgeMode` 覆盖 `Local` / `Remote` 两种场景
- [ ] `CliConfig` 正确使用 `CoreConfig` 的 flatten

---

## 2. 联调执行计划

### Day 1: 基线建立
```
上午: 
  - 与 solo-Turbo 确认 carpai-core 接口冻结（execute_agent_turn 签名）
  - cargo check --workspace 获取基线错误数
  
下午:
  - 修复 carpai-cli 自有的编译错误
  - 记录 carpai-core 接口变更对 cli 的影响
```

### Day 2: 跨组集成
```
[ ] Merge gamma/cli-build → main (解决冲突)
[ ] cargo check -p carpai-cli (post-merge)
[ ] 与 solo-Turbo 同步:
    - core 层新增/修改的类型
    - 需要适配的 trait 变更
    - 配置项的变更
```

### Day 3-4: Bug 修复
```
按 solo-Turbo 分派的 Bug 优先级:

P0 (阻塞):
  - cli 无法编译
  - bridge 调用 core API 失败
  - CliConfig 加载崩溃

P1 (高):
  - 远程模式连接失败
  - 通知模块依赖错误
  - TUI 渲染异常

P2 (中):
  - 配置热重载不触发
  - 重试层与 core 错误类型不匹配
  - 测试失败
```

### Day 5: E2E 测试跑通
```
E2E 链路验证:

1. CLI local mode  ✅ (无外部依赖)
   $ carpai chat --dir /tmp/test
   → 输入消息 → 收到回复 → 退出

2. CLI remote mode ⏳ (需 server)
   $ carpai --remote http://localhost:8080 ask "Hello"
   → 连接到 server → 收到回复

3. Config chain  ✅ (无外部依赖)
   $ CARPAI_REMOTE_URL=... carpai ask "test"
   → 环境变量覆盖 → 正确加载

4. Notifications  ⏳ (需外部服务)
   Telegram/Gmail/Browser opener
```

---

## 3. 已知风险与缓解

| 风险 | 概率 | 影响 | 缓解 | 责任人 |
|------|------|------|------|--------|
| core 接口签名变更 | 中 | 高 | Wk3 已冻结；变更需要 RFC | solo-Turbo |
| gRPC 类型不匹配 | 中 | 中 | 使用 proto 文件作为单一事实源 | solo-Turbo |
| 配置字段重命名 | 低 | 中 | serde(flatten) 会自动适配 | Paw-brave |
| TUI 在新版 ratatui 上渲染异常 | 低 | 低 | 版本锁定为 0.29 | Paw-brave |

---

## 4. Bug 分派协议

```
Bug 发现 → solo-Turbo 复现 → 分类:

├─→ cli 模块 (cli/*, tui/*, agent_bridge.rs)
│   └─→ Paw-brave fix → PR → solo-Turbo review → merge
│
├─→ 跨组 (bridge 调 core)
│   ├─→ 接口契约问题 → solo-Turbo 修 contract
│   └─→ 实现问题 → 对应组修复
│
└─→ 配置/构建问题
    └─→ solo-Turbo 协调
```

---

## 5. 交付物检查清单

### 联调完成标志
- [ ] `cargo check -p carpai-cli` 通过
- [ ] `cargo test -p carpai-cli` 全绿
- [ ] `cargo test -p carpai-cli --test e2e_test` local_mode 测试通过
- [ ] 无 P0/P1 级别未修复 Bug

### 可选的性能基线
- [ ] `cargo build -p carpai-cli --release` 编译时间
- [ ] 二进制大小 `ls -lh target/release/carpai`
- [ ] TUI 启动时间 < 500ms (从命令输入到渲染第一帧)
- [ ] `execute_agent_turn` p50/p95 延迟 (基于 mock provider)
