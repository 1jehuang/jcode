# LSP Code Actions 开发状态报告

**检查日期**: 2026-05-22  
**状态**: 🟡 **部分完成** (客户端60%，服务器端0%)

---

## 📊 当前完成度评估

### ✅ 已完成部分

#### 1. LSP客户端实现 (60%完成)

**文件**: `crates/jcode-lsp/src/client.rs` (Line 951-981)

```rust
✅ code_action() 方法已实现
   - 构建CodeActionParams
   - 发送 "textDocument/codeAction" 请求到LSP服务器
   - 解析响应为 Vec<CodeActionOrCommand>
   
✅ ServerManager集成
   - crates/jcode-lsp/src/server_manager.rs (Line 620-630)
   - 通过get_or_start_server_for_file获取客户端
   - 转发code_action请求
```

**功能**:
- ✅ 可以向LSP服务器发送Code Action请求
- ✅ 正确解析LSP协议响应
- ✅ 错误处理完善

---

#### 2. Quick Fix基础框架 (40%完成)

**文件**: `src/diagnostics/mod.rs` (316行)

```rust
✅ Diagnostic结构体
   - file, line, column, severity, message
   - source标识（cargo/clippy/rust-analyzer）
   
✅ QuickFix结构体
   - diagnostic关联
   - title描述
   - edit操作（old_string → new_string）
   
✅ DiagnosticsEngine
   - run_check() - 运行cargo check收集诊断
   - suggest_fix() - 基于错误码生成修复方案
   - apply_fix() - 应用修复到文件
```

**支持的快速修复**:
- ✅ unused_variable → 添加 `_` 前缀
- ✅ needless_return → 移除return关键字
- ✅ missing_safety_doc → 添加Safety文档
- ✅ unused_import → 删除未使用的import

**局限性**:
- ⚠️ 仅支持硬编码的错误码匹配
- ⚠️ 缺少LLM智能修复生成
- ⚠️ 无重构类Code Actions（Extract Method, Rename等）

---

### ❌ 缺失部分

#### 1. LSP服务器端Handler (0%完成) 🔴

**问题**: 没有实现接收IDE请求并返回Code Actions的逻辑

**需要的组件**:
```rust
❌ LSP Message Handler
   - 监听 "textDocument/codeAction" 请求
   - 解析Range和CodeActionContext
   - 调用diagnostics或refactor引擎
   - 返回CodeAction数组
   
❌ Code Action Provider接口
   - trait CodeActionProvider {
       async fn provide_code_actions(
           &self,
           file: &str,
           range: Range,
           context: CodeActionContext,
       ) -> Result<Vec<CodeAction>>;
   }
   
❌ 集成DiagnosticsEngine
   - 将现有的suggest_fix()转换为LSP CodeAction格式
   - 添加quick fix kind标记
   
❌ 集成Refactor Engine
   - Extract Method
   - Rename Symbol
   - Move Function/Class
   - Inline Variable
```

---

#### 2. VSCode插件集成 (0%完成) 🔴

**文件**: `editors/vscode-carpai/src/extension.ts`

**缺失**:
```typescript
❌ registerCodeActionsProvider()
   - 未在VSCode中注册Code Actions provider
   - 无法在编辑器中显示灯泡图标💡
   
❌ CodeAction命令处理
   - 用户点击Code Action后的执行逻辑
   - WorkspaceEdit应用
   
❌ Diagnostic与Code Action关联
   - diagnostics.code字段设置
   - codeActionProvider配置
```

---

#### 3. 重构类Code Actions (0%完成) 🔴

**当前状态**: 只有Quick Fix（修复错误），没有Refactoring（代码改进）

**需要实现**:
```rust
❌ Extract Method
   - 选中代码块 → 提取为新函数
   - 自动推断参数和返回值
   - 更新所有调用点
   
❌ Rename Symbol
   - 跨文件重命名
   - 使用LSP rename或Tree-sitter符号表
   
❌ Move Class/Function
   - 移动到新文件
   - 更新imports
   
❌ Inline Variable/Function
   - 内联变量引用
   - 删除原定义
```

---

## 🎯 完整实现路线图

### Phase 1: LSP服务器端Handler (Week 1)

#### Day 1-2: 创建Code Action Handler

**实现位置**: `crates/jcode-lsp/src/code_action_handler.rs` (新建)

