//! # UI 集成模块
//!
//! 将 TUI 和 Web 协作组件集成到主应用中

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

use super::super::tui::collab_cursors::{TuiCursorRenderer, RemoteCursorState, CursorPosition, CursorMode, RgbColor, Viewport};
use super::super::ui::collab_components::{CollaboratorInfo, CollaboratorRole};

/// UI 集成管理器
pub struct UiIntegrationManager {
    tui_renderer: TuiCursorRenderer,
    web_components: WebComponentRegistry,
    cursor_updates: broadcast::Sender<CursorUpdate>,
    collaborator_updates: broadcast::Sender<CollaboratorUpdate>,
    session_id: String,
}

/// 光标更新消息
#[derive(Debug, Clone)]
pub struct CursorUpdate {
    pub participant_id: String,
    pub position: CursorPosition,
    pub selection: Option<SelectionRange>,
    pub is_typing: bool,
}

/// 协作者更新消息
#[derive(Debug, Clone)]
pub enum CollaboratorUpdate {
    Joined(CollaboratorInfo),
    Left(String),
    Updated(CollaboratorInfo),
}

/// Web 组件注册表
pub struct WebComponentRegistry {
    components: HashMap<String, WebComponent>,
}

/// Web 组件
pub struct WebComponent {
    pub id: String,
    pub component_type: ComponentType,
    pub props: serde_json::Value,
    pub mounted: bool,
}

/// 组件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentType {
    CollaboratorList,
    CollabEditor,
    ConflictResolver,
    StatusBar,
    NotificationCenter,
}

impl UiIntegrationManager {
    pub fn new(session_id: &str) -> Self {
        let (cursor_tx, _) = broadcast::channel(100);
        let (collab_tx, _) = broadcast::channel(100);
        
        Self {
            tui_renderer: TuiCursorRenderer::with_defaults(),
            web_components: WebComponentRegistry::new(),
            cursor_updates: cursor_tx,
            collaborator_updates: collab_tx,
            session_id: session_id.to_string(),
        }
    }

    /// 更新远程光标
    pub fn update_remote_cursor(&mut self, cursor: RemoteCursorState) {
        self.tui_renderer.update_cursor(cursor.clone());
        
        // 广播更新到 Web
        let update = CursorUpdate {
            participant_id: cursor.participant_id,
            position: cursor.position,
            selection: None,
            is_typing: false,
        };
        let _ = self.cursor_updates.send(update);
    }

    /// 移除远程光标
    pub fn remove_remote_cursor(&mut self, participant_id: &str) {
        self.tui_renderer.remove_cursor(participant_id);
    }

    /// 更新协作者状态
    pub fn update_collaborator(&mut self, info: CollaboratorInfo) {
        // 更新 Web 组件
        self.web_components.update_collaborator(&info);
        
        // 广播更新
        let _ = self.collaborator_updates.send(CollaboratorUpdate::Updated(info));
    }

    /// 协作者加入
    pub fn collaborator_joined(&mut self, info: CollaboratorInfo) {
        // 注册新的远程光标
        let cursor = RemoteCursorState {
            participant_id: info.id.clone(),
            display_name: info.display_name.clone(),
            position: CursorPosition::zero(),
            selection: None,
            color: info.color.clone(),
            is_online: info.is_online,
            last_activity: info.last_activity,
            cursor_mode: CursorMode::Normal,
        };
        self.tui_renderer.update_cursor(cursor);
        
        // 更新 Web 组件
        self.web_components.add_collaborator(&info);
        
        // 广播
        let _ = self.collaborator_updates.send(CollaboratorUpdate::Joined(info));
    }

    /// 协作者离开
    pub fn collaborator_left(&mut self, participant_id: &str) {
        self.tui_renderer.remove_cursor(participant_id);
        self.web_components.remove_collaborator(participant_id);
        
        let _ = self.collaborator_updates.send(CollaboratorUpdate::Left(participant_id.to_string()));
    }

    /// 渲染 TUI 光标
    pub fn render_tui_cursors(&self, viewport: &Viewport) -> String {
        self.tui_renderer.render(viewport)
    }

    /// 获取 Web 组件渲染 HTML
    pub fn get_web_component_html(&self, component_id: &str) -> String {
        self.web_components.render(component_id)
    }

