//! The forwarding table and the summary in its title.

use std::time::{Instant, SystemTime};

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Padding, Paragraph, Row, Table, TableState};

use super::state::App;
use super::theme as t;
use crate::config::Entry;
use crate::forward::Direction;
use crate::sessions::Status;

/// Every word the State cell can show; the column is as wide as the longest.
const STATES: [&str; 6] = ["disabled", "starting", "waiting", "stopped", "failed", "up"];

/// What a row reads as before the engine has said anything about it.
const UNKNOWN: Status = Status::Stopped;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.config.forward.is_empty() && app.filter.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(t::muted("  No forwards yet")),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Press ", Style::default().fg(t::MUTED)),
                Span::styled(
                    "a",
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

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from("Host"),
        Cell::from("Listen"),
        Cell::from("Target"),
        Cell::from("State"),
        Cell::from("Up"),
        Cell::from("Retry"),
    ])
    .style(Style::default().fg(t::MUTED).add_modifier(Modifier::BOLD));

    let widths = widths(app);

    let visible = app.visible();
    let mut rows = Vec::with_capacity(visible.len());
    for index in visible {
        let Some(entry) = app.config.forward.get(index) else {
            continue;
        };
        rows.push(Row::new(cells(app, entry)));
    }

    let table = Table::new(rows, widths.map(Constraint::Length))
        .header(header)
        .column_spacing(2)
        .block(Block::bordered().title(title(app)))
        .row_highlight_style(Style::default().bg(t::HIGHLIGHT_BG));

    let mut state = TableState::default();
    state.select(Some(app.selected));
    f.render_stateful_widget(table, area, &mut state);
}

/// Column widths: the longest value each column can show, its header included.
/// Free-form columns follow the whole config, so filtering never moves them.
fn widths(app: &App) -> [u16; 7] {
    let mut host = "Host".len() as u16;
    let mut listen = "Listen".len() as u16;
    let mut target = "Target".len() as u16;
    for entry in &app.config.forward {
        host = host.max(entry.spec.host.as_str().chars().count() as u16);
        listen = listen.max(entry.spec.listen_display().chars().count() as u16);
        target = target.max(entry.spec.target_display().chars().count() as u16);
    }
    let state = STATES
        .iter()
        .map(|word| word.len() as u16)
        .max()
        .unwrap_or(0)
        .max("State".len() as u16);
    // Up fits `17h47`; Retry fits `#17280 17h47`, a day of 5s retries.
    [3, host, listen, target, state, 5, 12]
}

fn cells(app: &App, entry: &Entry) -> Vec<Cell<'static>> {
    let status = app.status_ref(&entry.spec).unwrap_or(&UNKNOWN);
    let mark_color = match entry.spec.direction {
        Direction::Local => t::SUCCESS,
        Direction::Remote => t::PRIMARY,
    };
    let up = match status {
        Status::Up { since } => elapsed(*since),
        _ => "-".to_string(),
    };
    let retry = match status {
        Status::Waiting { attempt, retry_at } => {
            let left = retry_at.saturating_duration_since(Instant::now()).as_secs();
            format!("#{attempt} {}", compact(left))
        }
        _ => "-".to_string(),
    };
    vec![
        Cell::from(Span::styled(
            format!("[{}]", entry.spec.direction.tag()),
            Style::default().fg(mark_color),
        )),
        Cell::from(Span::styled(
            entry.spec.host.to_string(),
            Style::default().fg(if entry.enabled { t::TEXT } else { t::MUTED }),
        )),
        Cell::from(entry.spec.listen_display()),
        Cell::from(entry.spec.target_display()),
        Cell::from(state_cell(entry, status)),
        Cell::from(Span::styled(up, Style::default().fg(t::MUTED))),
        Cell::from(Span::styled(retry, Style::default().fg(t::MUTED))),
    ]
}

fn state_cell(entry: &Entry, status: &Status) -> Span<'static> {
    if !entry.enabled {
        return Span::styled("disabled", Style::default().fg(t::MUTED));
    }
    match status {
        Status::Starting => Span::styled("starting", Style::default().fg(t::WARNING)),
        Status::Up { .. } => Span::styled("up", Style::default().fg(t::SUCCESS)),
        Status::Failed { .. } => Span::styled("failed", Style::default().fg(t::ERROR)),
        Status::Waiting { .. } => Span::styled("waiting", Style::default().fg(t::WARNING)),
        Status::Stopped => Span::styled("stopped", Style::default().fg(t::MUTED)),
    }
}

fn title(app: &App) -> Line<'static> {
    let statuses: Vec<Status> = app
        .config
        .forward
        .iter()
        .map(|entry| app.status_of(&entry.spec))
        .collect();
    let up = statuses.iter().filter(|status| status.is_up()).count();
    let failed = statuses
        .iter()
        .filter(|status| matches!(status, Status::Failed { .. }))
        .count();
    let mut spans = vec![
        Span::styled(
            " sl ",
            Style::default().fg(t::PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} rows  ", app.config.forward.len()),
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

fn elapsed(since: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(since)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    compact(secs)
}

/// One unit, so a duration fits a cell: `45s`, `3m`, `2h05`, `7d`.
fn compact(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h{:02}", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_compact() {
        let now = SystemTime::now();
        assert_eq!(elapsed(now), "0s");
        assert_eq!(elapsed(now - std::time::Duration::from_secs(90)), "1m");
        assert_eq!(elapsed(now - std::time::Duration::from_secs(3900)), "1h05");
    }
}
