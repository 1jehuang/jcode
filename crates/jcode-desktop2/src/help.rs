//! Desktop2's local help reference and its responsive card geometry.
//!
//! The content lives beside the layout rather than being borrowed from the TUI:
//! Desktop2 has a desktop text field, session pages, and local-only aliases. A
//! shared command dump would advertise commands this client cannot execute.

use vello::kurbo::Rect;

pub const ALIASES: &[&str] = &["/help", "/?", "/commands"];

pub fn is_alias(input: &str) -> bool {
    ALIASES.contains(&input.trim())
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub key: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub title: &'static str,
    pub rows: &'static [Row],
}

const COMPOSE_ROWS: &[Row] = &[
    Row {
        key: "Enter",
        description: "send prompt",
    },
    Row {
        key: "Shift+Enter",
        description: "insert newline",
    },
    Row {
        key: "Esc",
        description: "cancel, clear, or follow tail",
    },
    Row {
        key: "Ctrl/Cmd+C",
        description: "copy selection or interrupt",
    },
    Row {
        key: "Up / Down",
        description: "draft history",
    },
];

const NAVIGATE_ROWS: &[Row] = &[
    Row {
        key: "Ctrl/Cmd+Tab",
        description: "next session",
    },
    Row {
        key: "Ctrl+Shift+Tab",
        description: "previous session",
    },
    Row {
        key: "Ctrl+Shift+N",
        description: "new session",
    },
    Row {
        key: "Ctrl/Cmd+R",
        description: "resume stored session",
    },
    Row {
        key: "Ctrl+J / K",
        description: "next / previous prompt",
    },
    Row {
        key: "PageUp / Down",
        description: "scroll transcript",
    },
];

const VIEW_ROWS: &[Row] = &[
    Row {
        key: "F1",
        description: "open or close this help",
    },
    Row {
        key: "Ctrl/Cmd+M",
        description: "choose model",
    },
    Row {
        key: "Ctrl/Cmd+,",
        description: "settings",
    },
    Row {
        key: "Ctrl+Shift+D",
        description: "toggle theme",
    },
    Row {
        key: "Ctrl+Shift+R",
        description: "cycle reasoning display",
    },
    Row {
        key: "Ctrl + / - / 0",
        description: "zoom in, out, or reset",
    },
];

const EDIT_ROWS: &[Row] = &[
    Row {
        key: "Ctrl/Cmd+A",
        description: "select all",
    },
    Row {
        key: "Ctrl/Cmd+Z",
        description: "undo",
    },
    Row {
        key: "Ctrl+Shift+Z",
        description: "redo",
    },
    Row {
        key: "Ctrl/Cmd+Left/Right",
        description: "move by word",
    },
];

const COMMAND_ROWS: &[Row] = &[
    Row {
        key: "/help",
        description: "open this help locally",
    },
    Row {
        key: "/?",
        description: "same local help",
    },
    Row {
        key: "/commands",
        description: "same local help",
    },
];

const COMPOSE: Section = Section {
    title: "COMPOSE",
    rows: COMPOSE_ROWS,
};
const NAVIGATE: Section = Section {
    title: "SESSIONS & TRANSCRIPT",
    rows: NAVIGATE_ROWS,
};
const VIEW: Section = Section {
    title: "VIEW",
    rows: VIEW_ROWS,
};
const EDIT: Section = Section {
    title: "EDIT",
    rows: EDIT_ROWS,
};
const COMMANDS: Section = Section {
    title: "LOCAL SLASH COMMANDS",
    rows: COMMAND_ROWS,
};

const LEFT_SECTIONS: &[Section] = &[COMPOSE, NAVIGATE];
const RIGHT_SECTIONS: &[Section] = &[VIEW, EDIT, COMMANDS];
const ALL_SECTIONS: &[Section] = &[COMPOSE, NAVIGATE, VIEW, EDIT, COMMANDS];

pub fn sections(columns: usize, column: usize) -> &'static [Section] {
    if columns == 1 {
        ALL_SECTIONS
    } else if column == 0 {
        LEFT_SECTIONS
    } else {
        RIGHT_SECTIONS
    }
}

pub const CARD_RADIUS: f64 = 10.0;
pub const CARD_PAD: f64 = 24.0;
pub const TITLE_HEIGHT: f64 = 62.0;
pub const COLUMN_GAP: f64 = 34.0;
pub const SECTION_HEADING_HEIGHT: f64 = 25.0;
pub const ROW_HEIGHT: f64 = 21.0;

/// Geometry shared by renderer and pixel tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub card: Rect,
    pub body: Rect,
    pub columns: usize,
}

impl Layout {
    pub fn new(width: f64, height: f64) -> Self {
        let short = width.min(height).max(1.0);
        let inset = (short * 0.06).clamp(14.0, 56.0);
        let available_width = (width - inset * 2.0).max(1.0);
        let card_width = available_width.min(920.0);
        let provisional_inner = (card_width - CARD_PAD * 2.0).max(1.0);
        let columns = if provisional_inner >= 660.0 { 2 } else { 1 };
        let desired_height: f64 = if columns == 2 { 500.0 } else { 680.0 };
        let available_height = (height - inset * 2.0).max(1.0);
        let card_height = desired_height.min(available_height);
        let left = (width - card_width) / 2.0;
        let top = (height - card_height) / 2.0;
        let card = Rect::new(left, top, left + card_width, top + card_height);
        let body = Rect::new(
            card.x0 + CARD_PAD,
            (card.y0 + CARD_PAD + TITLE_HEIGHT).min(card.y1 - CARD_PAD),
            (card.x1 - CARD_PAD).max(card.x0 + CARD_PAD),
            (card.y1 - CARD_PAD).max(card.y0 + CARD_PAD),
        );
        Self {
            card,
            body,
            columns,
        }
    }

    pub fn column(self, index: usize) -> Rect {
        if self.columns == 1 {
            return self.body;
        }
        let width = (self.body.width() - COLUMN_GAP) / 2.0;
        let left = self.body.x0 + index.min(1) as f64 * (width + COLUMN_GAP);
        Rect::new(left, self.body.y0, left + width, self.body.y1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_documented_local_aliases_open_help() {
        for alias in ALIASES {
            assert!(is_alias(alias), "{alias}");
            assert!(is_alias(&format!("  {alias}\n")), "trimmed {alias}");
        }
        for unsupported in ["/quit", "/model", "/help now", "help"] {
            assert!(!is_alias(unsupported), "accepted unsupported {unsupported}");
        }
    }

    #[test]
    fn help_card_centres_clips_and_collapses_responsively() {
        let wide = Layout::new(1400.0, 900.0);
        assert_eq!(wide.columns, 2);
        assert!((wide.card.center().x - 700.0).abs() < 0.01);
        assert!((wide.card.center().y - 450.0).abs() < 0.01);
        assert!(wide.card.x0 > 0.0 && wide.card.y0 > 0.0);
        assert!(wide.card.x1 < 1400.0 && wide.card.y1 < 900.0);
        assert!(wide.column(0).x1 < wide.column(1).x0);

        let narrow = Layout::new(430.0, 620.0);
        assert_eq!(narrow.columns, 1);
        assert_eq!(narrow.column(0), narrow.body);
        assert!(narrow.body.x0 >= narrow.card.x0);
        assert!(narrow.body.y0 >= narrow.card.y0);
        assert!(narrow.body.x1 <= narrow.card.x1);
        assert!(narrow.body.y1 <= narrow.card.y1);
    }
}
