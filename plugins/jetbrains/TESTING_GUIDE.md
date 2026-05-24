# JetBrains Plugin 测试指南

**版本**: v1.1.0-dev
**日期**: 2026-05-24

---

## 前置条件

1. **CarpAI Server 运行中**
   ```bash
   # Terminal 1: Start CarpAI Server
   cd D:/studying/Codecargo/CarpAI
   cargo run --package carpai-server
   
   # Verify gRPC port is listening
   netstat -an | findstr 50051
   ```

2. **IntelliJ IDEA 安装**
   - 推荐版本: 2023.3+ (Community 或 Ultimate)
   - 已打开 CarpAI 项目

---

## 测试步骤

### Step 1: 构建插件

```bash
cd plugins/jetbrains
./gradlew buildPlugin
```

预期输出:
```
BUILD SUCCESSFUL in XXs
Build plugin to: build/distributions/carpai-1.1.0-dev.zip
```

### Step 2: 在 IntelliJ 中运行插件

1. **打开 Run Configuration**
   - 右上角点击 `Edit Configurations...`
   - 选择 `Run Plugin` (Gradle runIde)

2. **点击 Run 按钮** (绿色三角形)
   - IntelliJ 会启动一个新的沙盒 IDE 实例
   - 等待沙盒 IDE 完全加载 (~30秒)

3. **验证插件加载**
   - 检查右下角状态栏是否有 CarpAI 图标
   - 或者: `File → Settings → Tools` 应该看到 "CarpAI" 配置页

### Step 3: 配置 Server 连接

1. **打开 Settings**
   - `File → Settings → Tools → CarpAI`

2. **填写配置**
   ```
   Server Host: localhost
   Server Port: 50051
   API Key: (留空，当前未启用认证)
   Default Model: claude-sonnet-4
   ```

3. **点击 Apply → OK**

### Step 4: 测试聊天功能

1. **打开 Chat Tool Window**
   - 右侧边栏点击 "CarpAI Chat" 标签
   - 或者: `View → Tool Windows → CarpAI Chat`

2. **发送消息**
   - 在底部输入框输入: `Hello, CarpAI!`
   - 点击 "Send" 按钮或按 Enter

3. **预期响应**
   - 消息列表中显示:
     ```
     You: Hello, CarpAI!
     CarpAI: <AI response from server>
     ```

### Step 5: 测试编辑器右键菜单

1. **打开任意代码文件**
   - 例如: `crates/carpai-core/src/lib.rs`

2. **选中一段代码**
   - 用鼠标选中几行代码

3. **右键 → CarpAI → Explain Code**
   - 应该在 Chat Tool Window 中看到解释

---

## 故障排查

### 问题 1: "Connection refused" 错误

**原因**: CarpAI Server 未运行或端口不正确

**解决**:
```bash
# 检查 Server 是否运行
netstat -an | findstr 50051

# 如果没有输出，启动 Server
cargo run --package carpai-server
```

### 问题 2: 插件未出现在 Tool Windows

**原因**: 插件未正确加载

**解决**:
1. 关闭沙盒 IDE
2. 在主 IDE 中: `File → Invalidate Caches → Restart`
3. 重新运行 `Run Plugin` configuration

### 问题 3: Gradle 构建失败

**原因**: protobuf 插件或依赖问题

**解决**:
```bash
cd plugins/jetbrains
./gradlew clean
./gradlew generateProto
./gradlew buildPlugin
```

---

## 已知限制

| 功能 | 状态 | 备注 |
|------|------|------|
| Chat Tool Window | ✅ 可用 | 基本聊天功能正常 |
| Settings 页面 | ✅ 可用 | 配置持久化正常 |
| Explain Code | 🚧 开发中 | UI 入口已创建，后端调用待完善 |
| Refactor | 🚧 开发中 | Diff preview 待实现 |
| Fix Bug | 🚧 开发中 | 同 Explain |
| Inline Completions | ❌ 未实现 | v1.1.0 后续版本 |

---

## 下一步开发

1. **完善 Explain/Refactor/Fix 命令**
   - 在 `actions/ExplainCodeAction.kt` 中调用 GrpcClient
   - 将响应显示在 Chat Tool Window

2. **实现 Diff Preview**
   - 创建 `ui/DiffPreviewDialog.kt`
   - 使用 IntelliJ 的 `DiffManager` API

3. **添加 Inlay Hints**
   - 实现 `providers/CodeInsightProvider.kt`
   - 显示 AI 生成的类型提示/文档

---

> **文档状态**: Draft — 待测试验证后更新