```rust
use lsp_types::{CodeAction, CodeActionKind, CodeActionParams, WorkspaceEdit};
use std::sync::Arc;

pub struct CodeActionHandler {
    diagnostics_engine: Arc<DiagnosticsEngine>,
    refactor_engine: Arc<RefactorEngine>,
}

impl CodeActionHandler {
    pub fn new(diagnostics: Arc<DiagnosticsEngine>) -> Self {
        Self {
            diagnostics_engine: diagnostics,
            refactor_engine: Arc::new(RefactorEngine::new()),
        }
    }
    
    /// 处理 textDocument/codeAction 请求
    pub async fn handle_code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Vec<CodeAction>, LspError> {
        
        let mut actions = Vec::new();
        
        // 1. 添加Quick Fixes（来自diagnostics）
        let diags = self.diagnostics_engine
            .file_diagnostics(params.text_document.uri.path().as_ref())
            .await;
            
        for diag in diags {
            if let Some(fix) = self.diagnostics_engine.suggest_fix(&diag).await {
                actions.push(CodeAction {
                    title: fix.title,
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.to_lsp_diagnostic()]),
                    edit: Some(self.fix_to_workspace_edit(&fix).await?),
                    ..Default::default()
                });
            }
        }
        
        // 2. 添加Refactoring Actions
        actions.extend(self.refactor_actions(&params).await?);
        
        Ok(actions)
    }
    
    async fn refactor_actions(&self, params: &CodeActionParams) 
        -> Result<Vec<CodeAction>, LspError> {
        
        let mut actions = Vec::new();
        
        // Extract Method (如果选中了代码块)
        if !params.range.is_empty() {
            actions.push(CodeAction {
                title: "Extract Method".to_string(),
                kind: Some(CodeActionKind::REFACTOR_EXTRACT),
                command: Some(Command {
                    title: "Extract Method".to_string(),
                    command: "carpai.refactor.extractMethod".to_string(),
                    arguments: Some(vec![
                        serde_json::to_value(&params.text_document.uri)?,
                        serde_json::to_value(&params.range)?,
                    ]),
                }),
                ..Default::default()
            });
        }
        
        // Rename Symbol
        actions.push(CodeAction {
            title: "Rename Symbol".to_string(),
            kind: Some(CodeActionKind::REFACTOR_RENAME),
            command: Some(Command {
                title: "Rename".to_string(),
                command: "editor.action.rename".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        
        Ok(actions)
    }
    
    async fn fix_to_workspace_edit(&self, fix: &QuickFix) 
        -> Result<WorkspaceEdit, LspError> {
        
        let uri = Url::from_file_path(&fix.edit.file)
            .map_err(|_| LspError::InvalidUri)?;
            
        let edit = TextEdit {
            range: fix.diagnostic.to_range(),
            new_text: fix.edit.new_string.clone(),
        };
        
        let mut changes = HashMap::new();
        changes.insert(uri, vec![edit]);
        
        Ok(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }
}
```

---

#### Day 3-4: 集成到LSP消息循环

**修改位置**: `crates/jcode-lsp/src/client.rs` 或新建server模块

```rust
// 在LSP消息处理循环中添加
match message.method.as_str() {
    "textDocument/codeAction" => {
        let params: CodeActionParams = serde_json::from_value(message.params)?;
        let handler = CodeActionHandler::new(diagnostics_engine.clone());
        let actions = handler.handle_code_action(params).await?;
        send_response(message.id, actions);
    }
    // ... other handlers ...
}
```

---

#### Day 5: 测试与调试

**验收标准**:
- [ ] LSP服务器能接收codeAction请求
- [ ] 正确返回Quick Fix列表
- [ ] 返回Refactoring Actions
- [ ] 无协议错误

---

### Phase 2: VSCode插件集成 (Week 2)

#### Day 1-2: 注册Code Actions Provider

**文件**: `editors/vscode-carpai/src/extension.ts`

