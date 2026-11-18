//! One error type for everything the binary reports to a user.

use std::fmt;

use crate::forward::SpecError;

#[derive(Debug)]
pub enum Error {
    Spec(SpecError),
    Config(String),
    Runtime(String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spec(e) => write!(f, "{e}"),
            Error::Config(msg) => write!(f, "{msg}"),
            Error::Runtime(msg) => write!(f, "{msg}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<SpecError> for Error {
    fn from(e: SpecError) -> Self {
        Error::Spec(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
