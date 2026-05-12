# 🏗️ CarpAI IDE 集成架构设计文档

## 核心问题：服务端 vs 客户端职责划分

### ❌ 错误做法：服务端做 VS Code 插件

**问题**:
- 服务端（jcode-grpc）是 **后端服务**，不应该包含前端 UI 代码
- VS Code 扩展属于 **客户端层**，应该独立于后端
- 混合会导致：
  - 职责不清
  - 难以维护
  - 无法支持多 IDE（VS Code, JetBrains, Neovim, Emacs）
  - 违反关注点分离原则

### ✅ 正确做法：三层架构

```
┌─────────────────────────────────────────────────────┐
│                   用户界面层 (UI Layer)               │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │ VS Code   │ │JetBrains │ │Neovim    │ │Emacs   │ │
│  │ Extension │ │ Plugin   │ │Plugin    │ │Plugin  │ │
│  └─────┬────┘ └─────┬────┘ └─────┬────┘ └───┬────┘ │
│        │            │           │          │       │
├────────┴────────────┴───────────┴──────────┴───────┤
│              CarpAI Client SDK (统一接口)             │
│         - 统一 API 调用                              │
│         - 缓存管理                                  │
│         - 离线模式                                  │
│         - 配置同步                                  │
├─────────────────────────────────────────────────────┤
│                 协议层 (Protocol Layer)               │
│     gRPC / REST / WebSocket / SSE                  │
├─────────────────────────────────────────────────────┤
│                CarpAI Server (服务端)                │
│   - LLM Provider 抽象                               │
│   - RAG 系统                                        │
│   - 流式传输                                        │
│   - 多模态处理                                      │
│   - Agent 工作流引擎                                 │
└─────────────────────────────────────────────────────┘
```

## 🎯 推荐架构：CarpAI 作为"智能客户端"

### 方案 A: **CarpAI Desktop App** (推荐 ⭐⭐⭐⭐⭐)

```
CarpAI Desktop (Electron/Tauri)
├── 内置轻量级 Web Server (本地)
├── 统一的 AI 功能封装
├── 多 IDE 插件通过本地 API 通信
└── 独立运行，不依赖特定 IDE
```

**优势**:
- ✅ **真正的跨平台** - 一个应用服务所有 IDE
- ✅ **统一体验** - 无论用哪个 IDE，CarpAI 行为一致
- ✅ **离线能力** - 可以缓存模型和上下文
- ✅ **易于分发** - 单个安装包（像 Cursor 一样）
- ✅ **独立更新** - 不依赖 IDE 更新周期

**架构示例**:

```typescript
// carpai-client/src/core/CarpAIClient.ts

export class CarpAIClient {
  private server: LocalServer;
  private cache: ResponseCache;
  private config: ConfigManager;
  
  constructor() {
    // 启动本地服务器 (端口随机或配置)
    this.server = new LocalServer({
      port: 0, // 自动分配
      host: '127.0.0.1'
    });
    
    // 初始化缓存
    this.cache = new ResponseCache({
      maxSize: 100MB,
      ttl: 5min
    });
    
    // 加载配置
    this.config = new ConfigManager();
  }
  
  async initialize(): Promise<void> {
    await this.server.start();
    await this.config.load();
    
    // 连接到后端 (远程或本地)
    if (this.config.get('useLocalModel')) {
      await this.connectToLocalProvider();
    } else {
      await this.connectToCloudProvider();
    }
  }
  
  // 统一 API - 所有 IDE 插件调用这些方法
  async getInlineCompletion(
    context: EditorContext,
    position: Position
  ): Promise<CompletionResult> {
    // 1. 检查缓存
    const cacheKey = this.generateCacheKey(context, position);
    const cached = await this.cache.get(cacheKey);
    if (cached) return cached;
    
    // 2. 构建请求
    const request = this.buildCompletionRequest(context, position);
    
    // 3. 发送到后端 (gRPC/REST)
    const response = await this.server.call('GetCompletion', request);
    
    // 4. 缓存结果
    await this.cache.set(cacheKey, response);
    
    return response;
  }
  
  async chat(message: string, context?: ChatContext): Promise<AsyncIterable<ChatChunk>> {
    // 流式聊天实现
    return this.server.stream('Chat', { message, context });
  }
  
  async explainCode(code: string, language: string): Promise<Explanation> {
    return this.server.call('ExplainCode', { code, language });
  }
}
```

