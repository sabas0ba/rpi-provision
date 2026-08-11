//! A strict reader over a parsed TOML table.
//!
//! Every access records the key it consumed; [`Reader::finish`] then fails if
//! the document contained keys nobody asked for. Silently ignoring a typo in a
//! provisioning specification is the failure mode this guards against.

use std::collections::BTreeSet;

use rpi_provision_toml::{Node, Table, Value};

use crate::error::{Error, Result};

pub struct Reader<'a> {
    table: &'a Table,
    path: String,
    used: BTreeSet<String>,
    position: Option<(usize, usize)>,
}

impl<'a> Reader<'a> {
    pub fn root(table: &'a Table) -> Self {
        Self { table, path: String::new(), used: BTreeSet::new(), position: None }
    }

    fn child(table: &'a Table, path: String, position: Option<(usize, usize)>) -> Self {
        Self { table, path, used: BTreeSet::new(), position }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    fn key_path(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{}", self.path, key)
        }
    }

    fn error(&self, key: &str, message: impl AsRef<str>) -> Error {
        let text = format!("`{}`: {}", self.key_path(key), message.as_ref());
        match self.node(key).map(|n| (n.line, n.col)).or(self.position) {
            Some((line, col)) => Error::at(line, col, text),
            None => Error::new(text),
        }
    }

    /// Error attached to this table as a whole rather than to one key.
    pub fn table_error(&self, message: impl AsRef<str>) -> Error {
        let text = if self.path.is_empty() {
            message.as_ref().to_string()
        } else {
            format!("`{}`: {}", self.path, message.as_ref())
        };
        match self.position {
            Some((line, col)) => Error::at(line, col, text),
            None => Error::new(text),
        }
    }

