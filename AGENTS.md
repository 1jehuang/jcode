# Repository Guidelines

## Development Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- If the git state is not clean, or there are other agents working in the codebase in parallel, do your best to still commit your work. 
- **Push when done** - Push all commits to remote when finishing a task or session
- **Use fast iteration by default** - Prefer `cargo check`, targeted tests, and dev builds while iterating
- **Rebuild when done** - When you are done making changes, build the source.
- **Bump version for releases** - Update version in `Cargo.toml` when making releases. When cutting a new release, look at all the changes that happened since the last release and determine what the version bump should be ie patch or minor, etc. 
- **Remote builds available** - Use `scripts/remote_build.sh` to offload heavy cargo work to another machine. If your build is terminated, likely is because there are not enough resources on this machine to build. use remote build in that case. Try checking the resource avaliablity on the machine before you run a build. 

## Logs
- Logs are written to `~/.jcode/logs/` (daily files like `jcode-YYYY-MM-DD.log`).

## Debug Socket
- Use the debug socket for runtime level debugging

## Compilation Error/Warning Repair Principles (分层分模块修复法)

### 总体流程
运行 `cargo check 2>&1` 获取当前状态 → 按层分类 → 分配 agents 并行修复 → 验证

### 第一层（全局错误 + 模块间接口错误）
- **由一个 agent 统一修复**，因其涉及跨模块影响
- 包括：Edition 不兼容、全局类型缺失、trait 与 struct 混用、跨模块导入错误、pub 接口类型不匹配
- 修复后立即 `cargo check` 验证

### 第二层（模块内部错误和警告）
- **按模块拆分**，一个 agent 一次 ≤3 个模块
- 错误类型及处置策略：

| 错误/警告类型 | 处置策略 |
|---|---|
| **未使用的代码**（死函数/死字段/死变量） | 优先尝试**激活使用**（补全调用链）。如确为预留/未完成，则 `#[allow(dead_code)]` 并注释原因 |
| **命名规范**（non_snake_case） | 改为 snake_case。若涉及 `fn item` 无法捕获外层变量导致无法重命名，用 `#[allow(non_snake_case)]` |
| **语法错误**（E0425/E0433/E0599 等） | 修复语法：补 import、改 API 调用、加类型标注 |
| **未使用导入/变量** | 移除或加 `_` 前缀 |
| **无意义比较**（usize < 0 等） | 简化条件 |
| **不可达模式**（unreachable_patterns） | 简化 patterns 或加 `#[allow(unreachable_patterns)]` |

### 第三层（跨模块协调）
- 修复完成后运行完整 `cargo check`
- 新引入的错误回退到第一层

## Install Notes
- `~/.local/bin/jcode` is the launcher symlink used from `PATH`.
- `~/.jcode/builds/current/jcode` is the active local/source-build channel; self-dev builds and `scripts/install_release.sh` point the launcher here.
- `~/.jcode/builds/stable/jcode` is the stable release channel; `scripts/install.sh` installs this and points the launcher here.
- `~/.jcode/builds/versions/<version>/jcode` stores immutable binaries.
- `~/.jcode/builds/canary/jcode` still exists for canary/testing flows, but it is not the primary self-dev install path.
- On Windows, the equivalents are `%LOCALAPPDATA%\\jcode\\bin\\jcode.exe` for the launcher, `%LOCALAPPDATA%\\jcode\\builds\\stable\\jcode.exe` for stable, and `%LOCALAPPDATA%\\jcode\\builds\\versions\\<version>\\jcode.exe` for immutable installs; `scripts/install.ps1` currently installs the stable channel.
- Ensure `~/.local/bin` is **before** `~/.cargo/bin` in `PATH`.

