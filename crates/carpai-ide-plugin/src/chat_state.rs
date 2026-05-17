//! Chat state management

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct ChatState {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub is_loading: bool,
    pub session_id: String,
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            messages: vec![],
            input: String::new(),
            is_loading: false,
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn add_message(&mut self, role: Role, content: String) {
        self.messages.push(ChatMessage {
            role,
            content,
            timestamp: chrono::Utc::now(),
        });
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

impl Default for ChatState {
    fn default() -> Self {
        Self::new()
    }
}
