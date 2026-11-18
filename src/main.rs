use clap::Parser;
use sl::config::{self, Config};
use sl::error::Result;
use sl::forward::{Direction, ForwardSpec};
use sl::report;
use sl::runtime::RuntimeDir;
use sl::sessions::{EngineConfig, Sessions};

#[derive(Parser)]
#[command(name = "sl", version, about = "SSH port forwarding with a panel")]
struct Cli {
    /// Patterns: <host>:<port>[,<port>...] or <entry>:<host>:<target>
    patterns: Vec<String>,

    /// Forward in the remote direction
    #[arg(short = 'r', long)]
    remote: bool,
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
    if cli.patterns.is_empty() {
        return sl::tui::run(config::config_path());
    }
    headless(&cli.patterns, cli.remote)
}

/// Without a panel: the same engine, printed as mapping lines, until Ctrl-C.
fn headless(patterns: &[String], remote: bool) -> Result<i32> {
    let direction = if remote {
        Direction::Remote
    } else {
        Direction::Local
    };
    let specs = ForwardSpec::parse_all(patterns, direction)?;
    let options = load_options();
    let runtime = RuntimeDir::resolve()?;
    let cfg = EngineConfig::new(options, runtime);
    let sessions = Sessions::start(specs.iter().map(|s| (s.clone(), true)).collect(), cfg)?;
    report::install_signal_handlers();
    Ok(report::run(&sessions, &specs))
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
