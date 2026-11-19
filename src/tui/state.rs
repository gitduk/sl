//! Panel state: the rows on screen, what the engine says about them, and the
//! one overlay that is open.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::{Config, Entry};
use crate::error::Result;
use crate::forward::{Direction, ForwardSpec};
use crate::runtime::RuntimeDir;
use crate::sessions::{EngineConfig, Sessions, Status};

use super::input::TextInput;

pub enum Mode {
    Normal,
    Editor(Editor),
    Confirm(Confirm),
    Help,
}

pub struct Editor {
    pub input: TextInput,
    pub direction: Direction,
    /// The row being edited, when this is not a fresh row.
    pub original: Option<ForwardSpec>,
    pub error: Option<String>,
}

impl Editor {
    pub fn for_spec(original: Option<ForwardSpec>) -> Self {
        let (pattern, direction) = match &original {
            Some(spec) => (spec.pattern(), spec.direction),
            None => (String::new(), Direction::Local),
        };
        Editor {
            input: TextInput::new(&pattern),
            direction,
            original,
            error: None,
        }
    }
}

pub enum Confirm {
    Delete(ForwardSpec),
}

pub struct Message {
    pub text: String,
    pub level: Level,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Info,
    Success,
    Error,
}

pub struct App {
    pub config: Config,
    pub path: PathBuf,
    pub runtime: RuntimeDir,
    pub statuses: HashMap<ForwardSpec, Status>,
    pub selected: usize,
    pub mode: Mode,
    pub message: Option<Message>,
    pub filter: String,
    pub filtering: bool,
    pub log: Vec<String>,
    pub sessions: Option<Sessions>,
    pub quit: bool,
}

impl App {
    pub fn new(path: PathBuf) -> Result<App> {
        let config = Config::load(&path)?;
        let runtime = RuntimeDir::resolve()?;
        let mut app = App {
            config,
            path,
            runtime,
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
        app.runtime.prune_dead();
        app.start_engine()?;
        app.drain_events();
        app.refresh_log();
        Ok(app)
    }

    fn start_engine(&mut self) -> Result<()> {
        self.prime_statuses();
        let rows: Vec<(ForwardSpec, bool)> = self
            .config
            .forward
            .iter()
            .map(|entry| (entry.spec.clone(), entry.enabled))
            .collect();
        let cfg =
            EngineConfig::new(self.config.options.clone(), self.runtime.clone()).batch_mode(true);
        self.sessions = Some(Sessions::start(rows, cfg)?);
        Ok(())
    }

    /// Seeds each row with the state it is really in, read from the runtime
    /// records: a forward a previous panel left running is already up, so the
    /// first frame never shows a placeholder for it.
    fn prime_statuses(&mut self) {
        let seeded: Vec<(ForwardSpec, Status)> = self
            .config
            .forward
            .iter()
            .filter(|entry| !self.statuses.contains_key(&entry.spec))
            .map(|entry| {
                let status = match crate::sessions::adopted(&self.runtime, &entry.spec) {
                    Some(record) => Status::Up {
                        since: std::time::UNIX_EPOCH + Duration::from_secs(record.started_at),
                    },
                    None if entry.enabled => Status::Starting,
                    None => Status::Stopped,
                };
                (entry.spec.clone(), status)
            })
            .collect();
        self.statuses.extend(seeded);
    }

    /// Applies every queued engine event; returns how many arrived, which is
    /// also "the screen is now stale" for the caller.
    pub fn drain_events(&mut self) -> usize {
        let Some(sessions) = &self.sessions else {
            return 0;
        };
        let mut applied = 0;
        while let Ok(event) = sessions.events().try_recv() {
            self.statuses.insert(event.spec, event.status);
            applied += 1;
        }
        applied
    }

    pub fn status_ref(&self, spec: &ForwardSpec) -> Option<&Status> {
        self.statuses.get(spec)
    }

    pub fn status_of(&self, spec: &ForwardSpec) -> Status {
        self.statuses.get(spec).cloned().unwrap_or(Status::Stopped)
    }

    pub fn refresh_log(&mut self) {
        self.log = match self.selected_row() {
            Some(row) => self.runtime.log_tail(&row.spec, 200),
            None => Vec::new(),
        };
    }

    fn matches_filter(entry: &Entry, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let spec = &entry.spec;
        spec.host.as_str().to_lowercase().contains(needle)
            || spec.entry.to_string().contains(needle)
            || spec.target.to_string().contains(needle)
    }

    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.config
            .forward
            .iter()
            .enumerate()
            .filter(|(_, entry)| Self::matches_filter(entry, &needle))
            .map(|(idx, _)| idx)
            .collect()
    }

