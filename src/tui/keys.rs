//! Keys, routed by the overlay that is open.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{App, Mode};
use super::{dialogs, editor};

/// One entry per binding, in help order. A `bar` label also shows it in the
/// status bar, so a new key is described in exactly one place.
pub struct Binding {
    pub keys: &'static str,
    pub what: &'static str,
    pub bar: Option<&'static str>,
}

const fn binding(keys: &'static str, what: &'static str, bar: Option<&'static str>) -> Binding {
    Binding { keys, what, bar }
}

pub const BINDINGS: &[Binding] = &[
    binding("j/k", "move down / up", Some("select")),
    binding("g/G", "first / last row", None),
    binding("a", "add a forward", Some("add")),
    binding("e", "edit the selected forward", Some("edit")),
    binding("d", "delete the selected forward", Some("del")),
    binding("enter", "restart the selected forward", Some("restart")),
    binding("space", "start or stop the selected row", Some("run")),
    binding("s/S", "start / stop every row", None),
    binding("t", "probe the local listen port", Some("test")),
    binding("/", "filter by host or port", Some("filter")),
    binding("?", "show these keys", Some("help")),
    binding("esc", "close this help", None),
    binding("q", "quit; running forwards keep running", Some("quit")),
];

pub fn handle(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    match app.mode {
        Mode::Normal => normal(app, key),
        Mode::Editor(_) => editor::handle_key(app, key),
        Mode::Confirm(_) => dialogs::handle_confirm(app, key),
        Mode::Help => app.mode = Mode::Normal,
    }
}

/// Bracketed paste lands here: the editor takes it whole, the filter grows by it.
pub fn handle_paste(app: &mut App, text: &str) {
    if matches!(app.mode, Mode::Editor(_)) {
        editor::paste(app, text);
        return;
    }
    if app.filtering {
        let cleaned = super::input::sanitize_paste(text);
        if !cleaned.is_empty() {
            app.filter.push_str(&cleaned);
            app.select(0);
        }
    }
}

fn normal(app: &mut App, key: KeyEvent) {
    if app.filtering {
        match key.code {
            KeyCode::Esc => {
                app.filtering = false;
                app.filter.clear();
                app.select(app.selected);
            }
            KeyCode::Enter => app.filtering = false,
            KeyCode::Backspace => {
                app.filter.pop();
                app.select(0);
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.select(0);
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc => {
            if !app.filter.is_empty() {
                app.filter.clear();
                app.select(0);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
        KeyCode::Char('g') => app.select(0),
        KeyCode::Char('G') => app.select(usize::MAX),
        KeyCode::Char('e') => app.open_editor(),
        KeyCode::Char('d') => app.request_delete(),
        KeyCode::Enter => app.restart_selected(),
        KeyCode::Char(' ') => app.toggle_selected(),
        KeyCode::Char('a') => app.new_editor(),
        KeyCode::Char('s') => app.set_all(true),
        KeyCode::Char('S') => app.set_all(false),
        KeyCode::Char('t') => app.test_selected(),
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('c') => app.message = None,
        _ => {}
    }
}
