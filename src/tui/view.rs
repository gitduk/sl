//! Screen layout: table, log pane, status bar and the open overlay.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::state::{App, Level, Mode};
use super::{dialogs, editor, logs, table, theme as t};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let log_height = if area.height >= 24 {
        (area.height / 5).max(6)
    } else {
        3
    };
    let chunks = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(log_height),
        Constraint::Length(2),
    ])
    .split(area);

    table::draw(f, app, chunks[0]);
    logs::draw(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);

    match &app.mode {
        Mode::Normal => {}
        Mode::Editor(_) => editor::draw(f, app, popup(area, 78, 12)),
        Mode::Confirm(_) => dialogs::draw_confirm(f, app, popup(area, 60, 10)),
        Mode::Help => dialogs::draw_help(f, app, popup(area, 78, 22)),
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let hints: Line = if app.filtering {
        Line::from(vec![
            Span::styled("/", Style::default().fg(t::WARNING)),
            Span::styled(app.filter.clone(), Style::default().fg(t::TEXT)),
            Span::styled("  enter: keep  esc: clear", Style::default().fg(t::MUTED)),
        ])
    } else {
        let mut spans = Vec::new();
        for binding in super::keys::BINDINGS {
            let Some(label) = binding.bar else {
                continue;
            };
            spans.push(Span::styled(
                binding.keys,
                Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {label}  "),
                Style::default().fg(t::MUTED),
            ));
        }
        Line::from(spans)
    };

    let detail = match &app.message {
        Some(message) => Line::from(Span::styled(
            format!(" {}", message.text),
            Style::default().fg(match message.level {
                Level::Info => t::TEXT,
                Level::Success => t::SUCCESS,
                Level::Error => t::ERROR,
            }),
        )),
        None => Line::from(Span::styled(
            format!(" {}", app.path.display()),
            Style::default().fg(t::MUTED),
        )),
    };

    f.render_widget(
        Paragraph::new(vec![hints, detail]).block(
            ratatui::widgets::Block::default().padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
        ),
        area,
    );
}

/// A fixed-size box in the middle, clamped so a small terminal still shows it.
fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}