    /// 获取光标更新订阅者
    pub fn subscribe_cursor_updates(&self) -> broadcast::Receiver<CursorUpdate> {
        self.cursor_updates.subscribe()
    }

    /// 获取协作者更新订阅者
    pub fn subscribe_collaborator_updates(&self) -> broadcast::Receiver<CollaboratorUpdate> {
        self.collaborator_updates.subscribe()
    }

    /// 获取会话 ID
    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }
}

impl WebComponentRegistry {
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    pub fn register_component(&mut self, component: WebComponent) {
        self.components.insert(component.id.clone(), component);
    }

    pub fn unregister_component(&mut self, id: &str) {
        self.components.remove(id);
    }

    pub fn get_component(&self, id: &str) -> Option<&WebComponent> {
        self.components.get(id)
    }

    pub fn update_collaborator(&mut self, info: &CollaboratorInfo) {
        // 更新所有相关组件
        for component in self.components.values_mut() {
            match component.component_type {
                ComponentType::CollaboratorList => {
                    self.update_collaborator_list_props(component, info);
                }
                ComponentType::StatusBar => {
                    self.update_status_bar_props(component, info);
                }
                _ => {}
            }
        }
    }

    pub fn add_collaborator(&mut self, info: &CollaboratorInfo) {
        for component in self.components.values_mut() {
            match component.component_type {
                ComponentType::CollaboratorList => {
                    self.add_to_collaborator_list(component, info);
                }
                _ => {}
            }
        }
    }

    pub fn remove_collaborator(&mut self, participant_id: &str) {
        for component in self.components.values_mut() {
            match component.component_type {
                ComponentType::CollaboratorList => {
                    self.remove_from_collaborator_list(component, participant_id);
                }
                _ => {}
            }
        }
    }

