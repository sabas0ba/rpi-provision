//! Command line overrides applied to a parsed document before the
//! specification model is built.
//!
//! Two forms exist:
//!
//! - `--set network.ethernet[0].address=192.168.1.60/24`
//! - `--set-secret user.password_hash=env:RPI_PASSWORD_HASH`
//!
//! The first is for ordinary values; the second replaces a secret declaration
//! wholesale. Values passed with `--set` are visible in the process table, so
//! secrets belong in `--set-secret` (or in the specification itself as an
//! `env`/`file` reference).

use rpi_provision_toml::{Node, Table, Value};

use crate::error::{Error, Result};
use crate::secret::SecretSource;

#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    pub path: Vec<Segment>,
    pub value: Value,
    /// The original text, for diagnostics.
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
}

impl Segment {
    fn describe(&self) -> String {
        match self {
            Segment::Key(key) => key.clone(),
            Segment::Index(index) => format!("[{index}]"),
        }
    }
}

fn describe(path: &[Segment]) -> String {
    let mut out = String::new();
    for segment in path {
        match segment {
            Segment::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Segment::Index(index) => out.push_str(&format!("[{index}]")),
        }
    }
    out
}

/// Parse `path=value`. The value is interpreted as a TOML scalar when
/// possible and as a plain string otherwise.
pub fn parse_set(text: &str) -> Result<Override> {
    let (path_text, value_text) = text
        .split_once('=')
        .ok_or_else(|| Error::new(format!("`{text}` is not of the form `path=value`")))?;
    let path = parse_path(path_text)?;
    let value = parse_scalar(value_text);
    Ok(Override { path, value, origin: text.to_string() })
}

/// Parse `path=env:NAME`, `path=file:PATH` or `path=value:LITERAL`.
pub fn parse_set_secret(text: &str) -> Result<Override> {
    let (path_text, source_text) = text
        .split_once('=')
        .ok_or_else(|| Error::new(format!("`{text}` is not of the form `path=source`")))?;
    let path = parse_path(path_text)?;
    let source = SecretSource::parse_cli(source_text)?;
    let (key, literal) = match source {
        SecretSource::Env(name) => ("env", name),
        SecretSource::File(path) => ("file", path.to_string_lossy().into_owned()),
        SecretSource::Value(literal) => ("value", literal),
    };
    let mut table = Table::new();
    table.insert(key.to_string(), Node::new(Value::String(literal), 0, 0));
    Ok(Override { path, value: Value::Table(table), origin: text.to_string() })
}

fn parse_scalar(text: &str) -> Value {
    // Reuse the document parser so that `1_000`, `0xff`, `true` and quoted
    // strings behave exactly as they do inside a specification file.
    match rpi_provision_toml::parse(&format!("value = {text}\n")) {
        Ok(table) => match table.get("value") {
            Some(node) => node.value.clone(),
            None => Value::String(text.to_string()),
        },
        Err(_) => Value::String(text.to_string()),
    }
}

fn parse_path(text: &str) -> Result<Vec<Segment>> {
    if text.is_empty() {
        return Err(Error::new("the override path is empty"));
    }
    let mut segments = Vec::new();
    for part in text.split('.') {
        let (name, rest) = match part.find('[') {
            Some(offset) => part.split_at(offset),
            None => (part, ""),
        };
        if name.is_empty() {
            return Err(Error::new(format!("`{text}` contains an empty path segment")));
        }
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(Error::new(format!("`{name}` is not a valid key in `{text}`")));
        }
        segments.push(Segment::Key(name.to_string()));

        let mut remainder = rest;
        while !remainder.is_empty() {
            let close = remainder
                .find(']')
                .ok_or_else(|| Error::new(format!("`{text}` has an unterminated `[`")))?;
            let index: usize = remainder[1..close].parse().map_err(|_| {
                Error::new(format!(
                    "`{}` is not a valid array index in `{text}`",
                    &remainder[1..close]
                ))
            })?;
            segments.push(Segment::Index(index));
            remainder = &remainder[close + 1..];
        }
    }
    Ok(segments)
}

/// Apply an override in place. Missing intermediate tables are created;
/// missing array elements are an error, because inventing one would silently
/// change the meaning of the specification.
pub fn apply(root: &mut Table, over: &Override) -> Result<()> {
    let (last, parents) =
        over.path.split_last().ok_or_else(|| Error::new("the override path is empty"))?;

    let mut cursor = Cursor::Table(root);
    for (depth, segment) in parents.iter().enumerate() {
        cursor = step(cursor, segment, &over.path[..=depth], &over.origin)?;
    }

    match (cursor, last) {
        (Cursor::Table(table), Segment::Key(key)) => {
            table.insert(key.clone(), Node::new(over.value.clone(), 0, 0));
            Ok(())
        }
        (Cursor::Array(items), Segment::Index(index)) => match items.get_mut(*index) {
            Some(slot) => {
                *slot = Node::new(over.value.clone(), 0, 0);
                Ok(())
            }
            None => Err(Error::new(format!(
                "`{}`: index {index} is out of range ({} entries)",
                over.origin,
                items.len()
            ))),
        },
        (_, segment) => Err(Error::new(format!(
            "`{}`: cannot assign to `{}` at this position",
            over.origin,
            segment.describe()
        ))),
    }
}

