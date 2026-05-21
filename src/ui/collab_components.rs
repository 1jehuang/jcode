//! # Web IDE 协作 UI 组件
//!
//! 提供 Web 前端所需的协作功能组件。

use std::collections::{HashMap, BTreeMap};
use serde::{Deserialize, Serialize};

/// 协作者信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaboratorInfo {
    pub id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub color: String,
    pub role: CollaboratorRole,
    pub is_online: bool,
    pub is_typing: bool,
    pub cursor_position: Option<CursorInfo>,
    pub last_activity: i64,
}

/// 协作者角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollaboratorRole {
    Owner,
    Editor,
    Viewer,
    Commenter,
}

/// 协作者角色标签
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollaboratorRoleLabel {
    Owner,
    Editor,
    Viewer,
    Commenter,
    Guest,
}

/// 光标信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInfo {
    pub line: usize,
    pub column: usize,
    pub selection_start: Option<Position>,
    pub selection_end: Option<Position>,
}

/// 位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// 协作者列表状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaboratorListState {
    pub collaborators: Vec<CollaboratorInfo>,
    pub total_count: usize,
    pub online_count: usize,
}

/// 协作编辑状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabEditingState {
    pub session_id: String,
    pub document_name: String,
    pub is_editable: bool,
    pub readonly_reason: Option<String>,
    pub current_content: String,
    pub version: String,
    pub collaborators: Vec<CollaboratorInfo>,
    pub pending_changes: usize,
}

/// 冲突提示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictHint {
    pub conflict_id: String,
    pub conflict_type: ConflictType,
    pub description: String,
    pub affected_range: TextRange,
    pub suggested_resolution: ResolutionSuggestion,
    pub local_change: String,
    pub remote_change: String,
}

/// 冲突类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    OverlappingEdit,
    ConcurrentDelete,
    VersionMismatch,
    PermissionDenied,
}

/// 文本范围
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextRange {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// 解决方案建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResolutionSuggestion {
    UseLocal,
    UseRemote,
    Merge,
    AskUser,
}

/// 协作通知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabNotification {
    pub notification_id: String,
    pub notification_type: NotificationType,
    pub title: String,
    pub message: String,
    pub timestamp: i64,
    pub participant_id: Option<String>,
    pub dismissible: bool,
}

/// 通知类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationType {
    Info,
    Success,
    Warning,
    Error,
    CollaboratorJoined,
    CollaboratorLeft,
    ConflictDetected,
    SaveSuccess,
    SaveFailed,
}

/// Web IDE 协作面板组件
#[derive(Debug, Clone)]
pub struct CollabPanelComponent {
    pub panel_id: String,
    pub state: CollabPanelState,
    pub config: PanelConfig,
}

/// 面板状态
#[derive(Debug, Clone)]
pub enum CollabPanelState {
    Expanded,
    Collapsed,
    Hidden,
}

/// 面板配置
#[derive(Debug, Clone)]
pub struct PanelConfig {
    pub default_position: PanelPosition,
    pub show_collaborator_list: bool,
    pub show_notification_center: bool,
    pub show_conflict_resolver: bool,
    pub show_chat: bool,
    pub auto_hide_on_inactivity_secs: u64,
}

/// 面板位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPosition {
    Left,
    Right,
    Bottom,
}

/// Web IDE 协作工具栏组件
#[derive(Debug, Clone)]
pub struct CollabToolbarComponent {
    pub tools: Vec<ToolItem>,
    pub active_tools: Vec<String>,
}

/// 工具项
#[derive(Debug, Clone)]
pub struct ToolItem {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub tooltip: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

/// 协作状态栏组件
#[derive(Debug, Clone)]
pub struct CollabStatusBarComponent {
    pub status_items: Vec<StatusItem>,
    pub collaborator_avatars: Vec<AvatarItem>,
}

/// 状态项
#[derive(Debug, Clone)]
pub struct StatusItem {
    pub id: String,
    pub label: String,
    pub value: String,
    pub status_type: StatusType,
}

/// 状态类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusType {
    Neutral,
    Success,
    Warning,
    Error,
}

/// 头像项
#[derive(Debug, Clone)]
pub struct AvatarItem {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub color: String,
    pub is_online: bool,
    pub tooltip: String,
}

/// 冲突解决对话框组件
#[derive(Debug, Clone)]
pub struct ConflictResolverComponent {
    pub conflicts: Vec<ConflictDisplay>,
    pub current_conflict_index: usize,
    pub resolution_options: Vec<ResolutionOption>,
}

