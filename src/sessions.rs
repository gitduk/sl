//! The session engine: one supervisor thread per row, at most one ssh child
//! at a time, every state change reported on one channel.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Options;
use crate::error::Result;
use crate::forward::ForwardSpec;
use crate::runtime::{RuntimeDir, SessionRecord};

const TICK: Duration = Duration::from_millis(50);
const IDLE_TICK: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Up { since: SystemTime },
    Failed { reason: String },
    Waiting { attempt: u32, retry_at: Instant },
    Stopped,
}

impl Status {
    pub fn is_up(&self) -> bool {
        matches!(self, Status::Up { .. })
    }
}

#[derive(Clone, Debug)]
pub struct SessionEvent {
    pub spec: ForwardSpec,
    pub status: Status,
}

#[derive(Clone, Debug)]
pub enum Request {
    SetRunning(ForwardSpec, bool),
    Restart(ForwardSpec),
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub options: Options,
    pub runtime: RuntimeDir,
    pub ssh_program: PathBuf,
    pub batch_mode: bool,
}

impl EngineConfig {
    pub fn new(options: Options, runtime: RuntimeDir) -> Self {
        EngineConfig {
            options,
            runtime,
            ssh_program: PathBuf::from("ssh"),
            batch_mode: false,
        }
    }

    pub fn batch_mode(mut self, on: bool) -> Self {
        self.batch_mode = on;
        self
    }
}

struct RowCtl {
    /// What the user asked for: true = should be running.
    running: AtomicBool,
    /// Cut the current ssh and start a fresh one right away.
    restart: AtomicBool,
    pid: AtomicU32,
    shutdown: Arc<AtomicBool>,
    /// Set when the handle is released: supervise stops watching, the ssh stays.
    released: Arc<AtomicBool>,
}

impl RowCtl {
    fn new(running: bool, shutdown: Arc<AtomicBool>, released: Arc<AtomicBool>) -> Self {
        RowCtl {
            running: AtomicBool::new(running),
            restart: AtomicBool::new(false),
            pid: AtomicU32::new(0),
            shutdown,
            released,
        }
    }

    /// True once this row must be left alone: shutting down, or handed over.
    fn finished(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst) || self.released.load(Ordering::SeqCst)
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst) || !self.is_running()
    }

    /// Clears the slot before signalling so a recycled pid is never hit twice.
    fn kill_pid(&self) {
        let pid = self.pid.swap(0, Ordering::SeqCst);
        signal(pid);
    }

    fn set_running(&self, on: bool) {
        self.running.store(on, Ordering::SeqCst);
        if !on {
            self.kill_pid();
        }
    }

    fn restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
        self.kill_pid();
    }
}

enum Outcome {
    /// Cut short by a stop/restart request, not a failure.
    Interrupted,
    Exited {
        status: Option<std::process::ExitStatus>,
        reason: Option<String>,
    },
    SpawnFailed(String),
}

/// Everything a supervisor needs besides its own row: shared by the dispatcher
/// and by every row thread it starts.
#[derive(Clone)]
struct Engine {
    cfg: EngineConfig,
    shutdown: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    evt_tx: Sender<SessionEvent>,
    ctls: Arc<Mutex<HashMap<ForwardSpec, Arc<RowCtl>>>>,
}

impl Engine {
    fn spawn_row(&self, spec: &ForwardSpec, running: bool) {
        let ctl = Arc::new(RowCtl::new(
            running,
            self.shutdown.clone(),
            self.released.clone(),
        ));
        {
            let mut map = match self.ctls.lock() {
                Ok(map) => map,
                Err(poisoned) => poisoned.into_inner(),
            };
            if map.contains_key(spec) {
                return;
            }
            map.insert(spec.clone(), ctl.clone());
        }
        let spec = spec.clone();
        let cfg = self.cfg.clone();
        let evt_tx = self.evt_tx.clone();
        thread::spawn(move || supervise(spec, cfg, ctl, evt_tx));
    }
}

pub struct Sessions {
    cmd_tx: Sender<Request>,
    evt_rx: Receiver<SessionEvent>,
    engine: Engine,
}

impl Sessions {
    pub fn start(rows: Vec<(ForwardSpec, bool)>, cfg: EngineConfig) -> Result<Sessions> {
        cfg.runtime.ensure()?;
        let (evt_tx, evt_rx) = mpsc::channel::<SessionEvent>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Request>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let ctls: Arc<Mutex<HashMap<ForwardSpec, Arc<RowCtl>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let engine = Engine {
            cfg,
            shutdown: shutdown.clone(),
            released: released.clone(),
            evt_tx: evt_tx.clone(),
            ctls: ctls.clone(),
        };
        for (spec, running) in rows {
            engine.spawn_row(&spec, running);
        }
        let dispatch_engine = engine.clone();
        thread::spawn(move || dispatch(cmd_rx, dispatch_engine));

        Ok(Sessions {
            cmd_tx,
            evt_rx,
            engine,
        })
    }

