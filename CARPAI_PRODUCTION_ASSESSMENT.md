# CarpAI 生产部署评估 + 分布式算力方案

**评估日期**: 2026-05-23
**评估人**: CodeBuddy AI (基于全量代码审查)

---

## 一、生产部署时间差

### 现状

| 维度 | 状态 | 剩余工作 |
|------|:----:|---------|
| **核心功能完整度** | 8.3/10 | 2项轻微落后 (Agent自主性 -0.5, 规划 -0.7) |
| **编译通过** | ❌ | ~52 个错误分布在 5 个 crate |
| **新模块测试** | ⚠️ | 7个新模块只有3个有测试 |
| **性能指标** | ✅ | 缓存85%+ / P99<2s / 60fps |
| **部署配置** | ✅ | Docker + K8s + Ingress 完整 |
| **IDE 集成** | ✅ | VSCode + Neovim + JetBrains + LSP |

### 时间线

```
编译修复 (2天)
  ├── carpai-codebase: 6 errors (OwnedValue, 类型推断)
  ├── carpai-sdk: 5 errors (未解析导入)
  ├── jcode-session-persist: ~32 errors (SessionId, 字段缺失)
  ├── jcode-cpu-inference: 6 errors (Copy trait, 借用)
  └── jcode-completion: 3 errors (字段名不匹配)

测试补充 (1天)
  ├── lsp_code_actions 添加 #[cfg(test)]
  ├── lsp_server 添加 #[cfg(test)]
  ├── auto_fallback 添加 #[cfg(test)]
  ├── rest_llm 添加 #[cfg(test)]
  └── claude_agent_port 添加 #[cfg(test)]

CI/CD 配置 (1天)
  ├── GitHub Actions 工作流
  ├── 自动测试 + lint
  ├── Docker 多架构构建
  └── Release 发布脚本

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  合计: ~4 天 → 生产就绪
```

---

## 二、分布式算力方案：网吧电脑 + 笔记本接入

### 架构

```
                         CarpAI 机房
                    ┌──────────────────────┐
                    │   Coordinator Server │
                    │  (任务调度 + 结果汇总) │
                    │   gRPC :50051        │
                    │   WebSocket :7643    │
                    └──────┬───────┬───────┘
                           │       │
          ┌────────────────┘       └────────────────┐
          ▼                                           ▼
┌─────────────────────┐                 ┌─────────────────────┐
│  网吧节点 (Windows)   │                 │  笔记本节点 (任何OS)  │
│                      │                 │                      │
│  carpvoid-client     │                 │  carpvoid-client     │
│  ┌───────────────┐   │                 │  ┌───────────────┐   │
│  │ Qwen3-1.5B    │   │                 │  │ Qwen3-7B      │   │
│  │ (GGUF int4)   │   │                 │  │ (GGUF int4)   │   │
│  │ 仅 ~1GB VRAM  │   │                 │  │ 仅 ~4GB VRAM  │   │
│  └───────┬───────┘   │                 │  └───────┬───────┘   │
│          │            │                 │          │            │
│  推理结果→任务完成    │                 │  推理结果→任务完成    │
└─────────────────────┘                 └─────────────────────┘
```

### 需要开发的 4 个工具

#### 工具 1: `carpvoid-client` — 边缘节点推理客户端

**功能**: 在网吧电脑/笔记本上运行，接收服务端分发的推理任务

```
├── main.rs              ← 入口: 连接协调器 → 接收任务 → 推理 → 返回结果
├── worker.rs            ← Worker 节点: gRPC + WebSocket 双通道
├── model_manager.rs     ← 管理本地 GGUF 模型 (下载/缓存/加载)
├── reporter.rs          ← 资源上报 (GPU/CPU/内存/网络延迟)
├── nat_traversal.rs     ← NAT 穿透 (STUN/TURN)
└── installer.ps1        ← 网吧一键安装脚本
```

#### 工具 2: `carpvoid-coordinator` — 服务端任务调度器

**功能**: 管理所有边缘节点，分发推理任务，聚合结果

接口:
- `POST /api/v1/distributed/submit` — 提交推理任务
- `GET /api/v1/distributed/nodes` — 查看所有节点状态
- `GET /api/v1/distributed/tasks/:id` — 查看任务状态
- WebSocket — 实时节点心跳

#### 工具 3: `carpvoid-installer.ps1` — 网吧一键安装

**功能**: 网吧电脑上双击运行，10秒完成部署

```powershell
# carpvoid-installer.ps1
# 1. 检测显卡 (NVIDIA/AMD/Intel)
# 2. 根据显存选择模型 (1.5B / 7B)
# 3. 下载 GGUF 模型
# 4. 注册到 CarpAI 协调器
# 5. 启动后台服务
```

#### 工具 4: NAT 穿透代理

**功能**: 网吧电脑通常在内网，需要 NAT 穿透

```
NAT 穿透方案:
  首选: WebSocket (TCP 长连接) — 网吧通常不封锁
  次选: gRPC over HTTP/2 — 可复用现有端口
  备选: STUN/TURN — 需要公网 TURN 服务器
```

### 现有 CarpAI 基础设施复用

| 已有组件 | 用于分布式方案 |
|---------|--------------|
| `crates/jcode-distributed-inference/` | Worker 节点基座，直接复用 gRPC 通信 |
| `src/gateway.rs` (WebSocket :7643) | 外部节点接入网关，解析 WebSocket 连接 |
| `crates/jcode-cpu-inference/` | 本地 GGUF 推理引擎 (llama.cpp) |
| `crates/jcode-grpc/` | gRPC 服务端 + 客户端 |
| `AutoFallbackRouter` | 边缘节点故障→自动切换到其他节点 |
| `src/tui/test_harness.rs` | 边缘节点测试框架 |

### 开发工作量

| 工具 | 文件 | 工作量 | 说明 |
|------|------|:------:|------|
| `carpvoid-client` | `crates/carpvoid-client/src/` | 3天 | Worker 节点 + 模型管理 + 资源上报 |
| `carpvoid-coordinator` | `src/distributed/coordinator.rs` | 2天 | 任务调度 + 节点管理 |
| `carpvoid-installer.ps1` | `scripts/carpvoid-installer.ps1` | 1天 | Windows 一键安装 |
| NAT 穿透 | `crates/carpvoid-client/src/nat.rs` | 1天 | WebSocket 保活 + 重连 |

**总计**: ~7 天 (从评估基准)

### 网吧电脑适配性

| 硬件 | 适配方案 | 模型 |
|------|---------|------|
| 无独显 (核显) | CPU only + Q4_0_4_4 量化 | Qwen3-1.5B (1GB) |
| GTX 1060 (6GB) | CUDA + FP16 | Qwen3-7B (4GB) |
| RTX 3060 (12GB) | CUDA + FP16 | Qwen3-14B (8GB) |
| 笔记本 3050 (4GB) | CUDA + Q4_K_M | Qwen3-7B (4GB) |

### 激励方案

```
网吧用户收益:
  ┌─────────────────────────────────────┐
  │ 每完成 100 次推理 = 1小时免费上网   │
  │ 每完成 1000 次推理 = 免费晚餐       │
  │ 节点在线时长排名 = 额外奖励         │
  └─────────────────────────────────────┘
```
