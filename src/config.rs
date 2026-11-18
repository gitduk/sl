//! What the user wants forwarded — the only state worth persisting.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::forward::{Direction, ForwardSpec};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub retry_secs: u64,
    pub keepalive_secs: u64,
    pub keepalive_count_max: u32,
    pub up_grace_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            retry_secs: 5,
            keepalive_secs: 15,
            keepalive_count_max: 3,
            up_grace_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub spec: ForwardSpec,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub options: Options,
    #[serde(default)]
    pub forward: Vec<Entry>,
}

impl Config {
    /// Missing file is an empty config, not an error.
    pub fn load(path: &Path) -> Result<Config> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(Error::Io(e)),
        };
        let config: Config =
            toml::from_str(&raw).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        config.validate()?;
        Ok(config)
    }

    /// Writes through a sibling temp file so a crash never leaves half a config.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let body = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("cannot serialize config: {e}")))?;
        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Rejects rows that would fight over one listening port, naming both rows.
    pub fn validate(&self) -> Result<()> {
        for (i, a) in self.forward.iter().enumerate() {
            for b in &self.forward[i + 1..] {
                let (sa, sb) = (&a.spec, &b.spec);
                if sa == sb {
                    return Err(Error::Config(format!("duplicate row {sa}")));
                }
                if sa.host == sb.host && sa.direction == sb.direction && sa.entry == sb.entry {
                    return Err(Error::Config(format!(
                        "{sa} and {sb} share one listening port"
                    )));
                }
                if sa.direction == Direction::Local
                    && sb.direction == Direction::Local
                    && sa.entry == sb.entry
                {
                    return Err(Error::Config(format!(
                        "{sa} and {sb} would both bind 127.0.0.1:{}",
                        sa.entry
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn enabled_specs(&self) -> Vec<ForwardSpec> {
        self.forward
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.spec.clone())
            .collect()
    }

    pub fn position(&self, spec: &ForwardSpec) -> Option<usize> {
        self.forward.iter().position(|e| &e.spec == spec)
    }

    pub fn upsert(&mut self, spec: ForwardSpec, enabled: bool) {
        match self.position(&spec) {
            Some(idx) => self.forward[idx].enabled = enabled,
            None => self.forward.push(Entry { spec, enabled }),
        }
    }

    pub fn remove(&mut self, spec: &ForwardSpec) -> bool {
        match self.position(spec) {
            Some(idx) => {
                self.forward.remove(idx);
                true
            }
            None => false,
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("sl").join("config.toml");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("sl").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::Direction;

    fn spec(pattern: &str, direction: Direction) -> ForwardSpec {
        let args = vec![pattern.to_string()];
        ForwardSpec::parse_all(&args, direction).unwrap().remove(0)
    }

    fn entry(spec: ForwardSpec, enabled: bool) -> Entry {
        Entry { spec, enabled }
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            options: Options {
                retry_secs: 7,
                ..Options::default()
            },
            forward: vec![
                entry(spec("wk:8000", Direction::Local), true),
                entry(spec("5056:wk:8080", Direction::Remote), false),
            ],
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.forward.len(), 2);
        assert_eq!(back.forward[0].spec, config.forward[0].spec);
        assert_eq!(back.forward[1].spec, config.forward[1].spec);
        assert!(back.forward[0].enabled);
        assert!(!back.forward[1].enabled);
        assert_eq!(back.options.retry_secs, 7);
        assert_eq!(back.options.up_grace_ms, Options::default().up_grace_ms);
    }

    #[test]
    fn enabled_defaults_to_true() {
        let config: Config = toml::from_str(
            "[[forward]]\nhost = 'wk'\ndirection = 'local'\nentry = 8000\ntarget = 8000\n",
        )
        .unwrap();
        assert!(config.forward[0].enabled);
    }

    #[test]
    fn unknown_direction_is_an_error() {
        let parsed = toml::from_str::<Config>(
            "[[forward]]\nhost = 'wk'\ndirection = 'sideways'\nentry = 8000\ntarget = 8000\n",
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn duplicate_rows_are_refused() {
        let config = Config {
            options: Options::default(),
            forward: vec![
                entry(spec("wk:8000", Direction::Local), true),
                entry(spec("wk:8000", Direction::Local), true),
            ],
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate row"), "{err}");
    }

    #[test]
    fn same_local_port_on_two_hosts_is_refused() {
        let config = Config {
            options: Options::default(),
            forward: vec![
                entry(spec("wk:8000", Direction::Local), true),
                entry(spec("ds:8000", Direction::Local), true),
            ],
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("127.0.0.1:8000"), "{err}");
    }

    #[test]
    fn same_remote_port_on_two_hosts_is_allowed() {
        let config = Config {
            options: Options::default(),
            forward: vec![
                entry(spec("5056:wk:8080", Direction::Remote), true),
                entry(spec("5056:ds:8080", Direction::Remote), true),
            ],
        };
        config.validate().unwrap();
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = std::env::temp_dir().join(format!("sl-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let config = Config::load(&path).unwrap();
        assert!(config.forward.is_empty());
    }

    #[test]
    fn save_then_load_keeps_rows() {
        let dir = std::env::temp_dir().join(format!("sl-cfg-save-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut config = Config::default();
        config.upsert(spec("wk:8000", Direction::Local), true);
        config.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back.enabled_specs(), config.enabled_specs());
        assert!(!path.with_extension("toml.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
