//! # Internationalization (i18n) - 国际化框架
//!
//! 提供完整的多语言支持能力，包括：
//! - **语言管理** - 支持20+主流语言
//! - **翻译系统** - 键值对翻译映射
//! - **插值支持** - 动态变量替换
//! - **复数形式** - 智能单复数处理
//! - **RTL支持** - 从右到左语言布局
//!
//! ## 支持的语言
//!
//! | 代码 | 语言 | 方向 |
//! |------|------|------|
//! | en | English (英语) | LTR |
//! | zh-CN | 简体中文 | LTR |
//! | zh-TW | 繁體中文 | LTR |
//! | ja | 日本語 | LTR |
//! | ko | 한국어 | LTR |
//! | es | Español | LTR |
//! | fr | Français | LTR |
//! | de | Deutsch | LTR |
//! | ru | Русский | LTR |
//! | ar | العربية | RTL |

pub mod locale;
pub mod translator;
pub mod translations;

pub use locale::{Locale, LanguageDirection};
pub use translator::Translator;