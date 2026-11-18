//! The panel: terminal setup, the event loop, and the render cadence.

pub mod dialogs;
pub mod editor;
pub mod keys;
pub mod logs;
pub mod state;
pub mod table;
pub mod theme;
pub mod view;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
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
const RECORD_SYNC: Duration = Duration::from_secs(1);

pub fn run(path: PathBuf) -> Result<i32> {
    let mut app = App::new(path)?;
    let mut terminal = enter()?;
    let outcome = event_loop(&mut terminal, &mut app);
    let restored = restore();
    app.shutdown();
    outcome?;
    restored?;
    Ok(0)
}

fn enter() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let mut next_log = Instant::now();
    let mut next_sync = Instant::now() + RECORD_SYNC;
    loop {
        terminal.draw(|frame| view::draw(frame, app))?;

        if event::poll(EVENT_POLL)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            keys::handle(app, key);
        }
        app.drain_events();

        let now = Instant::now();
        if now >= next_log {
            app.refresh_log();
            next_log = now + LOG_REFRESH;
        }
        if now >= next_sync {
            app.rebuild_rows();
            next_sync = now + RECORD_SYNC;
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
        let mut app = App {
            config,
            path: dir.join("config.toml"),
            runtime: RuntimeDir::at(dir.join("rt")),
            rows: Vec::new(),
            statuses: HashMap::new(),
            selected: 0,
            mode: Mode::Normal,
            message: None,
            filter: String::new(),
            filtering: false,
            log: Vec::new(),
            sessions: None,
            quit: false,
        };
        app.rebuild_rows();
        app
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
            pattern: "wk:8x".into(),
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

    #[test]
    fn help_overlay_lists_the_keys() {
        let mut app = panel(&["wk:8000"]);
        app.mode = Mode::Help;
        let text = screen(&mut app, 100, 34);
        assert!(text.contains("restart the selected forward"), "{text}");
        assert!(text.contains("sl -k"), "{text}");
    }
}
