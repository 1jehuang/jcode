//! Enhanced Application State Management
//!
//! Ported from claude_code_src with Rust adaptations:
//! - Centralized state management with observer pattern
//! - Selector pattern for efficient state queries
//! - State persistence and recovery
//! - Undo/redo support
//! - Performance-optimized state updates

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

/// Core application state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub version: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,

    // Session info
    #[serde(default)]
    pub session: SessionState,

    // UI state
    #[serde(default)]
    pub ui: UiState,

    // Configuration
    #[serde(default)]
    pub config: ConfigState,

    // Tool states
    #[serde(default)]
    pub tools: ToolsState,

    // Custom data
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            version: 1,
            timestamp: chrono::Utc::now(),
            session: SessionState::default(),
            ui: UiState::default(),
            config: ConfigState::default(),
            tools: ToolsState::default(),
            custom: HashMap::new(),
        }
    }
}

// -- Sub-states --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub message_count: u64,
    pub tool_call_count: u64,
    #[serde(default)]
    pub current_task: Option<String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            id: format!("{}", chrono::Utc::now().timestamp_millis()),
            started_at: chrono::Utc::now(),
            message_count: 0,
            tool_call_count: 0,
            current_task: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiState {
    pub theme: String,
    pub font_size: u8,
    pub show_line_numbers: bool,
    pub sidebar_visible: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            font_size: 14,
            show_line_numbers: true,
            sidebar_visible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigState {
    pub model_name: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub auto_save: bool,
}

impl Default for ConfigState {
    fn default() -> Self {
        Self {
            model_name: "default".to_string(),
            temperature: 0.7,
            max_tokens: 4096,
            auto_save: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsState {
    pub enabled_tools: Vec<String>,
    pub recent_tools: Vec<String>,
    pub tool_configs: HashMap<String, serde_json::Value>,
}

// -- Selector Pattern --

/// Trait for state selectors
pub trait StateSelector<T>: Send + Sync {
    fn select(&self, state: &AppState) -> T;
}

/// Common selectors
pub struct SessionIdSelector;

impl StateSelector<String> for SessionIdSelector {
    fn select(&self, state: &AppState) -> String {
        state.session.id.clone()
    }
}

pub struct MessageCountSelector;

impl StateSelector<u64> for MessageCountSelector {
    fn select(&self, state: &AppState) -> u64 {
        state.session.message_count
    }
}

pub struct ThemeSelector;

impl StateSelector<String> for ThemeSelector {
    fn select(&self, state: &AppState) -> String {
        state.ui.theme.clone()
    }
}

pub struct ModelNameSelector;

impl StateSelector<String> for ModelNameSelector {
    fn select(&self, state: &AppState) -> String {
        state.config.model_name.clone()
    }
}

/// Composed selector
pub struct ComposedSelector<A, B, SA, SB>
where
    SA: StateSelector<A> + ?Sized,
    SB: StateSelector<B> + ?Sized,
{
    selector_a: Arc<SA>,
    selector_b: Arc<SB>,
    _marker_a: std::marker::PhantomData<A>,
    _marker_b: std::marker::PhantomData<B>,
}

impl<A, B, SA, SB> ComposedSelector<A, B, SA, SB>
where
    SA: StateSelector<A> + ?Sized,
    SB: StateSelector<B> + ?Sized,
{
    pub fn new(selector_a: Arc<SA>, selector_b: Arc<SB>) -> Self {
        Self {
            selector_a,
            selector_b,
            _marker_a: Default::default(),
            _marker_b: Default::default(),
        }
    }

    pub fn select_both(&self, state: &AppState) -> (A, B) {
        (
            self.selector_a.select(state),
            self.selector_b.select(state),
        )
    }
}

// -- Observer Pattern --

type StateListener = Arc<dyn Fn(&AppState, &AppState) + Send + Sync>;

pub struct AppStateManager {
    current: RwLock<AppState>,
    history: RwLock<Vec<AppState>>,
    max_history: usize,
    listeners: RwLock<Vec<StateListener>>,
    tx: broadcast::Sender<StateChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub version: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub changed_paths: Vec<String>,
}

impl AppStateManager {
    pub fn new(max_history: usize) -> Self {
        let (tx, _) = broadcast::channel(100);

        Self {
            current: RwLock::new(AppState::default()),
            history: RwLock::new(Vec::with_capacity(max_history)),
            max_history,
            listeners: RwLock::new(Vec::new()),
            tx,
        }
    }

    /// Get current state snapshot
    pub async fn get_state(&self) -> AppState {
        self.current.read().await.clone()
    }

    /// Select specific data using a selector
    pub async fn select<T, S>(&self, selector: &S) -> T
    where
        S: StateSelector<T> + ?Sized,
    {
        let state = self.current.read().await;
        selector.select(&state)
    }

    /// Update state with automatic versioning
    pub async fn update<F>(&self, updater: F) -> Result<()>
    where
        F: FnOnce(&mut AppState),
    {
        let mut state = self.current.write().await;

        // Save to history before modification
        if state.version > 0 {
            let mut history = self.history.write().await;
            history.push(state.clone());
            while history.len() > self.max_history {
                history.remove(0);
            }
        }

        // Apply update
        updater(&mut mut state);

        // Increment version
        state.version += 1;
        state.timestamp = chrono::Utc::now();

        // Notify listeners
        let state_clone = state.clone();
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener(&state_clone, &state_clone);
        }

        // Broadcast change event
        let change = StateChange {
            version: state.version,
            timestamp: state.timestamp,
            changed_paths: vec!["root".to_string()],
        };
        let _ = self.tx.send(change);

        debug!("State updated to version {}", state.version);
        Ok(())
    }

    /// Undo last change
    pub async fn undo(&self) -> Result<bool> {
        let mut history = self.history.write().await;

        if let Some(previous) = history.pop() {
            *self.current.write().await = previous;
            info!("State undone");
            Ok(true)
        } else {
            warn!("No history to undo");
            Ok(false)
        }
    }

    /// Get history length
    pub async fn history_length(&self) -> usize {
        self.history.read().await.len()
    }

    /// Subscribe to state changes
    pub async fn subscribe<F>(&self, listener: F)
    where
        F: Fn(&AppState, &AppState) + Send + Sync + 'static,
    {
        let listener = Arc::new(listener) as StateListener;
        self.listeners.write().await.push(listener);
    }

    /// Subscribe via broadcast channel
    pub fn subscribe_channel(&self) -> broadcast::Receiver<StateChange> {
        self.tx.subscribe()
    }

    /// Persist state to disk
    pub async fn persist(&self, path: &std::path::Path) -> Result<()> {
        let state = self.get_state().await;
        let data = serde_json::to_vec(&state)?;
        tokio::fs::write(path, data).await?;
        info!("State persisted to {:?}", path);
        Ok(())
    }

    /// Load state from disk
    pub async fn load(&self, path: &std::path::Path) -> Result<()> {
        if path.exists() {
            let data = tokio::fs::read(path).await?;
            let state: AppState = serde_json::from_slice(&data)?;
            *self.current.write().await = state;
            info!("State loaded from {:?}", path);
        }
        Ok(())
    }

    /// Reset state to defaults
    pub async fn reset(&self) -> Result<()> {
        self.update(|state| *state = AppState::default()).await
    }

    /// Merge custom data into state
    pub async fn merge_custom_data(&self, data: HashMap<String, serde_json::Value>) -> Result<()> {
        self.update(move |state| {
            for (key, value) in data {
                state.custom.insert(key, value);
            }
        }).await
    }

    /// Get custom data value
    pub async fn get_custom_value(&self, key: &str) -> Option<serde_json::Value> {
        self.current.read().await.custom.get(key).cloned()
    }

    /// Update statistics
    pub async fn increment_message_count(&self) -> Result<()> {
        self.update(|state| {
            state.session.message_count += 1;
        }).await
    }

    pub async fn increment_tool_call_count(&self) -> Result<()> {
        self.update(|state| {
            state.session.tool_call_count += 1;
        }).await
    }

    pub async fn set_current_task(&self, task: Option<String>) -> Result<()> {
        self.update(move |state| {
            state.session.current_task = task;
        }).await
    }

    /// Get state summary for debugging
    pub async fn summary(&self) -> String {
        let state = self.get_state().await;
        format!(
            "AppState v{} @ {:?}\nSession: {} messages, {} tool calls\nTask: {}",
            state.version,
            state.timestamp,
            state.session.message_count,
            state.session.tool_call_count,
            state.session.current_task.as_deref().unwrap_or("none"),
        )
    }
}

impl Default for AppStateManager {
    fn default() -> Self {
        Self::new(50)
    }
}

// -- Utility Functions --

/// Create a new state manager with common subscriptions
pub async fn create_state_manager_with_defaults() -> AppStateManager {
    let manager = AppStateManager::new(50);

    // Log all state changes
    manager.subscribe(|_old, new| {
        debug!("State changed to v{}", new.version);
    }).await;

    manager
}

/// Batch multiple state updates atomically
pub async fn batch_update(
    manager: &AppStateManager,
    updaters: Vec<Box<dyn FnOnce(&mut AppState)>>,
) -> Result<()> {
    manager.update(move |state| {
        for updater in updaters {
            updater(state);
        }
    }).await
}
