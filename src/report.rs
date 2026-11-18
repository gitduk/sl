//! Headless front-end: the same engine, printed as mapping lines.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::forward::ForwardSpec;
use crate::sessions::{SessionEvent, Sessions, Status};

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

pub fn install_signal_handlers() {
    // SAFETY: the handler only sets an atomic flag, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
}

/// Prints status changes until Ctrl-C, or until every row has failed at least
/// once — the same promise the old script made when it exited non-zero.
pub fn run(sessions: &Sessions, specs: &[ForwardSpec], quiet: bool) -> i32 {
    let mut statuses: HashMap<ForwardSpec, Status> = specs
        .iter()
        .map(|spec| (spec.clone(), Status::Starting))
        .collect();

    loop {
        if SIGNALLED.swap(false, Ordering::SeqCst) {
            sessions.shutdown();
            return 0;
        }
        while let Ok(event) = sessions.events().try_recv() {
            if !quiet {
                print_event(&event);
            }
            statuses.insert(event.spec, event.status);
        }
        let settled: Vec<&Status> = statuses
            .values()
            .filter(|s| !matches!(s, Status::Starting))
            .collect();
        if settled.len() == statuses.len() && !statuses.values().any(Status::is_up) {
            if !quiet {
                eprintln!("sl: no forward is up, giving up");
            }
            sessions.shutdown();
            return 1;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn print_event(event: &SessionEvent) {
    let spec = &event.spec;
    let mark = spec.direction.tag();
    let mark_color = match event.status {
        Status::Up { .. } => 32,
        Status::Failed { .. } => 31,
        _ => 33,
    };
    match &event.status {
        Status::Starting => {}
        Status::Up { .. } => println!(
            "\x1b[{mark_color}m[{mark}]\x1b[0m \x1b[{mark_color}m{:<10}\x1b[0m ⇒  \x1b[34m{}\x1b[0m",
            spec.listen_display(),
            spec.target_display()
        ),
        Status::Failed { reason } => {
            println!(
                "\x1b[31m[x]\x1b[0m \x1b[31m{:<10}\x1b[0m ⇒  \x1b[31m{}\x1b[0m",
                spec.listen_display(),
                spec.target_display()
            );
            eprintln!("    {reason}");
        }
        Status::Waiting { attempt, retry_at } => {
            let left = retry_at
                .saturating_duration_since(Instant::now())
                .as_secs()
                .max(1);
            eprintln!("[~] {}: retry #{attempt} in {left}s", spec.listen_display());
        }
        Status::Stopped => eprintln!("[~] {}: stopped", spec.listen_display()),
    }
}