/// 冲突显示
#[derive(Debug, Clone)]
pub struct ConflictDisplay {
    pub conflict_id: String,
    pub description: String,
    pub local_content: String,
    pub remote_content: String,
    pub merged_preview: Option<String>,
    pub affected_lines: TextRange,
}

/// 解决选项
#[derive(Debug, Clone)]
pub struct ResolutionOption {
    pub option_id: String,
    pub label: String,
    pub description: String,
    pub preview: Option<String>,
}

/// 协作文本编辑器组件属性
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabEditorProps {
    pub session_id: String,
    pub document_id: String,
    pub initial_content: String,
    pub language: String,
    pub theme: String,
    pub readonly: bool,
    pub show_line_numbers: bool,
    pub collaborators: Vec<CollaboratorInfo>,
    pub remote_cursors: Vec<RemoteCursorDisplay>,
    pub pending_operations: usize,
}

/// 远程光标显示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCursorDisplay {
    pub participant_id: String,
    pub display_name: String,
    pub color: String,
    pub position: CursorDisplayPosition,
    pub selection: Option<SelectionDisplay>,
    pub timestamp: i64,
}

/// 光标显示位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorDisplayPosition {
    pub line: usize,
    pub column: usize,
}

/// 选择显示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionDisplay {
    pub start: CursorDisplayPosition,
    pub end: CursorDisplayPosition,
}

/// 协作者头像列表组件属性
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaboratorAvatarListProps {
    pub collaborators: Vec<CollaboratorAvatarProps>,
    pub max_display: usize,
    pub show_tooltip: bool,
}

/// 协作者头像属性
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaboratorAvatarProps {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub color: String,
    pub is_online: bool,
    pub is_typing: bool,
    pub cursor_position: Option<String>,
}

/// 生成 HTML/JSX 组件的渲染器
pub mod renderer {
    use super::*;

    /// 渲染协作者头像列表的 HTML
    pub fn render_collaborator_avatars(props: &CollaboratorAvatarListProps) -> String {
        let visible = props.collaborators.iter().take(props.max_display);
        let remaining = props.collaborators.len().saturating_sub(props.max_display);

        let mut html = String::new();
        html.push_str("<div class='collab-avatars'>");

        for collab in visible {
            let status_class = if collab.is_online { "online" } else { "offline" };
            let typing_indicator = if collab.is_typing { "typing" } else { "" };
            
            html.push_str(&format!(
                r#"<div class='collab-avatar {} {}' data-id='{}' title='{}'>
                    <img src='{}' alt='{}' onerror="this.style.display='none'"/>
                    <span class='collab-avatar-initials'>{}</span>
                    <span class='collab-avatar-status {}'></span>
                    {}
                </div>"#,
                status_class,
                typing_indicator,
                collab.id,
                collab.name,
                collab.avatar_url.as_deref().unwrap_or(""),
                collab.name,
                get_initials(&collab.name),
                if collab.is_online { "" } else { "offline" }
            ));
        }

        if remaining > 0 {
            html.push_str(&format!(
                "<div class='collab-avatar-more'>+{}</div>",
                remaining
            ));
        }

        html.push_str("</div>");
        html
    }

    /// 渲染协作者列表的 HTML
    pub fn render_collaborator_list(state: &CollaboratorListState) -> String {
        let mut html = String::new();
        html.push_str("<div class='collab-list'>");
        html.push_str(&format!(
            "<div class='collab-list-header'>Collaborators ({}/{})</div>",
            state.online_count, state.total_count
        ));

        for collab in &state.collaborators {
            let status_text = if collab.is_online { "Online" } else { "Offline" };
            let typing_text = if collab.is_typing { " (typing...)" } else { "" };
            
            html.push_str(&format!(
                r#"<div class='collab-item' data-id='{}'>
                    <div class='collab-item-avatar' style='background-color: {}'>
                        {}
                    </div>
                    <div class='collab-item-info'>
                        <div class='collab-item-name'>{}</div>
                        <div class='collab-item-status'>
                            <span class='status-dot {}'></span>
                            {}{}
                        </div>
                    </div>
                </div>"#,
                collab.id,
                collab.color,
                get_initials(&collab.display_name),
                collab.display_name,
                if collab.is_online { "online" } else { "offline" },
                status_text,
                typing_text
            ));
        }

        html.push_str("</div>");
        html
    }

