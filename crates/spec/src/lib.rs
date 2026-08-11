//! Specification loading for `rpi-provision`.
//!
//! A specification is a TOML document describing the desired state of a
//! Raspberry Pi 5 after its first boot. This crate parses it, applies command
//! line overrides, resolves secrets and validates the result. It performs no
//! rendering and touches no boot partition.

pub mod error;
pub mod model;
pub mod net;
pub mod overrides;
pub mod reader;
pub mod secret;
pub mod sha256;

pub use error::{Error, Result};
pub use model::{
    EthernetConnection, GadgetFunction, Hardware, I2c, IpConfig, IpMethod, Meta, Network, OneWire,
    Provisioning, Spec, Spi, Ssh, SudoMode, System, Target, Uart, UsbGadget, User, WifiConnection,
    WifiSecurity, SCHEMA_VERSION,
};
pub use net::{Ipv4Cidr, MacAddr};
pub use overrides::Override;
pub use secret::{MapSecrets, SecretProvider, SecretSource, SystemSecrets};

use std::path::{Path, PathBuf};

/// A loaded specification together with any non-fatal observations.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub spec: Spec,
    pub warnings: Vec<String>,
    /// SHA-256 of the specification source after overrides were applied.
    /// Recorded in generated artefacts so a provisioned card can be traced
    /// back to its input.
    pub digest: String,
}

/// How to load a specification.
pub struct LoadOptions<'a> {
    /// Where relative `file = "..."` secret paths are resolved from.
    pub base_dir: PathBuf,
    pub provider: &'a dyn SecretProvider,
    pub sets: Vec<Override>,
    pub set_secrets: Vec<Override>,
}

impl<'a> LoadOptions<'a> {
    pub fn new(provider: &'a dyn SecretProvider) -> Self {
        Self {
            base_dir: PathBuf::from("."),
            provider,
            sets: Vec::new(),
            set_secrets: Vec::new(),
        }
    }

    pub fn base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.base_dir = dir.into();
        self
    }

    pub fn set(mut self, text: &str) -> Result<Self> {
        self.sets.push(overrides::parse_set(text)?);
        Ok(self)
    }

    pub fn set_secret(mut self, text: &str) -> Result<Self> {
        self.set_secrets.push(overrides::parse_set_secret(text)?);
        Ok(self)
    }
}

/// Parse and validate a specification held in memory.
pub fn load_str(source: &str, options: &LoadOptions<'_>) -> Result<Loaded> {
    let mut document = rpi_provision_toml::parse(source)?;

    for over in options.sets.iter().chain(options.set_secrets.iter()) {
        overrides::apply(&mut document, over)?;
    }

    let digest = sha256::sha256_hex(&canonical_bytes(&document));

    let mut ctx = model::Context::new(options.provider, &options.base_dir);
    let mut reader = reader::Reader::root(&document);
    let spec = Spec::from_reader(&mut reader, &mut ctx)?;
    reader.finish()?;

    Ok(Loaded { spec, warnings: ctx.warnings, digest })
}

/// Read, parse and validate a specification file.
pub fn load_file(path: &Path, options: &LoadOptions<'_>) -> Result<Loaded> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| Error::new(format!("cannot read `{}`: {err}", path.display())))?;
    let mut options_with_base = LoadOptions {
        base_dir: path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
        provider: options.provider,
        sets: options.sets.clone(),
        set_secrets: options.set_secrets.clone(),
    };
    if options.base_dir != Path::new(".") {
        options_with_base.base_dir = options.base_dir.clone();
    }
    load_str(&source, &options_with_base).map_err(|err| err.in_file(path.display().to_string()))
}

/// Serialise a document to a canonical byte string for digesting.
///
/// The parser stores tables in sorted order, so this is stable regardless of
/// the order keys appeared in the source, and independent of comments and
/// whitespace. Two specifications with the same digest describe the same
/// desired state.
fn canonical_bytes(table: &rpi_provision_toml::Table) -> Vec<u8> {
    let mut out = Vec::new();
    write_table(&mut out, table);
    out
}

fn write_table(out: &mut Vec<u8>, table: &rpi_provision_toml::Table) {
    out.push(b'{');
    for (key, node) in table {
        out.extend_from_slice(key.as_bytes());
        out.push(b'=');
        write_value(out, &node.value);
        out.push(b';');
    }
    out.push(b'}');
}

fn write_value(out: &mut Vec<u8>, value: &rpi_provision_toml::Value) {
    use rpi_provision_toml::Value;
    match value {
        Value::String(s) => {
            out.push(b's');
            out.extend_from_slice(s.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(s.as_bytes());
        }
        Value::Integer(i) => {
            out.push(b'i');
            out.extend_from_slice(i.to_string().as_bytes());
        }
        Value::Float(f) => {
            out.push(b'f');
            out.extend_from_slice(format!("{f:?}").as_bytes());
        }
        Value::Boolean(b) => out.push(if *b { b'T' } else { b'F' }),
        Value::Array(items) => {
            out.push(b'[');
            for item in items {
                write_value(out, &item.value);
                out.push(b',');
            }
            out.push(b']');
        }
        Value::Table(table) => write_table(out, table),
    }
}

#[cfg(test)]
mod tests;
