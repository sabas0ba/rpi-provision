//! Tests for the parts of the front end that are not the window.
//!
//! The Tauri commands are thin: they load a specification, hand it to the
//! same three crates the command line uses, and shape the result for the
//! page. What is worth testing here is the shaping, and `set_value`, which
//! is the only place this crate manipulates a specification itself.

use super::*;

const SPEC: &str = r#"# A comment that must survive being edited.
[meta]
schema_version = 1

[system]
hostname = "dev-pi-01"   # and a trailing one

[user]
name = "engineer"
authorized_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host"]
"#;

fn spec_input(text: &str) -> SpecInput {
    SpecInput { text: text.to_string(), base_dir: ".".to_string(), secrets: BTreeMap::new() }
}

#[test]
fn a_valid_specification_is_summarised() {
    let result = validate(spec_input(SPEC));
    assert!(result.ok, "{:?}", result.error);
    let summary = result.summary.expect("a summary");
    assert_eq!(summary.hostname, "dev-pi-01");
    assert_eq!(summary.user, "engineer");
    assert_eq!(summary.target, "pi5");
    assert_eq!(summary.hardware, "none");
    assert!(summary.ssh.contains("1 key(s)"), "{}", summary.ssh);
    assert!(!summary.digest.is_empty());
}

#[test]
fn an_invalid_specification_reports_the_message_the_cli_would() {
    let result = validate(spec_input("[meta]\nschema_version = 1\n"));
    assert!(!result.ok);
    let error = result.error.expect("an error");
    assert!(error.contains("system"), "{error}");
}

#[test]
fn a_warning_is_passed_through_rather_than_being_an_error() {
    // A gateway outside the subnet is accepted with a warning.
    let text = format!(
        "{SPEC}\n[[network.ethernet]]\nid = \"eth0\"\nmethod = \"manual\"\n\
         address = \"192.168.1.50/24\"\ngateway = \"10.0.0.1\"\n"
    );
    let result = validate(spec_input(&text));
    assert!(result.ok, "{:?}", result.error);
    assert!(!result.warnings.is_empty(), "the gateway warning must reach the window");
}

#[test]
fn secrets_typed_into_the_window_are_used() {
    let text = format!("{SPEC}password_hash = {{ env = \"GUI_TEST_HASH\" }}\n");
    let mut input = spec_input(&text);

    // Without the value, loading fails on the missing variable.
    let missing = validate(SpecInput { secrets: BTreeMap::new(), ..spec_input(&text) });
    assert!(!missing.ok);
    assert!(missing.error.unwrap().contains("GUI_TEST_HASH"));

    input.secrets.insert(
        "GUI_TEST_HASH".to_string(),
        "$6$rounds=4096$abcdefgh$0123456789abcdef".to_string(),
    );
    let result = validate(input);
    assert!(result.ok, "{:?}", result.error);
}

// --------------------------------------------------------------- set_value

fn set(text: &str, path: &str, value: serde_json::Value) -> String {
    set_value(text.to_string(), path.to_string(), value).expect("the edit must apply")
}

#[test]
fn editing_a_key_keeps_the_comments_around_it() {
    let edited = set(SPEC, "system.hostname", "dev-pi-02".into());
    assert!(edited.contains("dev-pi-02"));
    assert!(!edited.contains("dev-pi-01"));
    assert!(
        edited.contains("# A comment that must survive being edited."),
        "a specification is meant to stay in version control:\n{edited}"
    );
    assert!(edited.contains("# and a trailing one"), "{edited}");
    assert!(validate(spec_input(&edited)).ok);
}

#[test]
fn a_missing_table_is_created() {
    let edited = set(SPEC, "ssh.port", serde_json::json!(2222));
    assert!(edited.contains("[ssh]"), "{edited}");
    let result = validate(spec_input(&edited));
    assert!(result.ok, "{:?}", result.error);
}

#[test]
fn types_survive_the_round_trip() {
    let edited = set(SPEC, "ssh.password_authentication", serde_json::json!(true));
    assert!(edited.contains("password_authentication = true"), "{edited}");

    let edited = set(&edited, "hardware.i2c.baudrate", serde_json::json!(400000));
    assert!(edited.contains("baudrate = 400000"), "{edited}");

    let edited = set(&edited, "user.groups", serde_json::json!(["gpio", "i2c"]));
    assert!(edited.contains(r#"["gpio", "i2c"]"#), "{edited}");
    assert!(validate(spec_input(&edited)).ok);
}

#[test]
fn clearing_a_key_removes_it_so_the_default_applies() {
    let with = set(SPEC, "system.timezone", "Asia/Tokyo".into());
    assert!(with.contains("Asia/Tokyo"));

    let without = set(&with, "system.timezone", "".into());
    assert!(!without.contains("timezone"), "{without}");
    assert!(validate(spec_input(&without)).ok);
}

#[test]
fn clearing_a_key_in_a_table_that_does_not_exist_is_harmless() {
    let unchanged = set(SPEC, "hardware.uart.baudrate", "".into());
    assert_eq!(unchanged, SPEC, "nothing to remove must not create the table");
}

#[test]
fn an_edit_that_would_corrupt_the_document_is_refused() {
    // `system` is a table, so it cannot also hold a value at `system.x.y`.
    let error = set_value(
        "[system]\nhostname = \"a\"\n".to_string(),
        "system.hostname.deeper".to_string(),
        "x".into(),
    )
    .unwrap_err();
    assert!(error.contains("not a table"), "{error}");
}

#[test]
fn a_specification_that_does_not_parse_is_reported_before_editing() {
    let error =
        set_value("[system".to_string(), "system.hostname".to_string(), "a".into()).unwrap_err();
    assert!(error.contains("does not parse"), "{error}");
}

// ------------------------------------------------------------------- dates

#[test]
fn the_timestamp_is_a_utc_instant() {
    let stamp = now_utc();
    assert_eq!(stamp.len(), 20, "{stamp}");
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29), "2000 was a leap year");
    assert_eq!(civil_from_days(47_482), (2100, 1, 1), "2100 is not");
}
