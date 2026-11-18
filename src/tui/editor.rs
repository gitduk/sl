//! The one-line pattern editor, sharing the CLI's parser.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
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
    let (before, after) = editor.input.split();
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  pattern     ", Style::default().fg(t::MUTED)),
            Span::styled(before.to_string(), Style::default().fg(t::TEXT)),
            Span::styled("▏", Style::default().fg(t::PRIMARY)),
            Span::styled(after.to_string(), Style::default().fg(t::TEXT)),
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
            t::key("  tab"),
            t::muted(" direction  "),
            t::key("enter"),
            t::muted(" save  "),
            t::key("esc"),
            t::muted(" cancel  "),
            t::key("←→"),
            t::muted(" move  "),
            t::key("ctrl-w"),
            t::muted(" word"),
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
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            return;
        }
        KeyCode::Enter => {
            app.save_editor();
            return;
        }
        _ => {}
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let Mode::Editor(editor) = &mut app.mode else {
        return;
    };
    match key.code {
        KeyCode::Tab => editor.direction = editor.direction.toggled(),
        KeyCode::Backspace => editor.input.backspace(),
        KeyCode::Char('h') if ctrl => editor.input.backspace(),
        KeyCode::Delete => editor.input.delete(),
        KeyCode::Left => editor.input.left(),
        KeyCode::Right => editor.input.right(),
        KeyCode::Home => editor.input.home(),
        KeyCode::End => editor.input.end(),
        KeyCode::Char('a') if ctrl => editor.input.home(),
        KeyCode::Char('e') if ctrl => editor.input.end(),
        KeyCode::Char('w') if ctrl => editor.input.delete_word_back(),
        KeyCode::Char(c) if !ctrl => editor.input.insert(c),
        _ => return,
    }
    editor.error = None;
}

pub fn paste(app: &mut App, text: &str) {
    let cleaned = super::input::sanitize_paste(text);
    if cleaned.is_empty() {
        return;
    }
    if let Mode::Editor(editor) = &mut app.mode {
        editor.input.insert_str(&cleaned);
        editor.error = None;
    }
}
