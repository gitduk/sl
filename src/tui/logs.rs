//! The selected row's ssh stderr, tailed from disk.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
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
        .map(|line| Line::from(t::muted(line.clone())))
        .collect();
    let body = if lines.is_empty() {
        vec![Line::from(t::muted(" nothing yet"))]
    } else {
        lines
    };
    f.render_widget(
        Paragraph::new(body).block(Block::bordered().title(title)),
        area,
    );
}