    /// 渲染冲突解决器的 HTML
    pub fn render_conflict_resolver(comp: &ConflictResolverComponent) -> String {
        let mut html = String::new();
        html.push_str("<div class='conflict-resolver'>");

        if let Some(current) = comp.conflicts.get(comp.current_conflict_index) {
            html.push_str(&format!(
                r#"<div class='conflict-header'>
                    <h3>Conflict {} of {}</h3>
                    <p>{}</p>
                </div>
                <div class='conflict-content'>
                    <div class='conflict-pane local'>
                        <h4>Your Changes</h4>
                        <pre>{}</pre>
                    </div>
                    <div class='conflict-pane remote'>
                        <h4>Remote Changes</h4>
                        <pre>{}</pre>
                    </div>
                </div>
                <div class='conflict-actions'>"#,
                comp.current_conflict_index + 1,
                comp.conflicts.len(),
                current.description,
                escape_html(&current.local_content),
                escape_html(&current.remote_content)
            ));

            for option in &comp.resolution_options {
                html.push_str(&format!(
                    "<button class='conflict-btn' data-action='{}'>{}</button>",
                    option.option_id,
                    option.label
                ));
            }

            html.push_str("</div>");
        }

        html.push_str("</div>");
        html
    }

    /// 渲染协作文本编辑器的 HTML
    pub fn render_collab_editor(props: &CollabEditorProps) -> String {
        let readonly_attr = if props.readonly { "readonly" } else { "" };
        let line_numbers = if props.show_line_numbers { "show" } else { "hide" };

        let mut html = String::new();
        html.push_str("<div class='collab-editor'>");

        // 渲染远程光标
        for cursor in &props.remote_cursors {
            html.push_str(&format!(
                r#"<div class='remote-cursor' data-participant='{}' style='color: {}'>
                    <div class='remote-cursor-caret'></div>
                    <div class='remote-cursor-label'>{}</div>
                </div>"#,
                cursor.participant_id,
                cursor.color,
                escape_html(&cursor.display_name)
            ));
        }

        // 渲染编辑器内容
        html.push_str(&format!(
            r#"<textarea class='collab-editor-content' data-session='{}' {}>{}</textarea>
                <div class='collab-editor-status'>
                    <span class='collab-status-version'>v{}</span>
                    <span class='collab-status-pending'>{} pending</span>
                </div>"#,
            props.session_id,
            readonly_attr,
            escape_html(&props.initial_content),
            props.version,
            props.pending_operations
        ));

        html.push_str("</div>");
        html
    }

    /// 获取名字的首字母
    fn get_initials(name: &str) -> String {
        name.split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }

    /// HTML 转义
    fn escape_html(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collaborator_info_serialization() {
        let info = CollaboratorInfo {
            id: "user1".to_string(),
            display_name: "Alice".to_string(),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            color: "#FF0000".to_string(),
            role: CollaboratorRole::Editor,
            is_online: true,
            is_typing: false,
            cursor_position: Some(CursorInfo {
                line: 10,
                column: 5,
                selection_start: None,
                selection_end: None,
            }),
            last_activity: 1234567890,
        };

        let json = serde_json::to_string(&info).unwrap();
        let restored: CollaboratorInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, "user1");
        assert_eq!(restored.display_name, "Alice");
        assert_eq!(restored.is_online, true);
    }

    #[test]
    fn test_conflict_hint_serialization() {
        let conflict = ConflictHint {
            conflict_id: "conflict1".to_string(),
            conflict_type: ConflictType::OverlappingEdit,
            description: "Overlapping edit detected".to_string(),
            affected_range: TextRange {
                start_line: 10,
                start_column: 0,
                end_line: 15,
                end_column: 50,
            },
            suggested_resolution: ResolutionSuggestion::Merge,
            local_change: "local change".to_string(),
            remote_change: "remote change".to_string(),
        };

        let json = serde_json::to_string(&conflict).unwrap();
        let restored: ConflictHint = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.conflict_type, ConflictType::OverlappingEdit);
    }

    #[test]
    fn test_renderer_avatars() {
        let props = CollaboratorAvatarListProps {
            collaborators: vec![
                CollaboratorAvatarProps {
                    id: "user1".to_string(),
                    name: "Alice".to_string(),
                    avatar_url: None,
                    color: "#FF0000".to_string(),
                    is_online: true,
                    is_typing: false,
                    cursor_position: Some("10:5".to_string()),
                },
                CollaboratorAvatarProps {
                    id: "user2".to_string(),
                    name: "Bob".to_string(),
                    avatar_url: None,
                    color: "#00FF00".to_string(),
                    is_online: false,
                    is_typing: false,
                    cursor_position: None,
                },
            ],
            max_display: 5,
            show_tooltip: true,
        };

        let html = renderer::render_collaborator_avatars(&props);
        assert!(html.contains("Alice"));
        assert!(html.contains("Bob"));
    }
}
