//! Keys, routed by the overlay that is open.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{App, Mode};
use super::{dialogs, editor};

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
        KeyCode::Char('q') => app.request_quit(),
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
        KeyCode::Char('n') => app.new_editor(),
        KeyCode::Char('e') => app.open_editor(),
        KeyCode::Char('d') => app.request_delete(),
        KeyCode::Enter => app.restart_selected(),
        KeyCode::Char(' ') => app.toggle_selected(),
        KeyCode::Char('a') => app.set_all(true),
        KeyCode::Char('s') => app.set_all(false),
        KeyCode::Char('t') => app.test_selected(),
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('c') => app.message = None,
        _ => {}
    }
}
