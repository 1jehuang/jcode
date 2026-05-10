# 银河麒麟 KylinOS V10 — JCode 部署指南

## 支持架构

| 架构 | CPU 厂商 | 验证状态 |
|------|----------|----------|
| `x86_64` | 兆芯 / Intel / AMD | ✅ 已验证 |
| `aarch64` | 鲲鹏 920 / 飞腾 | ⚠️ 需要验证 |

## 前置条件

### 系统依赖

```bash
# 基础开发工具
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    libfontconfig1-dev \
    libdbus-1-dev

# Rust 工具链
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### 验证环境

```bash
# 运行兼容性测试（约 5-10 分钟）
bash packaging/kylin/test_compatibility.sh
```

## 安装方式

### 方式 1：源码编译（推荐）

```bash
git clone https://github.com/1jehuang/jcode.git
cd jcode
cargo build --release
sudo cp target/release/jcode /usr/local/bin/
sudo cp target/release/jcode-server /usr/local/bin/
sudo cp target/release/jcode-grpc /usr/local/bin/
```

### 方式 2：交叉编译（从 x86_64 开发机）

```bash
# 在 x86_64 开发机上执行：
bash packaging/kylin/cross_compile.sh aarch64   # 对应 鲲鹏/飞腾
# 或
bash packaging/kylin/cross_compile.sh x86_64    # 对应 兆芯

# 将产物复制到 KylinOS 目标机
scp /tmp/jcode-kylin-*.tar.gz user@kylin:~/
```

### 方式 3：Docker 容器

```bash
# 构建 KylinOS 兼容镜像
podman build -t jcode-kylin -f packaging/kylin/Dockerfile .
podman run -it --rm jcode-kylin jcode --help
```

## 已知问题

| 问题 | 影响 | 缓解方案 |
|------|------|----------|
| UKUI qterminal 256color | TUI 颜色显示不完整 | 设置 `TERM=xterm-256color` |
| 中文 locale 渲染 | Unicode 字符可能乱码 | 确保 `LANG=zh_CN.UTF-8` |
| glibc 2.28 | 若链接较新 glibc 则无法启动 | 使用 KylinOS 源码编译 |
| selinux enforce 模式 | 文件操作权限受限 | 添加 selinux policy |
| 鲲鹏/飞腾 aarch64 | 需要交叉编译 | 使用 `cross_compile.sh` |

## TUI 终端兼容性

```bash
# 验证终端能力
tput colors          # 应 >= 256
tput longname        # 应显示终端类型

# 测试中文显示
echo "你好，世界！➜ ✅ 🚀"

# 运行冒烟测试
cargo run -- --help
cargo run -- --version

# 启动 gRPC 服务器（测试服务模式）
JCODE_GRPC_PORT=50051 cargo run --bin jcode-grpc
```

## 桌面入口

桌面入口文件 `packaging/linux/jcode-desktop.desktop` 可在 UKUI 桌面环境中注册使用：

```bash
sudo cp packaging/linux/jcode-desktop.desktop /usr/share/applications/
```

## 系统服务

```bash
# 安装 systemd 服务
sudo cp packaging/kylin/jcode-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable jcode-server
sudo systemctl start jcode-server
```

## 国产化适配清单

- [x] 兆芯 x86_64 编译通过
- [ ] 鲲鹏 aarch64 编译验证
- [ ] 飞腾 aarch64 编译验证
- [ ] UKUI 桌面 TUI 渲染测试
- [ ] 中文 locale 完整测试
- [ ] glibc 2.28 兼容性验证
- [ ] selinux policy 编写
- [ ] NeoCertify 认证准备
