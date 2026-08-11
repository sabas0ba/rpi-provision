//! Minimal, dependency-free TOML parser covering the subset used by
//! `rpi-provision` specification files.
//!
//! Supported:
//! - comments, bare and quoted keys, dotted keys
//! - tables `[a.b]` and arrays of tables `[[a.b]]`
//! - inline tables `{ a = 1 }` and arrays `[1, 2]` (trailing commas allowed)
//! - basic strings, literal strings, multi-line variants of both
//! - integers (decimal with `_`, `0x`, `0o`, `0b`), floats, booleans
//!
//! Not supported (rejected with a diagnostic): date-time values.
//!
//! Every value keeps its source position so that semantic errors raised by
//! downstream crates can point at the offending line.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub type Table = BTreeMap<String, Node>;

/// A parsed value together with the position it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub value: Value,
    pub line: usize,
    pub col: usize,
}

impl Node {
    pub fn new(value: Value, line: usize, col: usize) -> Self {
        Self { value, line, col }
    }

    /// Human readable name of the contained value, used in error messages.
    pub fn type_name(&self) -> &'static str {
        self.value.type_name()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<Node>),
    Table(Table),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Integer(_) => "integer",
            Value::Float(_) => "float",
            Value::Boolean(_) => "boolean",
            Value::Array(_) => "array",
            Value::Table(_) => "table",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Parse a TOML document into a table.
