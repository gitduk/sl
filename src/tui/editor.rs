//! The one-line pattern editor, sharing the CLI's parser.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::state::{App, Mode};
use super::theme as t;
use crate::forward::Direction;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Editor(editor) = &app.mode else {
        return;
    };
    f.render_widget(Clear, area);
    let title = if editor.original.is_some() {
        " edit forward "
    } else {
        " new forward "
    };
    let direction = match editor.direction {
        Direction::Local => "local    (-L 127.0.0.1:<entry> ⇒ <host>:<target>)",
        Direction::Remote => "remote   (-R <host>:<entry> ⇒ localhost:<target>)",
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  pattern     ", Style::default().fg(t::MUTED)),
            Span::styled(editor.pattern.clone(), Style::default().fg(t::TEXT)),
            Span::styled("▏", Style::default().fg(t::PRIMARY)),
        ]),
    ];
    if let Some(error) = &editor.error {
        lines.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(t::ERROR),
        )));
    }
    lines.extend([
        Line::from(""),
        Line::from(vec![
            Span::styled("  direction   ", Style::default().fg(t::WARNING)),
            Span::styled(direction, Style::default().fg(t::TEXT)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  <host>:<port>[,<port>...]   or   <entry>:<host>:<target>",
            Style::default().fg(t::MUTED),
        )),
        Line::from(vec![
            Span::styled(
                "  tab",
                Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" direction  ", Style::default().fg(t::MUTED)),
            Span::styled(
                "enter",
                Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" save  ", Style::default().fg(t::MUTED)),
            Span::styled(
                "esc",
                Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" cancel", Style::default().fg(t::MUTED)),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title)
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        area,
    );
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if !matches!(app.mode, Mode::Editor(_)) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => app.save_editor(),
        KeyCode::Tab => {
            if let Mode::Editor(editor) = &mut app.mode {
                editor.direction = editor.direction.toggled();
                editor.error = None;
            }
        }
        KeyCode::Backspace => {
            if let Mode::Editor(editor) = &mut app.mode {
                editor.pattern.pop();
                editor.error = None;
            }
        }
        KeyCode::Char(c) => {
            if let Mode::Editor(editor) = &mut app.mode {
                editor.pattern.push(c);
                editor.error = None;
            }
        }
        _ => {}
    }
}
