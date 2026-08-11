use super::*;

fn get<'a>(table: &'a Table, path: &str) -> &'a Value {
    let mut node: Option<&Node> = None;
    let mut cursor = table;
    for segment in path.split('.') {
        node = cursor.get(segment);
        let n = node.unwrap_or_else(|| panic!("missing key `{segment}` in path `{path}`"));
        if let Value::Table(next) = &n.value {
            cursor = next;
        }
    }
    &node.expect("empty path").value
}

#[test]
fn parses_scalars() {
    let table = parse(
        r#"
        # a comment
        name = "dev-pi-01"
        literal = 'raw\nstring'
        count = 1_000
        hex = 0xff
        oct = 0o755
        bin = 0b1010
        negative = -42
        ratio = 1.5
        yes = true
        no = false
        "#,
    )
    .unwrap();

    assert_eq!(get(&table, "name"), &Value::String("dev-pi-01".into()));
    assert_eq!(get(&table, "literal"), &Value::String("raw\\nstring".into()));
    assert_eq!(get(&table, "count"), &Value::Integer(1000));
    assert_eq!(get(&table, "hex"), &Value::Integer(255));
    assert_eq!(get(&table, "oct"), &Value::Integer(0o755));
    assert_eq!(get(&table, "bin"), &Value::Integer(10));
    assert_eq!(get(&table, "negative"), &Value::Integer(-42));
    assert_eq!(get(&table, "ratio"), &Value::Float(1.5));
    assert_eq!(get(&table, "yes"), &Value::Boolean(true));
    assert_eq!(get(&table, "no"), &Value::Boolean(false));
}

#[test]
fn parses_escapes() {
    let table = parse(r#"s = "tab:\t nl:\n quote:\" back:\\ uni:é""#).unwrap();
    assert_eq!(get(&table, "s"), &Value::String("tab:\t nl:\n quote:\" back:\\ uni:é".into()));
}

#[test]
fn parses_multiline_strings() {
    let table = parse("basic = \"\"\"\nline one\nline two\"\"\"\nliteral = '''\nraw \\n here'''\n")
        .unwrap();
    assert_eq!(get(&table, "basic"), &Value::String("line one\nline two".into()));
    assert_eq!(get(&table, "literal"), &Value::String("raw \\n here".into()));
}

#[test]
fn multiline_backslash_trims_whitespace() {
    let table = parse("s = \"\"\"a \\\n      b\"\"\"\n").unwrap();
    assert_eq!(get(&table, "s"), &Value::String("a b".into()));
}

#[test]
fn parses_tables_and_dotted_keys() {
    let table = parse(
        r#"
        [system]
        hostname = "pi"

        [hardware]
        uart.enabled = true
        i2c = { enabled = true, baudrate = 400000 }
        "#,
    )
    .unwrap();

    assert_eq!(get(&table, "system.hostname"), &Value::String("pi".into()));
    assert_eq!(get(&table, "hardware.uart.enabled"), &Value::Boolean(true));
    assert_eq!(get(&table, "hardware.i2c.baudrate"), &Value::Integer(400_000));
}

#[test]
fn parses_arrays_and_array_tables() {
    let table = parse(
        r#"
        keys = [
          "one",
          "two",
        ]

        [[network.ethernet]]
        id = "a"

        [[network.ethernet]]
        id = "b"
        "#,
    )
    .unwrap();

    let Value::Array(keys) = get(&table, "keys") else { panic!("expected array") };
    assert_eq!(keys.len(), 2);

    let Value::Array(items) = get(&table, "network.ethernet") else { panic!("expected array") };
    assert_eq!(items.len(), 2);
    let Value::Table(second) = &items[1].value else { panic!("expected table") };
    assert_eq!(second["id"].value, Value::String("b".into()));
}

#[test]
fn records_positions() {
    let table = parse("a = 1\n\nb = 2\n").unwrap();
    assert_eq!((table["a"].line, table["a"].col), (1, 5));
    assert_eq!((table["b"].line, table["b"].col), (3, 5));
}

#[test]
fn rejects_duplicate_keys() {
    let err = parse("a = 1\na = 2\n").unwrap_err();
    assert!(err.message.contains("more than once"), "{}", err.message);
    assert_eq!(err.line, 2);
}

#[test]
fn rejects_duplicate_tables() {
    let err = parse("[a]\n[a]\n").unwrap_err();
    assert!(err.message.contains("more than once"), "{}", err.message);
}

#[test]
fn rejects_datetime() {
    let err = parse("when = 2026-08-11T00:00:00Z\n").unwrap_err();
    assert!(err.message.contains("date-time"), "{}", err.message);
}

#[test]
fn rejects_trailing_garbage() {
    let err = parse("a = 1 oops\n").unwrap_err();
    assert!(err.message.contains("unexpected trailing character"), "{}", err.message);
}

#[test]
fn rejects_unterminated_string() {
    let err = parse("a = \"oops\n").unwrap_err();
    assert!(err.message.contains("unterminated"), "{}", err.message);
}

#[test]
fn accepts_crlf_line_endings() {
    let table = parse("a = 1\r\n[t]\r\nb = 2\r\n").unwrap();
    assert_eq!(get(&table, "a"), &Value::Integer(1));
    assert_eq!(get(&table, "t.b"), &Value::Integer(2));
}

#[test]
fn empty_table_is_materialised() {
    let table = parse("[empty]\n").unwrap();
    assert_eq!(get(&table, "empty"), &Value::Table(Table::new()));
}
