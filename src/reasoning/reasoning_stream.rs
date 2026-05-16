//! Reasoning Content 实时回传流
//!
//! ## 为什么需要实时回传?
//!
//! **传统模式的局限性:**
//! - ❌ 推理过程不透明，用户只能看到最终结果
//! - ❌ 无法了解AI的思考路径和决策依据
//! - ❌ 长时间推理时用户不知道是否在工作
//! - ❌ 调试困难，无法定位推理错误
//!
//! **实时回传的优势:**
//! - ✅ 完全透明的思维过程
//! - ✅ 用户可以看到每一步推理
//! - ✅ 支持中断和引导推理方向
//! - ✅ 便于调试和优化推理策略
//! - ✅ 提升用户体验和信任度
//!
//! ## 架构设计
//!
//! ```
//! +-----------------------------------------------------+
//! |           Reasoning Stream (实时回传)               |
//! +-----------------------------------------------------+
//!                                                     |
//!  CoT Engine                                         |
//!    v                                                |
//!  +-----------------+                              |
//!  | Event Emitter   | <- 生成推理事件                |
//!  +--------+--------+                              |
//!           v                                       |
//!  +-----------------+                              |
//!  | Event Bus       | <- 事件广播                   |
//!  +--------+--------+                              |
//!           v                                       |
//!  +-----------------+     +------------------+     |
//!  | Listener 1      |     | Listener 2       |     |
//!  | (UI Display)    |     | (Log/Debug)      |     |
//!  +-----------------+     +------------------+     |
//!           v                       v                |
//!  +-----------------+     +------------------+     |
//!  | WebSocket       |     | File Logger      |     |
//!  | (Real-time)     |     | (Persistence)    |     |
//!  +-----------------+     +------------------+     |
//!                                                     |
//!  事件类型:                                          |
//!  +-- ReasoningStarted                               |
//!  +-- StepStarted                                    |
//!  +-- StepReasoning (核心: 思维内容)                 |
//!  +-- StepCompleted                                  |
//!  +-- SelfReflection                                 |
//!  +-- CorrectionApplied                              |
//!  +-- ReasoningCompleted                             |
//!                                                     |
//! +-----------------------------------------------------+
//! ```

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info};

// --- Types ---------------------------------

/// 推理事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReasoningEventType {
    /// 推理开始
    Started,
    
    /// 步骤开始
    StepStarted,
    
    /// 步骤推理中 (核心事件: 包含思维内容)
    StepReasoning,
    
    /// 步骤完成
    StepCompleted,
    
    /// 自我反思
    SelfReflection,
    
    /// 纠正应用
    CorrectionApplied,
    
    /// 推理完成
    Completed,
    
    /// 错误发生
    Error,
}

impl std::fmt::Display for ReasoningEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started => write!(f, "🚀 推理开始"),
            Self::StepStarted => write!(f, "📝 步骤开始"),
            Self::StepReasoning => write!(f, "🧠 思考中"),
            Self::StepCompleted => write!(f, "✅ 步骤完成"),
            Self::SelfReflection => write!(f, "🪞 自我反思"),
            Self::CorrectionApplied => write!(f, "🔧 纠正应用"),
            Self::Completed => write!(f, "🎯 推理完成"),
            Self::Error => write!(f, "❌ 错误"),
        }
    }
}

/// 推理事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningEvent {
    /// 事件ID
    pub event_id: u64,
    
    /// 事件类型
    pub event_type: ReasoningEventType,
    
    #[serde(skip)]
    pub timestamp: std::time::Instant,
    
    /// 步骤序号 (如果是步骤相关事件)
    pub step_number: Option<usize>,
    
    /// 步骤类型
    pub step_type: Option<super::cot_engine::StepType>,
    
    /// 核心内容: 推理思维文本 (这是最重要的字段!)
    pub content: String,
    
    /// 附加数据
    pub metadata: serde_json::Value,
    
    /// 进度百分比 (0.0-1.0)
    pub progress: f64,
    
    /// 置信度
    pub confidence: Option<f64>,
}

impl ReasoningEvent {
    /// 创建新事件
    pub fn new(
        event_type: ReasoningEventType,
        content: &str,
        progress: f64,
    ) -> Self {
        static EVENT_COUNTER: std::sync::atomic::AtomicU64 = 
            std::sync::atomic::AtomicU64::new(1);
        
        Self {
            event_id: EVENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            event_type,
            timestamp: std::time::Instant::now(),
            step_number: None,
            step_type: None,
            content: content.to_string(),
            metadata: serde_json::json!({}),
            progress,
            confidence: None,
        }
    }
    
    /// 设置步骤信息
    pub fn with_step(mut self, step_number: usize, step_type: super::cot_engine::StepType) -> Self {
        self.step_number = Some(step_number);
        self.step_type = Some(step_type);
        self
    }
    
