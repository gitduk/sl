//! The selected row's ssh stderr, tailed from disk.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::state::App;
use super::theme as t;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.selected_row() {
        Some(row) => format!(
            " log · {} ⇒ {} ",
            row.spec.listen_display(),
            row.spec.target_display()
        ),
        None => " log ".to_string(),
    };
    let visible = area.height.saturating_sub(2) as usize;
    let skip = app.log.len().saturating_sub(visible);
    let lines: Vec<Line> = app.log[skip..]
        .iter()
        .map(|line| Line::from(Span::styled(line.clone(), Style::default().fg(t::MUTED))))
        .collect();
    let body = if lines.is_empty() {
        vec![Line::from(Span::styled(
            " nothing yet",
            Style::default().fg(t::MUTED),
        ))]
    } else {
        lines
    };
    f.render_widget(
        Paragraph::new(body).block(Block::bordered().title(title)),
        area,
    );
}
