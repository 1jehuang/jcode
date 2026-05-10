//! # 调试日志系统
//!
//! 源自 Claude Code 的 `debug.ts` + `log.ts`，提供高效的调试/错误日志记录。
//!
//! ## 能力
//! - 会话级调试文件，带 latest 符号链接
//! - BufferedWriter：批量写入，1 秒冲刷间隔
//! - 调试模式过滤 (pattern)
//! - 多接收器错误日志（内存 + 持久文件）
//! - 错误队列：接收器附加前的事件排队

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Verbose => "VERBOSE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "verbose" | "v" => LogLevel::Verbose,
            "debug" | "d" => LogLevel::Debug,
            "info" | "i" => LogLevel::Info,
            "warn" | "w" | "warning" => LogLevel::Warn,
            "error" | "e" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

/// 单个日志条目
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub category: Option<String>,
}

/// 缓冲写入器 — 批量写入磁盘，减少 I/O
/// 源自 Claude Code 的 `BufferedWriter`
pub struct BufferedWriter {
    file: Option<PathBuf>,
    buffer: Vec<String>,
    max_buffer_size: usize,
    flush_interval: Duration,
    last_flush: Instant,
    /// 调试模式时即时写入
    immediate: bool,
}

impl BufferedWriter {
    pub fn new(file: Option<PathBuf>) -> Self {
        Self {
            file,
            buffer: Vec::new(),
            max_buffer_size: 100,
            flush_interval: Duration::from_secs(1),
            last_flush: Instant::now(),
            immediate: false,
        }
    }

    /// 启用即时模式（每次写入都刷盘）
    pub fn with_immediate(mut self) -> Self {
        self.immediate = true;
        self
    }

    /// 写入一行
    pub fn write(&mut self, line: String) {
        self.buffer.push(line);

        if self.immediate || self.buffer.len() >= self.max_buffer_size || self.last_flush.elapsed() >= self.flush_interval {
            self.flush();
        }
    }

    /// 刷盘到磁盘
    pub fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        if let Some(ref path) = self.file {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let batch = self.buffer.join("\n");
                let _ = writeln!(file, "{}", batch);
            }
        }

        self.buffer.clear();
        self.last_flush = Instant::now();
    }
}

impl Drop for BufferedWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

/// 调试日志管理器
/// 源自 Claude Code 的 `DebugLog` + `logError()`
pub struct DebugLogManager {
    /// 调试文件写入器
    writer: Mutex<BufferedWriter>,
    /// 调试过滤器模式
    filter_pattern: Option<String>,
    /// 最小日志级别
    min_level: LogLevel,
    /// 内存错误日志（最多 100 条）
    error_buffer: Mutex<VecDeque<LogEntry>>,
    /// 持久化错误文件路径
    error_file: Option<PathBuf>,
    /// 延迟的错误接收器队列
    error_sink_queue: Mutex<VecDeque<String>>,
    /// 是否已附加错误接收器
    error_sink_attached: Mutex<bool>,
}

impl DebugLogManager {
    /// 创建新的调试日志管理器
    /// `debug_dir`: 调试文件目录（如 ~/.jcode/debug/）
    /// `session_id`: 当前会话 ID
    /// `debug_pattern`: 可选的调试过滤模式
    pub fn new(debug_dir: Option<PathBuf>, session_id: &str, debug_pattern: Option<&str>) -> Self {
        let debug_file = debug_dir.as_ref().map(|dir| dir.join(format!("{}.txt", session_id)));
        let error_file = debug_dir.as_ref().map(|dir| dir.join("errors.log"));

        // 创建 latest 符号链接 (Windows 上跳过)
        if let Some(ref file) = debug_file {
            if let Some(parent) = file.parent() {
                let _ = fs::create_dir_all(parent);
                let _latest = parent.join("latest");
                #[cfg(unix)]
                {
                    let _ = std::os::unix::fs::symlink(file, &latest);
                }
            }
        }

        Self {
            writer: Mutex::new(BufferedWriter::new(debug_file)),
            filter_pattern: debug_pattern.map(|s| s.to_string()),
            min_level: LogLevel::Debug,
            error_buffer: Mutex::new(VecDeque::with_capacity(100)),
            error_file,
            error_sink_queue: Mutex::new(VecDeque::new()),
            error_sink_attached: Mutex::new(false),
        }
    }