```typescript
import * as vscode from 'vscode';

class CarpaiCodeActionProvider implements vscode.CodeActionProvider {
    public static readonly providedCodeActionKinds = [
        vscode.CodeActionKind.QuickFix,
        vscode.CodeActionKind.RefactorExtract,
        vscode.CodeActionKind.RefactorRename,
    ];

    provideCodeActions(
        document: vscode.TextDocument,
        range: vscode.Range | vscode.Selection,
        context: vscode.CodeActionContext,
        token: vscode.CancellationToken
    ): vscode.CodeAction[] | Thenable<vscode.CodeAction[]> {
        
        const actions: vscode.CodeAction[] = [];
        
        // 为每个diagnostic添加quick fix
        for (const diagnostic of context.diagnostics) {
            const fix = this.createQuickFix(diagnostic, document);
            if (fix) {
                actions.push(fix);
            }
        }
        
        // 添加refactoring actions
        if (!range.isEmpty) {
            const extractAction = new vscode.CodeAction(
                'Extract Method',
                vscode.CodeActionKind.RefactorExtract
            );
            extractAction.command = {
                command: 'carpai.refactor.extractMethod',
                title: 'Extract Method',
                arguments: [document.uri, range]
            };
            actions.push(extractAction);
        }
        
        return actions;
    }
    
    private createQuickFix(
        diagnostic: vscode.Diagnostic,
        document: vscode.TextDocument
    ): vscode.CodeAction | null {
        // TODO: Call LSP server to get quick fixes
        return null;
    }
}

export function activate(context: vscode.ExtensionContext) {
    // Register code action provider
    const provider = new CarpaiCodeActionProvider();
    context.subscriptions.push(
        vscode.languages.registerCodeActionsProvider(
            { scheme: 'file', language: 'rust' },
            provider,
            {
                providedCodeActionKinds: CarpaiCodeActionProvider.providedCodeActionKinds
            }
        )
    );
}
```

---

#### Day 3-4: 实现Code Action命令处理

```typescript
// 注册命令处理器
context.subscriptions.push(
    vscode.commands.registerCommand(
        'carpai.refactor.extractMethod',
        async (uri: vscode.Uri, range: vscode.Range) => {
            const document = await vscode.workspace.openTextDocument(uri);
            const selectedCode = document.getText(range);
            
            // Call LSP server to perform extraction
            const response = await lspClient.executeCommand(
                'carpai.refactor.extractMethod',
                [uri.toString(), range, selectedCode]
            );
            
            // Apply workspace edit
            if (response.edit) {
                await vscode.workspace.applyEdit(response.edit);
            }
        }
    )
);
```

---

#### Day 5: 测试VSCode集成

**验收标准**:
- [ ] 编辑器中显示灯泡图标💡
- [ ] 点击灯泡显示Code Actions菜单
- [ ] Quick Fix能正确应用
- [ ] Refactoring命令能执行

---

### Phase 3: 高级重构功能 (Week 3-4)

#### Week 3: Extract Method实现

**技术方案**:
1. 使用Tree-sitter分析选中的代码块
2. 识别局部变量作为参数
3. 推断返回类型
4. 生成新函数
5. 替换原代码为函数调用
6. 更新所有引用

---

#### Week 4: Rename + Move实现

**技术方案**:
1. **Rename**: 使用LSP rename或符号表查找所有引用
2. **Move**: 移动代码到新文件，更新imports

---

## 📈 当前 vs 目标对比

| 功能 | Claude Code | Cursor | CarpAI现状 | 差距 |
|------|------------|--------|-----------|------|
| Quick Fix | ✅ 完整 | ✅ 完整 | 🟡 40% | 需LSP集成 |
| Extract Method | ✅ 完整 | ✅ 完整 | ❌ 0% | 需实现 |
| Rename Symbol | ✅ 完整 | ✅ 完整 | ❌ 0% | 需实现 |
| Move Class | ✅ 完整 | 🟡 部分 | ❌ 0% | 需实现 |
| Inline Variable | ✅ 完整 | ❌ 无 | ❌ 0% | 需实现 |
| LSP Handler | ✅ | ✅ | ❌ 0% | 🔴 关键缺失 |
| VSCode集成 | ✅ | ✅ | ❌ 0% | 🔴 关键缺失 |

**综合完成度**: **20%** （距离合格线60%仍有差距）

---

## 🎯 立即行动建议

### 优先级1: 实现LSP服务器端Handler (本周)
1. 创建`code_action_handler.rs`
2. 集成DiagnosticsEngine
3. 添加到LSP消息循环
4. 测试端到端流程

### 优先级2: VSCode插件集成 (下周)
1. 注册CodeActionProvider
2. 实现命令处理器
3. 测试UI交互

### 优先级3: 高级重构 (后续)
1. Extract Method
2. Rename Symbol
3. Move Class

---

## 💡 结论

**LSP Code Actions开发状态**: 
- ✅ **客户端调用**: 已完成（可以发送请求）
- ❌ **服务器端Handler**: 未实现（无法接收和处理请求）
- ❌ **VSCode集成**: 未实现（用户看不到Code Actions）
- ❌ **重构功能**: 未实现（只有基础Quick Fix）

**建议**: 立即启动Phase 1，优先实现LSP服务器端Handler，这是最关键的缺失环节。

---

**报告作者**: AI技术评估团队  
**最后更新**: 2026-05-22