    /// 设置置信度
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }
    
    /// 添加元数据
    pub fn with_metadata(mut self, key: &str, value: serde_json::Value) -> Self {
        self.metadata[key] = value;
        self
    }
    
    /// 转换为可显示的格式 (用于UI)
    pub fn to_display_format(&self) -> String {
        let mut output = String::new();
        
        // 时间戳
        let elapsed = self.timestamp.elapsed().as_millis();
        output.push_str(&format!("[{:>5}ms] ", elapsed));
        
        // 进度条
        let bar_width = 20;
        let filled = (self.progress * bar_width as f64) as usize;
        let empty = bar_width - filled;
        output.push_str(&format!(
            "[{}{}] ",
            "█".repeat(filled),
            "░".repeat(empty)
        ));
        
        // 事件类型
        output.push_str(&format!("{} ", self.event_type));
        
        // 步骤信息
        if let (Some(num), Some(stype)) = (&self.step_number, &self.step_type) {
            output.push_str(&format!("(步骤{}-{}) ", num, stype));
        }
        
        // 核心内容 (截断显示)
        let display_content = if self.content.len() > 100 {
            format!("{}...", &self.content[..100])
        } else {
            self.content.clone()
        };
        output.push_str(&display_content);
        
        // 置信度
        if let Some(conf) = self.confidence {
            output.push_str(&format!(" [置信度: {:.1}%]", conf * 100.0));
        }
        
        output
    }
}

/// 事件监听器 trait
#[async_trait]
pub trait ReasoningEventListener: Send + Sync {
    /// 处理推理事件
    async fn on_event(&self, event: &ReasoningEvent);
    
    /// 监听器名称
    fn name(&self) -> &str;
}

// --- Core Stream ------------------------------

/// 推理流 (事件总线)
pub struct ReasoningStream {
    /// 事件发送器
    sender: broadcast::Sender<ReasoningEvent>,
    
    /// 监听器列表
    listeners: Arc<RwLock<Vec<Arc<dyn ReasoningEventListener>>>>,
    
    /// 是否启用
    enabled: Arc<std::sync::atomic::AtomicBool>,
    
    /// 历史记录 (最近N个事件)
    history: Arc<RwLock<Vec<ReasoningEvent>>>,
    
    /// 最大历史记录数
    max_history: usize,
}

impl Default for ReasoningStream {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            listeners: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            history: Arc::new(RwLock::new(Vec::new())),
            max_history: 100,
        }
    }
}

impl ReasoningStream {
    /// 创建新的推理流
    pub fn new() -> Self {
        Self::default()
    }
    
    /// 启用/禁用流
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
    
    /// 检查是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }
    
    /// 发送事件
    pub async fn emit(&self, event: ReasoningEvent) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        
        debug!(
            event_id = event.event_id,
            event_type = ?event.event_type,
            progress = event.progress,
            "Emitting reasoning event"
        );
        
        // 广播给所有订阅者
        let _ = self.sender.send(event.clone());
        
        // 通知所有监听器
        {
            let listeners = self.listeners.read().await;
            for listener in listeners.iter() {
                listener.on_event(&event).await;
            }
        }
        
        // 记录历史
        {
            let mut history = self.history.write().await;
            history.push(event);
            
            // 保持历史记录在限制内
            if history.len() > self.max_history {
                history.remove(0);
            }
        }
        
        Ok(())
    }
    
    /// 订阅事件流
    pub fn subscribe(&self) -> broadcast::Receiver<ReasoningEvent> {
        self.sender.subscribe()
    }
    
    /// 添加监听器
    pub async fn add_listener(&self, listener: Arc<dyn ReasoningEventListener>) {
        let mut listeners = self.listeners.write().await;
        listeners.push(listener);
        info!(
            listener_name = listener.name(),
            total_listeners = listeners.len(),
            "Listener added"
        );
    }
    
    /// 移除监听器
    pub async fn remove_listener(&self, name: &str) {
        let mut listeners = self.listeners.write().await;
        listeners.retain(|l| l.name() != name);
    }
    
    /// 获取历史记录
    pub async fn get_history(&self) -> Vec<ReasoningEvent> {
        self.history.read().await.clone()
    }
    
    /// 清空历史记录
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }
    
    /// 获取所有事件的完整思维链 (用于导出)
    pub async fn get_full_reasoning_chain(&self) -> String {
        let history = self.history.read().await;
        
        let mut chain = String::new();
        chain.push_str("# 🧠 完整推理链\n\n");
        
        for event in history.iter() {
            chain.push_str(&format!("## {} {}\n\n", event.event_type, event.event_id));
            
            if let (Some(num), Some(stype)) = (&event.step_number, &event.step_type) {
                chain.push_str(&format!("**步骤 {}: {}**\n\n", num, stype));
            }
            
            chain.push_str(&format!("{}\n\n", event.content));
            
            if let Some(conf) = event.confidence {
                chain.push_str(&format!("*置信度: {:.1}%*\n\n", conf));
            }
            
            chain.push_str("---\n\n");
        }
        
        chain
    }
}