enum Cursor<'a> {
    Table(&'a mut Table),
    Array(&'a mut Vec<Node>),
}

fn step<'a>(
    cursor: Cursor<'a>,
    segment: &Segment,
    path_so_far: &[Segment],
    origin: &str,
) -> Result<Cursor<'a>> {
    match (cursor, segment) {
        (Cursor::Table(table), Segment::Key(key)) => {
            let node = table
                .entry(key.clone())
                .or_insert_with(|| Node::new(Value::Table(Table::new()), 0, 0));
            match &mut node.value {
                Value::Table(inner) => Ok(Cursor::Table(inner)),
                Value::Array(items) => Ok(Cursor::Array(items)),
                other => Err(Error::new(format!(
                    "`{origin}`: `{}` is a {} and cannot be traversed",
                    describe(path_so_far),
                    other.type_name()
                ))),
            }
        }
        (Cursor::Array(items), Segment::Index(index)) => {
            let len = items.len();
            let node = items.get_mut(*index).ok_or_else(|| {
                Error::new(format!("`{origin}`: index {index} is out of range ({len} entries)"))
            })?;
            match &mut node.value {
                Value::Table(inner) => Ok(Cursor::Table(inner)),
                Value::Array(inner) => Ok(Cursor::Array(inner)),
                other => Err(Error::new(format!(
                    "`{origin}`: `{}` is a {} and cannot be traversed",
                    describe(path_so_far),
                    other.type_name()
                ))),
            }
        }
        (Cursor::Table(_), Segment::Index(index)) => Err(Error::new(format!(
            "`{origin}`: `{}` is a table but was indexed with [{index}]",
            describe(&path_so_far[..path_so_far.len().saturating_sub(1)])
        ))),
        (Cursor::Array(_), Segment::Key(key)) => Err(Error::new(format!(
            "`{origin}`: `{}` is an array; use an index before `{key}`",
            describe(&path_so_far[..path_so_far.len().saturating_sub(1)])
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Table {
        rpi_provision_toml::parse(text).unwrap()
    }

    #[test]
    fn parses_scalar_types() {
        assert_eq!(parse_set("a=1").unwrap().value, Value::Integer(1));
        assert_eq!(parse_set("a=true").unwrap().value, Value::Boolean(true));
        assert_eq!(parse_set("a=0xff").unwrap().value, Value::Integer(255));
        assert_eq!(parse_set("a=\"quoted\"").unwrap().value, Value::String("quoted".into()));
        // Bare words fall back to strings, which is what a host name needs.
        assert_eq!(parse_set("a=dev-pi-01").unwrap().value, Value::String("dev-pi-01".into()));
        assert_eq!(
            parse_set("a=192.168.1.5/24").unwrap().value,
            Value::String("192.168.1.5/24".into())
        );
    }

    #[test]
    fn sets_nested_key() {
        let mut table = doc("[system]\nhostname = \"old\"\n");
        apply(&mut table, &parse_set("system.hostname=new-name").unwrap()).unwrap();
        let Value::Table(system) = &table["system"].value else { panic!() };
        assert_eq!(system["hostname"].value, Value::String("new-name".into()));
    }

    #[test]
    fn creates_missing_tables() {
        let mut table = doc("");
        apply(&mut table, &parse_set("ssh.port=2222").unwrap()).unwrap();
        let Value::Table(ssh) = &table["ssh"].value else { panic!() };
        assert_eq!(ssh["port"].value, Value::Integer(2222));
    }

    #[test]
    fn indexes_array_of_tables() {
        let mut table =
            doc("[[network.ethernet]]\nid = \"a\"\n\n[[network.ethernet]]\nid = \"b\"\n");
        apply(&mut table, &parse_set("network.ethernet[1].id=changed").unwrap()).unwrap();
        let Value::Table(network) = &table["network"].value else { panic!() };
        let Value::Array(items) = &network["ethernet"].value else { panic!() };
        let Value::Table(second) = &items[1].value else { panic!() };
        assert_eq!(second["id"].value, Value::String("changed".into()));
    }

    #[test]
    fn rejects_out_of_range_index() {
        let mut table = doc("[[network.ethernet]]\nid = \"a\"\n");
        let err = apply(&mut table, &parse_set("network.ethernet[5].id=x").unwrap()).unwrap_err();
        assert!(err.message.contains("out of range"), "{}", err.message);
    }

    #[test]
    fn set_secret_builds_inline_table() {
        let over = parse_set_secret("user.password_hash=env:RPI_HASH").unwrap();
        let Value::Table(table) = &over.value else { panic!() };
        assert_eq!(table["env"].value, Value::String("RPI_HASH".into()));
    }

    #[test]
    fn set_secret_replaces_existing_source() {
        let mut table = doc("[user]\npassword_hash = { file = \"secrets/pw\" }\n");
        apply(&mut table, &parse_set_secret("user.password_hash=env:RPI_HASH").unwrap()).unwrap();
        let Value::Table(user) = &table["user"].value else { panic!() };
        let Value::Table(secret) = &user["password_hash"].value else { panic!() };
        assert_eq!(secret.len(), 1, "the previous source must be replaced, not merged");
        assert_eq!(secret["env"].value, Value::String("RPI_HASH".into()));
    }

    #[test]
    fn rejects_malformed_secret_source() {
        assert!(parse_set_secret("user.password_hash=nonsense").is_err());
        assert!(parse_set_secret("user.password_hash").is_err());
    }

    #[test]
    fn rejects_traversal_through_scalar() {
        let mut table = doc("[system]\nhostname = \"pi\"\n");
        let err = apply(&mut table, &parse_set("system.hostname.oops=1").unwrap()).unwrap_err();
        assert!(err.message.contains("cannot be traversed"), "{}", err.message);
    }
}
