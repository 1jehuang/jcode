# Claude Code 101个命令详细分类分析

> **分析日期**: 2026-05-21
> **源码路径**: `D:\studying\Codecargo\opensource\claude_code_src\src\commands`
> **总命令数**: 101个（不含动态技能/插件命令）

---

## 📊 命令分类统计

| 类别 | 数量 | 占比 | 说明 |
|------|------|------|------|
| **会话管理** | 12 | 12% | session/resume/compact等 |
| **Git工作流** | 8 | 8% | commit/branch/pr等 |
| **代码操作** | 10 | 10% | review/refactor/diff等 |
| **文件管理** | 6 | 6% | files/rename/copy等 |
| **配置管理** | 8 | 8% | config/env/model等 |
| **认证授权** | 5 | 5% | login/logout/permissions等 |
| **IDE集成** | 7 | 7% | ide/desktop/chrome等 |
| **Agent系统** | 6 | 6% | agents/skills/plugin等 |
| **监控分析** | 8 | 8% | insights/cost/stats等 |
| **工具集成** | 10 | 10% | mcp/hooks/terminalSetup等 |
| **UI/UX** | 8 | 8% | theme/color/vim等 |
| **高级功能** | 13 | 13% | voice/buddy/bridge等 |

---

## 📋 完整命令清单（按类别）

### 1. 会话管理 (12个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `session` | `session/index.ts` | 会话列表/切换 | P0 |
| `resume` | `resume/index.ts` | 恢复历史会话 | P0 |
| `compact` | `compact/index.ts` | 压缩上下文 | P1 |
| `clear` | `clear/index.ts` | 清屏/清除历史 | P1 |
| `exit` | `exit/index.ts` | 退出CLI | P1 |
| `rewind` | `rewind/index.ts` | 回退到历史状态 | P2 |
| `fork` | `fork/index.ts` | Fork子Agent | P2 |
| `thinkback` | `thinkback/index.ts` | 回顾历史决策 | P3 |
| `thinkback-play` | `thinkback-play/index.ts` | 回放历史记录 | P3 |
| `backfill-sessions` | `backfill-sessions/index.ts` | 批量填充会话 | P3 |
| `share` | `share/index.ts` | 分享会话链接 | P1 |
| `summary` | `summary/index.ts` | 生成会话摘要 | P1 |

**CarpAI现状**: 已有`session`, `replay`，缺10个

---

### 2. Git工作流 (8个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `commit` | `commit.ts` | 智能提交消息生成 | P0 |
| `commit-push-pr` | `commit-push-pr.ts` | 一键提交+推送+PR | P0 |
| `pr_comments` | `pr_comments/index.ts` | PR评论管理 | P0 |
| `autofix-pr` | `autofix-pr/index.ts` | 自动修复PR问题 | P1 |
| `subscribe-pr` | `subscribe-pr.js` | 订阅PR通知 | P2 |
| `branch` | `branch/index.ts` | 分支管理 | P1 |
| `install-github-app` | `install-github-app/index.ts` | GitHub App安装 | P1 |
| `issue` | `issue/index.ts` | Issue管理 | P2 |

**CarpAI现状**: 已有基础`git`命令，缺8个专用命令

---

### 3. 代码操作 (10个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `review` | `review.ts` | AI代码审查 | **P0** |
| `ultrareview` | `review.ts` | 深度代码审查 | P1 |
| `security-review` | `security-review.ts` | 安全审查 | **P0** |
| `diff` | `diff/index.ts` | Diff查看 | P1 |
| `context` | `context/index.ts` | 上下文分析 | P1 |
| `ctx_viz` | `ctx_viz/index.ts` | 上下文可视化 | P2 |
| `debug-tool-call` | `debug-tool-call/index.ts` | 调试工具调用 | P2 |
| `bughunter` | `bughunter/index.ts` | Bug查找 | P2 |
| `perf-issue` | `perf-issue/index.ts` | 性能问题分析 | P2 |
| `ant-trace` | `ant-trace/index.ts` | 追踪分析 | P3 |

**CarpAI现状**: 全缺，需实现10个

---

### 4. 文件管理 (6个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `files` | `files/index.ts` | 文件浏览/搜索 | P0 |
| `rename` | `rename/index.ts` | 重命名（更新引用） | P1 |
| `copy` | `copy/index.ts` | 复制文件 | P1 |
| `add-dir` | `add-dir/index.ts` | 添加目录到上下文 | P1 |
| `desktop` | `desktop/index.ts` | 桌面文件操作 | P2 |
| `mobile` | `mobile/index.ts` | 移动端文件同步 | P3 |

**CarpAI现状**: 已有基础文件工具，缺6个命令

