# CarpAI 对齐 Claude Code 完整实施计划

> **目标**: 补齐100+命令、227+工具、104个Hook文件
> **参考源码**: `D:\studying\Codecargo\opensource\claude_code_src\src`
> **预计周期**: 6-9个月

---

## 📊 差距量化分析

### 当前状态 (2026-05-21)

| 维度 | CarpAI | Claude Code | 差距 | 优先级 |
|------|--------|-------------|------|--------|
| **顶层命令** | 33个 | 101个 | ❌ 缺68个 | P0-P2 |
| **工具实现** | ~50个 | 227+ | ❌ 缺~180个 | P0-P3 |
| **Hook系统** | 0个 | 85-104个 | ❌ 全缺 | P1-P2 |
| **命令模块化** | 单体commands.rs | 每命令独立目录 | ⚠️ 需重构 | P0 |

---

## 🏗️ 第一阶段：架构重构（Week 1-2）

### 1.1 命令系统模块化重构

**现状问题**:
- `src/commands.rs` 单体文件 138KB
- 所有命令耦合在一起，难以维护

**目标架构**:
```
src/commands/
├── mod.rs                    # 命令注册中心
├── auth/                     # 认证相关命令
│   ├── mod.rs
│   ├── login.rs
│   ├── logout.rs
│   └── permissions.rs
├── session/                  # 会话管理
│   ├── mod.rs
│   ├── list.rs
│   ├── resume.rs
│   └── compact.rs
├── code/                     # 代码操作
│   ├── mod.rs
│   ├── review.rs
│   ├── refactor.rs
│   └── debug.rs
├── git/                      # Git工作流
│   ├── mod.rs
│   ├── commit.rs
│   ├── branch.rs
│   └── pr.rs
├── admin/                    # 管理命令
│   ├── mod.rs
│   ├── usage.rs
│   ├── config.rs
│   └── doctor.rs
└── ... (共20+子模块)
```

**实施步骤**:
1. 创建 `src/commands/` 目录结构
2. 将现有33个命令迁移到独立模块
3. 更新命令注册机制
4. 保持向后兼容

**工作量**: 2周

---

## 🎯 第二阶段：P0核心命令补齐（Week 3-8）

### 2.1 代码审查系统（最高优先级）

#### [P0.1] `review` 命令
**对标**: `src/commands/review.ts` (Claude Code)

**功能**:
- 分析git diff或staged变更
- AI驱动的代码质量审查
- 安全漏洞检测
- 性能问题识别
- 最佳实践建议

**实现**:
```rust
// src/commands/code/review.rs
pub struct ReviewCommand {
    staged: bool,
    diff_ref: Option<String>,
    security_mode: bool,
    output_format: OutputFormat, // table/json/markdown
}

impl Command for ReviewCommand {
    async fn execute(&self) -> Result<()> {
        // 1. 获取diff
        let diff = self.get_diff().await?;

        // 2. AI分析
        let issues = self.analyze_with_ai(&diff).await?;

        // 3. 分类输出
        self.render_issues(issues)?;

        Ok(())
    }
}
```

**工作量**: 1周

---

#### [P0.2] `security-review` 命令
**对标**: `src/commands/security-review.ts`

**功能**:
- OWASP Top 10扫描
- 依赖漏洞检查
- 密钥泄露检测
- 权限配置审计

**工作量**: 3-5天

---

### 2.2 Git工作流增强

#### [P0.3] `commit-push-pr` 一键PR
**对标**: `src/commands/commit-push-pr.ts`

**功能**:
```bash
carpai commit-push-pr --title "Fix: ..." --description "..."
# 自动执行:
# 1. git add -A
# 2. git commit -m "AI生成的提交消息"
# 3. git push origin HEAD
# 4. gh pr create --title "..." --body "..."
```

**实现**:
```rust
// src/commands/git/commit_push_pr.rs
pub struct CommitPushPrCommand {
    title: String,
    description: Option<String>,
    base_branch: String,
}
```

**工作量**: 1周

---

#### [P0.4] `pr_comments` PR评论集成
**对标**: `src/commands/pr_comments/index.ts`

**功能**:
- 获取GitHub PR评论
- AI自动回复建议
- 批量处理评论

**工作量**: 1周

---

### 2.3 会话分析与洞察

#### [P0.5] `insights` 会话分析
**对标**: `src/commands/insights.ts` (113KB大文件)

**功能**:
- Token消耗趋势分析
- 工具使用频率统计
- 错误模式识别
- 效率优化建议
- 导出报告（Markdown/JSON/HTML）

**实现**:
```rust
// src/commands/admin/insights.rs
pub struct InsightsCommand {
    session_id: Option<String>,
    time_range: TimeRange, // day/week/month
    export_format: Option<ExportFormat>,
}

pub struct SessionInsights {
    pub token_usage: TokenUsageStats,
    pub tool_frequency: HashMap<String, u32>,
    pub error_patterns: Vec<ErrorPattern>,
    pub suggestions: Vec<String>,
}
```

