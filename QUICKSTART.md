# 🚀 jcode 快速启动指南 - 开箱即用体验

> **目标**: 让 jcode 的开箱即用体验达到或超过 Cursor 水平！

## 📋 安装步骤（3分钟搞定）

### 方法一：一键安装脚本（推荐）⭐

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/jcode-dev/jcode/main/scripts/install.sh | bash

# Windows PowerShell
irm https://raw.githubusercontent.com/jcode-dev/jcode/main/scripts/install.ps1 | iex
```

**安装脚本会自动完成**:
- ✅ 检测系统环境（Python、Rust、VS Code）
- ✅ 创建配置目录 `~/.jcode/`
- ✅ 生成智能默认配置
- ✅ 检测 API Key（DEEPSEEK_API_KEY / OPENAI_API_KEY）
- ✅ 配置 PATH 环境变量
- ✅ 提供下一步操作指引

### 方法二：手动安装

#### Step 1: 获取 jcode

**选项 A: 从源码编译**
```bash
git clone https://github.com/jcode-dev/jcode.git
cd jcode
cargo build --release
```

**选项 B: 下载预编译二进制**（即将提供）
```bash
# 下载对应平台的二进制文件
# Windows: jcode-x86_64-pc-windows-msvc.zip
# macOS:   jcode-aarch64-apple-darwin.tar.gz
# Linux:   jcode-x86_64-unknown-linux-gnu.tar.gz

# 解压到 ~/.local/bin/
tar -xzf jcode-*.tar.gz -C ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

#### Step 2: 配置 API Key（30秒）

**最简单的方式 - 环境变量**:
```bash
# Deepseek（推荐中国用户，性价比高）
export DEEPSEEK_API_KEY='your-deepseek-api-key'

# 或 OpenAI（国际用户）
export OPENAI_API_KEY='your-openai-api-key'
```

**或者使用 VS Code 设置 UI**:
1. 打开 VS Code → Settings (`Ctrl+,`)
2. 搜索 `jcode.llm.apiKey`
3. 粘贴你的 API Key

#### Step 3: 启动服务器

```bash
jcode server start
# 输出:
# ✅ jcode server started successfully!
#    gRPC endpoint: [::]:50051
#    REST API:      http://127.0.0.1:3000
#    Health check:  http://127.0.0.1:3000/health
```

#### Step 4: 安装 VS Code 扩展

```bash
# 从 VS Code 命令行
code --install-extension jcode.jcode

# 或在 VS Code 中搜索 "jcode" 并点击 Install
```

---

## 🎯 第一次使用（像 Cursor 一样简单）

### 场景一：Inline Completion（代码补全）

1. **打开任意代码文件**
2. **开始输入代码**，例如：
   ```python
   def fibonacci(n):
       # 按 Alt+\ 触发补全
   ```
3. **按 `Alt+\`** （Windows/Linux）或 **`Option+\`** (macOS)
4. **看到 AI 生成的建议后，按 `Tab` 接受**

> 💡 **提示**: 就像 GitHub Copilot / Cursor 一样自然！

### 场景二：Chat Panel（聊天面板）

1. **按 `Ctrl+Shift+J`** 打开聊天面板
2. **输入问题**，例如：
   ```
   这个函数的时间复杂度是什么？
   如何优化这段代码的性能？
   给这个类添加单元测试
   ```
3. **按 Enter 发送**，实时看到回复流式输出

### 场景三：选中代码操作

1. **选中一段代码**
2. **右键菜单选择**:
   - 📖 **Explain with jcode** - 解释代码
   - 🔧 **Refactor with jcode** - 重构代码
   - 🐛 **Fix Error with jcode** - 修复错误
   - 🧪 **Generate Tests** - 生成测试
   - 📝 **Add Documentation** - 添加文档

---

## ⚙️ 配置选项（渐进式披露）

### Level 0: 零配置（开箱即用）

**什么都不用做！** jcode 会自动：
- 检测可用的 LLM Provider
- 选择最优模型
- 根据硬件调整性能参数
- 启用 RAG 增强功能

### Level 1: 基础配置（~5分钟）

编辑 `~/.jcode/config.toml`:

```toml
[llm]
# 选择你喜欢的 Provider
default_provider = "deepseek"  # deepseek | openai-compatible | vllm | llamacpp
default_model = "deepseek-chat"

[vscode]
# 开启/关闭功能
inline_completion_enabled = true   # Tab 补全（像 Copilot）
chat_panel_enabled = true          # 聊天面板
terminal_integration = true        # 终端集成
```

### Level 2: 高级调优（~15分钟）

```toml
[llm]
temperature = 0.7          # 创造性：0.0=专注, 1.0=创意
max_tokens = 4096          # 最大响应长度
timeout_secs = 60          # 超时时间（秒）

[performance]
worker_threads = 4         # 工作线程数
stream_buffer_size = 4096  # 流式缓冲区大小
enable_cache = true        # 启用缓存
cache_ttl_secs = 300       # 缓存有效期（秒）

[rag]
enabled = true             # 启用代码库感知
max_retrieved_snippets = 8 # 检索上下文数量

