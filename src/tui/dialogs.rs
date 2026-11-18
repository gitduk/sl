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
    let (title, lines) = match confirm {
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
        Confirm::Quit => {
            let running = app
                .rows
                .iter()
                .filter(|row| app.status_of(&row.spec).is_up())
                .count();
            (
                " quit ",
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!("  Quit and stop {running} running forward(s)?"),
                        Style::default().fg(t::TEXT),
                    )),
                    Line::from(Span::styled(
                        "  Background rows started with -d keep running.",
                        Style::default().fg(t::MUTED),
                    )),
                ],
            )
        }
    };
    let mut lines = lines;
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
    let rows = [
        ("j / k", "move down / up"),
        ("g / G", "first / last row"),
        ("n", "new forward"),
        ("e", "edit the selected forward"),
        ("d", "delete the selected forward"),
        ("enter", "restart the selected forward"),
        ("space", "start or stop the selected row"),
        ("a / s", "start / stop every row"),
        ("t", "probe the local listen port"),
        ("/", "filter by host or port"),
        ("esc", "close this help"),
        ("q", "quit (stops the rows it started)"),
    ];
    let mut lines = vec![Line::from("")];
    for (key, what) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:<8}"), Style::default().fg(t::WARNING)),
            Span::styled(what, Style::default().fg(t::TEXT)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Background rows (src: daemon) are stopped with: sl -k [host...]",
        Style::default().fg(t::MUTED),
    )));
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
                    Confirm::Quit => app.quit = true,
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        _ => {}
    }
}