**工作量**: 1-2周

---

### 2.4 文件管理命令

#### [P0.6] `files` 文件浏览
**对标**: `src/commands/files/index.ts`

**功能**:
```bash
carpai files --type rs --modified today
carpai files --large --limit 10
```

**工作量**: 3天

---

#### [P0.7] `rename/copy` 文件操作
**对标**: `src/commands/rename/index.ts`, `src/commands/copy/index.ts`

**功能**:
- 智能重命名（自动更新引用）
- 文件复制（带冲突检测）

**工作量**: 1周

---

### 2.5 执行策略控制

#### [P0.8] `effort/fast/passes` 执行控制
**对标**: `src/commands/effort/index.ts`, `fast/index.ts`, `passes/index.ts`

**功能**:
```bash
carpai effort auto|conserve|high    # 控制AI推理深度
carpai fast on|off                   # 跳过非关键工具调用
carpai passes 1-10                   # 设置自动迭代次数
```

**工作量**: 3-5天

---

### P0阶段总结

| 命令 | 工作量 | 优先级 | 状态 |
|------|--------|--------|------|
| review | 1周 | P0 | ⏳ |
| security-review | 3-5天 | P0 | ⏳ |
| commit-push-pr | 1周 | P0 | ⏳ |
| pr_comments | 1周 | P0 | ⏳ |
| insights | 1-2周 | P0 | ⏳ |
| files | 3天 | P0 | ⏳ |
| rename/copy | 1周 | P0 | ⏳ |
| effort/fast/passes | 3-5天 | P0 | ⏳ |
| **小计** | **6-8周** | - | - |

**新增命令数**: 8个核心命令

---

## 🔧 第三阶段：P1重要命令补齐（Week 9-16）

### 3.1 GitHub/Slack集成

#### [P1.1] `install-github-app`
**对标**: `src/commands/install-github-app/` (13文件)

**功能**:
- GitHub App安装向导
- Webhook配置
- 权限设置

**工作量**: 2周

---

#### [P1.2] `install-slack-app`
**对标**: `src/commands/install-slack-app/`

**工作量**: 1周

---

### 3.2 会话管理增强

#### [P1.3] `tag/summary` 标签与摘要
**对标**: `src/commands/summary/index.ts`

**功能**:
```bash
carpai tag key=value              # 添加标签
carpai summary --export markdown  # 生成会话摘要
```

**工作量**: 3-5天

---

#### [P1.4] `export` 会话导出增强
**对标**: `src/commands/export/index.ts`

**工作量**: 3天

---

### 3.3 环境与配置

#### [P1.5] `env` 环境变量管理
**对标**: `src/commands/env/index.ts`

**功能**:
```bash
carpai env list
carpai env set KEY=value
carpai env unset KEY
```

**工作量**: 3-5天

---

#### [P1.6] `config` 配置管理增强
**对标**: `src/commands/config/index.ts`

**工作量**: 3天

---

### 3.4 其他P1命令

| 命令 | 功能 | 工作量 |
|------|------|--------|
| `logout` | 登出 | 1天 |
| `upgrade` | 升级检查 | 3天 |
| `feedback` | 用户反馈 | 3天 |
| `doctor` | 诊断工具 | 1周 |
| `cost` | 成本追踪 | 1周 |
| `teleport` | 快速导航 | 3天 |
| `status` | 状态显示 | 3天 |
| `share` | 会话分享 | 1周 |

**P1小计**: 15个命令，8-10周

---

## 🚀 第四阶段：P2增强命令补齐（Week 17-24）

### 4.1 高级功能

| 命令 | 功能 | 工作量 |
|------|------|--------|
| `buddy` | 结对编程模式 | 2周 |
| `voice` | 语音模式 | 3周 |
| `agents` | Agent管理 | 2周 |
| `plugin` | 插件管理 | 2周 |
| `workflows` | 工作流脚本 | 2周 |
| `plan` | 规划模式 | 1周 |
| `vim` | Vim模式 | 1周 |
| `chrome` | Chrome集成 | 2周 |

### 4.2 辅助命令

| 命令 | 功能 | 工作量 |
|------|------|--------|
| `color` | 主题颜色 | 2天 |
| `clear` | 清屏 | 1天 |
| `help` | 帮助系统增强 | 3天 |
| `keybindings` | 快捷键管理 | 1周 |
| `hooks` | Hook管理 | 1周 |
| `branch` | 分支管理增强 | 1周 |
| `compact` | 上下文压缩 | 3天 |
| `memory` | 记忆管理 | 1周 |

**P2小计**: 25个命令，12-14周

---

## 🛠️ 第五阶段：工具体系补齐（并行进行）

### 5.1 P0核心工具（Week 3-12）

#### [Tool-P0.1] `multi_file_edit` 多文件编辑
**对标**: Claude Code的FileEditTool

**功能**:
- 跨文件批量编辑
- 依赖感知
- Diff预览

