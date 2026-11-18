//! Colours shared with `ccs` so the two panels look like one family.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub const PRIMARY: Color = Color::Rgb(97, 175, 239);
pub const SUCCESS: Color = Color::Rgb(152, 195, 121);
pub const WARNING: Color = Color::Rgb(229, 192, 123);
pub const ERROR: Color = Color::Rgb(224, 108, 117);
pub const MUTED: Color = Color::Rgb(92, 99, 112);
pub const TEXT: Color = Color::Rgb(171, 178, 191);
pub const HIGHLIGHT_BG: Color = Color::Rgb(62, 68, 81);

/// A key name, as it reads in hints and help.
pub fn key(text: impl Into<String>) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
    )
}

/// Secondary text: hints, paths, log lines.
pub fn muted(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(MUTED))
}
