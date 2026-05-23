# 编译错误修复进度

**目标**: 从 ~244 错误到 0

---

## 已修复 (分层分类)

### 全局层 (6个)
| 错误 | 文件 | 修复 |
|------|------|------|
| edition = "2024" | `carpai-codebase/Cargo.toml` | → "2021" |
| edition = "2024" | `jcode-unified-scheduler/Cargo.toml` | → "2021" |
| `Option<Tree>` vs `Result` | `carpai-codebase/parser.rs:44` | `Ok(Some(tree))` → `Some(tree)` |
| `OwnedValue` 类型无法解析 | `carpai-codebase/indexer.rs:103,107` | 移除冗余类型标注 |
| `Copy` on enum with String | `jcode-cpu-inference/graceful_manager.rs` | 移除 `Copy` derive |

### 模块接口层 (7个)
| 错误 | 文件 | 修复 |
|------|------|------|
| 缺少 `pub mod ide;` | `carpai-sdk/lib.rs` | 添加声明 |
| 缺少 `pub mod protocol;` | `carpai-sdk/lib.rs` | 添加声明 |
| `SessionId` 类型缺失 | `jcode-session-persist/types.rs` | 添加 `pub struct SessionId(pub String)` |

### 局部层 (3个)
| 错误 | 文件 | 修复 |
|------|------|------|
| `suffix` / `line_content` 字段不存在 | `jcode-completion/streaming_prefetch.rs:356-357` | 改用正确的 `CompletionContext` 字段 |

---

## 剩余 (~228个 → 待按相同方法修复)

下一步: 逐 crate 修复 jcode-session-persist (约30个) + jcode-unified-scheduler (约45个) + jcode-cpu-inference (约6个)
