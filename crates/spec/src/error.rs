use std::fmt;

/// A specification-level error, optionally carrying the position in the
/// source document that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub position: Option<(usize, usize)>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), position: None }
    }

    pub fn at(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self { message: message.into(), position: Some((line, col)) }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some((line, col)) => write!(f, "line {line}, column {col}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<rpi_provision_toml::Error> for Error {
    fn from(value: rpi_provision_toml::Error) -> Self {
        Error::at(value.line, value.col, value.message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