### 方案 B: **纯 SDK 模式** (轻量级)

```rust
// carpai-sdk/src/lib.rs

pub struct CarpAiSdk {
    client: reqwest::Client,
    config: SdkConfig,
    cache: Arc<RwLock<LruCache>>,
}

impl CarpAiSdk {
    pub fn new(config: SdkConfig) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
            config,
            cache: Arc::new(RwLock::new(LruCache::new(1000))),
        }
    }
    
    /// 所有 IDE 插件只需调用这个 SDK
    pub async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        // 缓存检查
        let key = format!("{:?}", request);
        if let Some(cached) = self.cache.read().get(&key) {
            return Ok(cached.clone());
        }
        
        // 调用后端
        let url = format!("{}/v1/completions", self.config.server_url);
        let response = self.client.post(&url)
            .json(&request)
            .send()
            .await?
            .json::<CompletionResponse>()
            .await?;
        
        // 缓存结果
        self.cache.write().put(key, response.clone());
        
        Ok(response)
    }
}
```

## 📋 各层职责明确划分

### 1️⃣ **CarpAI Server** (服务端 - 我们已实现的)

**职责**:
- ✅ LLM Provider 管理 (Deepseek/vLLM/llama.cpp)
- ✅ RAG 系统 (代码库索引、检索)
- ✅ 流式传输 (SSE/gRPC Stream)
- ✅ 多模态处理 (图像/音频/视频)
- ✅ Agent 工作流引擎
- ✅ 分布式协调
- ✅ 性能优化 (QUIC/GPU/边缘节点)

**不应该做**:
- ❌ VS Code 扩展 UI
- ❌ 编辑器集成逻辑
- ❌ 前端渲染
- ❌ IDE 特定的快捷键绑定

### 2️⃣ **CarpAI Client SDK** (中间层 - 新增)

**职责**:
- ✅ 统一 API 封装 (对上层透明)
- ✅ 缓存管理 (响应缓存、上下文缓存)
- ✅ 离线模式 (本地队列、重试机制)
- ✅ 配置管理 (统一配置格式)
- ✅ 错误处理 (重试、降级、fallback)
- ✅ 性能监控 (延迟统计、成功率)
- ✅ 多协议适配 (gRPC/REST/WebSocket)

**文件结构**:
```
carpai-sdk/
├── src/
│   ├── lib.rs              # SDK 入口
│   ├── client.rs           # HTTP/gRPC 客户端
│   ├── cache.rs            # LRU 缓存实现
│   ├── config.rs           # 配置管理器
│   ├── streaming.rs        # 流式传输抽象
│   ├── error.rs            # 统一错误类型
│   └── types.rs            # 共享类型定义
├── Cargo.toml
└── README.md
```

### 3️⃣ **IDE Plugins** (客户端 - 各 IDE 独立开发)