---

### 5. 配置管理 (8个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `config` | `config/index.ts` | 配置管理 | P0 |
| `env` | `env/index.ts` | 环境变量管理 | P1 |
| `model` | `model/index.ts` | 模型选择/切换 | P0 |
| `theme` | `theme/index.ts` | 主题切换 | P2 |
| `color` | `color/index.ts` | 颜色配置 | P2 |
| `output-style` | `output-style/index.ts` | 输出格式 | P2 |
| `privacy-settings` | `privacy-settings/index.ts` | 隐私设置 | P1 |
| `remote-env` | `remote-env/index.ts` | 远程环境配置 | P2 |

**CarpAI现状**: 已有`config`，缺7个

---

### 6. 认证授权 (5个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `login` | `login/index.ts` | 登录 | P0 |
| `logout` | `logout/index.ts` | 登出 | P1 |
| `permissions` | `permissions/index.ts` | 权限管理 | P1 |
| `oauth-refresh` | `oauth-refresh/index.ts` | OAuth刷新 | P2 |
| `rate-limit-options` | `rate-limit-options/index.ts` | 速率限制配置 | P2 |

**CarpAI现状**: 已有`login`, `auth`, `permissions`，基本覆盖

---

### 7. IDE集成 (7个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `ide` | `ide/index.ts` | IDE连接状态 | P1 |
| `chrome` | `chrome/index.ts` | Chrome浏览器集成 | P2 |
| `desktop` | `desktop/index.ts` | 桌面应用集成 | P2 |
| `terminalSetup` | `terminalSetup/index.ts` | 终端设置向导 | P1 |
| `keybindings` | `keybindings/index.ts` | 快捷键管理 | P2 |
| `statusline` | `statusline.tsx` | 状态栏配置 | P2 |
| `hooks` | `hooks/index.ts` | Hook管理 | P1 |

**CarpAI现状**: 部分已有，缺5个

---

### 8. Agent系统 (6个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `agents` | `agents/index.ts` | Agent管理 | P1 |
| `skills` | `skills/index.ts` | 技能管理 | P1 |
| `plugin` | `plugin/index.ts` | 插件管理 | P1 |
| `reload-plugins` | `reload-plugins/index.ts` | 重载插件 | P2 |
| `mcp` | `mcp/index.ts` | MCP服务器管理 | P0 |
| `agents-platform` | `agents-platform/index.ts` | Agent平台（ANT专用） | P3 |

**CarpAI现状**: 已有`mcp`, `skills`，缺4个

---

### 9. 监控分析 (8个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `insights` | `insights.ts` | 会话分析报告 | **P0** |
| `cost` | `cost/index.ts` | 成本追踪 | P1 |
| `stats` | `stats/index.ts` | 使用统计 | P1 |
| `usage` | `usage/index.ts` | Token用量 | P0 |
| `extra-usage` | `extra-usage/index.ts` | 额外用量详情 | P2 |
| `doctor` | `doctor/index.ts` | 系统诊断 | P1 |
| `heapdump` | `heapdump/index.ts` | 内存堆快照 | P3 |
| `mock-limits` | `mock-limits/index.ts` | 模拟限制（测试） | P3 |

**CarpAI现状**: 已有`usage`, `doctor`，缺6个

---

### 10. 工具集成 (10个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `help` | `help/index.ts` | 帮助系统 | P0 |
| `version` | `version.ts` | 版本信息 | P0 |
| `init` | `init.ts` | 项目初始化 | P0 |
| `init-verifiers` | `init-verifiers.ts` | 验证器初始化 | P2 |
| `install` | `install.tsx` | 安装向导 | P1 |
| `feedback` | `feedback/index.ts` | 用户反馈 | P2 |
| `release-notes` | `release-notes/index.ts` | 发布说明 | P2 |
| `upgrade` | `upgrade/index.ts` | 升级检查 | P1 |
| `remote-setup` | `remote-setup/index.ts` | 远程设置 | P2 |
| `sandbox-toggle` | `sandbox-toggle/index.ts` | 沙盒开关 | P2 |

**CarpAI现状**: 大部分已有，缺3个

---

### 11. UI/UX (8个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `vim` | `vim/index.ts` | Vim模式 | P2 |
| `stickers` | `stickers/index.ts` | 贴纸/表情 | P3 |
| `good-claude` | `good-claude/index.ts` | 点赞反馈 | P2 |
| `btw` | `btw/index.ts` | 顺便提醒 | P3 |
| `advisor` | `advisor.ts` | 顾问模式 | P2 |
| `plan` | `plan/index.ts` | 规划模式 | P1 |
| `effort` | `effort/index.ts` | 努力程度控制 | P1 |
| `fast` | `fast/index.ts` | 快速模式 | P1 |
| `passes` | `passes/index.ts` | 迭代次数控制 | P1 |