// --- Built-in Listeners ----------------------

/// 控制台输出监听器 (用于调试)
pub struct ConsoleListener;

#[async_trait]
impl ReasoningEventListener for ConsoleListener {
    async fn on_event(&self, event: &ReasoningEvent) {
        println!("{}", event.to_display_format());
    }
    
    fn name(&self) -> &str {
        "console"
    }
}

/// 日志监听器 (写入文件)
pub struct LogListener {
    file_path: std::path::PathBuf,
}

impl LogListener {
    pub fn new(file_path: &str) -> Self {
        Self {
            file_path: std::path::PathBuf::from(file_path),
        }
    }
}

#[async_trait]
impl ReasoningEventListener for LogListener {
    async fn on_event(&self, event: &ReasoningEvent) {
        use tokio::io::AsyncWriteExt;
        
        let log_line = format!("{}\n", event.to_display_format());
        
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
            .await
        {
            let _ = file.write_all(log_line.as_bytes()).await;
        }
    }
    
    fn name(&self) -> &str {
        "log"
    }
}

/// WebSocket推送监听器 (用于前端实时展示)
pub struct WebSocketListener {
    sender: Arc<broadcast::Sender<ReasoningEvent>>,
}

impl WebSocketListener {
    pub fn new(sender: broadcast::Sender<ReasoningEvent>) -> Self {
        Self {
            sender: Arc::new(sender),
        }
    }
}

#[async_trait]
impl ReasoningEventListener for WebSocketListener {
    async fn on_event(&self, event: &ReasoningEvent) {
        let _ = self.sender.send(event.clone());
    }
    
    fn name(&self) -> &str {
        "websocket"
    }
}

// --- Tests ----------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_reasoning_stream_basic() {
        let stream = ReasoningStream::new();
        
        // 订阅
        let mut rx = stream.subscribe();
        
        // 发送事件
        let event = ReasoningEvent::new(
            ReasoningEventType::Started,
            "开始推理",
            0.0,
        );
        
        stream.emit(event).await.unwrap();
        
        // 接收事件
        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, ReasoningEventType::Started);
        assert_eq!(received.content, "开始推理");
        
        println!("✅ 基本流测试通过");
    }
    
    #[tokio::test]
    async fn test_reasoning_chain_export() {
        let stream = ReasoningStream::new();
        
        // 发送一系列事件
        stream.emit(
            ReasoningEvent::new(ReasoningEventType::Started, "开始分析问题", 0.0)
        ).await.unwrap();
        
        stream.emit(
            ReasoningEvent::new(
                ReasoningEventType::StepReasoning,
                "让我先理解问题的核心...",
                0.3,
            ).with_step(1, super::super::cot_engine::StepType::Understanding)
             .with_confidence(0.9)
        ).await.unwrap();
        
        stream.emit(
            ReasoningEvent::new(
                ReasoningEventType::StepReasoning,
                "问题涉及三个方面: A, B, C",
                0.6,
            ).with_step(2, super::super::cot_engine::StepType::LogicalDeduction)
        ).await.unwrap();
        
        stream.emit(
            ReasoningEvent::new(ReasoningEventType::Completed, "推理完成", 1.0)
        ).await.unwrap();
        
        // 导出完整思维链
        let chain = stream.get_full_reasoning_chain().await;
        
        assert!(chain.contains("完整推理链"));
        assert!(chain.contains("开始分析问题"));
        assert!(chain.contains("让我先理解问题的核心"));
        assert!(chain.contains("推理完成"));
        
        println!("📝 导出的思维链:\n{}", chain);
        println!("✅ 思维链导出测试通过");
    }
    
    #[tokio::test]
    async fn test_console_listener() {
        let stream = ReasoningStream::new();
        
        // 添加控制台监听器
        stream.add_listener(Arc::new(ConsoleListener)).await;
        
        // 发送事件 (应该打印到控制台)
        println!("--- 控制台输出开始 ---");
        stream.emit(
            ReasoningEvent::new(
                ReasoningEventType::StepReasoning,
                "这是一个测试推理步骤",
                0.5,
            ).with_step(1, super::super::cot_engine::StepType::Calculation)
        ).await.unwrap();
        println!("--- 控制台输出结束 ---");
        
        println!("✅ 控制台监听器测试通过");
    }
}
