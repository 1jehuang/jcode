---
name: codes-quality-assessment-fix-plan
overview: 对 CarpAI 系统进行代码质量评估，列出编译错误和警告的分类清单，制定分阶段的针对性修复方案。
design:
  styleKeywords:
    - 分析报告
    - 分类清单
    - 分级修复
  fontSystem:
    fontFamily: PingFang SC
    heading:
      size: 24px
      weight: 600
    subheading:
      size: 18px
      weight: 500
    body:
      size: 14px
      weight: 400
  colorSystem:
    primary:
      - "#DC3545"
      - "#FD7E14"
      - "#FFC107"
      - "#28A745"
      - "#17A2B8"
    background:
      - "#FFFFFF"
      - "#F8F9FA"
      - "#E9ECEF"
    text:
      - "#212529"
      - "#6C757D"
    functional:
      - "#28A745"
      - "#DC3545"
      - "#FFC107"
      - "#007BFF"
todos:
  - id: generate-error-report
    content: Generate full error classification report from errors.txt (835 errors, 139 warnings)
    status: completed
  - id: categorize-warnings
    content: Categorize 139 warnings into 6 patterns and produce fix recommendations
    status: completed
    dependencies:
      - generate-error-report
  - id: prioritize-fixes
    content: Create P0-P4 prioritized fix plan with file-level work estimates
    status: completed
    dependencies:
      - categorize-warnings
  - id: generate-file-work-orders
    content: Generate per-file work orders listing specific errors and fix methods
    status: completed
    dependencies:
      - prioritize-fixes
---

## 需求概述

对 CarpAI（25万+ 行 Rust monorepo，91 个 workspace crate）的代码质量进行全面评估，具体包括：

1. **编译错误分类清单**：基于 errors.txt（14265行）的全量分析，列出所有唯一错误代码（E0xxx），按类别分组（如 Import/Resolution、类型不匹配、借用检查器等），统计每类错误数量、涉及文件和根因
2. **编译警告分类清单**：139个警告按模式分类（未使用导入、死代码、弃用项等）
3. **分级修复方案**：按 P0-P4 优先级排列，对每类错误给出具体的修复策略（文件路径、修复方法、预估工作量）
4. **各文件修复难度评估**：按受影响文件列出修复工作量和风险等级

## 产出文档格式

- 完整的评估报告（含统计表格）
- 分级修复路线图
- 各文件修复工单清单

## 技术方案

### 技术栈

- 分析对象：Rust 项目（edition 2024），tokio 异步运行时
- 错误数据源：errors.txt（14265行，最近一次 cargo check 完整输出）
- 辅助数据源：check_errs.txt, check_full.txt, check_errors.txt
- 源码目录：src/ (500+ .rs 文件) + crates/ (392 .rs 文件)

### 评估方法

基于 errors.txt 中提取的 error[E....] 错误代码和 warning 模式进行自动分类统计，辅以人工根因分析。

### 核心指标

| 指标 | 数值 |
| --- | --- |
| 编译错误总数 | 835 |
| 编译警告总数 | 139 |
| 唯一错误代码类型 | 40+ 种 |
| TODO 注释 | 700+ |
| unwrap() 调用 | 500+（大部分在测试） |
| 最大源文件 | `src/cli/commands.rs` ~5000+ 行 |


### 数据来源

- errors.txt: 14,265 行，835 errors + 139 warnings
- check_errs.txt: 构建锁等待（未完整）
- check_full.txt: 6 errors (早期快照)
- check_result.txt: 3 errors (早期快照)

## 输出设计

### 文档结构

1. **错误分类清单** - 按错误代码/类别分组的完整表格，含涉及文件和根因
2. **警告分类清单** - 6大类警告的统计和修复建议
3. **分级修复方案** - P0到P4优先级排列，每类给出具体文件级修复策略
4. **文件级工单** - 按受影响文件的修复工作量和风险评估

### 数据呈现

- 使用 markdown 表格呈现分类统计
- 错误代码以 E0xxx 格式标注
- 涉及文件用绝对路径标注
- 修复策略中包含具体的代码修改指导

### 评估维度

- **严重程度**: P0(编译阻断) / P1(类型系统) / P2(借用检查器) / P3(API适配) / P4(代码质量)
- **影响范围**: 涉及的文件数和错误数
- **修复成本**: 简单(单行修复) / 中等(多行/跨文件) / 困难(架构调整)
- **根因分类**: Rust 2024 edition breakage / API变更 / 拆分遗留 / 原始代码错误