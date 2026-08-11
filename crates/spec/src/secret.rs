//! Secret values (password hashes, Wi-Fi pre-shared keys).
//!
//! A specification file is expected to live under version control, so secrets
//! are declared as a *source* rather than a literal:
//!
//! ```toml
//! password_hash = { env = "RPI_PASSWORD_HASH" }
//! psk           = { file = "secrets/wifi.psk" }
//! psk           = { value = "literal" }   # discouraged, accepted explicitly
//! ```
//!
//! A bare string is rejected so that a secret can never be committed by
//! accident.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::reader::Reader;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// Read from a process environment variable.
    Env(String),
    /// Read from a file, relative to the specification file's directory.
    File(PathBuf),
    /// An inline literal.
    Value(String),
}

impl SecretSource {
    /// A description safe to print in diagnostics (never reveals the value).
    pub fn describe(&self) -> String {
        match self {
            SecretSource::Env(name) => format!("environment variable `{name}`"),
            SecretSource::File(path) => format!("file `{}`", path.display()),
            SecretSource::Value(_) => "inline literal".to_string(),
        }
    }

    /// Parse `env:NAME`, `file:PATH` or `value:LITERAL`, as accepted by the
    /// `--set-secret` command line option.
    pub fn parse_cli(text: &str) -> Result<Self> {
        match text.split_once(':') {
            Some(("env", name)) if !name.is_empty() => Ok(SecretSource::Env(name.to_string())),
            Some(("file", path)) if !path.is_empty() => Ok(SecretSource::File(PathBuf::from(path))),
            Some(("value", literal)) => Ok(SecretSource::Value(literal.to_string())),
            _ => Err(Error::new(format!(
                "invalid secret source `{text}`; expected `env:NAME`, `file:PATH` or `value:LITERAL`"
            ))),
        }
    }
}

/// Supplies secret material during specification loading.
pub trait SecretProvider {
    fn env(&self, name: &str) -> Option<String>;
    fn read_file(&self, path: &Path) -> std::io::Result<String>;
}

/// Reads from the real process environment and filesystem.
pub struct SystemSecrets;

impl SecretProvider for SystemSecrets {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn read_file(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// An in-memory provider, used by the test-suite.
#[derive(Default)]
pub struct MapSecrets {
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<PathBuf, String>,
}

impl MapSecrets {
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_string(), value.to_string());
        self
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>, value: &str) -> Self {
        self.files.insert(path.into(), value.to_string());
        self
    }
}

impl SecretProvider for MapSecrets {
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    fn read_file(&self, path: &Path) -> std::io::Result<String> {
        self.files.get(path).cloned().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found", path.display()),
            )
        })
    }
}

/// Read a secret declaration out of an inline table.
pub fn read_secret(reader: &mut Reader<'_>, key: &str) -> Result<Option<SecretSource>> {
    if reader.peek_type(key) == Some("string") {
        return Err(reader.key_error(
            key,
            "must not be a bare string; use `{ env = \"NAME\" }`, `{ file = \"PATH\" }` \
             or, if you really mean it, `{ value = \"...\" }`",
        ));
    }

    let Some(mut table) = reader.opt_table(key)? else {
        return Ok(None);
    };

    let env = table.opt_string("env")?;
    let file = table.opt_string("file")?;
    let value = table.opt_string("value")?;
    table.finish()?;

    let provided = [&env, &file, &value].iter().filter(|slot| slot.is_some()).count();
    if provided != 1 {
        return Err(reader.key_error(key, "must declare exactly one of `env`, `file` or `value`"));
    }

    Ok(Some(match (env, file, value) {
        (Some(name), _, _) => SecretSource::Env(name),
        (_, Some(path), _) => SecretSource::File(PathBuf::from(path)),
        (_, _, Some(literal)) => SecretSource::Value(literal),
        _ => unreachable!("exactly one source was verified above"),
    }))
}

/// Resolve a declared source into the secret's actual value.
pub fn resolve(
    source: &SecretSource,
    provider: &dyn SecretProvider,
    base_dir: &Path,
    what: &str,
) -> Result<String> {
    let raw = match source {
        SecretSource::Env(name) => provider.env(name).ok_or_else(|| {
            Error::new(format!("{what}: environment variable `{name}` is not set"))
        })?,
        SecretSource::File(path) => {
            let full = if path.is_absolute() { path.clone() } else { base_dir.join(path) };
            provider.read_file(&full).map_err(|err| {
                Error::new(format!("{what}: cannot read `{}`: {err}", full.display()))
            })?
        }
        SecretSource::Value(literal) => literal.clone(),
    };

    // Secret files conventionally end with a newline; trailing whitespace is
    // never part of a hash or pre-shared key.
    let trimmed = raw.trim_end_matches(['\n', '\r']).to_string();
    if trimmed.is_empty() {
        return Err(Error::new(format!("{what}: {} is empty", source.describe())));
    }
    if trimmed.contains('\n') {
        return Err(Error::new(format!("{what}: {} contains a line break", source.describe())));
    }
    Ok(trimmed)
}
