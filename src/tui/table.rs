//! The forwarding table and the summary in its title.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Padding, Paragraph, Row as TableRow, Table, TableState};

use super::state::{App, Row};
use super::theme as t;
use crate::runtime::Owner;
use crate::sessions::Status;

const STATE_WIDTH: u16 = 24;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.rows.is_empty() && app.filter.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No forwards yet",
                Style::default().fg(t::MUTED),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Press ", Style::default().fg(t::MUTED)),
                Span::styled(
                    "n",
                    Style::default().fg(t::WARNING).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to add one, e.g. ", Style::default().fg(t::MUTED)),
                Span::styled("wk:8000", Style::default().fg(t::PRIMARY)),
            ]),
        ])
        .block(Block::default().padding(Padding::new(1, 1, 1, 0)));
        f.render_widget(hint, area);
        return;
    }

    let header = TableRow::new(vec![
        Cell::from(""),
        Cell::from("Host"),
        Cell::from("Listen"),
        Cell::from("Target"),
        Cell::from("State"),
        Cell::from("Up"),
        Cell::from("Retry"),
        Cell::from("Src"),
    ])
    .style(Style::default().fg(t::MUTED).add_modifier(Modifier::BOLD));

    let host_width = app
        .rows
        .iter()
        .map(|row| row.spec.host.as_str().len())
        .max()
        .unwrap_or(4)
        .max(4) as u16;

    let visible = app.visible();
    let mut rows = Vec::with_capacity(visible.len());
    for index in visible {
        let Some(row) = app.rows.get(index) else {
            continue;
        };
        let (state_text, state_style) = status_cell(app, row);
        let mark_color = if row.spec.direction == crate::forward::Direction::Local {
            t::SUCCESS
        } else {
            t::PRIMARY
        };
        let cells = vec![
            Cell::from(Span::styled(
                format!("[{}]", row.spec.direction.tag()),
                Style::default().fg(mark_color),
            )),
            Cell::from(Span::styled(
                row.spec.host.to_string(),
                Style::default().fg(if row.enabled { t::TEXT } else { t::MUTED }),
            )),
            Cell::from(row.spec.listen_display()),
            Cell::from(row.spec.target_display()),
            Cell::from(Span::styled(state_text, state_style)),
            Cell::from(uptime_cell(app, row)),
            Cell::from(retry_cell(app, row)),
            Cell::from(Span::styled(
                row.owner.as_str(),
                Style::default().fg(match row.owner {
                    Owner::Panel => t::TEXT,
                    Owner::Daemon => t::WARNING,
                }),
            )),
        ];
        rows.push(TableRow::new(cells));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(host_width),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(STATE_WIDTH),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .column_spacing(2)
    .block(Block::bordered().title(title(app)))
    .row_highlight_style(Style::default().bg(t::HIGHLIGHT_BG));

    let mut state = TableState::default();
    state.select(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

fn title(app: &App) -> Line<'static> {
    let up = app
        .rows
        .iter()
        .filter(|row| app.status_of(&row.spec).is_up())
        .count();
    let failed = app
        .rows
        .iter()
        .filter(|row| matches!(app.status_of(&row.spec), Status::Failed { .. }))
        .count();
    let mut spans = vec![
        Span::styled(
            " sl ",
            Style::default().fg(t::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} rows  ", app.rows.len()),
            Style::default().fg(t::MUTED),
        ),
        Span::styled(format!("{up} up"), Style::default().fg(t::SUCCESS)),
    ];
    if failed > 0 {
        spans.push(Span::styled(
            format!("  {failed} failed"),
            Style::default().fg(t::ERROR),
        ));
    }
    if !app.filter.is_empty() {
        spans.push(Span::styled(
            format!("  filter: {}", app.filter),
            Style::default().fg(t::WARNING),
        ));
    }
    Line::from(spans)
}

fn status_cell(app: &App, row: &Row) -> (String, Style) {
    if row.owner == Owner::Panel && !row.enabled {
        return ("disabled".into(), Style::default().fg(t::MUTED));
    }
    match app.status_of(&row.spec) {
        Status::Starting => ("starting".into(), Style::default().fg(t::WARNING)),
        Status::Up { .. } => ("up".into(), Style::default().fg(t::SUCCESS)),
        Status::Failed { reason } => (
            shorten(&reason, STATE_WIDTH as usize - 1),
            Style::default().fg(t::ERROR),
        ),
        Status::Waiting { attempt, retry_at } => {
            let left = retry_at
                .saturating_duration_since(std::time::Instant::now())
                .as_secs();
            (
                format!("retry #{attempt} in {left}s"),
                Style::default().fg(t::WARNING),
            )
        }
        Status::Stopped => ("stopped".into(), Style::default().fg(t::MUTED)),
    }
}

fn uptime_cell(app: &App, row: &Row) -> Span<'static> {
    match app.status_of(&row.spec) {
        Status::Up { since } => Span::styled(elapsed(since), Style::default().fg(t::MUTED)),
        _ => Span::styled("-", Style::default().fg(t::MUTED)),
    }
}

fn retry_cell(app: &App, row: &Row) -> Span<'static> {
    match app.status_of(&row.spec) {
        Status::Waiting { attempt, .. } => {
            Span::styled(attempt.to_string(), Style::default().fg(t::WARNING))
        }
        _ => Span::styled("-", Style::default().fg(t::MUTED)),
    }
}

fn elapsed(since: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(since)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h{:02}", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Keeps the ssh message readable in one cell: the tail is what names the port.
fn shorten(text: &str, width: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= width {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .skip(text.chars().count().saturating_sub(width - 1))
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_keeps_the_tail() {
        let text = "bind [127.0.0.1]:8000: Address already in use";
        let short = shorten(text, 12);
        assert_eq!(short.chars().count(), 12);
        assert!(short.ends_with("in use"), "{short}");
    }

    #[test]
    fn elapsed_is_compact() {
        let now = SystemTime::now();
        assert_eq!(elapsed(now), "0s");
        assert_eq!(elapsed(now - std::time::Duration::from_secs(90)), "1m");
        assert_eq!(elapsed(now - std::time::Duration::from_secs(3900)), "1h05");
    }
}
