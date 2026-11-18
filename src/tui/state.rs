//! Panel state: the rows on screen, what the engine says about them, and the
//! one overlay that is open.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::config::Config;
use crate::error::Result;
use crate::forward::{Direction, ForwardSpec};
use crate::runtime::{Owner, RuntimeDir, SessionRecord, process_alive};
use crate::sessions::{EngineConfig, Sessions, Status};

#[derive(Clone, Debug)]
pub struct Row {
    pub spec: ForwardSpec,
    pub enabled: bool,
    pub owner: Owner,
}

pub enum Mode {
    Normal,
    Editor(Editor),
    Confirm(Confirm),
    Help,
}

pub struct Editor {
    pub pattern: String,
    pub direction: Direction,
    /// The row being edited, when this is not a fresh row.
    pub original: Option<ForwardSpec>,
    pub error: Option<String>,
}

pub enum Confirm {
    Delete(ForwardSpec),
    Quit,
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
    pub rows: Vec<Row>,
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
        let reaped = app.runtime.reap_orphans();
        if reaped > 0 {
            app.info(format!(
                "reclaimed {reaped} forward(s) left by a closed panel"
            ));
        }
        app.rebuild_rows();
        app.start_engine()?;
        app.refresh_log();
        Ok(app)
    }

    fn start_engine(&mut self) -> Result<()> {
        let rows: Vec<(ForwardSpec, bool)> = self
            .rows
            .iter()
            .filter(|row| row.owner == Owner::Panel)
            .map(|row| (row.spec.clone(), row.enabled))
            .collect();
        let cfg = EngineConfig::new(
            self.config.options.clone(),
            self.runtime.clone(),
            Owner::Panel,
        )
        .batch_mode(true);
        self.sessions = Some(Sessions::start(rows, cfg)?);
        Ok(())
    }

    /// Rows come from the config plus whatever daemons are already running.
    pub fn rebuild_rows(&mut self) {
        let records = self.runtime.records().unwrap_or_default();
        let mut rows: Vec<Row> = Vec::new();

        for entry in &self.config.forward {
            match live_daemon(&records, &entry.spec) {
                Some(record) => {
                    self.statuses.insert(
                        entry.spec.clone(),
                        Status::Up {
                            since: started_at(record),
                        },
                    );
                    rows.push(Row {
                        spec: entry.spec.clone(),
                        enabled: true,
                        owner: Owner::Daemon,
                    });
                }
                None => rows.push(Row {
                    spec: entry.spec.clone(),
                    enabled: entry.enabled,
                    owner: Owner::Panel,
                }),
            }
        }
        for record in &records {
            if record.owner != Owner::Daemon || !process_alive(record.ssh_pid) {
                continue;
            }
            if rows.iter().any(|row| row.spec == record.spec) {
                continue;
            }
            self.statuses.insert(
                record.spec.clone(),
                Status::Up {
                    since: started_at(record),
                },
            );
            rows.push(Row {
                spec: record.spec.clone(),
                enabled: true,
                owner: Owner::Daemon,
            });
        }

        self.rows = rows;
        self.clamp_selection();
    }

    pub fn drain_events(&mut self) {
        let Some(sessions) = &self.sessions else {
            return;
        };
        while let Ok(event) = sessions.events().try_recv() {
            self.statuses.insert(event.spec, event.status);
        }
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

    pub fn matches_filter(&self, row: &Row) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_lowercase();
        row.spec.host.as_str().to_lowercase().contains(&needle)
            || row.spec.entry.to_string().contains(&needle)
            || row.spec.target.to_string().contains(&needle)
    }

    pub fn visible(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.matches_filter(row))
            .map(|(idx, _)| idx)
            .collect()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        let visible = self.visible();
        visible
            .get(self.selected)
            .and_then(|idx| self.rows.get(*idx))
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

    pub fn toggle_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.owner == Owner::Daemon {
            self.info("this row runs outside the panel — stop it with: sl -k <host>");
            return;
        }
        let run = !row.enabled;
        if let Some(entry) = self.config.forward.iter_mut().find(|e| e.spec == row.spec) {
            entry.enabled = run;
        }
        if !self.commit() {
            return;
        }
        if let Some(sessions) = &self.sessions {
            sessions.set_running(&row.spec, run);
        }
        self.rebuild_rows();
        self.info(format!(
            "{} {}",
            if run { "started" } else { "stopped" },
            row.spec
        ));
    }

    pub fn restart_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.owner == Owner::Daemon {
            self.info("this row runs outside the panel — restart it with: sl -k && sl -d");
            return;
        }
        if let Some(entry) = self.config.forward.iter_mut().find(|e| e.spec == row.spec) {
            entry.enabled = true;
        }
        let _ = self.commit();
        if let Some(sessions) = &self.sessions {
            sessions.restart(&row.spec);
        }
        self.rebuild_rows();
        self.success(format!("restarting {}", row.spec));
    }

    pub fn set_all(&mut self, run: bool) {
        for entry in &mut self.config.forward {
            entry.enabled = run;
        }
        let _ = self.commit();
        if let Some(sessions) = &self.sessions {
            for row in self.rows.iter().filter(|r| r.owner == Owner::Panel) {
                sessions.set_running(&row.spec, run);
            }
        }
        self.rebuild_rows();
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
        let is_daemon = self
            .rows
            .iter()
            .any(|row| row.spec == *spec && row.owner == Owner::Daemon);
        if is_daemon {
            self.runtime.kill_spec(spec);
        }
        if let Some(sessions) = &self.sessions {
            sessions.set_running(spec, false);
        }
        self.config.remove(spec);
        let _ = self.runtime.remove_record(spec);
        self.statuses.remove(spec);
        let _ = self.commit();
        self.rebuild_rows();
        self.refresh_log();
        self.success(format!("removed {spec}"));
    }

    pub fn request_quit(&mut self) {
        let running = self
            .rows
            .iter()
            .filter(|row| self.status_of(&row.spec).is_up())
            .count();
        if running == 0 {
            self.quit = true;
        } else {
            self.mode = Mode::Confirm(Confirm::Quit);
        }
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

    pub fn open_editor(&mut self) {
        let row = self.selected_row().cloned();
        match row {
            Some(row) if row.owner == Owner::Daemon => {
                self.info("this row runs outside the panel; it cannot be edited here")
            }
            Some(row) => {
                self.mode = Mode::Editor(Editor {
                    pattern: row.spec.pattern(),
                    direction: row.spec.direction,
                    original: Some(row.spec),
                    error: None,
                })
            }
            None => {
                self.mode = Mode::Editor(Editor {
                    pattern: String::new(),
                    direction: Direction::Local,
                    original: None,
                    error: None,
                })
            }
        }
    }

    pub fn new_editor(&mut self) {
        self.mode = Mode::Editor(Editor {
            pattern: String::new(),
            direction: Direction::Local,
            original: None,
            error: None,
        });
    }

    /// Parses the editor's single pattern line with the same parser the CLI
    /// uses, then applies it to a copy of the config so a rejection changes
    /// nothing.
    pub fn save_editor(&mut self) {
        let Mode::Editor(editor) = &self.mode else {
            return;
        };
        let patterns = vec![editor.pattern.trim().to_string()];
        let direction = editor.direction;
        let original = editor.original.clone();
        let spec = match ForwardSpec::parse_all(&patterns, direction) {
            Ok(mut specs) => specs.remove(0),
            Err(e) => {
                if let Mode::Editor(editor) = &mut self.mode {
                    editor.error = Some(e.to_string());
                }
                return;
            }
        };

        let mut next = self.config.clone();
        if let Some(old) = &original
            && *old != spec
        {
            next.remove(old);
        }
        next.upsert(spec.clone(), true);
        if let Err(e) = next.validate() {
            if let Mode::Editor(editor) = &mut self.mode {
                editor.error = Some(e.to_string());
            }
            return;
        }
        self.config = next;
        if !self.commit() {
            return;
        }
        if let Some(old) = &original
            && *old != spec
        {
            if let Some(sessions) = &self.sessions {
                sessions.set_running(old, false);
            }
            let _ = self.runtime.remove_record(old);
        }
        if let Some(sessions) = &self.sessions {
            sessions.set_running(&spec, true);
        }
        self.mode = Mode::Normal;
        self.rebuild_rows();
        if let Some(idx) = self.rows.iter().position(|row| row.spec == spec) {
            self.selected = self
                .visible()
                .iter()
                .position(|visible| *visible == idx)
                .unwrap_or(self.selected);
        }
        self.refresh_log();
        self.success(format!("saved {spec}"));
    }

    pub fn shutdown(&mut self) {
        if let Some(sessions) = self.sessions.take() {
            sessions.shutdown();
        }
    }
}

