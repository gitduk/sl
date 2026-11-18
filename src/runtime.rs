//! Runtime state: which sessions are live and where their ssh stderr goes.
//!
//! Nothing here is worth keeping across reboots, so it all lives under
//! `${XDG_RUNTIME_DIR:-/tmp}/sl` and stale records are thrown away on sight.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::forward::{ForwardSpec, Host};

/// A log file past this size is cut back to its most recent half.
pub const LOG_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    /// Started by the panel; dies with it.
    Panel,
    /// Started by a headless supervisor that outlives the terminal.
    Daemon,
}

impl Owner {
    pub fn as_str(self) -> &'static str {
        match self {
            Owner::Panel => "panel",
            Owner::Daemon => "daemon",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub spec: ForwardSpec,
    pub owner: Owner,
    pub ssh_pid: u32,
    #[serde(default)]
    pub supervisor_pid: Option<u32>,
    pub started_at: u64,
}

#[derive(Debug, Clone)]
pub struct RuntimeDir {
    root: PathBuf,
}

impl RuntimeDir {
    pub fn resolve() -> Result<Self> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = RuntimeDir::at(base.join("sl"));
        dir.ensure()?;
        Ok(dir)
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        RuntimeDir { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub fn record_path(&self, spec: &ForwardSpec) -> PathBuf {
        self.root.join(format!("{}.json", spec.record_stem()))
    }

    pub fn log_path(&self, spec: &ForwardSpec) -> PathBuf {
        self.root.join(format!("{}.log", spec.record_stem()))
    }

    pub fn write_record(&self, record: &SessionRecord) -> Result<()> {
        self.ensure()?;
        let body = serde_json::to_vec(record)
            .map_err(|e| Error::Runtime(format!("cannot serialize record: {e}")))?;
        let path = self.record_path(&record.spec);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn remove_record(&self, spec: &ForwardSpec) -> Result<()> {
        match fs::remove_file(self.record_path(spec)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Live records; anything unreadable is deleted rather than reported.
    pub fn records(&self) -> Result<Vec<SessionRecord>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut records = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<SessionRecord>(&raw).ok())
            {
                Some(record) => records.push(record),
                None => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        records.sort_by_key(|record| record.spec.record_stem());
        Ok(records)
    }

    pub fn record_for(&self, spec: &ForwardSpec) -> Option<SessionRecord> {
        let raw = fs::read(self.record_path(spec)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Append target for one session's stderr, rotating an oversized log first.
    pub fn open_log(&self, spec: &ForwardSpec) -> Result<File> {
        self.ensure()?;
        let path = self.log_path(spec);
        if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
            trim_log(&path)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(Error::Io)
    }

    pub fn log_tail(&self, spec: &ForwardSpec, max_lines: usize) -> Vec<String> {
        let path = self.log_path(spec);
        let Ok(mut file) = File::open(&path) else {
            return Vec::new();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let start = len.saturating_sub(LOG_LIMIT);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if file.take(LOG_LIMIT).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();
        let skip = lines.len().saturating_sub(max_lines);
        lines[skip..].to_vec()
    }

    pub fn last_log_line(&self, spec: &ForwardSpec) -> Option<String> {
        self.log_tail(spec, 1).pop()
    }

    /// Stops the sessions of `hosts` (all hosts when empty); returns how many
    /// records were dealt with, including ones found already dead.
    pub fn kill(&self, hosts: &[Host]) -> Result<usize> {
        let mut count = 0;
        for record in self.records()? {
            if !hosts.is_empty() && !hosts.contains(&record.spec.host) {
                continue;
            }
            terminate(&record);
            self.remove_record(&record.spec)?;
            count += 1;
        }
        Ok(count)
    }

    /// Drops records whose ssh is gone, and kills ssh processes the panel
    /// started but no longer supervises (their parent is init after a crash) —
    /// otherwise they hold their listen port and the next panel cannot rebind.
    pub fn reap_orphans(&self) -> usize {
        let mut killed = 0;
        for record in self.records().unwrap_or_default() {
            if record.owner != Owner::Panel {
                continue;
            }
            if !process_alive(record.ssh_pid) {
                let _ = self.remove_record(&record.spec);
                continue;
            }
            if parent_pid(record.ssh_pid) == Some(1) {
                terminate(&record);
                let _ = self.remove_record(&record.spec);
                killed += 1;
            }
        }
        killed
    }

    pub fn kill_spec(&self, spec: &ForwardSpec) -> bool {
        let Some(record) = self.record_for(spec) else {
            return false;
        };
        terminate(&record);
        let _ = self.remove_record(spec);
        true
    }
}

/// Signals a record's processes, but only when the pid still carries the
/// command line we started it with — a recycled pid must not be hit.
fn terminate(record: &SessionRecord) {
    if let Some(pid) = record.supervisor_pid
        && cmdline_contains(pid, "--daemon")
    {
        signal(pid);
    }
    if cmdline_contains(record.ssh_pid, &record.spec.forward_arg()) {
        signal(record.ssh_pid);
    }
}

fn cmdline_contains(pid: u32, needle: &str) -> bool {
    cmdline(pid).iter().any(|arg| arg.contains(needle))
}

pub fn cmdline(pid: u32) -> Vec<String> {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    raw.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// A zombie is dead for our purposes: it only waits for its parent to reap it.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let state = stat
        .rfind(')')
        .and_then(|idx| stat[idx + 1..].split_whitespace().next());
    !matches!(state, Some("Z") | Some("X") | Some("x"))
}

/// Field 4 of `/proc/<pid>/stat`, parsing past a comm that may contain spaces.
pub fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

fn signal(pid: u32) {
    if pid == 0 {
        return;
    }
    // SAFETY: kill(2) takes a pid and a signal number; a dead pid just returns ESRCH.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

fn trim_log(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(LOG_LIMIT / 2);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let body = match buf.iter().position(|b| *b == b'\n') {
        Some(idx) => buf[idx + 1..].to_vec(),
        None => buf,
    };
    let mut out = File::create(path)?;
    out.write_all(&body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::{Direction, ForwardSpec};
    use std::time::{Duration, Instant};

    fn spec(pattern: &str) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, Direction::Local)
            .unwrap()
            .remove(0)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sl-rt-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn records_round_trip_and_disappear() {
        let dir = RuntimeDir::at(temp_dir("round"));
        let spec = spec("wk:8000");
        let record = SessionRecord {
            spec: spec.clone(),
            owner: Owner::Daemon,
            ssh_pid: 4242,
            supervisor_pid: None,
            started_at: 1,
        };
        dir.write_record(&record).unwrap();
        assert_eq!(dir.records().unwrap().len(), 1);
        assert_eq!(dir.record_for(&spec).unwrap().owner, Owner::Daemon);
        dir.remove_record(&spec).unwrap();
        assert!(dir.records().unwrap().is_empty());
        dir.remove_record(&spec).unwrap();
        let _ = fs::remove_dir_all(dir.root());
    }

    #[test]
    fn corrupt_records_are_dropped() {
        let dir = RuntimeDir::at(temp_dir("corrupt"));
        dir.ensure().unwrap();
        fs::write(dir.root().join("garbage.json"), b"{not json").unwrap();
        assert!(dir.records().unwrap().is_empty());
        assert!(!dir.root().join("garbage.json").exists());
        let _ = fs::remove_dir_all(dir.root());
    }

    #[test]
    fn reap_orphans_forgets_dead_records_and_spares_supervised_children() {
        let dir = RuntimeDir::at(temp_dir("orphan"));
        dir.ensure().unwrap();
        let live = spec("wk:8000");

        let mut child = std::process::Command::new("yes")
            .arg("--")
            .arg(live.forward_arg())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawns yes");
        assert!(parent_pid(child.id()).is_some_and(|ppid| ppid != 1));
        dir.write_record(&SessionRecord {
            spec: live.clone(),
            owner: Owner::Panel,
            ssh_pid: child.id(),
            supervisor_pid: None,
            started_at: 0,
        })
        .unwrap();
        assert_eq!(dir.reap_orphans(), 0);
        assert!(process_alive(child.id()));
        assert!(dir.record_for(&live).is_some());

        let dead = spec("8001:wk:8000");
        dir.write_record(&SessionRecord {
            spec: dead.clone(),
            owner: Owner::Panel,
            ssh_pid: 999_999,
            supervisor_pid: None,
            started_at: 0,
        })
        .unwrap();
        assert_eq!(dir.reap_orphans(), 0);
        assert!(dir.record_for(&dead).is_none());

        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(dir.root());
    }

    #[test]
    fn oversized_log_is_cut_to_its_tail() {
        let dir = RuntimeDir::at(temp_dir("rotate"));
        let spec = spec("wk:8000");
        dir.ensure().unwrap();
        let mut body = String::new();
        for i in 0..60_000 {
            body.push_str(&format!("line {i}\n"));
        }
        fs::write(dir.log_path(&spec), &body).unwrap();
        drop(dir.open_log(&spec).unwrap());
        assert!(fs::metadata(dir.log_path(&spec)).unwrap().len() <= LOG_LIMIT);
        let tail = dir.log_tail(&spec, 3);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[2], "line 59999");
        let _ = fs::remove_dir_all(dir.root());
    }

    #[test]
    fn kill_only_touches_processes_we_started() {
        let dir = RuntimeDir::at(temp_dir("kill"));
        dir.ensure().unwrap();
        let spec = spec("8001:wk:8000");
        let flag = spec.forward_flag();

        // `yes` keeps the flag in its argv; `--` stops it reading "-L" as an option.
        let mut child = std::process::Command::new("yes")
            .arg("--")
            .arg(&flag)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawns yes");
        let pid = child.id();
        dir.write_record(&SessionRecord {
            spec: spec.clone(),
            owner: Owner::Daemon,
            ssh_pid: pid,
            supervisor_pid: None,
            started_at: 0,
        })
        .unwrap();

        assert!(wait_for_cmdline(pid, &flag, Duration::from_secs(2)));
        assert!(dir.kill_spec(&spec));
        wait_for_exit(pid, Duration::from_secs(2));
        let _ = child.wait();
        assert!(!dir.kill_spec(&spec));

        let mut other = std::process::Command::new("yes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawns yes");
        let other_pid = other.id();
        let stale = SessionRecord {
            spec: spec.clone(),
            owner: Owner::Daemon,
            ssh_pid: other_pid,
            supervisor_pid: None,
            started_at: 0,
        };
        dir.write_record(&stale).unwrap();
        assert!(dir.kill_spec(&spec));
        assert!(!cmdline_contains(other_pid, &flag));
        assert!(process_alive(other_pid), "unrelated pid must survive");
        let _ = other.kill();
        let _ = other.wait();
        let _ = fs::remove_dir_all(dir.root());
    }

    /// A child is briefly visible with its parent's cmdline before exec, so a
    /// single read right after spawn races.
    fn wait_for_cmdline(pid: u32, needle: &str, ceiling: Duration) -> bool {
        let deadline = Instant::now() + ceiling;
        while Instant::now() < deadline {
            if cmdline_contains(pid, needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn wait_for_exit(pid: u32, ceiling: Duration) {
        let deadline = Instant::now() + ceiling;
        while Instant::now() < deadline {
            if !process_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("process {pid} still alive after {ceiling:?}");
    }
}
