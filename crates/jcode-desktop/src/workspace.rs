#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputMode {
    Navigation,
    Insert,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

const EMPTY_WORKSPACE_MARGIN: i32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelSizePreset {
    Quarter,
    Half,
    ThreeQuarter,
    Full,
}

impl PanelSizePreset {
    pub fn screen_fraction(self) -> f32 {
        match self {
            Self::Quarter => 0.25,
            Self::Half => 0.50,
            Self::ThreeQuarter => 0.75,
            Self::Full => 1.00,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Quarter => "25%",
            Self::Half => "50%",
            Self::ThreeQuarter => "75%",
            Self::Full => "100%",
        }
    }

    pub fn storage_key(self) -> &'static str {
        match self {
            Self::Quarter => "quarter",
            Self::Half => "half",
            Self::ThreeQuarter => "three_quarter",
            Self::Full => "full",
        }
    }

    pub fn from_storage_key(raw: &str) -> Option<Self> {
        match raw {
            "quarter" | "25" | "25%" => Some(Self::Quarter),
            "half" | "50" | "50%" => Some(Self::Half),
            "three_quarter" | "75" | "75%" => Some(Self::ThreeQuarter),
            "full" | "100" | "100%" => Some(Self::Full),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyInput {
    Escape,
    Enter,
    Backspace,
    SpawnPanel,
    HotkeyHelp,
    RefreshSessions,
    SetPanelSize(PanelSizePreset),
    Character(String),
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyOutcome {
    None,
    Redraw,
    OpenSession { session_id: String, title: String },
    SpawnSession,
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCard {
    pub session_id: String,
    pub title: String,
    pub subtitle: String,
    pub detail: String,
    pub preview_lines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopPreferences {
    pub panel_size: PanelSizePreset,
    pub focused_session_id: Option<String>,
    pub workspace_lane: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Surface {
    pub id: u64,
    pub title: String,
    pub body_lines: Vec<String>,
    pub session_id: Option<String>,
    /// Vertical Niri-style workspace index. Each workspace is rendered as one
    /// full-height horizontal strip of columns.
    pub lane: i32,
    pub column: i32,
    pub color_index: usize,
}

impl Surface {
    fn new(id: u64, title: impl Into<String>, lane: i32, column: i32, color_index: usize) -> Self {
        Self {
            id,
            title: title.into(),
            body_lines: Vec::new(),
            session_id: None,
            lane,
            column,
            color_index,
        }
    }

    fn session(id: u64, card: SessionCard, lane: i32, column: i32, color_index: usize) -> Self {
        let mut body_lines = vec![card.subtitle, card.detail];
        if !card.preview_lines.is_empty() {
            body_lines.push("recent transcript".to_string());
            body_lines.extend(card.preview_lines);
        }

        Self {
            id,
            title: card.title,
            body_lines,
            session_id: Some(card.session_id),
            lane,
            column,
            color_index,
        }
    }

    fn is_placeholder_workspace(&self) -> bool {
        self.title == format!("workspace {}", self.lane)
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub mode: InputMode,
    pub surfaces: Vec<Surface>,
    pub focused_id: u64,
    pub zoomed: bool,
    pub draft: String,
    panel_size: PanelSizePreset,
    next_id: u64,
}

impl Workspace {
    #[cfg(test)]
    pub fn fake() -> Self {
        let surfaces = vec![
            Surface::new(1, "fox · coordinator", 0, 0, 0),
            Surface::new(2, "wolf · impl", 0, 1, 1),
            Surface::new(3, "owl · review", 0, 2, 2),
            Surface::new(4, "activity", 0, 3, 3),
            Surface::new(5, "diff", 0, 4, 4),
            Surface::new(6, "review workspace", -1, 0, 5),
            Surface::new(7, "build workspace", 1, 0, 6),
        ];

        Self {
            mode: InputMode::Navigation,
            surfaces,
            focused_id: 1,
            zoomed: false,
            draft: String::new(),
            panel_size: PanelSizePreset::Quarter,
            next_id: 8,
        }
    }

    pub fn from_session_cards(cards: Vec<SessionCard>) -> Self {
        if cards.is_empty() {
            return Self::empty_sessions();
        }

        let mut next_id = 1;
        let surfaces = cards
            .into_iter()
            .enumerate()
            .map(|(index, card)| {
                let id = next_id;
                next_id += 1;
                Surface::session(id, card, 0, index as i32, index)
            })
            .collect::<Vec<_>>();

        Self {
            mode: InputMode::Navigation,
            focused_id: surfaces.first().map(|surface| surface.id).unwrap_or(1),
            surfaces,
            zoomed: false,
            draft: String::new(),
            panel_size: PanelSizePreset::Quarter,
            next_id,
        }
    }

    fn empty_sessions() -> Self {
        Self {
            mode: InputMode::Navigation,
            surfaces: vec![Surface {
                id: 1,
                title: "no jcode sessions found".to_string(),
                body_lines: vec![
                    "start a session in the tui".to_string(),
                    "then restart this desktop prototype".to_string(),
                ],
                session_id: None,
                lane: 0,
                column: 0,
                color_index: 0,
            }],
            focused_id: 1,
            zoomed: false,
            draft: String::new(),
            panel_size: PanelSizePreset::Quarter,
            next_id: 2,
        }
    }

    pub fn preferred_panel_screen_fraction(&self) -> f32 {
        self.panel_size.screen_fraction()
    }

    pub fn current_workspace(&self) -> i32 {
        self.focused_surface()
            .map(|surface| surface.lane)
            .unwrap_or_default()
    }

    pub fn status_title(&self) -> String {
        let mode = match self.mode {
            InputMode::Navigation => "NAV",
            InputMode::Insert => "INSERT",
        };
        let zoom = if self.zoomed { " · ZOOM" } else { "" };
        let focused = self
            .focused_surface()
            .map(|surface| surface.title.as_str())
            .unwrap_or("no surface");
        let workspace = self.current_workspace();
        let panel_size = self.panel_size.label();

        match self.mode {
            InputMode::Navigation => format!(
                "Jcode Desktop · {mode}{zoom} · workspace {workspace} · panel {panel_size} · {focused} · h/l columns · j/k workspaces · Ctrl+1-4 panel size · Ctrl+R refresh · Ctrl+; new · Ctrl+? help · z zoom · i insert · Esc quit"
            ),
            InputMode::Insert => {
                format!(
                    "Jcode Desktop · {mode}{zoom} · workspace {workspace} · {focused} · typing captured · Esc NAV"
                )
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        match self.mode {
            InputMode::Navigation => self.handle_navigation_key(key),
            InputMode::Insert => self.handle_insert_key(key),
        }
    }

    pub fn replace_session_cards(&mut self, cards: Vec<SessionCard>) {
        let previous_mode = self.mode;
        let previous_panel_size = self.panel_size;
        let previous_session_id = self
            .focused_surface()
            .and_then(|surface| surface.session_id.clone());

        let mut replacement = Self::from_session_cards(cards);
        replacement.mode = previous_mode;
        replacement.panel_size = previous_panel_size;
        if let Some(previous_session_id) = previous_session_id
            && let Some(surface) = replacement
                .surfaces
                .iter()
                .find(|surface| surface.session_id.as_deref() == Some(previous_session_id.as_str()))
        {
            replacement.focused_id = surface.id;
        }

        *self = replacement;
    }

    pub fn preferences(&self) -> DesktopPreferences {
        DesktopPreferences {
            panel_size: self.panel_size,
            focused_session_id: self
                .focused_surface()
                .and_then(|surface| surface.session_id.clone()),
            workspace_lane: self.current_workspace(),
        }
    }

    pub fn apply_preferences(&mut self, preferences: DesktopPreferences) {
        self.panel_size = preferences.panel_size;

        if let Some(focused_session_id) = preferences.focused_session_id
            && let Some(surface) = self
                .surfaces
                .iter()
                .find(|surface| surface.session_id.as_deref() == Some(focused_session_id.as_str()))
        {
            self.focused_id = surface.id;
            self.zoomed = false;
            return;
        }

        if self.is_lane_navigable(preferences.workspace_lane) {
            self.focused_id = self.ensure_workspace_surface(preferences.workspace_lane, 0);
            self.zoomed = false;
        }
    }

    pub fn focused_surface(&self) -> Option<&Surface> {
        self.surfaces
            .iter()
            .find(|surface| surface.id == self.focused_id)
    }

    pub fn focused_session_target(&self) -> Option<(String, String)> {
        self.focused_surface().and_then(|surface| {
            surface
                .session_id
                .as_ref()
                .map(|id| (id.clone(), surface.title.clone()))
        })
    }

    pub fn is_focused(&self, surface_id: u64) -> bool {
        self.focused_id == surface_id
    }

    fn handle_navigation_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SpawnPanel => {
                return KeyOutcome::SpawnSession;
            }
            KeyInput::HotkeyHelp => {
                self.open_hotkey_help();
                return KeyOutcome::Redraw;
            }
            KeyInput::RefreshSessions => return KeyOutcome::Redraw,
            KeyInput::SetPanelSize(size) => {
                self.panel_size = size;
                return KeyOutcome::Redraw;
            }
            _ => {}
        }

        let KeyInput::Character(text) = key else {
            return match key {
                KeyInput::Escape => KeyOutcome::Exit,
                KeyInput::Enter => {
                    if let Some((session_id, title)) = self.focused_session_target() {
                        return KeyOutcome::OpenSession { session_id, title };
                    }
                    self.mode = InputMode::Insert;
                    KeyOutcome::Redraw
                }
                _ => KeyOutcome::None,
            };
        };

        match text.as_str() {
            "h" => self.focus_column(Direction::Left),
            "j" => self.focus_workspace(Direction::Down),
            "k" => self.focus_workspace(Direction::Up),
            "l" => self.focus_column(Direction::Right),
            "o" | "O" => {
                if let Some((session_id, title)) = self.focused_session_target() {
                    return KeyOutcome::OpenSession { session_id, title };
                }
                false
            }
            "H" => self.move_focused_column(Direction::Left),
            "J" => self.move_focused_workspace(Direction::Down),
            "K" => self.move_focused_workspace(Direction::Up),
            "L" => self.move_focused_column(Direction::Right),
            "i" => {
                self.mode = InputMode::Insert;
                true
            }
            "n" => {
                self.add_surface();
                true
            }
            "x" => self.close_focused(),
            "z" => {
                self.zoomed = !self.zoomed;
                true
            }
            _ => false,
        }
        .into()
    }

    fn handle_insert_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SpawnPanel => KeyOutcome::SpawnSession,
            KeyInput::HotkeyHelp => {
                self.open_hotkey_help();
                KeyOutcome::Redraw
            }
            KeyInput::RefreshSessions => KeyOutcome::Redraw,
            KeyInput::SetPanelSize(size) => {
                self.panel_size = size;
                KeyOutcome::Redraw
            }
            KeyInput::Escape => {
                self.mode = InputMode::Navigation;
                KeyOutcome::Redraw
            }
            KeyInput::Enter => {
                self.draft.push('\n');
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.draft.pop();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.draft.push_str(&text);
                KeyOutcome::Redraw
            }
            KeyInput::Other => KeyOutcome::None,
        }
    }

    fn focus_column(&mut self, direction: Direction) -> bool {
        if let Some(next_id) = self.column_neighbor_id(direction) {
            self.focused_id = next_id;
            true
        } else {
            false
        }
    }

    fn focus_workspace(&mut self, direction: Direction) -> bool {
        let Some(current) = self.focused_surface() else {
            return false;
        };
        let current_lane = current.lane;
        let current_column = current.column;
        let target_lane = match direction {
            Direction::Up => current_lane - 1,
            Direction::Down => current_lane + 1,
            Direction::Left | Direction::Right => return false,
        };
        if !self.is_lane_navigable(target_lane) {
            return false;
        }
        let target_id = self.ensure_workspace_surface(target_lane, current_column);
        self.focused_id = target_id;
        self.zoomed = false;
        true
    }

    fn is_lane_navigable(&self, lane: i32) -> bool {
        let (min_occupied_lane, max_occupied_lane) = self.occupied_lane_bounds();
        lane >= min_occupied_lane - EMPTY_WORKSPACE_MARGIN
            && lane <= max_occupied_lane + EMPTY_WORKSPACE_MARGIN
    }

    fn occupied_lane_bounds(&self) -> (i32, i32) {
        self.surfaces
            .iter()
            .filter(|surface| !surface.is_placeholder_workspace())
            .map(|surface| surface.lane)
            .fold(None::<(i32, i32)>, |bounds, lane| match bounds {
                Some((min_lane, max_lane)) => Some((min_lane.min(lane), max_lane.max(lane))),
                None => Some((lane, lane)),
            })
            .unwrap_or_else(|| {
                let current = self.current_workspace();
                (current, current)
            })
    }

    fn column_neighbor_id(&self, direction: Direction) -> Option<u64> {
        let current = self.focused_surface()?;
        let current_lane = current.lane;
        let current_column = current.column;

        self.surfaces
            .iter()
            .filter(|surface| surface.lane == current_lane)
            .filter(|surface| match direction {
                Direction::Left => surface.column < current_column,
                Direction::Right => surface.column > current_column,
                Direction::Up | Direction::Down => false,
            })
            .min_by_key(|surface| ((surface.column - current_column).abs(), surface.id))
            .map(|surface| surface.id)
    }

    fn move_focused_column(&mut self, direction: Direction) -> bool {
        let Some(focused_index) = self.focused_index() else {
            return false;
        };
        if !matches!(direction, Direction::Left | Direction::Right) {
            return false;
        }

        if let Some(neighbor_id) = self.column_neighbor_id(direction) {
            if let Some(neighbor_index) = self
                .surfaces
                .iter()
                .position(|surface| surface.id == neighbor_id)
            {
                let focused_column = self.surfaces[focused_index].column;
                let neighbor_column = self.surfaces[neighbor_index].column;
                self.surfaces[focused_index].column = neighbor_column;
                self.surfaces[neighbor_index].column = focused_column;
                return true;
            }
        }
        false
    }

    fn move_focused_workspace(&mut self, direction: Direction) -> bool {
        let Some(focused_index) = self.focused_index() else {
            return false;
        };
        let lane_delta = match direction {
            Direction::Up => -1,
            Direction::Down => 1,
            Direction::Left | Direction::Right => return false,
        };
        self.surfaces[focused_index].lane += lane_delta;
        self.zoomed = false;
        true
    }

    fn focused_index(&self) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|surface| surface.id == self.focused_id)
    }

    fn ensure_workspace_surface(&mut self, lane: i32, preferred_column: i32) -> u64 {
        if let Some(surface) = self
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane)
            .min_by_key(|surface| ((surface.column - preferred_column).abs(), surface.id))
        {
            return surface.id;
        }

        let id = self.next_id;
        self.next_id += 1;
        self.surfaces.push(Surface::new(
            id,
            format!("workspace {lane}"),
            lane,
            preferred_column,
            id as usize,
        ));
        id
    }

    fn add_surface(&mut self) {
        let lane = self.current_workspace();
        let column = self
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane)
            .map(|surface| surface.column)
            .max()
            .unwrap_or(-1)
            + 1;
        let id = self.next_id;
        self.next_id += 1;
        self.surfaces.push(Surface::new(
            id,
            format!("new session {id}"),
            lane,
            column,
            id as usize,
        ));
        self.focused_id = id;
        self.zoomed = false;
    }

    fn open_hotkey_help(&mut self) {
        let lane = self.current_workspace();
        let body_lines = self.hotkey_help_lines();
        if let Some(index) = self
            .surfaces
            .iter()
            .position(|surface| surface.lane == lane && surface.title == "hotkey help")
        {
            self.surfaces[index].body_lines = body_lines;
            self.focused_id = self.surfaces[index].id;
            self.zoomed = false;
            return;
        }

        let column = self
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane)
            .map(|surface| surface.column)
            .max()
            .unwrap_or(-1)
            + 1;
        let id = self.next_id;
        self.next_id += 1;
        let mut help = Surface::new(id, "hotkey help", lane, column, id as usize);
        help.body_lines = body_lines;
        self.surfaces.push(help);
        self.focused_id = id;
        self.zoomed = false;
    }

    fn hotkey_help_lines(&self) -> Vec<String> {
        match self.mode {
            InputMode::Navigation => {
                let mut lines = vec![
                    "NAV mode".to_string(),
                    "h l focus columns".to_string(),
                    "j k focus workspaces".to_string(),
                    "H L swap columns".to_string(),
                    "J K move panel workspaces".to_string(),
                    "ctrl 1 2 3 4 panel width".to_string(),
                    "ctrl r refresh sessions".to_string(),
                    "ctrl semicolon new panel".to_string(),
                    "ctrl slash help".to_string(),
                    "x close panel  z zoom".to_string(),
                ];
                if self.focused_session_target().is_some() {
                    lines.push("o or enter open session".to_string());
                } else {
                    lines.push("enter insert mode".to_string());
                }
                lines.push("i insert  esc quit".to_string());
                lines
            }
            InputMode::Insert => vec![
                "INSERT mode".to_string(),
                "type appends draft text".to_string(),
                "enter newline".to_string(),
                "backspace delete char".to_string(),
                "esc nav mode".to_string(),
                "ctrl r refresh sessions".to_string(),
                "ctrl semicolon new panel".to_string(),
                "ctrl slash help".to_string(),
            ],
        }
    }

    fn close_focused(&mut self) -> bool {
        if self.surfaces.len() <= 1 {
            return false;
        }
        let Some(position) = self.focused_index() else {
            return false;
        };
        let lane = self.surfaces[position].lane;
        self.surfaces.remove(position);

        if let Some(surface) = self
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane)
            .min_by_key(|surface| surface.column.abs())
        {
            self.focused_id = surface.id;
        } else {
            let new_position = position.min(self.surfaces.len() - 1);
            self.focused_id = self.surfaces[new_position].id;
        }
        self.zoomed = false;
        true
    }
}

