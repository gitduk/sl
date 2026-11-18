//! The panel: terminal setup, the event loop, and the render cadence.

pub mod dialogs;
pub mod editor;
pub mod input;
pub mod keys;
pub mod logs;
pub mod state;
pub mod table;
pub mod theme;
pub mod view;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::Result;
use state::App;

const EVENT_POLL: Duration = Duration::from_millis(100);
const LOG_REFRESH: Duration = Duration::from_millis(500);
/// Uptimes and retry countdowns tick in seconds, so one repaint a second is
/// enough when nothing else happens.
const IDLE_REPAINT: Duration = Duration::from_secs(1);

pub fn run(path: PathBuf) -> Result<i32> {
    let mut app = App::new(path)?;
    let mut terminal = enter()?;
    let outcome = event_loop(&mut terminal, &mut app);
    let restored = restore();
    app.release_forwarding();
    outcome?;
    restored?;
    Ok(0)
}

fn enter() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste)?;
    Ok(())
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let mut next_log = Instant::now();
    let mut next_repaint = Instant::now();
    let mut redraw = true;
    loop {
        if redraw {
            terminal.draw(|frame| view::draw(frame, app))?;
            redraw = false;
        }

        if event::poll(EVENT_POLL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    keys::handle(app, key);
                    redraw = true;
                }
                Event::Paste(text) => {
                    keys::handle_paste(app, &text);
                    redraw = true;
                }
                _ => {}
            }
        }
        let events = app.drain_events();
        redraw |= events > 0;

        let now = Instant::now();
        if now >= next_log {
            let log_before = app.log.len();
            app.refresh_log();
            redraw |= app.log.len() != log_before;
            next_log = now + LOG_REFRESH;
        }
        if now >= next_repaint {
            redraw = true;
            next_repaint = now + IDLE_REPAINT;
        }
        if app.quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::keys;
    use super::state::{App, Editor, Mode};
    use super::view;
    use crate::config::Config;
    use crate::forward::{Direction, ForwardSpec};
    use crate::runtime::RuntimeDir;
    use crate::sessions::Status;

    fn spec(pattern: &str) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, Direction::Local)
            .unwrap()
            .remove(0)
    }

    /// A panel with rows but no engine, so a draw cannot touch ssh.
    fn panel(rows: &[&str]) -> App {
        let dir = std::env::temp_dir().join(format!("sl-tui-{}-{:?}", std::process::id(), rows));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = Config::default();
        for pattern in rows {
            config.upsert(spec(pattern), true);
        }
        App {
            config,
            path: dir.join("config.toml"),
            runtime: RuntimeDir::at(dir.join("rt")),
            statuses: HashMap::new(),
            selected: 0,
            mode: Mode::Normal,
            message: None,
            filter: String::new(),
            filtering: false,
            log: Vec::new(),
            sessions: None,
            quit: false,
        }
    }

    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn empty_panel_explains_how_to_add_a_row() {
        let mut app = panel(&[]);
        let text = screen(&mut app, 90, 24);
        assert!(text.contains("No forwards yet"), "{text}");
        assert!(text.contains("wk:8000"), "{text}");
    }

    #[test]
    fn table_shows_rows_status_and_key_hints() {
        let mut app = panel(&["wk:8000", "8001:ds:8080"]);
        app.statuses.insert(
            spec("wk:8000"),
            Status::Up {
                since: std::time::SystemTime::now(),
            },
        );
        let text = screen(&mut app, 100, 30);
        assert!(text.contains("wk"), "{text}");
        assert!(text.contains(":8000"), "{text}");
        assert!(text.contains("ds:8080"), "{text}");
        assert!(text.contains("up"), "{text}");
        assert!(text.contains("stopped"), "{text}");
        assert!(text.contains("restart"), "{text}");
    }

    #[test]
    fn long_ssh_reason_stays_in_its_cell() {
        let mut app = panel(&["wk:8000"]);
        app.statuses.insert(
            spec("wk:8000"),
            Status::Failed {
                reason: "bind [127.0.0.1]:8000: Address already in use".into(),
            },
        );
        let text = screen(&mut app, 100, 30);
        assert!(text.contains("already in use"), "{text}");
        assert!(
            !text.contains("bind [127.0.0.1]"),
            "reason must be shortened:\n{text}"
        );
    }

    #[test]
    fn editor_overlay_shows_the_pattern_and_errors() {
        let mut app = panel(&[]);
        app.mode = Mode::Editor(Editor {
            input: super::input::TextInput::new("wk:8x"),
            direction: Direction::Remote,
            original: None,
            error: Some("invalid port '8x'".into()),
        });
        let text = screen(&mut app, 100, 30);
        assert!(text.contains("new forward"), "{text}");
        assert!(text.contains("wk:8x"), "{text}");
        assert!(text.contains("remote"), "{text}");
        assert!(text.contains("invalid port '8x'"), "{text}");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn text(value: &str) -> Vec<KeyEvent> {
        value
            .chars()
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .collect()
    }

    #[test]
    fn typing_edits_at_the_cursor() {
        let mut app = panel(&[]);
        app.new_editor();
        for event in text("8000") {
            keys::handle(&mut app, event);
        }
        keys::handle(&mut app, key(KeyCode::Home));
        for event in text("wk:") {
            keys::handle(&mut app, event);
        }
        keys::handle(&mut app, key(KeyCode::End));
        keys::handle(&mut app, key(KeyCode::Left));
        keys::handle(&mut app, key(KeyCode::Delete));
        match &app.mode {
            Mode::Editor(editor) => assert_eq!(editor.input.value(), "wk:800"),
            _ => panic!("editor must stay open"),
        }
    }

    #[test]
    fn pasting_a_list_reaches_the_editor_in_one_piece() {
        let mut app = panel(&[]);
        app.new_editor();
        keys::handle_paste(&mut app, "wk:8005,9200,16379\n");
        match &app.mode {
            Mode::Editor(editor) => assert_eq!(editor.input.value(), "wk:8005,9200,16379"),
            _ => panic!("editor must stay open"),
        }
        app.save_editor();
        assert_eq!(app.config.forward.len(), 3);
    }

    #[test]
    fn q_quits_without_asking_even_with_forwards_running() {
        let mut app = panel(&["wk:8000"]);
        app.statuses.insert(
            spec("wk:8000"),
            Status::Up {
                since: std::time::SystemTime::now(),
            },
        );
        keys::handle(&mut app, key(KeyCode::Char('q')));
        assert!(app.quit);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn pasting_into_the_filter_filters() {
        let mut app = panel(&["wk:8000", "ds:9000"]);
        app.filtering = true;
        keys::handle_paste(&mut app, " ds ");
        assert_eq!(app.filter, "ds");
        assert_eq!(app.visible().len(), 1);
    }

    #[test]
    fn help_overlay_lists_the_keys() {
        let mut app = panel(&["wk:8000"]);
        app.mode = Mode::Help;
        let text = screen(&mut app, 100, 34);
        assert!(text.contains("restart the selected forward"), "{text}");
    }
}
