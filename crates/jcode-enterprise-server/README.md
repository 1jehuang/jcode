# CarpAI Enterprise Server — 企业级服务版

> **零额外硬件投入**，将公司现有的低端台式机、网吧闲置电脑、员工笔记本
> 转化为企业级 AI 推理集群。

## 核心能力

| 能力 | 技术实现 | 适配您的硬件 |
|------|---------|-------------|
| 72B 大模型推理 | GGUF Q4_K_M 量化 + CPU 推理 | 128G 台式机单机运行 |
| 多模型混用 | Parallax 层分配 + Ruflo 任务调度 | 5 台服务器做流水线并行 |
| 闲置节点利用 | mDNS 自动发现 + 心跳检测 | 网吧 20 台 + 员工 200 台笔记本 |
| 超低内存推理 | mmap KV Cache ⇢ 虚拟内存 | 512G 网吧虚拟内存 |
| 企业级管理 | 多租户、RBAC、API Key、用量审计 | API 兼容 + 管理后台 |

## 快速开始

### 前置条件

1. **Rust** (≥1.75): [rustup.rs](https://rustup.rs)
2. **llama.cpp**: [安装指南](https://github.com/ggerganov/llama.cpp)
3. **Python 3** (仅量化时需要): 需要 `pip install huggingface-hub`

### 步骤 1: 下载并量化模型

```bash
# 安装依赖
pip install huggingface-hub

# 下载 Qwen3 72B 并量化为 Q4_K_M（约需 180G 临时磁盘空间）
python3 scripts/download_quantize.py \
    --model Qwen/Qwen3-72B \
    --quant Q4_K_M \
    --output ./models

# 下载 DeepSeek R1 32B
python3 scripts/download_quantize.py \
    --model deepseek-ai/DeepSeek-R1-Distill-Qwen-32B \
    --quant Q4_K_M \
    --output ./models
```

### 步骤 2: 启动企业版服务器

**Windows** — 双击 `deploy/deploy_enterprise.bat`
**Linux/macOS** — 运行:

```bash
chmod +x deploy/deploy_enterprise.sh
bash deploy/deploy_enterprise.sh
```

**手动启动:**

```bash
# 使用 SQLite（轻量）
CARPAI_CONFIG=./config/enterprise.toml \
CARPAI_DATABASE_URL="sqlite://./data/carpai.db?mode=rwc" \
cargo run --release --bin carpai-enterprise-server

# 使用 PostgreSQL（生产推荐）
CARPAI_DATABASE_URL="postgres://user:pass@localhost/carpai" \
cargo run --release --features postgres --bin carpai-enterprise-server
```

### 步骤 3: 注册节点

在每台需要加入集群的电脑上运行:

```bash
# 注册 128G 台式机
CARPAI_SERVER=http://master-ip:8000 \
CARPAI_NODE_NAME="办公台式机_01" \
cargo run --release --bin carpai-node-agent
```

### 步骤 4: 使用

**API 调用 (OpenAI 兼容):**

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-72b-int4",
    "messages": [{"role": "user", "content": "你好!"}]
  }'
```

**管理命令行:**

```bash
# 查看系统状态
CARPAI_SERVER=http://localhost:8000 cargo run --bin carpai-admin-cli -- metrics

# 生成 API Key
CARPAI_SERVER=http://localhost:8000 cargo run --bin carpai-admin-cli -- api-key generate
```

## 部署架构

```
┌────────────────────────────────────────────────────────────┐
│              主服务器 (5台 128G 台式机)                      │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  carpai-enterprise-server (API + Admin + Scheduler)  │  │
│  │  端口 8000: OpenAI 兼容 API                           │  │
│  │  端口 8001: 管理后台 API                              │  │
│  └──────────────────────────────────────────────────────┘  │
│        ↕ mDNS 发现 + HTTP 心跳 (每 10 秒)                  │
├────────────────────────────────────────────────────────────┤
│      动态节点 (20台网吧 + 200台笔记本)                      │
│  ┌──────────────────┐  ┌──────────────────┐               │
│  │ carpai-node-agent│  │ carpai-node-agent│  ...          │
│  │ 网吧_01          │  │ 笔记本_01        │               │
│  └──────────────────┘  └──────────────────┘               │
└────────────────────────────────────────────────────────────┘
```

## 模型推荐（根据你的硬件配置）

| 模型 | 量化后大小 | 推荐节点 | 适用场景 |
|------|-----------|---------|---------|
| Qwen3.5-72B (Q4_K_M) | ~36 GB | 128G 台式机 (×5) | 核心对话、文档分析、代码生成 |
| QwQ-32B (Q4_K_M) | ~18 GB | 32G 笔记本 | 推理、数学、逻辑问题 |
| DeepSeek-R1-32B (Q4_K_M) | ~18 GB | 32G 笔记本 | 代码辅助、技术问答 |
| GLM-5-9B (Q4_K_M) | ~6 GB | 16G 笔记本 + 网吧 | 日常对话、文本生成 |

## 文件结构

```
crates/jcode-enterprise-server/          # 企业版主 crate
├── src/
│   ├── config.rs                        # 配置（TOML/YAML/JSON + 环境变量覆盖）
│   ├── enterprise.rs                    # 主结构体 + 启动逻辑
│   ├── model_quant.rs                   # 模型量化适配
│   ├── cpu_inference.rs                 # CPU 推理引擎封装
│   ├── distributed.rs                   # Parallax 分布式推理桥接
│   ├── discovery.rs                     # mDNS 节点发现 + 心跳
│   ├── priority.rs                      # 优先级规则引擎
│   ├── virtual_memory.rs                # 虚拟内存 mmap 推理
│   ├── auth.rs                          # JWT + API Key + RBAC
│   ├── db.rs                            # SQLite/PostgreSQL
│   ├── usage.rs                         # 用量统计 + 配额
│   └── admin_api/                       # REST API
│       ├── openai_routes.rs             # OpenAI 兼容 API
│       ├── admin_routes.rs              # 管理后台 API
│       └── auth_middleware.rs           # 认证中间件
├── src/bin/server.rs                    # 主服务入口
├── src/bin/node_agent.rs                # 节点代理入口
└── src/bin/admin_cli.rs                 # 管理 CLI 入口
config/enterprise.toml                   # 默认配置文件
scripts/download_quantize.py             # 模型量化下载脚本
deploy/deploy_enterprise.bat             # Windows 一键部署
deploy/deploy_enterprise.sh              # Linux 一键部署
deploy/internet_cafe_node.bat            # 网吧节点脚本
```

## 开发路线

| 阶段 | 任务 | 状态 |
|------|------|------|
| P0 | 模型量化 + CPU推理 + 分布式推理 | ✅ 已实现 |
| P0 | 动态节点调度 + 心跳检测 | ✅ 已实现 |
| P1 | OpenAI兼容API + 企业管理API | ✅ 已实现 |
| P2 | 部署脚本 + 配置 + 文档 | ✅ 已实现 |

## 技术栈

- **后端**: Rust + Axum + Tokio + SQLx
- **推理**: llama.cpp + Parallax 调度 + Ruflo 任务规划
- **调度**: 已集成 UnifiedScheduler (注水算法/DP/流水线并行)
- **数据库**: SQLite (默认) / PostgreSQL (生产)
- **协议**: OpenAI API 兼容 + REST + mDNS 节点发现
