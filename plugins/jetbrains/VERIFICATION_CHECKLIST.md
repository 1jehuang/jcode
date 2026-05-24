# JetBrains Plugin 验证清单

**执行人**: ma-guoyang
**日期**: 2026-05-24
**版本**: v1.1.0-dev

---

## 自动化检查 (已执行)

| 检查项 | 状态 | 说明 |
|--------|------|------|
| Server gRPC 端口配置 | ✅ | 默认 50051 |
| Gradle 编译 | ✅ | `./gradlew buildPlugin` 通过 |
| Proto 文件生成 | ✅ | `build/generated/source/proto` 存在 |
| Settings 页面注册 | ✅ | plugin.xml 中已配置 |
| ChatPanel + gRPC 集成 | ✅ | GrpcClient 已注入 |

---

## 手动测试步骤 (待执行)

### 前置条件
- [ ] IntelliJ IDEA 2023.3+ 已安装
- [ ] CarpAI project 已在 IDEA 中打开
- [ ] Rust toolchain 已安装 (cargo/rustc)

### Step 1: 启动 CarpAI Server
```bash
# Terminal 1
cd D:/studying/Codecargo/CarpAI
cargo run --package carpai-server
```

**验证**:
```bash
# Terminal 2
netstat -an | findstr 50051
# 应看到: TCP    [::1]:50051    LISTENING
```

- [ ] Server 启动成功
- [ ] 端口 50051 正在监听

### Step 2: 运行插件
1. 在 IDEA 右上角选择 **"Run Plugin"** configuration
2. 点击绿色 Run 按钮
3. 等待沙盒 IDE 启动 (~30秒)

- [ ] 沙盒 IDE 成功启动
- [ ] 无错误弹窗

### Step 3: 验证插件加载
1. 在沙盒 IDE 中: `File → Settings → Tools`
2. 检查是否看到 **"CarpAI"** 条目

- [ ] CarpAI 出现在 Settings → Tools 列表中

### Step 4: 配置 Server 连接
1. 点击 **CarpAI** 进入设置页
2. 填写:
   - Server Host: `localhost`
   - Server Port: `50051`
   - API Key: (留空)
   - Default Model: `claude-sonnet-4`
3. 点击 **Apply** → **OK**

- [ ] 配置保存成功
- [ ] 无错误提示

### Step 5: 测试聊天
1. 打开 Tool Window: `View → Tool Windows → CarpAI Chat`
   - 或点击右侧边栏 "CarpAI Chat" 标签
2. 在底部输入框输入: `Hello, CarpAI!`
3. 按 Enter 或点击 Send

**预期结果**:
```
You: Hello, CarpAI!
CarpAI: <response from server>
```

- [ ] 消息发送成功
- [ ] 收到 AI 响应
- [ ] 无错误显示

### Step 6: 测试右键菜单 (可选)
1. 打开任意代码文件
2. 选中一段代码
3. 右键 → **CarpAI** → **Explain Code**

- [ ] 右键菜单显示 CarpAI 选项
- [ ] 点击后在 Chat 窗口显示解释

---

## 故障排查

### 问题 A: Server 无法启动
```bash
# 检查是否有其他进程占用端口
netstat -ano | findstr 50051
taskkill /F /PID <PID>
```

### 问题 B: 插件未出现在 Settings
1. 关闭沙盒 IDE
2. 主 IDE: `File → Invalidate Caches → Restart`
3. 重新 Run Plugin

### 问题 C: "Connection refused" 错误
1. 确认 Server 正在运行: `netstat -an | findstr 50051`
2. 检查 Settings 中的 Host/Port 是否正确
3. 防火墙是否阻止了本地连接

---

## 测试结果汇总

| 测试项 | 通过 | 失败 | 备注 |
|--------|------|------|------|
| Server 启动 | [ ] | [ ] | |
| 插件加载 | [ ] | [ ] | |
| Settings 配置 | [ ] | [ ] | |
| 聊天功能 | [ ] | [ ] | |
| 右键菜单 | [ ] | [ ] | 可选 |

---

## 签名

**测试人**: _______________  
**日期**: _______________  
**结论**: □ 通过  □ 不通过  

---

> **注意**: 请将此清单打印或复制到 Issue 中，逐项勾选验证