    pub fn selected_row(&self) -> Option<&Entry> {
        let visible = self.visible();
        visible
            .get(self.selected)
            .and_then(|idx| self.config.forward.get(*idx))
    }

    fn clamp_selection(&mut self) {
        let count = self.visible().len();
        if self.selected + 1 > count {
            self.selected = count.saturating_sub(1);
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let last = count - 1;
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, last as isize) as usize;
        self.refresh_log();
    }

    pub fn select(&mut self, index: usize) {
        let count = self.visible().len();
        if count > 0 {
            self.selected = index.min(count - 1);
        }
        self.refresh_log();
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            level: Level::Info,
        });
    }

    pub fn success(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            level: Level::Success,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            level: Level::Error,
        });
    }

    /// Saves the config, reporting a rejection instead of applying it.
    fn commit(&mut self) -> bool {
        match self.config.validate() {
            Ok(()) => match self.config.save(&self.path) {
                Ok(()) => true,
                Err(e) => {
                    self.error(format!("cannot save config: {e}"));
                    false
                }
            },
            Err(e) => {
                self.error(e.to_string());
                false
            }
        }
    }

    /// Writes `enabled` for these rows and saves the config.
    fn persist_enabled(&mut self, specs: &[ForwardSpec], run: bool) -> bool {
        for spec in specs {
            if let Some(entry) = self.config.forward.iter_mut().find(|e| &e.spec == spec) {
                entry.enabled = run;
            }
        }
        self.commit()
    }

    /// Writes `enabled` for these rows, saves the config, and tells the engine.
    fn apply_enabled(&mut self, specs: &[ForwardSpec], run: bool) {
        if !self.persist_enabled(specs, run) {
            return;
        }
        if let Some(sessions) = &self.sessions {
            for spec in specs {
                sessions.set_running(spec, run);
            }
        }
        self.clamp_selection();
    }

    pub fn toggle_selected(&mut self) {
        let Some(entry) = self.selected_row().cloned() else {
            return;
        };
        let run = !entry.enabled;
        self.apply_enabled(std::slice::from_ref(&entry.spec), run);
        self.info(format!(
            "{} {}",
            if run { "started" } else { "stopped" },
            entry.spec
        ));
    }

    pub fn restart_selected(&mut self) {
        let Some(entry) = self.selected_row().cloned() else {
            return;
        };
        if self.restart(std::slice::from_ref(&entry.spec)) {
            self.success(format!("restarting {}", entry.spec));
        }
    }

    pub fn restart_all(&mut self) {
        let specs = self.all_specs();
        if specs.is_empty() {
            return;
        }
        if self.restart(&specs) {
            self.success(format!("restarting {} rows", specs.len()));
        }
    }

    /// Enables these rows and asks the engine for a fresh child each; `Restart`
    /// alone starts a stopped row. False when the save fails, error kept.
    fn restart(&mut self, specs: &[ForwardSpec]) -> bool {
        if !self.persist_enabled(specs, true) {
            return false;
        }
        if let Some(sessions) = &self.sessions {
            for spec in specs {
                sessions.restart(spec);
            }
        }
        self.clamp_selection();
        true
    }

    /// Every configured row's spec, in display order.
    fn all_specs(&self) -> Vec<ForwardSpec> {
        self.config
            .forward
            .iter()
            .map(|entry| entry.spec.clone())
            .collect()
    }

    pub fn set_all(&mut self, run: bool) {
        let specs = self.all_specs();
        self.apply_enabled(&specs, run);
        self.info(if run {
            "started all rows"
        } else {
            "stopped all rows"
        });
    }

    pub fn request_delete(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.mode = Mode::Confirm(Confirm::Delete(row.spec));
    }

    pub fn delete_row(&mut self, spec: &ForwardSpec) {
        if let Some(sessions) = &self.sessions {
            sessions.set_running(spec, false);
        }
        self.config.remove(spec);
        let _ = self.runtime.remove_record(spec);
        self.statuses.remove(spec);
        let _ = self.commit();
        self.clamp_selection();
        self.refresh_log();
        self.success(format!("removed {spec}"));
    }

    pub fn test_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        match row.spec.direction {
            Direction::Local => {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], row.spec.entry.get()));
                let probe = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500));
                match probe {
                    Ok(_) => self.success(format!("127.0.0.1:{} is listening", row.spec.entry)),
                    Err(e) => {
                        self.error(format!("127.0.0.1:{} not listening: {e}", row.spec.entry))
                    }
                }
            }
            Direction::Remote => {
                self.info("remote rows: the state shown is what ssh reports");
            }
        }
    }

    /// Opens the editor on the selected row, or empty when nothing is selected.
    pub fn open_editor(&mut self) {
        let original = self.selected_row().map(|entry| entry.spec.clone());
        self.mode = Mode::Editor(Editor::for_spec(original));
    }

    pub fn new_editor(&mut self) {
        self.mode = Mode::Editor(Editor::for_spec(None));
    }

    /// Parses the editor's pattern line with the parser the CLI uses and
    /// applies every row it expands to; a rejection changes nothing.
    pub fn save_editor(&mut self) {
        let Mode::Editor(editor) = &self.mode else {
            return;
        };
        let patterns = vec![editor.input.value().trim().to_string()];
        let direction = editor.direction;
        let original = editor.original.clone();
        let specs = match ForwardSpec::parse_all(&patterns, direction) {
            Ok(specs) => specs,
            Err(e) => return self.editor_error(e.to_string()),
        };
        if original.is_some() && specs.len() > 1 {
            return self.editor_error(
                "editing one row takes one pattern; press n to add several".to_string(),
            );
        }

        let mut next = self.config.clone();
        if let Some(old) = &original
            && !specs.contains(old)
        {
            next.remove(old);
        }
        for spec in &specs {
            next.upsert(spec.clone(), true);
        }
        if let Err(e) = next.validate() {
            return self.editor_error(e.to_string());
        }

        self.config = next;
        if !self.commit() {
            return;
        }
        if let Some(old) = &original
            && !specs.contains(old)
        {
            if let Some(sessions) = &self.sessions {
                sessions.set_running(old, false);
            }
            let _ = self.runtime.remove_record(old);
        }
        if let Some(sessions) = &self.sessions {
            for spec in &specs {
                sessions.set_running(spec, true);
            }
        }

        self.mode = Mode::Normal;
        let first = specs[0].clone();
        if let Some(idx) = self.config.position(&first) {
            self.selected = self
                .visible()
                .iter()
                .position(|visible| *visible == idx)
                .unwrap_or(self.selected);
        }
        self.refresh_log();
        let message = match specs.len() {
            1 => format!("saved {first}"),
            count => format!("saved {count} rows for {}", first.host),
        };
        self.success(message);
    }

    fn editor_error(&mut self, message: String) {
        if let Mode::Editor(editor) = &mut self.mode {
            editor.error = Some(message);
        }
    }

    /// Forwards outlive the panel: quitting leaves them running for the next
    /// panel to adopt.
    pub fn release_forwarding(&mut self) {
        if let Some(sessions) = self.sessions.take() {
            sessions.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use super::*;
    use crate::forward::ForwardSpec;
    use crate::runtime::SessionRecord;

    fn spec(pattern: &str) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, Direction::Local)
            .unwrap()
            .remove(0)
    }

    fn app_with(rows: &[(&str, bool)]) -> App {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let tag = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("sl-app-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = Config::default();
        for (pattern, enabled) in rows {
            config.upsert(spec(pattern), *enabled);
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

    #[test]
    fn first_paint_shows_what_the_engine_is_about_to_do() {
        let mut app = app_with(&[("wk:8000", true), ("ds:9000", false)]);
        app.prime_statuses();
        assert!(matches!(app.status_of(&spec("wk:8000")), Status::Starting));
        assert!(matches!(app.status_of(&spec("ds:9000")), Status::Stopped));

        app.statuses.insert(
            spec("wk:8000"),
            Status::Up {
                since: SystemTime::now(),
            },
        );
        app.prime_statuses();
        assert!(
            app.status_of(&spec("wk:8000")).is_up(),
            "priming must not overwrite a known status"
        );
    }

    #[test]
    fn first_paint_reads_a_running_tunnel_from_its_record() {
        let mut app = app_with(&[("wk:8000", true)]);
        let live = spec("wk:8000");
        let mut stand_in = std::process::Command::new("yes")
            .arg("--")
            .arg(live.forward_arg())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawns yes");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !crate::runtime::process_carries(stand_in.id(), &live.forward_arg())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        app.runtime
            .write_record(&SessionRecord {
                spec: live.clone(),
                ssh_pid: stand_in.id(),
                started_at: 1_700_000_000,
            })
            .unwrap();

        app.prime_statuses();
        match app.status_of(&live) {
            Status::Up { since } => assert_eq!(
                since
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("epoch")
                    .as_secs(),
                1_700_000_000
            ),
            other => panic!("a running tunnel must prime as up: {other:?}"),
        }

        let _ = stand_in.kill();
        let _ = stand_in.wait();
    }

    #[test]
    fn filter_keeps_matching_rows_selectable() {
        let mut app = app_with(&[("wk:8000", true), ("ds:9000", true)]);
        assert_eq!(app.visible().len(), 2);
        app.filter = "90".into();
        assert_eq!(app.visible(), vec![1]);
        app.select(5);
        assert_eq!(app.selected, 0);
        assert_eq!(app.selected_row().unwrap().spec, spec("ds:9000"));
    }

    #[test]
    fn status_falls_back_to_stopped() {
        let app = app_with(&[("wk:8000", true)]);
        assert!(matches!(app.status_of(&spec("wk:8000")), Status::Stopped));
    }

    #[test]
    fn restart_all_enables_every_row() {
        let mut app = app_with(&[("wk:8000", false), ("ds:9000", false)]);
        app.restart_all();
        assert!(app.config.forward.iter().all(|entry| entry.enabled));
        assert!(app.message.is_some(), "the restart is reported");
    }

    #[test]
    fn restart_all_with_no_rows_reports_nothing() {
        let mut app = app_with(&[]);
        app.restart_all();
        assert!(app.message.is_none());
    }

    #[test]
    fn editor_rejects_bad_pattern_without_touching_config() {
        let mut app = app_with(&[]);
        app.mode = Mode::Editor(Editor {
            input: TextInput::new("wk:x"),
            direction: Direction::Local,
            original: None,
            error: None,
        });
        app.save_editor();
        assert!(app.config.forward.is_empty());
        match &app.mode {
            Mode::Editor(editor) => assert!(editor.error.is_some()),
            _ => panic!("editor must stay open on a bad pattern"),
        }
    }

    #[test]
    fn editor_expands_a_comma_list_into_one_row_per_port() {
        let mut app = app_with(&[]);
        app.mode = Mode::Editor(Editor {
            input: TextInput::new("wk:8005,9200,16379,19530,4000,3021,8006,10001"),
            direction: Direction::Local,
            original: None,
            error: None,
        });
        app.save_editor();
        let ports: Vec<u16> = app
            .config
            .forward
            .iter()
            .map(|entry| entry.spec.entry.get())
            .collect();
        assert_eq!(
            ports,
            vec![8005, 9200, 16379, 19530, 4000, 3021, 8006, 10001]
        );
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(Config::load(&app.path).unwrap().forward.len(), 8);
    }

    #[test]
    fn editing_one_row_refuses_a_comma_list() {
        let mut app = app_with(&[("wk:8000", true)]);
        let original = app.config.forward[0].spec.clone();
        app.mode = Mode::Editor(Editor {
            input: TextInput::new("wk:8000,8001"),
            direction: Direction::Local,
            original: Some(original),
            error: None,
        });
        app.save_editor();
        assert_eq!(app.config.forward.len(), 1);
        match &app.mode {
            Mode::Editor(editor) => {
                let error = editor.error.as_deref().unwrap_or_default();
                assert!(error.contains("press n"), "{error}");
            }
            _ => panic!("editor must stay open"),
        }
    }

    #[test]
    fn editor_refuses_a_port_conflict_and_stays_open() {
        let mut app = app_with(&[("wk:8000", true)]);
        app.mode = Mode::Editor(Editor {
            input: TextInput::new("ds:8000"),
            direction: Direction::Local,
            original: None,
            error: None,
        });
        app.save_editor();
        assert_eq!(app.config.forward.len(), 1);
        match &app.mode {
            Mode::Editor(editor) => {
                let error = editor.error.as_deref().unwrap_or_default();
                assert!(error.contains("127.0.0.1:8000"), "{error}");
            }
            _ => panic!("editor must stay open on a conflict"),
        }
    }
}