impl From<bool> for KeyOutcome {
    fn from(value: bool) -> Self {
        if value { Self::Redraw } else { Self::None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn h_and_l_focus_neighboring_columns_in_current_workspace() {
        let mut workspace = Workspace::fake();
        assert_eq!(workspace.focused_id, 1);
        assert_eq!(
            workspace.handle_key(KeyInput::Character("l".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.focused_id, 2);
        assert_eq!(
            workspace.handle_key(KeyInput::Character("h".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.focused_id, 1);
    }

    #[test]
    fn j_and_k_focus_workspace_below_and_above() {
        let mut workspace = Workspace::fake();
        assert_eq!(workspace.current_workspace(), 0);
        assert_eq!(
            workspace.handle_key(KeyInput::Character("j".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.current_workspace(), 1);
        assert_eq!(
            workspace.handle_key(KeyInput::Character("k".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.current_workspace(), 0);
        assert_eq!(
            workspace.handle_key(KeyInput::Character("k".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.current_workspace(), -1);
    }

    #[test]
    fn moving_to_missing_workspace_creates_placeholder_surface() {
        let mut workspace = Workspace::fake();
        workspace.handle_key(KeyInput::Character("j".to_string()));
        workspace.handle_key(KeyInput::Character("j".to_string()));
        assert_eq!(workspace.current_workspace(), 2);
        assert!(workspace.surfaces.iter().any(|surface| surface.lane == 2));
        assert_unique_positions(&workspace);
    }

    #[test]
    fn workspace_navigation_stops_two_empty_lanes_beyond_occupied_lanes() {
        let mut workspace = Workspace::fake();
        assert_eq!(workspace.occupied_lane_bounds(), (-1, 1));

        for expected_lane in [1, 2, 3] {
            assert_eq!(
                workspace.handle_key(KeyInput::Character("j".to_string())),
                KeyOutcome::Redraw
            );
            assert_eq!(workspace.current_workspace(), expected_lane);
        }
        assert_eq!(
            workspace.handle_key(KeyInput::Character("j".to_string())),
            KeyOutcome::None
        );
        assert_eq!(workspace.current_workspace(), 3);
        assert!(!workspace.surfaces.iter().any(|surface| surface.lane == 4));

        for expected_lane in [2, 1, 0, -1, -2, -3] {
            assert_eq!(
                workspace.handle_key(KeyInput::Character("k".to_string())),
                KeyOutcome::Redraw
            );
            assert_eq!(workspace.current_workspace(), expected_lane);
        }
        assert_eq!(
            workspace.handle_key(KeyInput::Character("k".to_string())),
            KeyOutcome::None
        );
        assert_eq!(workspace.current_workspace(), -3);
        assert!(!workspace.surfaces.iter().any(|surface| surface.lane == -4));
    }

    #[test]
    fn uppercase_h_and_l_swap_focused_surface_with_neighbor() {
        let mut workspace = Workspace::fake();
        workspace.handle_key(KeyInput::Character("L".to_string()));
        assert_eq!(
            workspace
                .focused_surface()
                .map(|surface| (surface.lane, surface.column)),
            Some((0, 1))
        );
        assert_unique_positions(&workspace);
    }

    #[test]
    fn uppercase_j_and_k_move_surface_between_workspaces() {
        let mut workspace = Workspace::fake();
        workspace.handle_key(KeyInput::Character("J".to_string()));
        assert_eq!(
            workspace.focused_surface().map(|surface| surface.lane),
            Some(1)
        );
        workspace.handle_key(KeyInput::Character("K".to_string()));
        assert_eq!(
            workspace.focused_surface().map(|surface| surface.lane),
            Some(0)
        );
    }

    #[test]
    fn insert_mode_captures_text_and_escape_returns_to_navigation() {
        let mut workspace = Workspace::fake();
        assert_eq!(
            workspace.handle_key(KeyInput::Character("i".to_string())),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.mode, InputMode::Insert);
        workspace.handle_key(KeyInput::Character("hello".to_string()));
        assert_eq!(workspace.draft, "hello");
        workspace.handle_key(KeyInput::Escape);
        assert_eq!(workspace.mode, InputMode::Navigation);
    }

    #[test]
    fn navigation_escape_exits() {
        let mut workspace = Workspace::fake();
        assert_eq!(workspace.handle_key(KeyInput::Escape), KeyOutcome::Exit);
    }

    #[test]
    fn new_and_close_surface_update_focus_without_overlapping() {
        let mut workspace = Workspace::fake();
        workspace.handle_key(KeyInput::Character("n".to_string()));
        assert_eq!(workspace.focused_id, 8);
        assert_eq!(workspace.surfaces.len(), 8);
        assert_eq!(
            workspace.focused_surface().map(|surface| surface.lane),
            Some(0)
        );
        assert_unique_positions(&workspace);
        workspace.handle_key(KeyInput::Character("x".to_string()));
        assert_eq!(workspace.surfaces.len(), 7);
        assert_ne!(workspace.focused_id, 8);
    }

    #[test]
    fn spawn_panel_shortcut_adds_surface_in_current_workspace() {
        let mut workspace = Workspace::fake();
        assert_eq!(
            workspace.handle_key(KeyInput::SpawnPanel),
            KeyOutcome::SpawnSession
        );
        assert_eq!(workspace.focused_id, 1);
        assert_unique_positions(&workspace);
    }

    #[test]
    fn hotkey_help_shortcut_opens_single_help_surface() {
        let mut workspace = Workspace::fake();
        assert_eq!(
            workspace.handle_key(KeyInput::HotkeyHelp),
            KeyOutcome::Redraw
        );
        assert_eq!(
            workspace
                .focused_surface()
                .map(|surface| surface.title.as_str()),
            Some("hotkey help")
        );
        let help_id = workspace.focused_id;
        assert_eq!(
            workspace.handle_key(KeyInput::HotkeyHelp),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.focused_id, help_id);
        assert_eq!(
            workspace
                .surfaces
                .iter()
                .filter(|surface| surface.title == "hotkey help")
                .count(),
            1
        );
        assert!(workspace.focused_surface().is_some_and(|surface| {
            surface
                .body_lines
                .contains(&"enter insert mode".to_string())
        }));
    }

    #[test]
    fn hotkey_help_mentions_opening_when_focused_on_real_session() {
        let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

        assert_eq!(
            workspace.handle_key(KeyInput::HotkeyHelp),
            KeyOutcome::Redraw
        );

        assert!(workspace.focused_surface().is_some_and(|surface| {
            surface
                .body_lines
                .contains(&"o or enter open session".to_string())
        }));
    }

    #[test]
    fn panel_size_presets_update_preferred_screen_fraction() {
        let mut workspace = Workspace::fake();
        assert_eq!(workspace.preferred_panel_screen_fraction(), 0.25);
        assert_eq!(
            workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Half)),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.preferred_panel_screen_fraction(), 0.50);
        assert_eq!(
            workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::ThreeQuarter)),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.preferred_panel_screen_fraction(), 0.75);
        assert_eq!(
            workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Full)),
            KeyOutcome::Redraw
        );
        assert_eq!(workspace.preferred_panel_screen_fraction(), 1.00);
    }

    #[test]
    fn session_cards_create_real_session_surfaces() {
        let workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

        assert_eq!(workspace.surfaces.len(), 1);
        assert_eq!(workspace.surfaces[0].title, "alpha");
        assert_eq!(workspace.surfaces[0].session_id.as_deref(), Some("a"));
        assert_eq!(workspace.surfaces[0].body_lines.len(), 4);
        assert!(
            workspace.surfaces[0]
                .body_lines
                .contains(&"recent transcript".to_string())
        );
    }

    #[test]
    fn replacing_session_cards_preserves_focus_when_possible() {
        let mut workspace = Workspace::from_session_cards(vec![
            session_card("a", "alpha"),
            session_card("b", "bravo"),
        ]);
        workspace.focused_id = 2;
        workspace.handle_key(KeyInput::SetPanelSize(PanelSizePreset::Half));

        workspace.replace_session_cards(vec![session_card("b", "bravo refreshed")]);

        assert_eq!(
            workspace
                .focused_surface()
                .map(|surface| surface.title.as_str()),
            Some("bravo refreshed")
        );
        assert_eq!(workspace.preferred_panel_screen_fraction(), 0.50);
    }

    #[test]
    fn o_opens_focused_session_surface() {
        let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);

        assert_eq!(
            workspace.handle_key(KeyInput::Character("o".to_string())),
            KeyOutcome::OpenSession {
                session_id: "a".to_string(),
                title: "alpha".to_string()
            }
        );
    }

    #[test]
    fn enter_opens_real_session_but_still_inserts_for_placeholder() {
        let mut workspace = Workspace::from_session_cards(vec![session_card("a", "alpha")]);
        assert_eq!(
            workspace.handle_key(KeyInput::Enter),
            KeyOutcome::OpenSession {
                session_id: "a".to_string(),
                title: "alpha".to_string()
            }
        );

        let mut placeholder_workspace = Workspace::fake();
        assert_eq!(
            placeholder_workspace.handle_key(KeyInput::Enter),
            KeyOutcome::Redraw
        );
        assert_eq!(placeholder_workspace.mode, InputMode::Insert);
    }

    fn assert_unique_positions(workspace: &Workspace) {
        let positions: HashSet<(i32, i32)> = workspace
            .surfaces
            .iter()
            .map(|surface| (surface.lane, surface.column))
            .collect();
        assert_eq!(positions.len(), workspace.surfaces.len());
    }

    fn session_card(id: &str, title: &str) -> SessionCard {
        SessionCard {
            session_id: id.to_string(),
            title: title.to_string(),
            subtitle: "active · model".to_string(),
            detail: "1 msgs · workspace".to_string(),
            preview_lines: vec!["user hello".to_string()],
        }
    }
}