[logging]
level = "info"             # trace | debug | info | warn | error
log_file = "~/.jcode/logs/jcode.log"
```

### Level 3: 专家级定制

参见完整文档：https://docs.jcode.dev/configuration

---

## 🆚 与 Cursor 对比

| 特性 | Cursor | jcode | 说明 |
|------|--------|-------|------|
| **安装复杂度** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | 同样简单 |
| **首次使用时间** | < 1 分钟 | < 2 分钟 | 接近 |
| **配置灵活性** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | jcode 更灵活 |
| **Provider 选择** | 仅 OpenAI/Anthropic | Deepseek/vLLM/llama.cpp/OpenAI | jcode 更多选择 |
| **本地模式** | ❌ 不支持 | ✅ 完整支持 | 隐私保护 |
| **价格** | $20/月 | 免费（自托管）/ 云API费用 | jcode 更经济 |
| **开源** | ❌ 闭源 | ✅ MIT 许可 | 完全透明 |
| **中文支持** | ⚠️ 一般 | ✅ 优秀 | Deepseek 原生支持 |

---

## 🎨 使用场景预设

### 预设 1: 中国开发者（推荐）

```bash
# 一键应用 Deepseek 优化配置
jcode preset apply deepseek-cloud
```

**特点**:
- ✅ 默认使用 DeepSeek API（性价比高）
- ✅ 优秀的中文理解能力
- ✅ 低延迟（国内服务器）
- ✅ 价格便宜（约 OpenAI 的 1/10）

### 预设 2: 隐私优先（本地部署）

```bash
# 本地 vLLM + Qwen2.5 模型
jcode preset apply local-vllm
```

**特点**:
- ✅ 数据完全不出本机
- ✅ 无 API 费用
- ✅ 需要 GPU（8GB+ VRAM 推荐）
- ✅ 适合企业/敏感项目

### 预设 3: Cursor 迁移者

```bash
# 匹配 Cursor 的所有行为和快捷键
jcode preset apply cursor-migration
```

**特点**:
- ✅ 相同的快捷键绑定
- ✅ 相同的端口和协议
- ✅ 无缝迁移，无需重新学习
- ✅ 保留所有 Cursor 使用习惯

### 预设 4: 性能极致

```bash
# 针对 CPU/GPU 优化
jcode preset apply production
```

---

## 🛠️ 故障排除

### 问题：找不到 API Key

**症状**: 启动时报错 "No API key configured"

**解决方案**:
```bash
# 方案 A: 设置环境变量
export DEEPSEEK_API_KEY='sk-xxx'

# 方案 B: 编辑配置文件
nano ~/.jcode/config.toml
# 在 [llm] 下添加:
# api_key = 'sk-xxx'
```

### 问题：VS Code 扩展无法连接

**症状**: 扩展显示 "Server not running"

**解决方案**:
```bash
# 手动启动服务器
jcode server start

# 检查端口是否被占用
netstat -an | grep 50051  # Linux/macOS
netstat -ano | findstr 50051  # Windows
```

### 问题：Inline Completion 不工作

**症状**: 按 Alt+\ 没有反应

**检查清单**:
1. ✅ VS Code 扩展已安装且启用
2. ✅ jcode server 正在运行
3. ✅ 光标在代码编辑器中
4. ✅ 设置中 `jcode.vscode.inlineCompletionEnabled = true`
5. ✅ 尝试重启 VS Code

### 问题：响应速度慢

**优化方案**:

```toml
# ~/.jcode/config.toml
[performance]
stream_buffer_size = 8192     # 增大缓冲区
enable_cache = true           # 启用缓存
cache_ttl_secs = 1800         # 缓存 30 分钟

[llm]
timeout_secs = 120            # 增加超时
max_retries = 2              # 减少重试次数
```

---

## 📚 进阶资源

### 文档
- 📘 **完整配置参考**: https://docs.jcode.dev/configuration
- 📗 **VS Code 扩展开发**: https://docs.jcode.dev/vscode-extension
- 📙 **Provider 自定义**: https://docs.jcode.dev/providers
- 📕 **RAG 系统详解**: https://docs.jcode.dev/rag-system

### 社区
- 💬 **Discord**: https://discord.gg/jcode
- 🐦 **Twitter/X**: @jcode_dev
- 📱 **微信群**: 扫描官网二维码
- 🌐 **论坛**: https://community.jcode.dev

### 示例项目
- 🚀 **快速开始示例**: examples/quickstart/
- 🔌 **自定义 Provider**: examples/custom-provider/
- 🧪 **性能基准测试**: examples/benchmark/

---

## 🎉 下一步

恭喜！你已经完成了 jcode 的初始设置。

**现在你可以**:
1. ✨ 开始编写代码，享受 AI 辅助
2. 🎮 探索更多功能和配置选项
3. 🤝 加入社区，分享你的使用经验
4. 🐛 发现问题？提交 Issue 或 PR

**记住**: jcode 是开源项目，你的贡献会让它变得更好！

---

*最后更新: 2026-05-12*
*版本: jcode v0.1.0*