    pub fn events(&self) -> &Receiver<SessionEvent> {
        &self.evt_rx
    }

    pub fn send(&self, request: Request) {
        let _ = self.cmd_tx.send(request);
    }

    pub fn set_running(&self, spec: &ForwardSpec, on: bool) {
        self.send(Request::SetRunning(spec.clone(), on));
    }

    pub fn restart(&self, spec: &ForwardSpec) {
        self.send(Request::Restart(spec.clone()));
    }

    pub fn pid_of(&self, spec: &ForwardSpec) -> u32 {
        self.engine
            .ctls
            .lock()
            .map(|ctls| {
                ctls.get(spec)
                    .map(|ctl| ctl.pid.load(Ordering::SeqCst))
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// Hands the running forwards over to nobody: the panel calls this on quit
    /// so the tunnels survive it, and a later panel adopts them.
    pub fn release(&self) {
        self.engine.released.store(true, Ordering::SeqCst);
    }

    /// Stops every child and waits (bounded) for them to leave, escalating to
    /// SIGKILL so no ssh outlives the caller.
    pub fn shutdown(&self) {
        self.engine.shutdown.store(true, Ordering::SeqCst);
        let ctls = match self.engine.ctls.lock() {
            Ok(ctls) => ctls,
            Err(poisoned) => poisoned.into_inner(),
        };
        let pids: Vec<u32> = ctls
            .values()
            .filter_map(|ctl| {
                let pid = ctl.pid.swap(0, Ordering::SeqCst);
                (pid != 0).then_some(pid)
            })
            .collect();
        drop(ctls);
        for pid in &pids {
            signal(*pid);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline
            && pids.iter().any(|pid| crate::runtime::process_alive(*pid))
        {
            thread::sleep(TICK);
        }
        for pid in pids {
            if crate::runtime::process_alive(pid) {
                // SAFETY: SIGKILL to a pid we started and already SIGTERMed.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        if !self.engine.released.load(Ordering::SeqCst) {
            self.shutdown();
        }
    }
}

fn dispatch(rx: Receiver<Request>, engine: Engine) {
    while let Ok(request) = rx.recv() {
        if engine.shutdown.load(Ordering::SeqCst) || engine.released.load(Ordering::SeqCst) {
            return;
        }
        let mut missing: Option<ForwardSpec> = None;
        {
            let map = match engine.ctls.lock() {
                Ok(map) => map,
                Err(poisoned) => poisoned.into_inner(),
            };
            match request {
                Request::SetRunning(spec, on) => match map.get(&spec) {
                    Some(ctl) => ctl.set_running(on),
                    None if on => missing = Some(spec),
                    None => {}
                },
                Request::Restart(spec) => match map.get(&spec) {
                    Some(ctl) if ctl.is_running() => ctl.restart(),
                    Some(ctl) => ctl.set_running(true),
                    None => missing = Some(spec),
                },
            }
        }
        if let Some(spec) = missing {
            engine.spawn_row(&spec, true);
        }
    }
}

fn supervise(spec: ForwardSpec, cfg: EngineConfig, ctl: Arc<RowCtl>, evt: Sender<SessionEvent>) {
    let mut attempt: u32 = 0;
    let mut stopped_announced = false;

    loop {
        if ctl.finished() {
            return;
        }
        if !ctl.is_running() {
            if !stopped_announced {
                publish(&evt, &spec, Status::Stopped);
                stopped_announced = true;
            }
            thread::sleep(IDLE_TICK);
            continue;
        }
        stopped_announced = false;

        let outcome = run_session(&spec, &cfg, &ctl, &evt);
        ctl.pid.store(0, Ordering::SeqCst);
        let _ = cfg.runtime.remove_record(&spec);

        if ctl.finished() {
            return;
        }
        let restarted = ctl.restart.swap(false, Ordering::SeqCst);
        match outcome {
            Outcome::Interrupted => continue,
            Outcome::SpawnFailed(reason) => {
                fail(&evt, &spec, reason, &mut attempt, &cfg, &ctl);
            }
            Outcome::Exited { status, reason } => {
                if restarted {
                    attempt = 0;
                    continue;
                }
                let reason = reason.unwrap_or_else(|| match status {
                    Some(status) => format!("ssh exited with {status}"),
                    None => "ssh exited".to_string(),
                });
                fail(&evt, &spec, reason, &mut attempt, &cfg, &ctl);
            }
        }
        if restarted {
            attempt = 0;
        }
    }
}

fn fail(
    evt: &Sender<SessionEvent>,
    spec: &ForwardSpec,
    reason: String,
    attempt: &mut u32,
    cfg: &EngineConfig,
    ctl: &RowCtl,
) {
    publish(evt, spec, Status::Failed { reason });
    *attempt += 1;
    let retry = Duration::from_secs(cfg.options.retry_secs.max(1));
    publish(
        evt,
        spec,
        Status::Waiting {
            attempt: *attempt,
            retry_at: Instant::now() + retry,
        },
    );
    sleep_interruptible(retry, ctl);
}

fn sleep_interruptible(total: Duration, ctl: &RowCtl) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if ctl.stopping() || ctl.restart.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(TICK.min(deadline - Instant::now()));
    }
}

/// Adopts a forward a previous panel left running, otherwise starts a new ssh.
fn run_session(
    spec: &ForwardSpec,
    cfg: &EngineConfig,
    ctl: &RowCtl,
    evt: &Sender<SessionEvent>,
) -> Outcome {
    match adoptable(spec, cfg) {
        Some(record) => watch_adopted(spec, cfg, ctl, evt, record),
        None => run_child(spec, cfg, ctl, evt),
    }
}

fn adoptable(spec: &ForwardSpec, cfg: &EngineConfig) -> Option<SessionRecord> {
    adopted(&cfg.runtime, spec)
}

/// The record of a forward that is already running and belongs to this row:
/// written by a previous panel and matched by command line.
pub fn adopted(runtime: &RuntimeDir, spec: &ForwardSpec) -> Option<SessionRecord> {
    let record = runtime.record_for(spec)?;
    if !crate::runtime::process_alive(record.ssh_pid)
        || !crate::runtime::process_carries(record.ssh_pid, &spec.forward_arg())
    {
        return None;
    }
    Some(record)
}

/// Watches an adopted ssh that we did not spawn: there is no `Child` to wait
/// on, so liveness is polled and killing goes through the recorded pid.
fn watch_adopted(
    spec: &ForwardSpec,
    cfg: &EngineConfig,
    ctl: &RowCtl,
    evt: &Sender<SessionEvent>,
    record: SessionRecord,
) -> Outcome {
    let pid = record.ssh_pid;
    ctl.pid.store(pid, Ordering::SeqCst);
    publish(
        evt,
        spec,
        Status::Up {
            since: UNIX_EPOCH + Duration::from_secs(record.started_at),
        },
    );
    loop {
        if ctl.stopping() || ctl.restart.load(Ordering::SeqCst) {
            ctl.kill_pid();
            return Outcome::Interrupted;
        }
        if !crate::runtime::process_alive(pid) {
            return Outcome::Exited {
                status: None,
                reason: cfg.runtime.last_log_line(spec),
            };
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn run_child(
    spec: &ForwardSpec,
    cfg: &EngineConfig,
    ctl: &RowCtl,
    evt: &Sender<SessionEvent>,
) -> Outcome {
    // Only a real spawn is "starting"; an adopted forward is already up, and
    // announcing Starting for it would flash a placeholder on the first frame.
    publish(evt, spec, Status::Starting);
    let log = match cfg.runtime.open_log(spec) {
        Ok(log) => log,
        Err(e) => return Outcome::SpawnFailed(format!("cannot open log: {e}")),
    };
    let mut command = Command::new(&cfg.ssh_program);
    command
        .args(ssh_args(spec, cfg))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // SAFETY: setsid(2) only puts the child in its own session, so closing the
    // terminal the panel ran in cannot hang the tunnel up.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Outcome::SpawnFailed(format!("cannot spawn ssh: {e}")),
    };
    let pid = child.id();
    ctl.pid.store(pid, Ordering::SeqCst);
    let _ = cfg.runtime.write_record(&SessionRecord {
        spec: spec.clone(),
        ssh_pid: pid,
        started_at: now_secs(),
    });

    let grace = Duration::from_millis(cfg.options.up_grace_ms.max(1));
    let deadline = Instant::now() + grace;
    loop {
        if ctl.stopping() || ctl.restart.load(Ordering::SeqCst) {
            terminate(&mut child);
            return Outcome::Interrupted;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Outcome::Exited {
                    status: Some(status),
                    reason: cfg.runtime.last_log_line(spec),
                };
            }
            Ok(None) => {}
            Err(e) => {
                terminate(&mut child);
                return Outcome::Exited {
                    status: None,
                    reason: Some(format!("cannot watch ssh: {e}")),
                };
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(TICK);
    }

    publish(
        evt,
        spec,
        Status::Up {
            since: SystemTime::now(),
        },
    );
    let status = child.wait().ok();
    let reason = cfg.runtime.last_log_line(spec);
    Outcome::Exited { status, reason }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn ssh_args(spec: &ForwardSpec, cfg: &EngineConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-N".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        format!("ServerAliveInterval={}", cfg.options.keepalive_secs),
        "-o".into(),
        format!("ServerAliveCountMax={}", cfg.options.keepalive_count_max),
        "-o".into(),
        "WarnWeakCrypto=no".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
    if cfg.batch_mode {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    let mode = match spec.direction {
        crate::forward::Direction::Local => "-L",
        crate::forward::Direction::Remote => "-R",
    };
    args.push(mode.to_string());
    args.push(spec.forward_arg());
    args.push(spec.host.to_string());
    args
}

fn publish(evt: &Sender<SessionEvent>, spec: &ForwardSpec, status: Status) {
    let _ = evt.send(SessionEvent {
        spec: spec.clone(),
        status,
    });
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn signal(pid: u32) {
    if pid == 0 {
        return;
    }
    // SAFETY: kill(2) with a pid we recorded; ESRCH is the only cost of a race.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::{Direction, ForwardSpec};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn spec(pattern: &str) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, Direction::Local)
            .unwrap()
            .remove(0)
    }

    fn fake_ssh(dir: &Path, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("ssh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn engine(tag: &str, dir: &Path, program: PathBuf, retry_secs: u64) -> EngineConfig {
        let runtime = RuntimeDir::at(dir.join(format!("rt-{tag}")));
        let options = Options {
            retry_secs,
            up_grace_ms: 50,
            ..Options::default()
        };
        let mut cfg = EngineConfig::new(options, runtime);
        cfg.ssh_program = program;
        cfg
    }

    fn next_status(
        rx: &Receiver<SessionEvent>,
        wanted: fn(&Status) -> bool,
        ceiling: Duration,
    ) -> Status {
        let deadline = Instant::now() + ceiling;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "no matching status within {ceiling:?}");
            match rx.recv_timeout(left) {
                Ok(event) => {
                    if wanted(&event.status) {
                        return event.status;
                    }
                }
                Err(e) => panic!("no matching status within {ceiling:?}: {e}"),
            }
        }
    }

    #[test]
    fn failed_spawn_reports_the_ssh_reason_then_waits() {
        let dir = std::env::temp_dir().join(format!("sl-sess-fail-{}", std::process::id()));
        let program = fake_ssh(
            &dir,
            "echo 'bind [127.0.0.1]:8000: Address already in use' >&2\nexit 255",
        );
        let cfg = engine("fail", &dir, program, 1);
        let spec = spec("wk:8000");
        let sessions = Sessions::start(vec![(spec.clone(), true)], cfg).unwrap();
        let rx = sessions.events();

        let failed = next_status(
            rx,
            |s| matches!(s, Status::Failed { .. }),
            Duration::from_secs(5),
        );
        match failed {
            Status::Failed { reason } => {
                assert!(reason.contains("Address already in use"), "{reason}")
            }
            other => panic!("unexpected {other:?}"),
        }
        let waiting = next_status(
            rx,
            |s| matches!(s, Status::Waiting { .. }),
            Duration::from_secs(2),
        );
        match waiting {
            Status::Waiting { attempt, .. } => assert_eq!(attempt, 1),
            other => panic!("unexpected {other:?}"),
        }
        sessions.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_child_comes_up_then_stops_on_request() {
        let dir = std::env::temp_dir().join(format!("sl-sess-up-{}", std::process::id()));
        let program = fake_ssh(&dir, "sleep 30");
        let cfg = engine("up", &dir, program, 1);
        let spec = spec("8001:wk:8000");
        let sessions = Sessions::start(vec![(spec.clone(), true)], cfg).unwrap();
        let rx = sessions.events();

        next_status(rx, |s| s.is_up(), Duration::from_secs(5));
        assert!(sessions.pid_of(&spec) != 0);

        sessions.set_running(&spec, false);
        next_status(rx, |s| matches!(s, Status::Stopped), Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while sessions.pid_of(&spec) != 0 && Instant::now() < deadline {
            thread::sleep(TICK);
        }
        assert_eq!(sessions.pid_of(&spec), 0);
        sessions.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_replaces_the_child() {
        let dir = std::env::temp_dir().join(format!("sl-sess-restart-{}", std::process::id()));
        let program = fake_ssh(&dir, "sleep 30");
        let cfg = engine("restart", &dir, program, 1);
        let spec = spec("wk:8000");
        let sessions = Sessions::start(vec![(spec.clone(), true)], cfg).unwrap();
        let rx = sessions.events();

        next_status(rx, |s| s.is_up(), Duration::from_secs(5));
        let first = sessions.pid_of(&spec);
        sessions.restart(&spec);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut second = first;
        while Instant::now() < deadline {
            second = sessions.pid_of(&spec);
            if second != 0 && second != first {
                break;
            }
            thread::sleep(TICK);
        }
        assert_ne!(first, second, "restart must spawn a new child");
        sessions.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopped_row_waits_for_a_start_request() {
        let dir = std::env::temp_dir().join(format!("sl-sess-idle-{}", std::process::id()));
        let program = fake_ssh(&dir, "sleep 30");
        let cfg = engine("idle", &dir, program, 1);
        let spec = spec("wk:8000");
        let sessions = Sessions::start(vec![(spec.clone(), false)], cfg).unwrap();
        let rx = sessions.events();

        next_status(rx, |s| matches!(s, Status::Stopped), Duration::from_secs(5));
        sessions.set_running(&spec, true);
        next_status(rx, |s| s.is_up(), Duration::from_secs(5));
        sessions.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adopts_a_running_forward_instead_of_spawning_a_second_ssh() {
        let dir = std::env::temp_dir().join(format!("sl-sess-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let spec = spec("8002:wk:8000");
        let started_at = 1_700_000_000;

        // A stand-in for the ssh a previous panel left running.
        let mut stand_in = std::process::Command::new("yes")
            .arg("--")
            .arg(spec.forward_arg())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawns yes");
        let runtime = RuntimeDir::at(dir.join("rt-adopt"));
        runtime
            .write_record(&SessionRecord {
                spec: spec.clone(),
                ssh_pid: stand_in.id(),
                started_at,
            })
            .unwrap();

        let options = Options {
            retry_secs: 1,
            up_grace_ms: 50,
            ..Options::default()
        };
        let mut cfg = EngineConfig::new(options, runtime).batch_mode(true);
        cfg.ssh_program = fake_ssh(&dir, "exit 99");
        let sessions = Sessions::start(vec![(spec.clone(), true)], cfg).unwrap();
        let rx = sessions.events();

        let first = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("engine event");
        match first.status {
            Status::Up { since } => {
                let seconds = since
                    .duration_since(UNIX_EPOCH)
                    .expect("start time")
                    .as_secs();
                assert_eq!(seconds, started_at);
            }
            other => panic!("an adopted row must not announce {other:?}"),
        }
        assert_eq!(
            sessions.pid_of(&spec),
            stand_in.id(),
            "must adopt, not respawn"
        );

        let _ = stand_in.kill();
        let _ = stand_in.wait();
        next_status(
            rx,
            |s| matches!(s, Status::Waiting { .. }),
            Duration::from_secs(5),
        );
        sessions.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_released_forward_outlives_its_handle() {
        let dir = std::env::temp_dir().join(format!("sl-sess-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let program = fake_ssh(&dir, "exec sleep 30");
        let cfg = engine("release", &dir, program, 1);
        let spec = spec("8003:wk:8000");
        let sessions = Sessions::start(vec![(spec.clone(), true)], cfg).unwrap();
        let rx = sessions.events();

        next_status(rx, |s| s.is_up(), Duration::from_secs(5));
        let pid = sessions.pid_of(&spec);
        sessions.release();
        drop(sessions);
        assert!(
            crate::runtime::process_alive(pid),
            "a released forward must survive the handle"
        );

        signal(pid);
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::runtime::process_alive(pid) && Instant::now() < deadline {
            thread::sleep(TICK);
        }
        assert!(
            !crate::runtime::process_alive(pid),
            "no respawn after release"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ssh_args_carry_the_forward_and_batch_mode() {
        let cfg =
            EngineConfig::new(Options::default(), RuntimeDir::at("/tmp/unused")).batch_mode(true);
        let args = ssh_args(&spec("8001:wk:8000"), &cfg);
        assert!(args.contains(&"-L".to_string()));
        assert!(args.contains(&"127.0.0.1:8001:localhost:8000".to_string()));
        assert!(args.contains(&"wk".to_string()));
        assert!(args.contains(&"BatchMode=yes".to_string()));
    }
}
