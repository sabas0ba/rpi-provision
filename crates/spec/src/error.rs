use std::fmt;

/// A specification-level error, optionally carrying the position in the
/// source document that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub position: Option<(usize, usize)>,
    /// The specification file the error came from, when it was read from disk.
    pub file: Option<String>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), position: None, file: None }
    }

    /// Values injected by `--set` carry no source position; line 0 marks them.
    pub fn at(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self { message: message.into(), position: (line > 0).then_some((line, col)), file: None }
    }

    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}: ")?;
        }
        if let Some((line, col)) = self.position {
            write!(f, "line {line}, column {col}: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<rpi_provision_toml::Error> for Error {
    fn from(value: rpi_provision_toml::Error) -> Self {
        Error::at(value.line, value.col, value.message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