    fn update_collaborator_list_props(&self, component: &mut WebComponent, info: &CollaboratorInfo) {
        if let Some(collabs) = component.props.as_object_mut() {
            if let Some(list) = collabs.get_mut("collaborators") {
                if let Some(arr) = list.as_array_mut() {
                    for item in arr {
                        if let Some(obj) = item.as_object_mut() {
                            if let Some(id) = obj.get("id") {
                                if id == info.id {
                                    obj.insert("is_online".to_string(), serde_json::json!(info.is_online));
                                    obj.insert("is_typing".to_string(), serde_json::json!(info.is_typing));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn update_status_bar_props(&self, component: &mut WebComponent, info: &CollaboratorInfo) {
        if let Some(props) = component.props.as_object_mut() {
            let online_count = props.get("onlineCount").and_then(|v| v.as_u64()).unwrap_or(0);
            props.insert("onlineCount".to_string(), serde_json::json!(online_count));
        }
    }

    fn add_to_collaborator_list(&self, component: &mut WebComponent, info: &CollaboratorInfo) {
        if let Some(collabs) = component.props.as_object_mut() {
            if let Some(list) = collabs.get_mut("collaborators") {
                if let Some(arr) = list.as_array_mut() {
                    arr.push(serde_json::json!({
                        "id": info.id,
                        "display_name": info.display_name,
                        "color": info.color,
                        "is_online": info.is_online,
                        "is_typing": info.is_typing,
                    }));
                }
            }
        }
    }

    fn remove_from_collaborator_list(&self, component: &mut WebComponent, participant_id: &str) {
        if let Some(collabs) = component.props.as_object_mut() {
            if let Some(list) = collabs.get_mut("collaborators") {
                if let Some(arr) = list.as_array_mut() {
                    arr.retain(|item| {
                        item.as_object()
                            .and_then(|o| o.get("id"))
                            .and_then(|id| id.as_str())
                            != Some(participant_id)
                    });
                }
            }
        }
    }

    pub fn render(&self, component_id: &str) -> String {
        match self.components.get(component_id) {
            Some(component) => self.render_component(component),
            None => "".to_string(),
        }
    }

    fn render_component(&self, component: &WebComponent) -> String {
        match component.component_type {
            ComponentType::CollaboratorList => {
                self.render_collaborator_list(component)
            }
            ComponentType::StatusBar => {
                self.render_status_bar(component)
            }
            _ => "".to_string(),
        }
    }

    fn render_collaborator_list(&self, component: &WebComponent) -> String {
        if let Some(collabs) = component.props.get("collaborators") {
            if let Some(list) = collabs.as_array() {
                let mut html = String::from("<div class='collaborator-list'>");
                for collab in list {
                    if let Some(obj) = collab.as_object() {
                        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let name = obj.get("display_name").and_then(|v| v.as_str()).unwrap_or("");
                        let color = obj.get("color").and_then(|v| v.as_str()).unwrap_or("#ccc");
                        let online = obj.get("is_online").and_then(|v| v.as_bool()).unwrap_or(false);
                        
                        html.push_str(&format!(
                            "<div class='collaborator-item' data-id='{}'>
                                <div class='collaborator-avatar' style='background-color: {}'>{}</div>
                                <div class='collaborator-name'>{}</div>
                                <div class='collaborator-status {}'></div>
                            </div>",
                            id,
                            color,
                            name.chars().next().unwrap_or('?'),
                            name,
                            if online { "online" } else { "offline" }
                        ));
                    }
                }
                html.push_str("</div>");
                return html;
            }
        }
        "".to_string()
    }

    fn render_status_bar(&self, component: &WebComponent) -> String {
        if let Some(props) = component.props.as_object() {
            let online_count = props.get("onlineCount").and_then(|v| v.as_u64()).unwrap_or(0);
            let total_count = props.get("totalCount").and_then(|v| v.as_u64()).unwrap_or(0);
            
            format!(
                "<div class='status-bar'>
                    <span class='status-item'>Collaborators: {}/{} online</span>
                </div>",
                online_count, total_count
            )
        } else {
            "".to_string()
        }
    }

    pub fn get_component_count(&self) -> usize {
        self.components.len()
    }
}

/// 全局 UI 集成实例
pub type GlobalUiIntegration = Arc<RwLock<UiIntegrationManager>>;

/// 创建全局 UI 集成实例
pub fn create_global_ui_integration(session_id: &str) -> GlobalUiIntegration {
    Arc::new(RwLock::new(UiIntegrationManager::new(session_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_integration_manager() {
        let mut manager = UiIntegrationManager::new("test-session");
        
        let cursor = RemoteCursorState {
            participant_id: "user1".to_string(),
            display_name: "Alice".to_string(),
            position: CursorPosition::new(10, 5),
            selection: None,
            color: "#FF0000".to_string(),
            is_online: true,
            last_activity: chrono::Utc::now().timestamp_millis(),
            cursor_mode: CursorMode::Normal,
        };
        
        manager.update_remote_cursor(cursor);
        
        let cursors = manager.tui_renderer.get_cursors();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].display_name, "Alice");
    }

    #[test]
    fn test_web_component_registry() {
        let mut registry = WebComponentRegistry::new();
        
        let component = WebComponent {
            id: "collab-list".to_string(),
            component_type: ComponentType::CollaboratorList,
            props: serde_json::json!({
                "collaborators": []
            }),
            mounted: true,
        };
        
        registry.register_component(component);
        assert_eq!(registry.get_component_count(), 1);
        
        let info = CollaboratorInfo {
            id: "user1".to_string(),
            display_name: "Alice".to_string(),
            avatar_url: None,
            color: "#FF0000".to_string(),
            role: CollaboratorRole::Editor,
            is_online: true,
            is_typing: false,
            cursor_position: None,
            last_activity: 0,
        };
        
        registry.add_collaborator(&info);
        
        let html = registry.render("collab-list");
        assert!(html.contains("Alice"));
    }

    #[test]
    fn test_collaborator_updates() {
        let mut manager = UiIntegrationManager::new("test-session");
        
        let info = CollaboratorInfo {
            id: "user1".to_string(),
            display_name: "Alice".to_string(),
            avatar_url: None,
            color: "#FF0000".to_string(),
            role: CollaboratorRole::Editor,
            is_online: true,
            is_typing: false,
            cursor_position: None,
            last_activity: 0,
        };
        
        manager.collaborator_joined(info.clone());
        
        let cursors = manager.tui_renderer.get_cursors();
        assert_eq!(cursors.len(), 1);
        
        manager.collaborator_left("user1");
        
        let cursors = manager.tui_renderer.get_cursors();
        assert_eq!(cursors.len(), 0);
    }
}