pub fn parse(input: &str) -> Result<Table> {
    Parser::new(input).parse_document()
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
    /// Paths declared with an explicit `[table]` header.
    explicit_tables: BTreeSet<Vec<String>>,
    /// Paths declared with an explicit `[[array]]` header.
    array_tables: BTreeSet<Vec<String>>,
    /// Paths that were created implicitly by a dotted key assignment and may
    /// therefore not be reopened with a header.
    dotted_tables: BTreeSet<Vec<String>>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            explicit_tables: BTreeSet::new(),
            array_tables: BTreeSet::new(),
            dotted_tables: BTreeSet::new(),
        }
    }

    // ---------------------------------------------------------------- cursor

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else if b & 0xC0 != 0x80 {
            // Only count UTF-8 lead bytes so that columns track characters.
            self.col += 1;
        }
        Some(b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(Error { message: message.into(), line: self.line, col: self.col })
    }

    fn err_at<T>(&self, line: usize, col: usize, message: impl Into<String>) -> Result<T> {
        Err(Error { message: message.into(), line, col })
    }

    // ------------------------------------------------------------ whitespace

    fn skip_inline_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.bump();
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some(b'#') {
            while let Some(b) = self.peek() {
                if b == b'\n' {
                    break;
                }
                self.bump();
            }
        }
    }

    /// Skip whitespace, comments and newlines until the next meaningful byte.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
                    self.bump();
                }
                Some(b'#') => self.skip_comment(),
                _ => return,
            }
        }
    }

    /// Consume the remainder of a line, allowing only whitespace and comments.
    fn expect_line_end(&mut self) -> Result<()> {
        self.skip_inline_whitespace();
        self.skip_comment();
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.bump();
                Ok(())
            }
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                self.bump();
                self.bump();
                Ok(())
            }
            Some(b) => self.err(format!("unexpected trailing character {:?}", b as char)),
        }
    }

    // -------------------------------------------------------------- document

    fn parse_document(mut self) -> Result<Table> {
        let mut root = Table::new();
        let mut current: Vec<String> = Vec::new();

        loop {
            self.skip_trivia();
            match self.peek() {
                None => break,
                Some(b'[') => {
                    if self.peek_at(1) == Some(b'[') {
                        current = self.parse_array_table_header(&mut root)?;
                    } else {
                        current = self.parse_table_header(&mut root)?;
                    }
                }
                _ => {
                    self.parse_keyval(&mut root, &current)?;
                }
            }
        }

        Ok(root)
    }

    fn parse_table_header(&mut self, root: &mut Table) -> Result<Vec<String>> {
        let (line, col) = (self.line, self.col);
        self.bump(); // '['
        self.skip_inline_whitespace();
        let path = self.parse_key_path()?;
        self.skip_inline_whitespace();
        if !self.eat(b']') {
            return self.err("expected `]` to close the table header");
        }
        self.expect_line_end()?;

        if self.explicit_tables.contains(&path) || self.dotted_tables.contains(&path) {
            return self.err_at(
                line,
                col,
                format!("table `{}` is defined more than once", path.join(".")),
            );
        }
        if self.array_tables.contains(&path) {
            return self.err_at(
                line,
                col,
                format!("`{}` is already defined as an array of tables", path.join(".")),
            );
        }
        self.explicit_tables.insert(path.clone());
        // Materialise the table so that empty sections are still present.
        self.navigate(root, &path, line, col)?;
        Ok(path)
    }

    fn parse_array_table_header(&mut self, root: &mut Table) -> Result<Vec<String>> {
        let (line, col) = (self.line, self.col);
        self.bump(); // '['
        self.bump(); // '['
        self.skip_inline_whitespace();
        let path = self.parse_key_path()?;
        self.skip_inline_whitespace();
        if !(self.eat(b']') && self.eat(b']')) {
            return self.err("expected `]]` to close the array-of-tables header");
        }
        self.expect_line_end()?;

        if self.explicit_tables.contains(&path) || self.dotted_tables.contains(&path) {
            return self.err_at(
                line,
                col,
                format!("`{}` is already defined as a table", path.join(".")),
            );
        }
        self.array_tables.insert(path.clone());

        let (parent_path, key) = path.split_at(path.len() - 1);
        let key = key[0].clone();
        let parent = self.navigate(root, parent_path, line, col)?;
        let entry = parent
            .entry(key.clone())
            .or_insert_with(|| Node::new(Value::Array(Vec::new()), line, col));
        match &mut entry.value {
            Value::Array(items) => {
                items.push(Node::new(Value::Table(Table::new()), line, col));
                Ok(path)
            }
            other => self.err_at(
                line,
                col,
                format!("cannot append to `{}`: it is a {}", path.join("."), other.type_name()),
            ),
        }
    }

    /// Resolve `path` inside `root`, creating intermediate tables and
    /// following the last element of arrays of tables.
    fn navigate<'t>(
        &self,
        root: &'t mut Table,
        path: &[String],
        line: usize,
        col: usize,
    ) -> Result<&'t mut Table> {
        let mut cursor = root;
        for (index, segment) in path.iter().enumerate() {
            let entry = cursor
                .entry(segment.clone())
                .or_insert_with(|| Node::new(Value::Table(Table::new()), line, col));
            cursor = match &mut entry.value {
                Value::Table(table) => table,
                Value::Array(items) => match items.last_mut() {
                    Some(Node { value: Value::Table(table), .. }) => table,
                    _ => {
                        return self.err_at(
                            line,
                            col,
                            format!("`{}` is not a table", path[..=index].join(".")),
                        )
                    }
                },
                other => {
                    return self.err_at(
                        line,
                        col,
                        format!(
                            "`{}` is a {} and cannot contain sub-keys",
                            path[..=index].join("."),
                            other.type_name()
                        ),
                    )
                }
            };
        }
        Ok(cursor)
    }

    // ------------------------------------------------------------------ keys

    fn parse_key_path(&mut self) -> Result<Vec<String>> {
        let mut path = vec![self.parse_key_segment()?];
        loop {
            self.skip_inline_whitespace();
            if self.peek() == Some(b'.') {
                self.bump();
                self.skip_inline_whitespace();
                path.push(self.parse_key_segment()?);
            } else {
                return Ok(path);
            }
        }
    }

    fn parse_key_segment(&mut self) -> Result<String> {
        match self.peek() {
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') => self.parse_literal_string(),
            Some(b) if is_bare_key_byte(b) => {
                let start = self.pos;
                while matches!(self.peek(), Some(b) if is_bare_key_byte(b)) {
                    self.bump();
                }
                Ok(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
            }
            Some(b) => self.err(format!("invalid key character {:?}", b as char)),
            None => self.err("unexpected end of input while reading a key"),
        }
    }

    // ---------------------------------------------------------- key = value

    fn parse_keyval(&mut self, root: &mut Table, current: &[String]) -> Result<()> {
        let (line, col) = (self.line, self.col);
        let key_path = self.parse_key_path()?;
        self.skip_inline_whitespace();
        if !self.eat(b'=') {
            return self.err("expected `=` after the key");
        }
        self.skip_inline_whitespace();
        let node = self.parse_value()?;
        self.expect_line_end()?;

        let mut full = current.to_vec();
        full.extend(key_path.iter().cloned());
        let (parent_path, key) = full.split_at(full.len() - 1);
        let key = key[0].clone();

        // Dotted keys implicitly define tables which may not be reopened.
        for depth in current.len()..parent_path.len() {
            self.dotted_tables.insert(parent_path[..=depth].to_vec());
        }

        let parent = self.navigate(root, parent_path, line, col)?;
        if parent.contains_key(&key) {
            return self.err_at(
                line,
                col,
                format!("key `{}` is defined more than once", full.join(".")),
            );
        }
        parent.insert(key, node);
        Ok(())
    }

    // ---------------------------------------------------------------- values

    fn parse_value(&mut self) -> Result<Node> {
        let (line, col) = (self.line, self.col);
        let value = match self.peek() {
            Some(b'"') | Some(b'\'') => Value::String(self.parse_string()?),
            Some(b'[') => self.parse_array()?,
            Some(b'{') => self.parse_inline_table()?,
            Some(b't') | Some(b'f') => self.parse_boolean()?,
            Some(b) if b == b'+' || b == b'-' || b.is_ascii_digit() => self.parse_number()?,
            Some(b) => return self.err(format!("unexpected value starting with {:?}", b as char)),
            None => return self.err("unexpected end of input while reading a value"),
        };
        Ok(Node::new(value, line, col))
    }

    fn parse_boolean(&mut self) -> Result<Value> {
        if self.consume_word("true") {
            Ok(Value::Boolean(true))
        } else if self.consume_word("false") {
            Ok(Value::Boolean(false))
        } else {
            self.err("expected `true` or `false`")
        }
    }

    fn consume_word(&mut self, word: &str) -> bool {
        let end = self.pos + word.len();
        if self.bytes.len() >= end && &self.bytes[self.pos..end] == word.as_bytes() {
            let next_is_ident = self
                .bytes
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-');
            if !next_is_ident {
                for _ in 0..word.len() {
                    self.bump();
                }
                return true;
            }
        }
        false
    }

    fn parse_number(&mut self) -> Result<Value> {
        let start = self.pos;
        let (line, col) = (self.line, self.col);
        while matches!(self.peek(), Some(b) if is_number_byte(b)) {
            self.bump();
        }
        let raw = String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned();

        if raw.contains(':') || (raw.matches('-').count() > 1 && !raw.starts_with('-')) {
            return self.err_at(
                line,
                col,
                "date-time values are not supported; quote the value as a string instead",
            );
        }

        let cleaned = raw.replace('_', "");
        if let Some(hex) = cleaned.strip_prefix("0x") {
            return i64::from_str_radix(hex, 16).map(Value::Integer).or_else(|_| {
                self.err_at(line, col, format!("invalid hexadecimal integer `{raw}`"))
            });
        }
        if let Some(oct) = cleaned.strip_prefix("0o") {
            return i64::from_str_radix(oct, 8)
                .map(Value::Integer)
                .or_else(|_| self.err_at(line, col, format!("invalid octal integer `{raw}`")));
        }
        if let Some(bin) = cleaned.strip_prefix("0b") {
            return i64::from_str_radix(bin, 2)
                .map(Value::Integer)
                .or_else(|_| self.err_at(line, col, format!("invalid binary integer `{raw}`")));
        }
        if cleaned.contains('.') || cleaned.contains('e') || cleaned.contains('E') {
            return cleaned
                .parse::<f64>()
                .map(Value::Float)
                .or_else(|_| self.err_at(line, col, format!("invalid float `{raw}`")));
        }
        cleaned
            .parse::<i64>()
            .map(Value::Integer)
            .or_else(|_| self.err_at(line, col, format!("invalid integer `{raw}`")))
    }

    fn parse_array(&mut self) -> Result<Value> {
        self.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(b']') {
                self.bump();
                return Ok(Value::Array(items));
            }
            if self.peek().is_none() {
                return self.err("unterminated array");
            }
            items.push(self.parse_value()?);
            self.skip_trivia();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                }
                Some(b']') => {}
                _ => return self.err("expected `,` or `]` in array"),
            }
        }
    }

    fn parse_inline_table(&mut self) -> Result<Value> {
        let (line, col) = (self.line, self.col);
        self.bump(); // '{'
        let mut table = Table::new();
        loop {
            self.skip_inline_whitespace();
            if self.peek() == Some(b'}') {
                self.bump();
                return Ok(Value::Table(table));
            }
            let key_line = self.line;
            let key_col = self.col;
            let path = self.parse_key_path()?;
            self.skip_inline_whitespace();
            if !self.eat(b'=') {
                return self.err("expected `=` in inline table");
            }
            self.skip_inline_whitespace();
            let node = self.parse_value()?;

            let (parent_path, key) = path.split_at(path.len() - 1);
            let key = key[0].clone();
            let parent = self.navigate(&mut table, parent_path, line, col)?;
            if parent.contains_key(&key) {
                return self.err_at(
                    key_line,
                    key_col,
                    format!("key `{}` is defined more than once", path.join(".")),
                );
            }
            parent.insert(key, node);

            self.skip_inline_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                }
                Some(b'}') => {}
                _ => return self.err("expected `,` or `}` in inline table"),
            }
        }
    }

    // --------------------------------------------------------------- strings

    fn parse_string(&mut self) -> Result<String> {
        match self.peek() {
            Some(b'"') if self.peek_at(1) == Some(b'"') && self.peek_at(2) == Some(b'"') => {
                self.parse_multiline_basic_string()
            }
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') if self.peek_at(1) == Some(b'\'') && self.peek_at(2) == Some(b'\'') => {
                self.parse_multiline_literal_string()
            }
            Some(b'\'') => self.parse_literal_string(),
            _ => self.err("expected a string"),
        }
    }

    fn parse_basic_string(&mut self) -> Result<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated basic string"),
                Some(b'"') => {
                    self.bump();
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.bump();
                    self.parse_escape(&mut out)?;
                }
                _ => self.push_char(&mut out),
            }
        }
    }

    fn parse_multiline_basic_string(&mut self) -> Result<String> {
        for _ in 0..3 {
            self.bump();
        }
        // A newline immediately after the opening delimiter is trimmed.
        if self.peek() == Some(b'\r') && self.peek_at(1) == Some(b'\n') {
            self.bump();
            self.bump();
        } else if self.peek() == Some(b'\n') {
            self.bump();
        }
        let mut out = String::new();
        loop {
            if self.peek() == Some(b'"')
                && self.peek_at(1) == Some(b'"')
                && self.peek_at(2) == Some(b'"')
            {
                for _ in 0..3 {
                    self.bump();
                }
                return Ok(out);
            }
            match self.peek() {
                None => return self.err("unterminated multi-line basic string"),
                Some(b'\\') => {
                    // A backslash at end of line trims the following whitespace.
                    let mut lookahead = self.pos + 1;
                    while matches!(
                        self.bytes.get(lookahead),
                        Some(b' ') | Some(b'\t') | Some(b'\r')
                    ) {
                        lookahead += 1;
                    }
                    if self.bytes.get(lookahead) == Some(&b'\n') {
                        self.bump(); // backslash
                        while matches!(
                            self.peek(),
                            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
                        ) {
                            self.bump();
                        }
                    } else {
                        self.bump();
                        self.parse_escape(&mut out)?;
                    }
                }
                _ => self.push_char(&mut out),
            }
        }
    }

    fn parse_literal_string(&mut self) -> Result<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated literal string"),
                Some(b'\'') => {
                    self.bump();
                    return Ok(out);
                }
                _ => self.push_char(&mut out),
            }
        }
    }

    fn parse_multiline_literal_string(&mut self) -> Result<String> {
        for _ in 0..3 {
            self.bump();
        }
        if self.peek() == Some(b'\r') && self.peek_at(1) == Some(b'\n') {
            self.bump();
            self.bump();
        } else if self.peek() == Some(b'\n') {
            self.bump();
        }
        let mut out = String::new();
        loop {
            if self.peek() == Some(b'\'')
                && self.peek_at(1) == Some(b'\'')
                && self.peek_at(2) == Some(b'\'')
            {
                for _ in 0..3 {
                    self.bump();
                }
                return Ok(out);
            }
            if self.peek().is_none() {
                return self.err("unterminated multi-line literal string");
            }
            self.push_char(&mut out);
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<()> {
        let b = match self.bump() {
            Some(b) => b,
            None => return self.err("unterminated escape sequence"),
        };
        let ch = match b {
            b'b' => '\u{0008}',
            b't' => '\t',
            b'n' => '\n',
            b'f' => '\u{000C}',
            b'r' => '\r',
            b'"' => '"',
            b'\\' => '\\',
            b'u' => return self.parse_unicode_escape(out, 4),
            b'U' => return self.parse_unicode_escape(out, 8),
            other => return self.err(format!("unknown escape sequence `\\{}`", other as char)),
        };
        out.push(ch);
        Ok(())
    }

    fn parse_unicode_escape(&mut self, out: &mut String, width: usize) -> Result<()> {
        let mut code: u32 = 0;
        for _ in 0..width {
            let b = match self.bump() {
                Some(b) => b,
                None => return self.err("truncated unicode escape"),
            };
            let digit = match (b as char).to_digit(16) {
                Some(d) => d,
                None => return self.err(format!("invalid hexadecimal digit {:?}", b as char)),
            };
            code = code * 16 + digit;
        }
        match char::from_u32(code) {
            Some(ch) => {
                out.push(ch);
                Ok(())
            }
            None => self.err(format!("`\\u{code:04X}` is not a valid Unicode scalar value")),
        }
    }

    /// Copy one whole UTF-8 character from the input into `out`.
    fn push_char(&mut self, out: &mut String) {
        let start = self.pos;
        self.bump();
        while matches!(self.peek(), Some(b) if b & 0xC0 == 0x80) {
            self.bump();
        }
        out.push_str(&String::from_utf8_lossy(&self.bytes[start..self.pos]));
    }
}

fn is_bare_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_number_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'+' || b == b'-' || b == b':'
}

#[cfg(test)]
mod tests;