#### VS Code Extension
```typescript
// vscode-extension/src/extension.ts

import { CarpAiSdk } from 'carpai-sdk';

export function activate(context: vscode.ExtensionContext) {
    // 初始化 SDK
    const sdk = new CarpAiSdk({
        serverUrl: 'http://localhost:50051', // 或远程地址
        cacheEnabled: true,
        offlineMode: false,
    });
    
    // 注册 Inline Completion Provider
    context.subscriptions.push(
        vscode.languages.registerInlineCompletionItemProvider(
            { pattern: '**' }, // 所有语言
            new CarpAiInlineProvider(sdk),
            { allLanguages: true }
        )
    );
    
    // 注册 Chat Panel
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            'carpai.chat',
            new CarpAiChatPanel(sdk)
        )
    );
    
    // 注册 Commands
    registerCommands(sdk, context);
}

class CarpAiInlineProvider implements vscode.InlineCompletionItemProvider {
    constructor(private sdk: CarpAiSdk) {}
    
    async provideInlineCompletionItems(
        document: vscode.TextDocument,
        position: vscode.Position,
        context: vscode.InlineCompletionContext,
        token: vscode.CancellationToken
    ): Promise<vscode.InlineCompletionList> {
        
        // 获取编辑器上下文
        const editorContext = this.buildEditorContext(document, position);
        
        // 调用 SDK (自动处理缓存、错误等)
        const result = await this.sdk.complete({
            code: editorContext.code,
            cursorPosition: editorContext.position,
            language: document.languageId,
            filePath: document.uri.fsPath,
        });
        
        // 返回补全项
        return new vscode.InlineCompletionList([
            new vscode.InlineCompletionItem(result.completion, {
                range: new vscode.Range(position, position)
            })
        ]);
    }
}
```

#### JetBrains Plugin (Kotlin)
```kotlin
// intellij-plugin/src/main/kotlin/com/carpai/CarpAiCompletionContributor.kt

class CarpAiCompletionContributor : CompletionContributor() {
    
    private val sdk = CarpAiSdk.builder()
        .serverUrl("http://localhost:50051")
        .build()
    
    init {
        extend(
            CompletionType.BASIC,
            PlatformPatterns.psiElement(),
            CarpAiCompletionProvider(sdk)
        )
    }
}

class CarpAiCompletionProvider(private val sdk: CarpAiSdk) : CompletionProvider<CompletionParameters>() {
    
    override fun addCompletions(
        parameters: CompletionParameters,
        context: ProcessingContext,
        resultSet: CompletionResultSet
    ) {
        val editor = parameters.editor
        val document = editor.document
        
        val result = sdk.complete(CompletionRequest(
            code = document.text,
            position = editor.caretModel.offset,
            language = file.language.displayName,
            filePath = file.path
        ))
        
        resultSet.addElement(LookupElementBuilder
            .create(result.completion)
            .withIcon(CarpaiIcons.AI))
    }
}
```

## 🚀 推荐实施路线图

### Phase 1: 基础 SDK (1-2 周)
```
✅ 实现 carpai-sdk crate
✅ 统一 API 定义
✅ 缓存和错误处理
✅ 基础文档
```

### Phase 2: VS Code 插件 (1 周)
```
✅ 基于 SDK 的 VS Code 扩展
✅ Inline Completion
✅ Chat Panel
✅ 基本命令
``]

### Phase 3: CarpAI Desktop (2-3 周)
```
🔄 Electron/Tauri 应用
🔄 内置本地服务器
🔄 设置向导
🔄 系统托盘图标
```

### Phase 4: 多 IDE 支持 (持续)
```
📋 JetBrains Plugin
📋 Neovim Plugin
📋 Emacs Package
📋 Vim Plugin
```

## 💡 关键结论

**你的直觉完全正确！**

❌ **不要在服务端做 VS Code 插件**
✅ **应该这样做**:

1. **服务端** (jcode-grpc): 保持纯粹的后端服务
2. **SDK 层** (carpai-sdk): 新建一个 crate，提供统一的客户端 API
3. **IDE 插件**: 各自独立，都依赖 SDK

这样做的优势:
- ✅ **解耦**: 后端改动不影响前端
- ✅ **复用**: 一个 SDK 服务所有 IDE
- ✅ **灵活**: 可以随时添加新的 IDE 支持
- ✅ **专业**: 每个插件可以针对该 IDE 的特性优化
- ✅ **可测试**: SDK 可以独立单元测试

---

*下一步*: 我建议先创建 `carpai-sdk` crate，然后基于它重构 VS Code 扩展。你觉得这个方案如何？*