**工作量**: 2周

---

#### [Tool-P0.2] `ast_search` AST搜索
**对标**: LSPTool

**功能**:
- 符号级别搜索
- 类型感知
- 引用查找

**工作量**: 2周

---

#### [Tool-P0.3] `symbol_refactor` 符号重构
**对标**: LSPTool的重构功能

**功能**:
- 重命名符号（跨文件）
- 提取方法
- 内联函数

**工作量**: 2周

---

#### [Tool-P0.4] `debug_integration` 调试器集成
**对标**: debug-tool-call

**功能**:
- DAP客户端
- 断点管理
- 单步执行
- 变量查看

**工作量**: 3-4周

---

### 5.2 P1重要工具（Week 13-20）

| 工具 | 功能 | 工作量 |
|------|------|--------|
| `file_diff` | 文件差异可视化 | 1周 |
| `git_commit_tool` | 带预览的提交 | 3天 |
| `test_runner` | 测试运行与解析 | 1周 |
| `project_map` | 项目结构地图 | 1周 |
| `dependency_graph` | 依赖关系图 | 2周 |
| `code_explain` | 代码解释 | 1周 |
| `performance_profile` | 性能分析 | 2周 |

---

### 5.3 P2/P3工具（Week 21-36）

需要补齐约160个工具，包括：
- Bash工具增强（安全性验证）
- MCP工具集成
- Agent工具（PlanAgent/ExploreAgent等）
- LSP工具完整实现
- Notebook编辑工具
- PowerShell工具（Windows）

**工具阶段总计**: 180个工具，30-36周

---

## 🔗 第六阶段：Hook系统实现（Week 13-28）

### 6.1 Hook架构设计

**参考**: `src/hooks/` 85-104个文件

**Hook类型**:
1. **文件Hook**: 文件打开/保存/关闭
2. **编辑器Hook**: 光标移动/选择变化
3. **工具Hook**: 工具执行前/后
4. **会话Hook**: 会话开始/结束
5. **Git Hook**: 提交/推送/合并

### 6.2 核心Hook实现

| Hook类别 | 数量 | 工作量 |
|----------|------|--------|
| 文件Hook | 15个 | 2周 |
| 编辑器Hook | 20个 | 3周 |
| 工具Hook | 25个 | 3周 |
| 会话Hook | 15个 | 2周 |
| Git Hook | 10个 | 1周 |
| 其他Hook | 19个 | 2周 |

**Hook系统总计**: 104个Hook，13-15周

---

## 📅 总体时间线

```
Week 1-2:   架构重构（命令模块化）
Week 3-8:   P0核心命令（8个）
Week 9-16:  P1重要命令（15个）
Week 17-24: P2增强命令（25个）
Week 3-36:  工具体系补齐（180个工具，并行）
Week 13-28: Hook系统（104个Hook，并行）

总周期: 9个月（36周）
```

---

## 📊 里程碑

| 里程碑 | 交付物 | 时间 |
|--------|--------|------|
| M1 | 命令模块化完成 | Week 2 |
| M2 | P0核心命令完成（41个命令） | Week 8 |
| M3 | P1命令完成（56个命令） | Week 16 |
| M4 | P2命令完成（81个命令） | Week 24 |
| M5 | P0/P1工具完成（100个工具） | Week 20 |
| M6 | 全部工具完成（230+工具） | Week 36 |
| M7 | Hook系统完成（104个Hook） | Week 28 |
| M8 | **全面对齐Claude Code** | Week 36 |

---

## 👥 资源需求

### 人力资源
- Rust后端工程师: 3-4人
- 前端工程师（IDE插件）: 1-2人
- QA工程师: 1人
- 技术作家: 0.5人

### 总工作量估算
- 命令系统: 2000-2500人时
- 工具体系: 3000-3500人时
- Hook系统: 1500-1800人时
- **总计**: 6500-7800人时

按4人团队计算: **9-12个月**

---

## ✅ 成功指标

### 数量指标
- [ ] 顶层命令: 100+个
- [ ] 工具实现: 227+个
- [ ] Hook文件: 104个
- [ ] 测试覆盖率: >85%

### 质量指标
- [ ] 命令响应时间: <100ms
- [ ] 工具执行成功率: >95%
- [ ] Hook触发延迟: <10ms
- [ ] 零崩溃率: 99.9%

### 兼容性指标
- [ ] 与Claude Code命令兼容度: >90%
- [ ] API接口兼容度: >85%
- [ ] 配置文件兼容度: >80%

---

## 🎯 下一步行动

### 立即开始（本周）
1. ✅ 创建命令模块化目录结构
2. ✅ 迁移现有33个命令
3. ⏳ 开始实现`review`命令

### 下周开始
1. 实现`security-review`命令
2. 启动`commit-push-pr`命令开发
3. 设计工具系统新架构

---

**文档版本**: v1.0
**最后更新**: 2026-05-21
**负责人**: CarpAI Core Team
**状态**: 待审批
