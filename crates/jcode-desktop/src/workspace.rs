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
    DeletePreviousWord,
    DeleteNextWord,
    DeleteNextChar,
    MoveCursorWordLeft,
    MoveCursorWordRight,
    MoveCursorLeft,
    MoveCursorRight,
    MoveToLineStart,
    MoveToLineEnd,
    DeleteToLineStart,
    DeleteToLineEnd,
    UndoInput,
    CancelGeneration,
    ScrollBodyPages(i32),
    JumpPrompt(i32),
    CopyLatestResponse,
    OpenModelPicker,
    ModelPickerMove(i32),
    CycleModel(i8),
    AttachClipboardImage,
    ClearAttachedImages,
    PasteText,
    QueueDraft,
    SubmitDraft,
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
    OpenSession {
        session_id: String,
        title: String,
    },
    SpawnSession,
    SendDraft {
        session_id: String,
        title: String,
        message: String,
        images: Vec<(String, String)>,
    },
    CancelGeneration,
    CopyLatestResponse(String),
    LoadModelCatalog,
    SetModel(String),
    CycleModel(i8),
    SendStdinResponse {
        request_id: String,
        input: String,
    },
    AttachClipboardImage,
    PasteText,
    StartFreshSession {
        message: String,
        images: Vec<(String, String)>,
    },
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCard {
    pub session_id: String,
    pub title: String,
    pub subtitle: String,
    pub detail: String,
    pub preview_lines: Vec<String>,
    pub detail_lines: Vec<String>,
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
    pub detail_lines: Vec<String>,
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
            detail_lines: Vec::new(),
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

        let mut detail_lines = vec!["session metadata".to_string()];
        detail_lines.extend(body_lines.iter().take(2).cloned());
        if !card.detail_lines.is_empty() {
            detail_lines.push("expanded transcript".to_string());
            detail_lines.extend(card.detail_lines);
        }

        Self {
            id,
            title: card.title,
            body_lines,
            detail_lines,
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
    pub detail_scroll: usize,
    pub draft: String,
    pub pending_images: Vec<(String, String)>,
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
            detail_scroll: 0,
            draft: String::new(),
            pending_images: Vec::new(),
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
            detail_scroll: 0,
            draft: String::new(),
            pending_images: Vec::new(),
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
                detail_lines: Vec::new(),
                session_id: None,
                lane: 0,
                column: 0,
                color_index: 0,
            }],
            focused_id: 1,
            zoomed: false,
            detail_scroll: 0,
            draft: String::new(),
            pending_images: Vec::new(),
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
            InputMode::Navigation if self.zoomed => format!(
                "Jcode Desktop · {mode}{zoom} · workspace {workspace} · panel {panel_size} · {focused} · j/k scroll · g/G top/bottom · z unzoom · o/Enter open · Esc quit"
            ),
            InputMode::Navigation => format!(
                "Jcode Desktop · {mode}{zoom} · workspace {workspace} · panel {panel_size} · {focused} · h/l columns · j/k workspaces · Ctrl+1-4 panel size · Ctrl+R refresh · Ctrl+; new · Ctrl+? help · z zoom · i insert · Esc quit"
            ),
            InputMode::Insert => {
                let images = match self.pending_images.len() {
                    0 => String::new(),
                    1 => " · 1 image".to_string(),
                    count => format!(" · {count} images"),
                };
                format!(
                    "Jcode Desktop · {mode}{zoom} · workspace {workspace} · {focused}{images} · Enter send · Shift+Enter newline · Ctrl+I image · Esc NAV"
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
        replacement.detail_scroll = self.detail_scroll;
        replacement.draft = self.draft.clone();
        replacement.pending_images = self.pending_images.clone();
        if let Some(previous_session_id) = previous_session_id
            && let Some(surface) = replacement
                .surfaces
                .iter()
                .find(|surface| surface.session_id.as_deref() == Some(previous_session_id.as_str()))
        {
            replacement.focused_id = surface.id;
        }

        *self = replacement;
        self.clamp_detail_scroll();
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
            self.detail_scroll = 0;
            return;
        }

        if self.is_lane_navigable(preferences.workspace_lane) {
            self.focused_id = self.ensure_workspace_surface(preferences.workspace_lane, 0);
            self.zoomed = false;
            self.detail_scroll = 0;
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

    pub fn paste_text(&mut self, text: &str) -> bool {
        if self.mode != InputMode::Insert || text.is_empty() {
            return false;
        }
        self.draft.push_str(text);
        true
    }

    pub fn attach_image(&mut self, media_type: String, base64_data: String) -> bool {
        if self.mode != InputMode::Insert {
            return false;
        }
        self.pending_images.push((media_type, base64_data));
        true
    }

    pub fn clear_attached_images(&mut self) -> bool {
        if self.pending_images.is_empty() {
            return false;
        }
        self.pending_images.clear();
        true
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
            KeyInput::SubmitDraft => {
                if let Some((session_id, title)) = self.focused_session_target() {
                    return KeyOutcome::OpenSession { session_id, title };
                }
                self.mode = InputMode::Insert;
                return KeyOutcome::Redraw;
            }
            KeyInput::CancelGeneration
            | KeyInput::ScrollBodyPages(_)
            | KeyInput::CycleModel(_)
            | KeyInput::OpenModelPicker
            | KeyInput::ModelPickerMove(_)
            | KeyInput::AttachClipboardImage
            | KeyInput::ClearAttachedImages
            | KeyInput::PasteText
            | KeyInput::QueueDraft => {
                return KeyOutcome::None;
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
            "j" if self.zoomed && self.focused_detail_line_count() > 0 => self.scroll_detail(1),
            "k" if self.zoomed && self.focused_detail_line_count() > 0 => self.scroll_detail(-1),
            "j" => self.focus_workspace(Direction::Down),
            "k" => self.focus_workspace(Direction::Up),
            "l" => self.focus_column(Direction::Right),
            "g" if self.zoomed && self.focused_detail_line_count() > 0 => {
                self.scroll_detail_to_top()
            }
            "G" if self.zoomed && self.focused_detail_line_count() > 0 => {
                self.scroll_detail_to_bottom()
            }
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
                self.detail_scroll = 0;
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
            KeyInput::SubmitDraft | KeyInput::QueueDraft => self.submit_draft(),
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
            KeyInput::DeletePreviousWord => {
                delete_previous_word(&mut self.draft);
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                self.draft.clear();
                KeyOutcome::Redraw
            }
            KeyInput::AttachClipboardImage => KeyOutcome::AttachClipboardImage,
            KeyInput::ClearAttachedImages => {
                if self.clear_attached_images() {
                    KeyOutcome::Redraw
                } else {
                    KeyOutcome::None
                }
            }
            KeyInput::DeleteNextChar
            | KeyInput::DeleteNextWord
            | KeyInput::MoveCursorLeft
            | KeyInput::MoveCursorRight
            | KeyInput::MoveCursorWordLeft
            | KeyInput::MoveCursorWordRight
            | KeyInput::MoveToLineStart
            | KeyInput::MoveToLineEnd
            | KeyInput::DeleteToLineEnd
            | KeyInput::UndoInput
            | KeyInput::CancelGeneration
            | KeyInput::ScrollBodyPages(_)
            | KeyInput::JumpPrompt(_)
            | KeyInput::CopyLatestResponse
            | KeyInput::OpenModelPicker
            | KeyInput::ModelPickerMove(_)
            | KeyInput::CycleModel(_) => KeyOutcome::None,
            KeyInput::PasteText => KeyOutcome::PasteText,
            KeyInput::Character(text) => {
                self.draft.push_str(&text);
                KeyOutcome::Redraw
            }
            KeyInput::Other => KeyOutcome::None,
        }
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        let Some((session_id, title)) = self.focused_session_target() else {
            return KeyOutcome::None;
        };

        let images = std::mem::take(&mut self.pending_images);
        self.draft.clear();
        self.mode = InputMode::Navigation;
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
            images,
        }
    }

    fn focus_column(&mut self, direction: Direction) -> bool {
        if let Some(next_id) = self.column_neighbor_id(direction) {
            self.focused_id = next_id;
            self.detail_scroll = 0;
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
        self.detail_scroll = 0;
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
                self.detail_scroll = 0;
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
        self.detail_scroll = 0;
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
        self.detail_scroll = 0;
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
            self.detail_scroll = 0;
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
        self.detail_scroll = 0;
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
                    lines.push("zoomed j k scroll detail".to_string());
                    lines.push("zoomed g G top bottom".to_string());
                } else {
                    lines.push("enter insert mode".to_string());
                }
                lines.push("i insert  esc quit".to_string());
                lines
            }
            InputMode::Insert => vec![
                "INSERT mode".to_string(),
                "type appends draft text".to_string(),
                "ctrl enter send draft".to_string(),
                "enter newline".to_string(),
                "backspace delete char".to_string(),
                "esc nav mode".to_string(),
                "ctrl r refresh sessions".to_string(),
                "ctrl semicolon new panel".to_string(),
                "ctrl v paste text or image".to_string(),
                "ctrl i attach clipboard image".to_string(),
                "ctrl shift i clear images".to_string(),
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
        self.detail_scroll = 0;
        true
    }

    fn focused_detail_line_count(&self) -> usize {
        self.focused_surface()
            .map(|surface| surface.detail_lines.len())
            .unwrap_or_default()
    }

    fn scroll_detail(&mut self, delta: isize) -> bool {
        let max_scroll = self.max_detail_scroll();
        let next = if delta.is_negative() {
            self.detail_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.detail_scroll.saturating_add(delta as usize)
        }
        .min(max_scroll);
        if next == self.detail_scroll {
            return false;
        }
        self.detail_scroll = next;
        true
    }

    fn scroll_detail_to_top(&mut self) -> bool {
        if self.detail_scroll == 0 {
            return false;
        }
        self.detail_scroll = 0;
        true
    }

    fn scroll_detail_to_bottom(&mut self) -> bool {
        let max_scroll = self.max_detail_scroll();
        if self.detail_scroll == max_scroll {
            return false;
        }
        self.detail_scroll = max_scroll;
        true
    }

    fn clamp_detail_scroll(&mut self) {
        self.detail_scroll = self.detail_scroll.min(self.max_detail_scroll());
    }

    fn max_detail_scroll(&self) -> usize {
        self.focused_detail_line_count().saturating_sub(1)
    }
}

fn delete_previous_word(text: &mut String) {
    while text.ends_with(char::is_whitespace) {
        text.pop();
    }
    while text.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
        text.pop();
    }
}

impl From<bool> for KeyOutcome {
    fn from(value: bool) -> Self {
        if value { Self::Redraw } else { Self::None }
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
