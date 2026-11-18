//! Confirmation and help overlays.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::state::{App, Confirm, Mode};
use super::theme as t;

pub fn draw_confirm(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Confirm(confirm) = &app.mode else {
        return;
    };
    f.render_widget(Clear, area);
    let (title, mut lines) = match confirm {
        Confirm::Delete(spec) => {
            let running = app.status_of(spec).is_up();
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  Remove {spec}?"),
                    Style::default().fg(t::TEXT),
                )),
            ];
            if running {
                lines.push(Line::from(Span::styled(
                    "  Its forward is running and will be stopped.",
                    Style::default().fg(t::MUTED),
                )));
            }
            (" delete row ", lines)
        }
    };
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  y",
            Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" confirm   ", Style::default().fg(t::MUTED)),
        Span::styled(
            "n",
            Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(t::MUTED)),
    ]));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title)
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        area,
    );
}

pub fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    if !matches!(app.mode, Mode::Help) {
        return;
    }
    f.render_widget(Clear, area);
    let mut lines = vec![Line::from("")];
    for binding in super::keys::BINDINGS {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<8}", binding.keys),
                Style::default().fg(t::WARNING),
            ),
            Span::styled(binding.what, Style::default().fg(t::TEXT)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" keys ")
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        area,
    );
}

pub fn handle_confirm(app: &mut App, key: KeyEvent) {
    if !matches!(app.mode, Mode::Confirm(_)) {
        return;
    }
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            let mode = std::mem::replace(&mut app.mode, Mode::Normal);
            if let Mode::Confirm(confirm) = mode {
                match confirm {
                    Confirm::Delete(spec) => app.delete_row(&spec),
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        _ => {}
    }
}
