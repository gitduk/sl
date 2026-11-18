//! One forwarding row: the identity shared by config, runtime records and the panel.

use std::fmt;
use std::hash::{Hash, Hasher};

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Host(String);

impl Host {
    pub fn parse(raw: &str) -> Result<Self, SpecError> {
        if raw.is_empty() {
            return Err(SpecError::EmptyHost);
        }
        if raw
            .chars()
            .any(|c| c == ':' || c == ',' || c.is_whitespace())
        {
            return Err(SpecError::BadHost(raw.to_string()));
        }
        Ok(Host(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Filename-safe form: keeps `[A-Za-z0-9._-]`, maps anything else to `_`.
    fn sanitized(&self) -> String {
        self.0
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for Host {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Host {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Host::parse(&raw).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port(u16);

impl Port {
    pub fn new(raw: u16) -> Result<Self, SpecError> {
        if raw == 0 {
            return Err(SpecError::ZeroPort);
        }
        Ok(Port(raw))
    }

    pub fn parse(raw: &str) -> Result<Self, SpecError> {
        raw.parse::<u16>()
            .map_err(|_| SpecError::BadPort(raw.to_string()))
            .and_then(Port::new)
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for Port {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = u16::deserialize(d)?;
        Port::new(raw).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Local,
    Remote,
}

impl Direction {
    pub fn tag(self) -> char {
        match self {
            Direction::Local => 'L',
            Direction::Remote => 'R',
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Direction::Local => Direction::Remote,
            Direction::Remote => Direction::Local,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Local => "local",
            Direction::Remote => "remote",
        }
    }
}

/// One forwarding row. Identity is the whole struct, so the same four fields
/// always name the same row across config, runtime records and the panel.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ForwardSpec {
    pub host: Host,
    pub direction: Direction,
    pub entry: Port,
    pub target: Port,
}

impl ForwardSpec {
    /// Parses every `<host>:<port>[,<port>...]` and `<entry>:<host>:<target>`
    /// argument into rows, rejecting duplicates inside the same call.
    pub fn parse_all(args: &[String], direction: Direction) -> Result<Vec<Self>, SpecError> {
        let mut rows: Vec<ForwardSpec> = Vec::new();
        for arg in args {
            let parts: Vec<&str> = arg.split(':').collect();
            let mut specs = match parts.as_slice() {
                [entry, host, target] => vec![ForwardSpec {
                    host: Host::parse(host)?,
                    direction,
                    entry: Port::parse(entry)?,
                    target: Port::parse(target)?,
                }],
                [host, ports] => {
                    let host = Host::parse(host)?;
                    let mut specs = Vec::new();
                    for port in ports.split(',') {
                        let port = Port::parse(port)?;
                        specs.push(ForwardSpec {
                            host: host.clone(),
                            direction,
                            entry: port,
                            target: port,
                        });
                    }
                    specs
                }
                _ => return Err(SpecError::BadPattern(arg.clone())),
            };
            for spec in specs.drain(..) {
                if rows.contains(&spec) {
                    return Err(SpecError::Duplicate(spec));
                }
                rows.push(spec);
            }
        }
        if rows.is_empty() {
            return Err(SpecError::NoPattern);
        }
        Ok(rows)
    }

    /// The row as written in a pattern: `wk:8000` when both ports match.
    pub fn pattern(&self) -> String {
        if self.entry == self.target {
            format!("{}:{}", self.host, self.entry)
        } else {
            format!("{}:{}:{}", self.entry, self.host, self.target)
        }
    }

    /// The value ssh receives after `-L`/`-R`. `-k` matches this against a
    /// pid's argv before signalling it, so it must stay in step with `ssh_args`.
    pub fn forward_arg(&self) -> String {
        let host_port = format!("localhost:{}", self.target);
        match self.direction {
            Direction::Local => format!("127.0.0.1:{}:{host_port}", self.entry),
            Direction::Remote => format!("{}:{host_port}", self.entry),
        }
    }

    /// `-L`/`-R` plus its value, for messages and help text.
    pub fn forward_flag(&self) -> String {
        let dir = match self.direction {
            Direction::Local => "-L",
            Direction::Remote => "-R",
        };
        format!("{dir} {}", self.forward_arg())
    }

    /// Stable, filename-safe stem for this row's runtime files.
    pub fn record_stem(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.canonical().hash(&mut hasher);
        format!(
            "{}-{}-{}-{}-{:08x}",
            self.direction.tag(),
            self.host.sanitized(),
            self.entry,
            self.target,
            hasher.finish() as u32
        )
    }

    fn canonical(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.direction.as_str(),
            self.host,
            self.entry,
            self.target
        )
    }

    pub fn listen_display(&self) -> String {
        match self.direction {
            Direction::Local => format!(":{}", self.entry),
            Direction::Remote => format!("{}:{}", self.host, self.entry),
        }
    }

    pub fn target_display(&self) -> String {
        match self.direction {
            Direction::Local => format!("{}:{}", self.host, self.target),
            Direction::Remote => format!(":{}", self.target),
        }
    }
}

impl fmt::Display for ForwardSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} ⇒ {}",
            self.direction.tag(),
            self.listen_display(),
            self.target_display()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    NoPattern,
    BadPattern(String),
    EmptyHost,
    BadHost(String),
    BadPort(String),
    ZeroPort,
    Duplicate(ForwardSpec),
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::NoPattern => f.write_str("missing pattern"),
            SpecError::BadPattern(raw) => write!(f, "invalid pattern '{raw}'"),
            SpecError::EmptyHost => f.write_str("host is empty"),
            SpecError::BadHost(raw) => write!(f, "invalid host '{raw}' (no ':' ',' or spaces)"),
            SpecError::BadPort(raw) => write!(f, "invalid port '{raw}'"),
            SpecError::ZeroPort => f.write_str("port must be 1-65535"),
            SpecError::Duplicate(spec) => write!(f, "duplicate row {spec}"),
        }
    }
}

impl std::error::Error for SpecError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(args: &[&str]) -> Vec<ForwardSpec> {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        ForwardSpec::parse_all(&owned, Direction::Local).expect("parses")
    }

    fn err(args: &[&str]) -> SpecError {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        ForwardSpec::parse_all(&owned, Direction::Local).expect_err("rejects")
    }

    #[test]
    fn parses_host_with_ports() {
        let rows = local(&["wk:8000"]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host.as_str(), "wk");
        assert_eq!(rows[0].entry.get(), 8000);
        assert_eq!(rows[0].target.get(), 8000);
        assert_eq!(rows[0].direction, Direction::Local);
    }

    #[test]
    fn parses_comma_separated_ports() {
        let rows = local(&["wk:4000,4001,4002"]);
        let entries: Vec<u16> = rows.iter().map(|r| r.entry.get()).collect();
        assert_eq!(entries, vec![4000, 4001, 4002]);
        assert!(rows.iter().all(|r| r.entry == r.target));
    }

    #[test]
    fn parses_entry_host_target() {
        let rows = local(&["8001:wk:8000"]);
        assert_eq!(rows[0].entry.get(), 8001);
        assert_eq!(rows[0].target.get(), 8000);
        assert_eq!(rows[0].pattern(), "8001:wk:8000");
    }

    #[test]
    fn direction_comes_from_the_caller() {
        let owned = vec!["5056:wk:8080".to_string()];
        let rows = ForwardSpec::parse_all(&owned, Direction::Remote).unwrap();
        assert_eq!(rows[0].direction, Direction::Remote);
        assert_eq!(rows[0].forward_flag(), "-R 5056:localhost:8080");
    }

    #[test]
    fn local_flag_binds_loopback() {
        let rows = local(&["8001:wk:8000"]);
        assert_eq!(rows[0].forward_flag(), "-L 127.0.0.1:8001:localhost:8000");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(err(&["wk:0"]), SpecError::ZeroPort));
        assert!(matches!(err(&["wk:x"]), SpecError::BadPort(_)));
        assert!(matches!(err(&["wk:"]), SpecError::BadPort(_)));
        assert!(matches!(err(&["a:b:c:8000"]), SpecError::BadPattern(_)));
        assert!(matches!(err(&[":8000"]), SpecError::EmptyHost));
        assert!(matches!(err(&["a b:8000"]), SpecError::BadHost(_)));
        assert!(matches!(err(&["wk:70000"]), SpecError::BadPort(_)));
        assert!(matches!(err(&[]), SpecError::NoPattern));
    }

    #[test]
    fn rejects_duplicate_ports_in_one_argument() {
        assert!(matches!(err(&["wk:8000,8000"]), SpecError::Duplicate(_)));
    }

    #[test]
    fn record_stem_is_filename_safe_and_distinct() {
        let a_b = local(&["a_b:8000"]).remove(0);
        let a_dash_b = local(&["a-b:8000"]).remove(0);
        for spec in [&a_b, &a_dash_b] {
            let stem = spec.record_stem();
            assert!(
                stem.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
                "{stem}"
            );
            assert_eq!(stem, spec.record_stem());
        }
        assert_ne!(a_b.record_stem(), a_dash_b.record_stem());
        let same_ports_other_host = local(&["ds:8000"]).remove(0);
        assert_ne!(a_b.record_stem(), same_ports_other_host.record_stem());
    }

    #[test]
    fn deserialize_rejects_invalid_host_and_port() {
        let bad_host = toml::from_str::<ForwardSpec>(
            "host = 'a:b'\nentry = 1\ntarget = 2\ndirection = 'local'",
        );
        assert!(bad_host.is_err());
        let zero_port = toml::from_str::<ForwardSpec>(
            "host = 'wk'\nentry = 0\ntarget = 2\ndirection = 'local'",
        );
        assert!(zero_port.is_err());
    }
}
