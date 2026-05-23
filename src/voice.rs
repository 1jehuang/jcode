//! # Voice — 语音模式（借鉴 Claude Code voice/ 目录）
//!
//! 支持语音输入和命令听写。可接入各平台语音识别引擎：
//! - Windows: 系统语音识别 API
//! - macOS: 系统语音识别
//! - Linux: Vosk / Whisper 本地引擎

use std::sync::Arc;
use tokio::sync::RwLock;

/// 语音识别引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEngine {
    /// 系统默认
    System,
    /// Whisper 本地
    Whisper,
}

/// 语音模式状态
#[derive(Debug, Clone)]
pub struct VoiceState {
    /// 是否正在监听
    pub is_listening: bool,
    /// 使用引擎
    pub engine: VoiceEngine,
    /// 唤醒词（可选）
    pub wake_word: Option<String>,
    /// 自动语言检测
    pub auto_lang_detect: bool,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            is_listening: false,
            engine: VoiceEngine::System,
            wake_word: None,
            auto_lang_detect: true,
        }
    }
}

/// 语音识别结果
#[derive(Debug, Clone)]
pub struct VoiceResult {
    pub text: String,
    pub confidence: f32,
    pub is_final: bool,
    pub language: Option<String>,
}

/// 语音模式管理器
pub struct VoiceManager {
    state: Arc<RwLock<VoiceState>>,
}

impl VoiceManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(VoiceState::default())),
        }
    }

    /// 开始语音监听
    pub async fn start_listening(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        if state.is_listening {
            return Err("Already listening".into());
        }
        state.is_listening = true;
        Ok(())
    }

    /// 停止语音监听
    pub async fn stop_listening(&self) {
        let mut state = self.state.write().await;
        state.is_listening = false;
    }

    /// 获取当前状态
    pub async fn get_state(&self) -> VoiceState {
        self.state.read().await.clone()
    }

    /// 设置唤醒词
    pub async fn set_wake_word(&self, word: Option<String>) {
        self.state.write().await.wake_word = word;
    }
}
