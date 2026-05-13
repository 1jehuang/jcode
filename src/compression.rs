use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: u64,
    pub token_count: usize,
    pub is_compressed: bool,
    pub original_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionStrategy {
    LengthBased,
    SemanticBased,
    TimeBased,
    Hybrid,
}

#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub strategy: CompressionStrategy,
    pub max_tokens: usize,
    pub min_tokens: usize,
    pub compression_ratio: f64,
    pub keep_recent_count: usize,
    pub time_threshold_hours: u64,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            strategy: CompressionStrategy::Hybrid,
            max_tokens: 8192,
            min_tokens: 1024,
            compression_ratio: 0.5,
            keep_recent_count: 5,
            time_threshold_hours: 24,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConversationCompressor {
    config: CompressionConfig,
    cache: HashMap<String, Vec<Message>>,
}

impl Default for ConversationCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationCompressor {
    pub fn new() -> Self {
        Self {
            config: CompressionConfig::default(),
            cache: HashMap::new(),
        }
    }

    pub fn with_config(config: CompressionConfig) -> Self {
        Self {
            config,
            cache: HashMap::new(),
        }
    }

    pub fn compress(&mut self, session_id: &str, messages: &[Message]) -> Result<Vec<Message>> {
        let total_tokens: usize = messages.iter().map(|m| m.token_count).sum();
        
        if total_tokens <= self.config.max_tokens {
            return Ok(messages.to_vec());
        }

        let target_tokens = (total_tokens as f64 * self.config.compression_ratio) as usize;
        let target_tokens = target_tokens.max(self.config.min_tokens);

        let result = match self.config.strategy {
            CompressionStrategy::LengthBased => {
                self.compress_length_based(messages, target_tokens)
            }
            CompressionStrategy::SemanticBased => {
                self.compress_semantic_based(messages, target_tokens)
            }
            CompressionStrategy::TimeBased => {
                self.compress_time_based(messages, target_tokens)
            }
            CompressionStrategy::Hybrid => {
                self.compress_hybrid(messages, target_tokens)
            }
        };

        self.cache.insert(session_id.to_string(), result.clone());
        Ok(result)
    }

    fn compress_length_based(&self, messages: &[Message], target_tokens: usize) -> Vec<Message> {
        let mut result = Vec::new();
        let mut current_tokens = 0;

        for (i, msg) in messages.iter().enumerate() {
            if i >= messages.len() - self.config.keep_recent_count {
                result.push(msg.clone());
                current_tokens += msg.token_count;
                continue;
            }

            if current_tokens + msg.token_count <= target_tokens {
                result.push(msg.clone());
                current_tokens += msg.token_count;
            } else {
                let compressed = self.compress_single_message(msg);
                result.push(compressed);
                current_tokens += 50;
            }
        }

        result
    }

    fn compress_semantic_based(&self, messages: &[Message], target_tokens: usize) -> Vec<Message> {
        let mut result = Vec::new();
        let mut current_tokens = 0;
        let mut seen_topics = HashMap::new();

        for (i, msg) in messages.iter().enumerate() {
            if i >= messages.len() - self.config.keep_recent_count {
                result.push(msg.clone());
                current_tokens += msg.token_count;
                continue;
            }

            let topic = self.extract_topic(msg);
            let topic_key = topic.clone().unwrap_or_else(|| "unknown".to_string());
            
            if seen_topics.contains_key(&topic_key) && current_tokens > target_tokens / 2 {
                let compressed = self.compress_single_message(msg);
                result.push(compressed);
                current_tokens += 50;
            } else {
                result.push(msg.clone());
                current_tokens += msg.token_count;
                seen_topics.insert(topic_key, ());
            }
        }

        result
    }

    fn compress_time_based(&self, messages: &[Message], _target_tokens: usize) -> Vec<Message> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let threshold = self.config.time_threshold_hours * 3600;

        messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                if i >= messages.len() - self.config.keep_recent_count {
                    msg.clone()
                } else if now - msg.timestamp > threshold {
                    self.compress_single_message(msg)
                } else {
                    msg.clone()
                }
            })
            .collect()
    }

    fn compress_hybrid(&self, messages: &[Message], target_tokens: usize) -> Vec<Message> {
        let length_result = self.compress_length_based(messages, target_tokens);
        let time_result = self.compress_time_based(messages, target_tokens);

        let mut result = Vec::new();
        for (l, t) in length_result.into_iter().zip(time_result.into_iter()) {
            if l.is_compressed && t.is_compressed {
                result.push(l);
            } else if l.is_compressed {
                result.push(l);
            } else if t.is_compressed {
                result.push(t);
            } else {
                result.push(l);
            }
        }

        result
    }

    fn compress_single_message(&self, msg: &Message) -> Message {
        let compressed_content = if msg.content.len() > 100 {
            format!(
                "[Compressed] {}... ({} chars)",
                &msg.content[..100],
                msg.content.len()
            )
        } else {
            msg.content.clone()
        };

        Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: msg.role.clone(),
            content: compressed_content,
            timestamp: msg.timestamp,
            token_count: 50,
            is_compressed: true,
            original_id: Some(msg.id.clone()),
        }
    }

    fn extract_topic(&self, msg: &Message) -> Option<String> {
        let content = &msg.content;
        let lines: Vec<&str> = content.lines().collect();
        
        if !lines.is_empty() {
            let first_line = lines[0].trim();
            if first_line.len() <= 50 {
                return Some(first_line.to_string());
            }
        }

        None
    }

    pub fn decompress(&self, session_id: &str, messages: &[Message]) -> Result<Vec<Message>> {
        let cached = self.cache.get(session_id);
        
        if let Some(cached_messages) = cached {
            let mut result = Vec::new();
            for msg in messages {
                if msg.is_compressed {
                    if let Some(original) = cached_messages
                        .iter()
                        .find(|m| m.id.as_str() == msg.original_id.as_deref().unwrap_or(""))
                    {
                        result.push(original.clone());
                    } else {
                        result.push(msg.clone());
                    }
                } else {
                    result.push(msg.clone());
                }
            }
            Ok(result)
        } else {
            Ok(messages.to_vec())
        }
    }

    pub fn get_compression_stats(&self, session_id: &str) -> Option<CompressionStats> {
        self.cache.get(session_id).map(|messages| {
            let original_count = messages.len();
            let compressed_count = messages.iter().filter(|m| m.is_compressed).count();
            let original_tokens: usize = messages.iter().map(|m| m.token_count).sum();
            let compressed_tokens: usize = messages
                .iter()
                .map(|m| if m.is_compressed { 50 } else { m.token_count })
                .sum();

            CompressionStats {
                original_count,
                compressed_count,
                original_tokens,
                compressed_tokens,
                compression_ratio: if original_tokens > 0 {
                    (original_tokens - compressed_tokens) as f64 / original_tokens as f64
                } else {
                    0.0
                },
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct CompressionStats {
    pub original_count: usize,
    pub compressed_count: usize,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub compression_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_messages(count: usize) -> Vec<Message> {
        (0..count)
            .map(|i| Message {
                id: uuid::Uuid::new_v4().to_string(),
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: "This is a test message that has some content to compress. ".repeat(10),
                timestamp: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    - (count - i) as u64 * 3600,
                token_count: 100,
                is_compressed: false,
                original_id: None,
            })
            .collect()
    }

    #[test]
    fn test_length_based_compression() {
        let mut compressor = ConversationCompressor::new();
        let messages = create_test_messages(20);
        
        let result = compressor.compress("test", &messages).unwrap();
        assert!(result.len() <= messages.len());
    }

    #[test]
    fn test_keeps_recent_messages() {
        let mut compressor = ConversationCompressor {
            config: CompressionConfig {
                keep_recent_count: 3,
                ..Default::default()
            },
            cache: HashMap::new(),
        };
        let messages = create_test_messages(10);
        
        let result = compressor.compress("test", &messages).unwrap();
        let last_three = &result[result.len() - 3..];
        assert!(last_three.iter().all(|m| !m.is_compressed));
    }

    #[test]
    fn test_compression_ratio() {
        let mut compressor = ConversationCompressor {
            config: CompressionConfig {
                compression_ratio: 0.3,
                ..Default::default()
            },
            cache: HashMap::new(),
        };
        let messages = create_test_messages(30);
        let original_tokens: usize = messages.iter().map(|m| m.token_count).sum();
        
        let result = compressor.compress("test", &messages).unwrap();
        let compressed_tokens: usize = result.iter().map(|m| m.token_count).sum();
        
        assert!(compressed_tokens <= (original_tokens as f64 * 0.4) as usize);
    }

    #[test]
    fn test_hybrid_strategy() {
        let mut compressor = ConversationCompressor::new();
        let messages = create_test_messages(25);
        
        let result = compressor.compress("test", &messages).unwrap();
        assert!(result.len() > 0);
    }

    #[test]
    fn test_compression_stats() {
        let mut compressor = ConversationCompressor::new();
        let messages = create_test_messages(20);
        
        compressor.compress("test", &messages).unwrap();
        let stats = compressor.get_compression_stats("test");
        
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert!(stats.compression_ratio > 0.0);
    }
}