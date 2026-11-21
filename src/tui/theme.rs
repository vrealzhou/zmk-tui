//! Colour palette and styles for the terminal UI.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(15, 17, 23);
pub const PANEL: Color = Color::Rgb(23, 26, 35);
pub const PANEL_ALT: Color = Color::Rgb(30, 34, 45);
pub const BORDER: Color = Color::Rgb(58, 66, 86);
pub const BORDER_FOCUS: Color = Color::Rgb(122, 162, 247);
pub const TEXT: Color = Color::Rgb(214, 220, 232);
pub const TEXT_DIM: Color = Color::Rgb(125, 134, 155);
pub const ACCENT: Color = Color::Rgb(122, 162, 247);
pub const ACCENT_ALT: Color = Color::Rgb(187, 154, 247);
pub const SUCCESS: Color = Color::Rgb(158, 206, 106);
pub const WARNING: Color = Color::Rgb(229, 192, 123);
pub const DANGER: Color = Color::Rgb(224, 108, 117);
pub const SELECTED_BG: Color = Color::Rgb(122, 162, 247);
pub const SELECTED_FG: Color = Color::Rgb(15, 17, 23);

pub const KEY_BG: Color = Color::Rgb(39, 44, 57);
pub const KEY_LAYER_BG: Color = Color::Rgb(36, 58, 78);
pub const KEY_HOLD_BG: Color = Color::Rgb(52, 45, 72);
pub const KEY_MEDIA_BG: Color = Color::Rgb(35, 58, 50);
pub const KEY_TRANS_BG: Color = Color::Rgb(25, 28, 37);
pub const KEY_NONE_BG: Color = Color::Rgb(31, 31, 36);
pub const KEY_UNKNOWN_BG: Color = Color::Rgb(66, 38, 43);

pub fn base() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

pub fn panel() -> Style {
    Style::default().fg(TEXT).bg(PANEL)
}

pub fn border(focused: bool) -> Style {
    Style::default()
        .fg(if focused { BORDER_FOCUS } else { BORDER })
        .bg(BG)
}

pub fn title() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::default().fg(TEXT_DIM).bg(BG)
}

pub fn selected() -> Style {
    Style::default()
        .fg(SELECTED_FG)
        .bg(SELECTED_BG)
        .add_modifier(Modifier::BOLD)
}