**CarpAI现状**: 缺9个

---

### 12. 高级功能 (13个)

| 命令 | 文件 | 功能描述 | 优先级 |
|------|------|----------|--------|
| `voice` | `voice/index.ts` | 语音模式 | P3 |
| `buddy` | `buddy/index.ts` | 结对编程 | P2 |
| `bridge` | `bridge/index.ts` | Bridge模式 | P3 |
| `bridge-kick` | `bridge-kick.ts` | 踢出Bridge | P3 |
| `brief` | `brief.ts` | Brief模式 | P3 |
| `assistant` | `assistant/index.ts` | Assistant模式 | P3 |
| `remoteControlServer` | `remoteControlServer/index.ts` | 远程控制 | P3 |
| `proactive` | `proactive.js` | 主动建议 | P3 |
| `ultraplan` | `ultraplan.tsx` | 超规划模式 | P3 |
| `torch` | `torch.js` | Torch功能 | P3 |
| `peers` | `peers/index.ts` | 对等协作 | P3 |
| `teleport` | `teleport/index.ts` | 快速跳转 | P2 |
| `tag` | `tag/index.ts` | 标签管理 | P1 |
| `export` | `export/index.ts` | 导出会话 | P1 |

**CarpAI现状**: 缺14个

---

## 🎯 CarpAI补齐优先级矩阵

### P0 - 必须立即实现（8个命令）

这些是Claude Code的核心差异化功能：

1. ✅ **review** - AI代码审查（已有基础，需增强）
2. ✅ **security-review** - 安全审查
3. ✅ **commit-push-pr** - 一键PR工作流
4. ✅ **pr_comments** - PR评论集成
5. ✅ **insights** - 会话分析报告
6. ✅ **files** - 文件浏览管理
7. ⏳ **effort/fast/passes** - 执行策略控制

**预计工作量**: 6-8周

---

### P1 - 重要功能（15个命令）

8. **rename/copy** - 文件操作
9. **branch** - 分支管理
10. **autofix-pr** - 自动修复PR
11. **diff** - Diff查看
12. **context** - 上下文分析
13. **env** - 环境变量
14. **logout** - 登出
15. **feedback** - 用户反馈
16. **doctor** - 系统诊断增强
17. **cost** - 成本追踪
18. **stats** - 使用统计
19. **install** - 安装向导
20. **upgrade** - 升级检查
21. **hooks** - Hook管理
22. **plan** - 规划模式
23. **tag** - 标签管理
24. **export** - 导出会话

**预计工作量**: 8-10周

---

### P2 - 增强功能（25个命令）

25-49. 包括IDE集成、Agent系统、UI/UX等

**预计工作量**: 12-14周

---

### P3 - 长期演进（53个命令）

50-101. 包括语音、Bridge、高级Agent等

**预计工作量**: 16-20周

---

## 📈 实施路线图

```
Week 1-2:   架构重构（命令模块化）
Week 3-8:   P0核心命令（8个）→ 累计41个命令
Week 9-16:  P1重要命令（15个）→ 累计56个命令
Week 17-24: P2增强命令（25个）→ 累计81个命令
Week 25-36: P3高级命令（20个）→ 累计101个命令

并行进行:
Week 3-36:  工具体系补齐（180个工具）
Week 13-28: Hook系统（104个Hook）
```

---

## 🔍 关键发现

### Claude Code的命令设计特点

1. **模块化架构**: 每个命令独立目录，包含`index.ts` + 辅助文件
2. **条件加载**: 使用feature flag控制高级功能
3. **动态扩展**: 支持skills/plugins/workflows动态注册
4. **类型安全**: TypeScript严格类型定义
5. **懒加载**: 大命令（如insights 113KB）采用懒加载

### CarpAI需要借鉴的设计

1. **命令注册中心**: 从单体`commands.rs`迁移到模块化注册
2. **Feature Flag系统**: 支持实验性功能开关
3. **插件系统**: 动态加载外部命令
4. **懒加载机制**: 优化启动性能
5. **类型定义**: 统一的Command trait

---

## 📝 下一步行动

### 本周任务
1. ✅ 完成命令分类分析
2. ⏳ 创建命令模块化目录结构
3. ⏳ 开始实现`review`命令

### 下周任务
1. 实现`security-review`命令
2. 实现`commit-push-pr`命令
3. 设计工具系统新架构

---

**分析完成时间**: 2026-05-21
**分析师**: CarpAI Core Team
**文档版本**: v1.0
