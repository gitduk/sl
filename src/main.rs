use std::os::unix::process::CommandExt;
use std::process::Stdio;

use clap::Parser;
use sl::config::{self, Config};
use sl::error::Result;
use sl::forward::{Direction, ForwardSpec, Host};
use sl::report;
use sl::runtime::{Owner, RuntimeDir};
use sl::sessions::{EngineConfig, Sessions};

#[derive(Parser)]
#[command(name = "sl", version, about = "SSH port forwarding with a panel")]
struct Cli {
    /// Patterns: <host>:<port>[,<port>...] or <entry>:<host>:<target>
    patterns: Vec<String>,

    /// Forward in the remote direction
    #[arg(short = 'r', long)]
    remote: bool,

    /// Start in the background and return
    #[arg(short = 'd', long)]
    detach: bool,

    /// Stop sessions: all of them, or those touching the given hosts
    #[arg(short = 'k', long)]
    kill: bool,

    /// Internal: set by -d on the detached supervisor
    #[arg(long, hide = true)]
    daemon: bool,
}

fn main() {
    let cli = Cli::parse();
    let code = match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sl: {e}");
            2
        }
    };
    std::process::exit(code);
}

fn dispatch(cli: Cli) -> Result<i32> {
    if cli.kill {
        return kill(&cli.patterns);
    }
    if cli.daemon {
        return headless(&cli, true);
    }
    if cli.patterns.is_empty() {
        return tui();
    }
    if cli.detach {
        return detach(&cli);
    }
    headless(&cli, false)
}

fn direction(remote: bool) -> Direction {
    if remote {
        Direction::Remote
    } else {
        Direction::Local
    }
}

fn kill(patterns: &[String]) -> Result<i32> {
    let hosts: Vec<Host> = patterns
        .iter()
        .map(|p| Host::parse(p))
        .collect::<std::result::Result<_, _>>()?;
    let runtime = RuntimeDir::resolve()?;
    let stopped = runtime.kill(&hosts)?;
    if stopped == 0 {
        let suffix = if hosts.is_empty() {
            String::new()
        } else {
            format!(
                " matching {}",
                hosts
                    .iter()
                    .map(|h| h.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        eprintln!("sl: no live session{suffix}");
        return Ok(1);
    }
    eprintln!("sl: stopped {stopped} session(s)");
    Ok(0)
}

fn headless(cli: &Cli, quiet: bool) -> Result<i32> {
    let specs = ForwardSpec::parse_all(&cli.patterns, direction(cli.remote))?;
    let options = load_options();
    let runtime = RuntimeDir::resolve()?;
    let cfg = EngineConfig::new(options, runtime, Owner::Daemon).batch_mode(quiet);
    let sessions = Sessions::start(specs.iter().map(|s| (s.clone(), true)).collect(), cfg)?;
    if !quiet {
        report::install_signal_handlers();
    }
    Ok(report::run(&sessions, &specs, quiet))
}

fn detach(cli: &Cli) -> Result<i32> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("--daemon");
    if cli.remote {
        command.arg("-r");
    }
    command.arg("--").args(&cli.patterns);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid(2) only detaches the child from the terminal's session.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = command.spawn()?;
    eprintln!(
        "sl: running in the background (supervisor {}), stop with: sl -k",
        child.id()
    );
    Ok(0)
}

fn tui() -> Result<i32> {
    sl::tui::run(config::config_path())
}

fn load_options() -> config::Options {
    match Config::load(&config::config_path()) {
        Ok(config) => config.options,
        Err(e) => {
            eprintln!("sl: ignoring config: {e}");
            config::Options::default()
        }
    }
}
