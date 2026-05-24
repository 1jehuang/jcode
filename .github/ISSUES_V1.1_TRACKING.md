# GitHub Issues for v1.1.0 IDE Plugins

Created: 2026-05-24
Plan: docs/V1.1_IDE_PLUGINS_PLAN.md

---

## SDK Enhancement (solo-Turbo)

### Issue #1: wasm-bindgen Setup
**Title**: [v1.1.0] SDK: Add wasm-bindgen support for browser/VSCode
**Labels**: enhancement, sdk, wasm
**Assignee**: solo-Turbo
**Body**:
```
## Task
Add wasm-bindgen to carpai-sdk for compiling to WebAssembly.

## Acceptance Criteria
- [ ] Add wasm-bindgen dependency to Cargo.toml
- [ ] Create wasm/ module with JS bindings
- [ ] Build pipeline for .wasm output
- [ ] Test in browser environment

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 4.2
```

### Issue #2: TypeScript Bindings
**Title**: [v1.1.0] SDK: Generate TypeScript type definitions
**Labels**: enhancement, sdk, typescript
**Assignee**: solo-Turbo
**Body**:
```
## Task
Auto-generate .d.ts files from Rust types for VSCode extension.

## Acceptance Criteria
- [ ] ts-bindgen or manual .d.ts for all public types
- [ ] NPM package structure (@carpai/sdk)
- [ ] Publish to npm registry

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 4.3
```

### Issue #3: Kotlin gRPC Client
**Title**: [v1.1.0] SDK: Generate Kotlin gRPC stubs
**Labels**: enhancement, sdk, kotlin, grpc
**Assignee**: solo-Turbo
**Body**:
```
## Task
Generate Kotlin client stubs from .proto files for JetBrains plugin.

## Acceptance Criteria
- [ ] protoc + grpc-kotlin setup in build.gradle.kts
- [ ] Generated stubs match server proto definitions
- [ ] Maven package (com.carpai:sdk)

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 4.3
```

---

## VSCode Extension (Paw-brave)

### Issue #4: VSCode Project Scaffold
**Title**: [v1.1.0] VSCode: Create extension project structure
**Labels**: enhancement, vscode, v1.1.0
**Assignee**: Paw-brave
**Body**:
```
## Task
Create extensions/vscode/ directory with full TypeScript + React scaffold.

## Acceptance Criteria
- [ ] package.json with commands registered
- [ ] tsconfig.json configured
- [ ] src/extension.ts entry point
- [ ] webview/ React app with Vite/esbuild
- [ ] vsce packaging works

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 2.2
```

### Issue #5: Sidebar Chat Panel
**Title**: [v1.1.0] VSCode: Implement sidebar chat webview
**Labels**: enhancement, vscode, ui
**Assignee**: Paw-brave
**Body**:
```
## Task
Implement ChatViewProvider with React webview for sidebar chat.

## Acceptance Criteria
- [ ] Webview renders chat UI
- [ ] Message send/receive works
- [ ] Streaming responses display correctly
- [ ] Session persistence

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 2.3
```

### Issue #6: Code Actions
**Title**: [v1.1.0] VSCode: Implement explain/refactor/fix commands
**Labels**: enhancement, vscode, commands
**Assignee**: Paw-brave
**Body**:
```
## Task
Implement right-click context menu actions for code operations.

## Acceptance Criteria
- [ ] "Explain Code" shows explanation in chat
- [ ] "Refactor Selection" shows diff preview
- [ ] "Fix Bug" applies fix inline
- [ ] All commands use carpai-sdk

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 2.3
```

---

## JetBrains Plugin (ma-guoyang)

### Issue #7: JetBrains Project Scaffold
**Title**: [v1.1.0] JetBrains: Create plugin project structure
**Labels**: enhancement, jetbrains, v1.1.0
**Assignee**: ma-guoyang
**Body**:
```
## Task
Create plugins/jetbrains/ directory with Kotlin + Gradle scaffold.

## Acceptance Criteria
- [ ] build.gradle.kts with IntelliJ plugin
- [ ] plugin.xml configured
- [ ] src/main/kotlin package structure
- [ ] ./gradlew buildPlugin works

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 3.2
```

### Issue #8: Chat Tool Window
**Title**: [v1.1.0] JetBrains: Implement chat tool window
**Labels**: enhancement, jetbrains, ui
**Assignee**: ma-guoyang
**Body**:
```
## Task
Implement ChatToolWindow with Swing UI for right-side panel.

## Acceptance Criteria
- [ ] Tool window factory registered
- [ ] Chat panel with message list
- [ ] Input box with send button
- [ ] gRPC communication with server

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 3.3
```

### Issue #9: Editor Actions
**Title**: [v1.1.0] JetBrains: Implement editor context menu actions
**Labels**: enhancement, jetbrains, actions
**Assignee**: ma-guoyang
**Body**:
```
## Task
Implement right-click and Alt+Enter actions for code operations.

## Acceptance Criteria
- [ ] Right-click menu "Explain with CarpAI"
- [ ] Alt+Enter intention "Fix with CarpAI"
- [ ] Diff preview for refactoring
- [ ] All actions use gRPC client

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md Section 3.4
```

---

## Release

### Issue #10: v1.1.0 Release
**Title**: [Release] Prepare v1.1.0 release
**Labels**: release
**Assignee**: solo-Turbo
**Body**:
```
## Task
Prepare v1.1.0 release with IDE plugins.

## Checklist
- [ ] VSCode .vsix published to Marketplace
- [ ] JetBrains .zip published to Plugin Repository
- [ ] SDK NPM package published
- [ ] SDK Maven package published
- [ ] RELEASE_NOTES.md updated
- [ ] git tag v1.1.0

## References
- Plan: docs/V1.1_IDE_PLUGINS_PLAN.md
```