    fn node(&self, key: &str) -> Option<&'a Node> {
        self.table.get(key)
    }

    /// Look at the type of a key without consuming it.
    pub fn peek_type(&self, key: &str) -> Option<&'static str> {
        self.table.get(key).map(|node| node.type_name())
    }

    /// Position of a key, for errors raised by callers.
    pub fn position_of(&self, key: &str) -> Option<(usize, usize)> {
        self.table.get(key).map(|node| (node.line, node.col))
    }

    /// Build an error attached to `key`.
    pub fn key_error(&self, key: &str, message: impl AsRef<str>) -> Error {
        self.error(key, message)
    }

    fn take(&mut self, key: &str) -> Option<&'a Node> {
        self.used.insert(key.to_string());
        self.table.get(key)
    }

    fn wrong_type(&self, key: &str, expected: &str, found: &str) -> Error {
        self.error(key, format!("expected {expected}, found {found}"))
    }

    // ------------------------------------------------------------- scalars

    pub fn opt_string(&mut self, key: &str) -> Result<Option<String>> {
        match self.take(key) {
            None => Ok(None),
            Some(Node { value: Value::String(s), .. }) => Ok(Some(s.clone())),
            Some(node) => Err(self.wrong_type(key, "a string", node.type_name())),
        }
    }

    pub fn req_string(&mut self, key: &str) -> Result<String> {
        self.opt_string(key)?.ok_or_else(|| self.error(key, "is required"))
    }

    pub fn string_or(&mut self, key: &str, default: &str) -> Result<String> {
        Ok(self.opt_string(key)?.unwrap_or_else(|| default.to_string()))
    }

    pub fn opt_bool(&mut self, key: &str) -> Result<Option<bool>> {
        match self.take(key) {
            None => Ok(None),
            Some(Node { value: Value::Boolean(b), .. }) => Ok(Some(*b)),
            Some(node) => Err(self.wrong_type(key, "a boolean", node.type_name())),
        }
    }

    pub fn bool_or(&mut self, key: &str, default: bool) -> Result<bool> {
        Ok(self.opt_bool(key)?.unwrap_or(default))
    }

    pub fn opt_integer(&mut self, key: &str) -> Result<Option<i64>> {
        match self.take(key) {
            None => Ok(None),
            Some(Node { value: Value::Integer(i), .. }) => Ok(Some(*i)),
            Some(node) => Err(self.wrong_type(key, "an integer", node.type_name())),
        }
    }

    pub fn integer_or(&mut self, key: &str, default: i64) -> Result<i64> {
        Ok(self.opt_integer(key)?.unwrap_or(default))
    }

    /// Read an integer and check that it fits in an inclusive range.
    pub fn integer_in_range(&mut self, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
        let value = self.integer_or(key, default)?;
        if value < min || value > max {
            return Err(self.error(key, format!("must be between {min} and {max}, got {value}")));
        }
        Ok(value)
    }

    /// Read a string that must be one of `allowed`.
    pub fn enumerated(&mut self, key: &str, default: &str, allowed: &[&str]) -> Result<String> {
        let value = self.string_or(key, default)?;
        if !allowed.contains(&value.as_str()) {
            return Err(
                self.error(key, format!("must be one of {}, got `{value}`", allowed.join(", ")))
            );
        }
        Ok(value)
    }

    // ------------------------------------------------------------- compound

    pub fn string_list(&mut self, key: &str) -> Result<Vec<String>> {
        match self.take(key) {
            None => Ok(Vec::new()),
            Some(Node { value: Value::Array(items), .. }) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match &item.value {
                        Value::String(s) => out.push(s.clone()),
                        other => {
                            return Err(Error::at(
                                item.line,
                                item.col,
                                format!(
                                    "`{}`: array entries must be strings, found {}",
                                    self.key_path(key),
                                    other.type_name()
                                ),
                            ))
                        }
                    }
                }
                Ok(out)
            }
            Some(node) => Err(self.wrong_type(key, "an array of strings", node.type_name())),
        }
    }

    /// Borrow a sub-table. Returns an empty reader when the key is absent so
    /// that callers can apply defaults uniformly.
    pub fn table(&mut self, key: &str) -> Result<Reader<'a>> {
        static EMPTY: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
        match self.take(key) {
            None => {
                Ok(Reader::child(EMPTY.get_or_init(Table::new), self.key_path(key), self.position))
            }
            Some(Node { value: Value::Table(table), line, col }) => {
                Ok(Reader::child(table, self.key_path(key), Some((*line, *col))))
            }
            Some(node) => Err(self.wrong_type(key, "a table", node.type_name())),
        }
    }

    /// Borrow a sub-table only if it was present.
    pub fn opt_table(&mut self, key: &str) -> Result<Option<Reader<'a>>> {
        match self.take(key) {
            None => Ok(None),
            Some(Node { value: Value::Table(table), line, col }) => {
                Ok(Some(Reader::child(table, self.key_path(key), Some((*line, *col)))))
            }
            Some(node) => Err(self.wrong_type(key, "a table", node.type_name())),
        }
    }

    /// Borrow an array of tables, as produced by `[[a.b]]` headers.
    pub fn table_list(&mut self, key: &str) -> Result<Vec<Reader<'a>>> {
        match self.take(key) {
            None => Ok(Vec::new()),
            Some(Node { value: Value::Array(items), .. }) => {
                let mut out = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    match &item.value {
                        Value::Table(table) => out.push(Reader::child(
                            table,
                            format!("{}[{index}]", self.key_path(key)),
                            Some((item.line, item.col)),
                        )),
                        other => {
                            return Err(Error::at(
                                item.line,
                                item.col,
                                format!(
                                    "`{}`: entries must be tables, found {}",
                                    self.key_path(key),
                                    other.type_name()
                                ),
                            ))
                        }
                    }
                }
                Ok(out)
            }
            Some(node) => Err(self.wrong_type(key, "an array of tables", node.type_name())),
        }
    }

    /// Fail if the document declared keys that were never read.
    pub fn finish(&self) -> Result<()> {
        let unknown: Vec<&str> =
            self.table.keys().filter(|key| !self.used.contains(*key)).map(String::as_str).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let node = self.table.get(unknown[0]);
        let location = node.map(|n| (n.line, n.col)).or(self.position);
        let scope = if self.path.is_empty() {
            "the document root".to_string()
        } else {
            format!("`{}`", self.path)
        };
        let message = format!(
            "unknown key{} in {scope}: {}",
            if unknown.len() == 1 { "" } else { "s" },
            unknown.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")
        );
        Err(match location {
            Some((line, col)) => Error::at(line, col, message),
            None => Error::new(message),
        })
    }
}