fn live_daemon<'a>(records: &'a [SessionRecord], spec: &ForwardSpec) -> Option<&'a SessionRecord> {
    records.iter().find(|record| {
        record.spec == *spec && record.owner == Owner::Daemon && process_alive(record.ssh_pid)
    })
}

fn started_at(record: &SessionRecord) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(record.started_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::ForwardSpec;

    fn spec(pattern: &str) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, Direction::Local)
            .unwrap()
            .remove(0)
    }

    fn app_with(rows: &[(&str, bool)]) -> App {
        let dir = std::env::temp_dir().join(format!("sl-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = Config::default();
        for (pattern, enabled) in rows {
            config.upsert(spec(pattern), *enabled);
        }
        App {
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
        }
    }

    #[test]
    fn filter_keeps_matching_rows_selectable() {
        let mut app = app_with(&[("wk:8000", true), ("ds:9000", true)]);
        app.rebuild_rows();
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
    fn editor_rejects_bad_pattern_without_touching_config() {
        let mut app = app_with(&[]);
        app.mode = Mode::Editor(Editor {
            pattern: "wk:x".into(),
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
    fn editor_refuses_a_port_conflict_and_stays_open() {
        let mut app = app_with(&[("wk:8000", true)]);
        app.mode = Mode::Editor(Editor {
            pattern: "ds:8000".into(),
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