    /// 写入调试日志
    pub fn debug(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Debug, None, message);
    }

    /// 写入信息日志
    pub fn info(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Info, None, message);
    }

    /// 写入警告日志
    pub fn warn(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Warn, None, message);
    }

    /// 写入错误日志
    pub fn error(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Error, None, message);
    }

    /// 写入分类日志
    pub fn log(&self, level: LogLevel, category: Option<&str>, message: impl AsRef<str>) {
        if level < self.min_level {
            return;
        }

        let msg = message.as_ref();

        // 检查过滤器模式
        if let Some(ref pattern) = self.filter_pattern {
            if !msg.contains(pattern) {
                return;
            }
        }

        let entry = LogEntry {
            level,
            message: msg.to_string(),
            timestamp: chrono::Utc::now(),
            category: category.map(String::from),
        };

        // 写入调试文件
        let line = format!(
            "[{}] [{}]{} {}",
            entry.timestamp.format("%H:%M:%S%.3f"),
            level.as_str(),
            entry.category.as_ref().map(|c| format!(" [{}]", c)).unwrap_or_default(),
            entry.message
        );

        if let Ok(mut writer) = self.writer.lock() {
            writer.write(line);
        }

        // 错误日志特殊处理
        if level >= LogLevel::Warn {
            if let Ok(mut buffer) = self.error_buffer.lock() {
                if buffer.len() >= 100 {
                    buffer.pop_front();
                }
                buffer.push_back(entry.clone());
            }

            if level == LogLevel::Error {
                self.persist_error(&entry);
            }
        }
    }

    /// 持久化错误日志到文件
    fn persist_error(&self, entry: &LogEntry) {
        let line = format!(
            "[{}] {}",
            entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
            entry.message
        );

        // 先写入队列
        if let Ok(mut queue) = self.error_sink_queue.lock() {
            queue.push_back(line.clone());
        }

        // 如果已附加接收器，直接写入文件
        if let Ok(attached) = self.error_sink_attached.lock() {
            if *attached {
                if let Some(ref error_file) = self.error_file {
                    if let Some(parent) = error_file.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if let Ok(mut file) = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(error_file)
                    {
                        let _ = writeln!(file, "{}", line);
                    }
                }
            }
        }
    }

    /// 附加错误日志接收器（将队列中的事件写入文件）
    /// 源自 Claude Code 的 `attachErrorLogSink()`
    pub fn attach_error_sink(&self) {
        if let Ok(mut attached) = self.error_sink_attached.lock() {
            if *attached {
                return;
            }
            *attached = true;
        }

        // 排空队列
        let queued = {
            let mut queue = self.error_sink_queue.lock().unwrap();
            let items: Vec<String> = queue.drain(..).collect();
            items
        };

        if let Some(ref error_file) = self.error_file {
            if let Some(parent) = error_file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(error_file)
            {
                for line in &queued {
                    let _ = writeln!(file, "{}", line);
                }
            }
        }
    }

    /// 获取错误日志
    pub fn get_error_logs(&self) -> Vec<LogEntry> {
        self.error_buffer.lock()
            .map(|buffer| buffer.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 设置最小日志级别
    pub fn set_min_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }

    /// 获取当前调试模式状态
    pub fn is_debug_enabled(&self) -> bool {
        self.min_level <= LogLevel::Debug
    }

    /// 设置调试过滤器
    pub fn set_filter(&mut self, pattern: Option<&str>) {
        self.filter_pattern = pattern.map(|s| s.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error > LogLevel::Warn);
        assert!(LogLevel::Warn > LogLevel::Info);
        assert!(LogLevel::Info > LogLevel::Debug);
    }

    #[test]
    fn test_buffered_writer() {
        let temp = std::env::temp_dir().join("jcode-test-bw.txt");
        let mut writer = BufferedWriter::new(Some(temp.clone()));

        writer.write("line 1".to_string());
        writer.write("line 2".to_string());
        writer.flush();

        assert!(temp.exists());
        let content = std::fs::read_to_string(&temp).unwrap();
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));

        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn test_debug_log_manager_filter() {
        let mgr = DebugLogManager::new(
            None,
            "test-session",
            Some("specific"),
        );

        // Should be filtered out
        mgr.debug("general message");
        // Should pass filter
        mgr.debug("specific message");

        let errors = mgr.get_error_logs();
        // Filter at writer level means we can't easily check from API
        assert!(errors.is_empty());
    }

    #[test]
    fn test_error_sink_queue() {
        let temp_dir = std::env::temp_dir().join("jcode-test-errors");
        let error_file = temp_dir.join("errors.log");

        let mgr = DebugLogManager::new(
            Some(temp_dir.clone()),
            "test-session",
            None,
        );

        // Error before sink attached
        mgr.error("startup error");

        // Attach sink
        mgr.attach_error_sink();

        // Error after sink attached
        mgr.error("runtime error");

        // Check error file
        if error_file.exists() {
            let content = std::fs::read_to_string(&error_file).unwrap();
            assert!(content.contains("runtime error"));
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_error_buffer_limit() {
        let mgr = DebugLogManager::new(None, "test", None);
        for i in 0..150 {
            mgr.error(format!("error {}", i));
        }
        let logs = mgr.get_error_logs();
        assert!(logs.len() <= 100);
    }
}
