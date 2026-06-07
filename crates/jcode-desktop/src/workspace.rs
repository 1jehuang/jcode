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
    CutInputLine,
    UndoInput,
    Autocomplete,
    CancelGeneration,
    ExitApp,
    ScrollBodyLines(i32),
    ScrollBodyPages(i32),
    ScrollBodyToTop,
    ScrollBodyToBottom,
    JumpPrompt(i32),
    CopyLatestResponse,
    CopyLatestCodeBlock,
    CopyTranscript,
    OpenModelPicker,
    OpenSessionSwitcher,
    ModelPickerMove(i32),
    CycleModel(i8),
    CycleReasoningEffort(i8),
    AttachClipboardImage,
    ClearAttachedImages,
    PasteText,
    QueueDraft,
    RetrieveQueuedDraft,
    SubmitDraft,
    SpawnPanel,
    HotkeyHelp,
    ToggleInputMode,
    ToggleSessionInfo,
    RefreshSessions,
    AdjustTextScale(i8),
    ResetTextScale,
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
    CopyText {
        text: String,
        success_notice: &'static str,
    },
    CutDraftToClipboard(String),
    LoadModelCatalog,
    LoadSessionSwitcher,
    RestoreCrashedSessions,
    SetModel(String),
    RefreshModelCatalog,
    SetReasoningEffort(String),
    SetServiceTier(String),
    SetTransport(String),
    SetCompactionMode(String),
    CompactSession,
    RenameSession(Option<String>),
    ClearServerSession,
    CycleModel(i8),
    CycleReasoningEffort(i8),
    SendStdinResponse {
        request_id: String,
        input: String,
    },
    AttachClipboardImage,
    PasteText,
    ForceReload,
    StartFreshSession {
        message: String,
        images: Vec<(String, String)>,
    },
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTranscriptMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCard {
    pub session_id: String,
    pub title: String,
    pub subtitle: String,
    pub detail: String,
    pub preview_lines: Vec<String>,
    pub detail_lines: Vec<String>,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopPreferences {
    pub panel_size: PanelSizePreset,
    pub focused_session_id: Option<String>,
    pub workspace_lane: i32,
    pub space_hold_toggle_ms: u64,
}

pub const DEFAULT_SPACE_HOLD_TOGGLE_MS: u64 = 225;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceKind {
    Session,
    Scratch,
    WorkspacePlaceholder,
    HotkeyHelp,
    Loading,
    Empty,
}

impl SurfaceKind {
    fn contributes_to_lane_bounds(self) -> bool {
        matches!(self, Self::Session | Self::Scratch | Self::HotkeyHelp)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Surface {
    pub id: u64,
    pub kind: SurfaceKind,
    pub title: String,
    pub body_lines: Vec<String>,
    pub detail_lines: Vec<String>,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
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
            kind: SurfaceKind::Scratch,
            title: title.into(),
            body_lines: Vec::new(),
            detail_lines: Vec::new(),
            transcript_messages: Vec::new(),
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
            kind: SurfaceKind::Session,
            title: card.title,
            body_lines,
            detail_lines,
            transcript_messages: card.transcript_messages,
            session_id: Some(card.session_id),
            lane,
            column,
            color_index,
        }
    }

    fn apply_session_card(&mut self, card: SessionCard) {
        let updated = Self::session(self.id, card, self.lane, self.column, self.color_index);
        self.kind = updated.kind;
        self.title = updated.title;
        self.body_lines = updated.body_lines;
        self.detail_lines = updated.detail_lines;
        self.transcript_messages = updated.transcript_messages;
        self.session_id = updated.session_id;
    }

    pub fn session_card(&self) -> Option<SessionCard> {
        let session_id = self.session_id.as_ref()?.clone();
        Some(SessionCard {
            session_id,
            title: self.title.clone(),
            subtitle: self.body_lines.first().cloned().unwrap_or_default(),
            detail: self.body_lines.get(1).cloned().unwrap_or_default(),
            preview_lines: self
                .body_lines
                .iter()
                .skip_while(|line| line.as_str() != "recent transcript")
                .skip(1)
                .cloned()
                .collect(),
            detail_lines: self
                .detail_lines
                .iter()
                .skip_while(|line| line.as_str() != "expanded transcript")
                .skip(1)
                .cloned()
                .collect(),
            transcript_messages: self.transcript_messages.clone(),
        })
    }

    fn workspace_placeholder(id: u64, lane: i32, column: i32, color_index: usize) -> Self {
        Self {
            id,
            kind: SurfaceKind::WorkspacePlaceholder,
            title: format!("workspace {lane}"),
            body_lines: Vec::new(),
            detail_lines: Vec::new(),
            transcript_messages: Vec::new(),
            session_id: None,
            lane,
            column,
            color_index,
        }
    }

    fn non_session_state(
        id: u64,
        kind: SurfaceKind,
        title: impl Into<String>,
        body_lines: Vec<String>,
    ) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            body_lines,
            detail_lines: Vec::new(),
            transcript_messages: Vec::new(),
            session_id: None,
            lane: 0,
            column: 0,
            color_index: 0,
        }
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
    pub draft_cursor: usize,
    pub pending_images: Vec<(String, String)>,
    panel_size: PanelSizePreset,
    space_hold_toggle_ms: u64,
    input_undo_stack: Vec<(String, usize)>,
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
            draft_cursor: 0,
            pending_images: Vec::new(),
            panel_size: PanelSizePreset::Quarter,
            space_hold_toggle_ms: DEFAULT_SPACE_HOLD_TOGGLE_MS,
            input_undo_stack: Vec::new(),
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
            draft_cursor: 0,
            pending_images: Vec::new(),
            panel_size: PanelSizePreset::Quarter,
            space_hold_toggle_ms: DEFAULT_SPACE_HOLD_TOGGLE_MS,
            input_undo_stack: Vec::new(),
            next_id,
        }
    }

    pub fn loading_sessions() -> Self {
        Self {
            mode: InputMode::Navigation,
            surfaces: vec![Surface::non_session_state(
                1,
                SurfaceKind::Loading,
                "loading jcode sessions…",
                vec![
                    "reading recent sessions off the UI thread".to_string(),
                    "the workspace will populate as soon as they are ready".to_string(),
                ],
            )],
            focused_id: 1,
            zoomed: false,
            detail_scroll: 0,
            draft: String::new(),
            draft_cursor: 0,
            pending_images: Vec::new(),
            panel_size: PanelSizePreset::Quarter,
            space_hold_toggle_ms: DEFAULT_SPACE_HOLD_TOGGLE_MS,
            input_undo_stack: Vec::new(),
            next_id: 2,
        }
    }

    fn empty_sessions() -> Self {
        Self {
            mode: InputMode::Navigation,
            surfaces: vec![Surface::non_session_state(
                1,
                SurfaceKind::Empty,
                "no jcode sessions found",
                vec![
                    "start a session in the tui".to_string(),
                    "then restart this desktop prototype".to_string(),
                ],
            )],
            focused_id: 1,
            zoomed: false,
            detail_scroll: 0,
            draft: String::new(),
            draft_cursor: 0,
            pending_images: Vec::new(),
            panel_size: PanelSizePreset::Quarter,
            space_hold_toggle_ms: DEFAULT_SPACE_HOLD_TOGGLE_MS,
            input_undo_stack: Vec::new(),
            next_id: 2,
        }
    }

    pub fn preferred_panel_screen_fraction(&self) -> f32 {
        self.panel_size.screen_fraction()
    }

    pub fn space_hold_toggle_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.space_hold_toggle_ms)
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
                "Jcode Desktop · {mode}{zoom} · workspace {workspace} · panel {panel_size} · {focused} · j/k or Super+J/K scroll · g/G or Ctrl+Home/End top/bottom · z unzoom · o/Enter open · Esc quit"
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
        let previous_focused_id = self.focused_id;
        let previous_session_id = self
            .focused_surface()
            .and_then(|surface| surface.session_id.clone());
        let previous_lane = self.current_workspace();
        let mut pending_cards = cards;
        let old_surfaces = std::mem::take(&mut self.surfaces);

        for mut surface in old_surfaces {
            match surface.session_id.as_deref() {
                Some(session_id) => {
                    if let Some(card_index) = pending_cards
                        .iter()
                        .position(|card| card.session_id == session_id)
                    {
                        let card = pending_cards.remove(card_index);
                        surface.apply_session_card(card);
                        self.surfaces.push(surface);
                    }
                }
                None if !matches!(surface.kind, SurfaceKind::Loading | SurfaceKind::Empty) => {
                    self.surfaces.push(surface);
                }
                None => {}
            }
        }

        for card in pending_cards {
            let lane = 0;
            let column = self.next_available_column(lane);
            let id = self.allocate_surface_id();
            self.surfaces
                .push(Surface::session(id, card, lane, column, id as usize));
        }

        if self.surfaces.is_empty() {
            let empty = Self::empty_sessions();
            self.surfaces = empty.surfaces;
            self.next_id = self.next_id.max(empty.next_id);
        } else {
            self.next_id = self.next_id.max(
                self.surfaces
                    .iter()
                    .map(|surface| surface.id)
                    .max()
                    .unwrap_or(0)
                    + 1,
            );
        }

        if let Some(previous_session_id) = previous_session_id
            && let Some(surface) = self
                .surfaces
                .iter()
                .find(|surface| surface.session_id.as_deref() == Some(previous_session_id.as_str()))
        {
            self.focused_id = surface.id;
        } else if self
            .surfaces
            .iter()
            .any(|surface| surface.id == previous_focused_id)
        {
            self.focused_id = previous_focused_id;
        } else if let Some(surface) = self
            .surfaces
            .iter()
            .filter(|surface| surface.lane == previous_lane)
            .min_by_key(|surface| (surface.column.abs(), surface.id))
            .or_else(|| self.surfaces.iter().min_by_key(|surface| surface.id))
        {
            self.focused_id = surface.id;
        }
        self.zoomed = false;
        self.clamp_detail_scroll();
    }

    pub fn preferences(&self) -> DesktopPreferences {
        DesktopPreferences {
            panel_size: self.panel_size,
            focused_session_id: self
                .focused_surface()
                .and_then(|surface| surface.session_id.clone()),
            workspace_lane: self.current_workspace(),
            space_hold_toggle_ms: self.space_hold_toggle_ms,
        }
    }

    pub fn apply_preferences(&mut self, preferences: DesktopPreferences) {
        self.panel_size = preferences.panel_size;
        self.space_hold_toggle_ms = preferences.space_hold_toggle_ms;

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

        if self.is_lane_navigable(preferences.workspace_lane)
            && let Some(surface) = self
                .surfaces
                .iter()
                .filter(|surface| surface.lane == preferences.workspace_lane)
                .min_by_key(|surface| (surface.column.abs(), surface.id))
        {
            self.focused_id = surface.id;
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

    pub fn focused_session_card(&self) -> Option<SessionCard> {
        self.focused_surface().and_then(Surface::session_card)
    }

    pub fn is_focused(&self, surface_id: u64) -> bool {
        self.focused_id == surface_id
    }

    pub fn paste_text(&mut self, text: &str) -> bool {
        if self.mode != InputMode::Insert || text.is_empty() {
            return false;
        }
        self.insert_draft_text(text);
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
            KeyInput::ExitApp => return KeyOutcome::Exit,
            KeyInput::RefreshSessions => return KeyOutcome::Redraw,
            KeyInput::ScrollBodyPages(pages)
                if self.zoomed && self.focused_detail_line_count() > 0 =>
            {
                return self.scroll_detail(-(pages as isize) * 12).into();
            }
            KeyInput::ScrollBodyLines(lines)
                if self.zoomed && self.focused_detail_line_count() > 0 =>
            {
                return self.scroll_detail(-(lines as isize)).into();
            }
            KeyInput::ScrollBodyToTop if self.zoomed && self.focused_detail_line_count() > 0 => {
                return self.scroll_detail_to_top().into();
            }
            KeyInput::ScrollBodyToBottom if self.zoomed && self.focused_detail_line_count() > 0 => {
                return self.scroll_detail_to_bottom().into();
            }
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
            KeyInput::ToggleInputMode => {
                self.mode = InputMode::Insert;
                return KeyOutcome::Redraw;
            }
            KeyInput::CancelGeneration
            | KeyInput::CycleModel(_)
            | KeyInput::OpenModelPicker
            | KeyInput::OpenSessionSwitcher
            | KeyInput::ToggleSessionInfo
            | KeyInput::ModelPickerMove(_)
            | KeyInput::AttachClipboardImage
            | KeyInput::ClearAttachedImages
            | KeyInput::PasteText
            | KeyInput::QueueDraft
            | KeyInput::RetrieveQueuedDraft
            | KeyInput::CutInputLine
            | KeyInput::Autocomplete => {
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
            KeyInput::ToggleInputMode => {
                self.mode = InputMode::Navigation;
                KeyOutcome::Redraw
            }
            KeyInput::SubmitDraft | KeyInput::QueueDraft => self.submit_draft(),
            KeyInput::Escape => {
                self.mode = InputMode::Navigation;
                KeyOutcome::Redraw
            }
            KeyInput::Enter => {
                self.insert_draft_text("\n");
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.delete_previous_char();
                KeyOutcome::Redraw
            }
            KeyInput::DeletePreviousWord => {
                self.delete_previous_word();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                self.delete_to_line_start();
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
            KeyInput::DeleteNextChar => {
                self.delete_next_char();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextWord => {
                self.delete_next_word();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.move_cursor_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.move_cursor_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordLeft => {
                self.move_cursor_word_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordRight => {
                self.move_cursor_word_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.move_to_line_start();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.move_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineEnd => {
                self.delete_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::CutInputLine => self.cut_input_line(),
            KeyInput::UndoInput => {
                self.undo_input_change();
                KeyOutcome::Redraw
            }
            KeyInput::Autocomplete => self.autocomplete_draft(),
            KeyInput::CancelGeneration
            | KeyInput::ScrollBodyLines(_)
            | KeyInput::ScrollBodyPages(_)
            | KeyInput::ScrollBodyToTop
            | KeyInput::ScrollBodyToBottom
            | KeyInput::JumpPrompt(_)
            | KeyInput::CopyLatestResponse
            | KeyInput::CopyLatestCodeBlock
            | KeyInput::CopyTranscript
            | KeyInput::OpenModelPicker
            | KeyInput::OpenSessionSwitcher
            | KeyInput::ToggleSessionInfo
            | KeyInput::ModelPickerMove(_)
            | KeyInput::CycleModel(_)
            | KeyInput::CycleReasoningEffort(_)
            | KeyInput::AdjustTextScale(_)
            | KeyInput::ResetTextScale => KeyOutcome::None,
            KeyInput::ExitApp => KeyOutcome::Exit,
            KeyInput::RetrieveQueuedDraft => KeyOutcome::None,
            KeyInput::PasteText => KeyOutcome::PasteText,
            KeyInput::Character(text) => {
                self.insert_draft_text(&text);
                KeyOutcome::Redraw
            }
            KeyInput::Other => KeyOutcome::None,
        }
    }

    fn insert_draft_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.remember_input_undo_state();
        self.clamp_draft_cursor();
        self.draft.insert_str(self.draft_cursor, text);
        self.draft_cursor += text.len();
    }

    fn delete_previous_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor == 0 {
            return;
        }
        self.remember_input_undo_state();
        let previous = previous_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(previous..self.draft_cursor, "");
        self.draft_cursor = previous;
    }

    fn delete_next_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor >= self.draft.len() {
            return;
        }
        self.remember_input_undo_state();
        let next = next_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(self.draft_cursor..next, "");
    }

    fn delete_previous_word(&mut self) {
        self.clamp_draft_cursor();
        let start = previous_word_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_next_word(&mut self) {
        self.clamp_draft_cursor();
        let end = next_word_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn move_cursor_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_word_start(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_word_end(&self.draft, self.draft_cursor);
    }

    fn move_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_start(&self.draft, self.draft_cursor);
    }

    fn move_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_end(&self.draft, self.draft_cursor);
    }

    fn delete_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        let start = line_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        let end = line_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn cut_input_line(&mut self) -> KeyOutcome {
        if self.draft.is_empty() {
            return KeyOutcome::None;
        }
        self.remember_input_undo_state();
        let text = std::mem::take(&mut self.draft);
        self.draft_cursor = 0;
        KeyOutcome::CutDraftToClipboard(text)
    }

    fn autocomplete_draft(&mut self) -> KeyOutcome {
        const WORKSPACE_SLASH_COMPLETIONS: &[&str] = &[
            "/help",
            "/clear",
            "/model",
            "/force-reload",
            "/reload",
            "/resume",
            "/sessions",
            "/status",
            "/quit",
        ];
        let Some((draft, cursor)) =
            complete_slash_command(&self.draft, self.draft_cursor, WORKSPACE_SLASH_COMPLETIONS)
        else {
            return KeyOutcome::None;
        };
        self.remember_input_undo_state();
        self.draft = draft;
        self.draft_cursor = cursor;
        KeyOutcome::Redraw
    }

    fn remember_input_undo_state(&mut self) {
        if self
            .input_undo_stack
            .last()
            .is_some_and(|(draft, cursor)| draft == &self.draft && *cursor == self.draft_cursor)
        {
            return;
        }
        self.input_undo_stack
            .push((self.draft.clone(), self.draft_cursor));
        const MAX_UNDO: usize = 64;
        if self.input_undo_stack.len() > MAX_UNDO {
            self.input_undo_stack.remove(0);
        }
    }

    fn undo_input_change(&mut self) {
        if let Some((draft, cursor)) = self.input_undo_stack.pop() {
            self.draft = draft;
            self.draft_cursor = cursor.min(self.draft.len());
            self.clamp_draft_cursor();
        }
    }

    fn clamp_draft_cursor(&mut self) {
        self.draft_cursor = self.draft_cursor.min(self.draft.len());
        while self.draft_cursor > 0 && !self.draft.is_char_boundary(self.draft_cursor) {
            self.draft_cursor -= 1;
        }
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        if self.pending_images.is_empty()
            && let Some(outcome) = self.handle_slash_command(&message)
        {
            return outcome;
        }
        let Some((session_id, title)) = self.focused_session_target() else {
            return KeyOutcome::None;
        };

        let images = std::mem::take(&mut self.pending_images);
        self.draft.clear();
        self.draft_cursor = 0;
        self.input_undo_stack.clear();
        self.mode = InputMode::Navigation;
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
            images,
        }
    }

    fn handle_slash_command(&mut self, message: &str) -> Option<KeyOutcome> {
        if !message.starts_with('/') {
            return None;
        }

        let mut parts = message.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();

        let outcome = match command {
            "/resume" | "/session" | "/sessions" => {
                self.clear_draft_after_local_command();
                KeyOutcome::LoadSessionSwitcher
            }
            "/reload" | "/force-reload" => {
                self.clear_draft_after_local_command();
                KeyOutcome::ForceReload
            }
            _ => return None,
        };
        Some(outcome)
    }

    fn clear_draft_after_local_command(&mut self) {
        self.draft.clear();
        self.draft_cursor = 0;
        self.input_undo_stack.clear();
        self.mode = InputMode::Navigation;
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
            .filter(|surface| surface.kind.contributes_to_lane_bounds())
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

        if let Some(neighbor_id) = self.column_neighbor_id(direction)
            && let Some(neighbor_index) = self
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

        let id = self.allocate_surface_id();
        self.surfaces.push(Surface::workspace_placeholder(
            id,
            lane,
            preferred_column,
            id as usize,
        ));
        id
    }

    fn add_surface(&mut self) {
        let lane = self.current_workspace();
        let column = self.next_available_column(lane);
        let id = self.allocate_surface_id();
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

        let column = self.next_available_column(lane);
        let id = self.allocate_surface_id();
        let mut help = Surface::new(id, "hotkey help", lane, column, id as usize);
        help.kind = SurfaceKind::HotkeyHelp;
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
                    lines.push("zoomed j k or super j/k scroll detail".to_string());
                    lines.push("zoomed g/G or ctrl home/end top bottom".to_string());
                    lines.push("zoomed page up/down jumps detail".to_string());
                } else {
                    lines.push("enter insert mode".to_string());
                }
                lines.push("i insert  esc or ctrl q quit".to_string());
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

    fn next_available_column(&self, lane: i32) -> i32 {
        self.surfaces
            .iter()
            .filter(|surface| surface.lane == lane)
            .map(|surface| surface.column)
            .max()
            .unwrap_or(-1)
            + 1
    }

    fn allocate_surface_id(&mut self) -> u64 {
        while self
            .surfaces
            .iter()
            .any(|surface| surface.id == self.next_id)
        {
            self.next_id += 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        id
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

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn previous_word_start(text: &str, cursor: usize) -> usize {
    let mut start = cursor.min(text.len());
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    start
}

fn next_word_end(text: &str, cursor: usize) -> usize {
    let mut end = cursor.min(text.len());
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        end = next;
    }
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        end = next;
    }
    end
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor.min(text.len())..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn complete_slash_command(
    input: &str,
    cursor: usize,
    completions: &[&'static str],
) -> Option<(String, usize)> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) || !input.starts_with('/') {
        return None;
    }
    let prefix = &input[..cursor];
    if prefix.contains(char::is_whitespace) {
        return None;
    }
    let suffix = &input[cursor..];
    let prefix_key = prefix.to_ascii_lowercase();
    let matches = completions
        .iter()
        .copied()
        .filter(|command| command.starts_with(&prefix_key))
        .collect::<Vec<_>>();
    let completion = match matches.as_slice() {
        [] => fuzzy_slash_completion(&prefix_key, completions)?,
        [only] => *only,
        _ => longest_common_prefix(&matches)?,
    };
    if completion.len() <= prefix.len() {
        return None;
    }
    let mut completed = completion.to_string();
    completed.push_str(suffix);
    Some((completed, completion.len()))
}

fn fuzzy_slash_completion(needle: &str, completions: &[&'static str]) -> Option<&'static str> {
    let mut matches = completions
        .iter()
        .copied()
        .filter_map(|command| {
            slash_fuzzy_score(needle, command).map(|score| (score, command.len(), command))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(b.2))
    });
    matches.first().map(|(_, _, command)| *command)
}

fn slash_fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }

    let needle = needle.strip_prefix('/').unwrap_or(needle);
    let haystack = haystack.strip_prefix('/').unwrap_or(haystack);
    if needle.is_empty() {
        return Some(0);
    }

    if let Some(first_char) = needle.chars().next()
        && !haystack.starts_with(&needle[..first_char.len_utf8()])
    {
        return None;
    }

    let mut score = 0usize;
    let mut position = 0usize;
    for ch in needle.chars() {
        let offset = haystack[position..].find(ch)?;
        score += offset;
        position += offset + ch.len_utf8();
    }

    if needle.len() > 1 && score > needle.len() * 3 {
        return None;
    }

    Some(score)
}

fn longest_common_prefix<'a>(values: &'a [&'a str]) -> Option<&'a str> {
    let first = *values.first()?;
    let mut end = first.len();
    for value in values.iter().skip(1) {
        while end > 0 && !value.starts_with(&first[..end]) {
            end = previous_char_boundary(first, end);
        }
    }
    (end > 0).then_some(&first[..end])
}

impl From<bool> for KeyOutcome {
    fn from(value: bool) -> Self {
        if value { Self::Redraw } else { Self::None }
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
