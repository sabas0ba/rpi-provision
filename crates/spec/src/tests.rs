use std::net::Ipv4Addr;

use super::*;

const MINIMAL: &str = r#"
[meta]
schema_version = 1

[system]
hostname = "dev-pi-01"

[user]
name = "engineer"
authorized_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host"]
"#;

fn secrets() -> MapSecrets {
    MapSecrets::default()
        .with_env("RPI_PASSWORD_HASH", "$6$rounds=4096$abcdefgh$0123456789abcdef")
        .with_env("WIFI_PSK", "correct-horse-battery")
}

fn load(source: &str) -> Result<Loaded> {
    let provider = secrets();
    let options = LoadOptions::new(&provider);
    load_str(source, &options)
}

#[test]
fn loads_minimal_specification() {
    let loaded = load(MINIMAL).unwrap();
    assert_eq!(loaded.spec.meta.target, Target::Pi5);
    assert_eq!(loaded.spec.system.hostname, "dev-pi-01");
    assert_eq!(loaded.spec.user.name, "engineer");
    assert!(loaded.spec.ssh.enabled);
    assert!(!loaded.spec.ssh.password_authentication);
    assert_eq!(loaded.spec.provisioning.boot_mount, "/boot/firmware");
    assert!(loaded.spec.provisioning.wipe_payload);
    assert_eq!(loaded.digest.len(), 64);
}

#[test]
fn digest_ignores_formatting_but_tracks_content() {
    let a = load(MINIMAL).unwrap();
    let reordered = format!("# leading comment\n{}\n\n", MINIMAL);
    let b = load(&reordered).unwrap();
    assert_eq!(a.digest, b.digest);

    let changed = MINIMAL.replace("dev-pi-01", "dev-pi-02");
    let c = load(&changed).unwrap();
    assert_ne!(a.digest, c.digest);
}

#[test]
fn rejects_unknown_keys() {
    let err = load(&format!("{MINIMAL}\n[system]\nhostnaem = \"typo\"\n")).unwrap_err();
    assert!(err.message.contains("more than once") || err.message.contains("unknown key"));

    let source =
        MINIMAL.replace("hostname = \"dev-pi-01\"", "hostname = \"pi\"\nhostnaem = \"typo\"");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("unknown key"), "{}", err.message);
    assert!(err.message.contains("hostnaem"), "{}", err.message);
}

#[test]
fn rejects_unknown_top_level_section() {
    let err = load(&format!("{MINIMAL}\n[netwrok]\n")).unwrap_err();
    assert!(err.message.contains("unknown key"), "{}", err.message);
    assert!(err.message.contains("netwrok"), "{}", err.message);
}

#[test]
fn reports_position_of_type_errors() {
    let source = MINIMAL.replace("hostname = \"dev-pi-01\"", "hostname = 42");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("expected a string"), "{}", err.message);
    assert!(err.position.is_some());
}

#[test]
fn requires_a_way_to_log_in() {
    let source = r#"
[meta]
schema_version = 1
[system]
hostname = "pi"
[user]
name = "engineer"
"#;
    let err = load(source).unwrap_err();
    assert!(err.message.contains("no way to log in"), "{}", err.message);
}

#[test]
fn rejects_password_login_without_keys_when_password_auth_is_off() {
    let source = r#"
[meta]
schema_version = 1
[system]
hostname = "pi"
[user]
name = "engineer"
password_hash = { env = "RPI_PASSWORD_HASH" }
"#;
    let err = load(source).unwrap_err();
    assert!(err.message.contains("unreachable"), "{}", err.message);
}

#[test]
fn accepts_password_login_when_explicitly_enabled() {
    let source = r#"
[meta]
schema_version = 1
[system]
hostname = "pi"
[user]
name = "engineer"
password_hash = { env = "RPI_PASSWORD_HASH" }
[ssh]
password_authentication = true
"#;
    let loaded = load(source).unwrap();
    assert!(loaded.spec.ssh.password_authentication);
    assert!(loaded.spec.user.password_hash.is_some());
}

#[test]
fn secrets_come_from_the_environment() {
    let source = format!("{MINIMAL}\npassword_hash = {{ env = \"RPI_PASSWORD_HASH\" }}\n");
    let loaded = load(&source).unwrap();
    assert_eq!(
        loaded.spec.user.password_hash.as_deref(),
        Some("$6$rounds=4096$abcdefgh$0123456789abcdef")
    );
}

#[test]
fn secrets_may_not_be_bare_strings() {
    let source = format!("{MINIMAL}\npassword_hash = \"$6$abc$def\"\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("must not be a bare string"), "{}", err.message);
}

#[test]
fn missing_environment_variable_is_reported() {
    let source = format!("{MINIMAL}\npassword_hash = {{ env = \"ABSENT\" }}\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("`ABSENT` is not set"), "{}", err.message);
}

#[test]
fn secret_from_file_trims_trailing_newline() {
    let provider = MapSecrets::default().with_file("/spec/pw.hash", "$6$abc$def\n");
    let options = LoadOptions::new(&provider).base_dir("/spec");
    let source = format!("{MINIMAL}\npassword_hash = {{ file = \"pw.hash\" }}\n[ssh]\npassword_authentication = true\n");
    let loaded = load_str(&source, &options).unwrap();
    assert_eq!(loaded.spec.user.password_hash.as_deref(), Some("$6$abc$def"));
}

#[test]
fn rejects_plaintext_password() {
    let provider = MapSecrets::default().with_env("PW", "hunter2");
    let options = LoadOptions::new(&provider);
    let source = format!("{MINIMAL}\npassword_hash = {{ env = \"PW\" }}\n");
    let err = load_str(&source, &options).unwrap_err();
    assert!(err.message.contains("crypt(3)"), "{}", err.message);
}

#[test]
fn rejects_malformed_authorized_key() {
    let source = MINIMAL.replace("ssh-ed25519", "ssh-nonsense");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("not a recognised SSH key type"), "{}", err.message);
}

#[test]
fn command_line_set_overrides_a_value() {
    let provider = secrets();
    let options = LoadOptions::new(&provider)
        .set("system.hostname=override-pi")
        .unwrap()
        .set("ssh.port=2222")
        .unwrap();
    let loaded = load_str(MINIMAL, &options).unwrap();
    assert_eq!(loaded.spec.system.hostname, "override-pi");
    assert_eq!(loaded.spec.ssh.port, 2222);
}

#[test]
fn command_line_set_secret_overrides_the_source() {
    let provider = MapSecrets::default().with_env("OTHER", "$6$xyz$123");
    let options = LoadOptions::new(&provider)
        .set_secret("user.password_hash=env:OTHER")
        .unwrap()
        .set("ssh.password_authentication=true")
        .unwrap();
    let source = format!("{MINIMAL}\npassword_hash = {{ env = \"RPI_PASSWORD_HASH\" }}\n");
    let loaded = load_str(&source, &options).unwrap();
    assert_eq!(loaded.spec.user.password_hash.as_deref(), Some("$6$xyz$123"));
}

#[test]
fn overrides_change_the_digest() {
    let provider = secrets();
    let plain = load_str(MINIMAL, &LoadOptions::new(&provider)).unwrap();
    let overridden =
        load_str(MINIMAL, &LoadOptions::new(&provider).set("system.hostname=other").unwrap())
            .unwrap();
    assert_ne!(plain.digest, overridden.digest);
}

#[test]
fn parses_static_ethernet() {
    let source = format!(
        r#"{MINIMAL}
[[network.ethernet]]
id = "eth0-static"
method = "manual"
address = "192.168.1.50/24"
gateway = "192.168.1.1"
dns = ["192.168.1.1", "1.1.1.1"]
autoconnect_priority = 100
"#
    );
    let loaded = load(&source).unwrap();
    let eth = &loaded.spec.network.ethernet[0];
    assert_eq!(eth.interface, "eth0");
    assert_eq!(eth.ip.method, IpMethod::Manual);
    assert_eq!(eth.ip.address.unwrap().to_string(), "192.168.1.50/24");
    assert_eq!(eth.ip.gateway, Some(Ipv4Addr::new(192, 168, 1, 1)));
    assert_eq!(eth.ip.dns.len(), 2);
    assert_eq!(eth.autoconnect_priority, 100);
    assert!(loaded.warnings.is_empty());
}

#[test]
fn manual_method_requires_an_address() {
    let source = format!("{MINIMAL}\n[[network.ethernet]]\nid = \"eth0\"\nmethod = \"manual\"\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("no `address` was given"), "{}", err.message);
}

#[test]
fn address_without_manual_method_is_rejected() {
    let source =
        format!("{MINIMAL}\n[[network.ethernet]]\nid = \"eth0\"\naddress = \"192.168.1.5/24\"\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("only meaningful when"), "{}", err.message);
}

#[test]
fn gateway_outside_subnet_warns() {
    let source = format!(
        r#"{MINIMAL}
[[network.ethernet]]
id = "eth0"
method = "manual"
address = "192.168.1.50/24"
gateway = "10.0.0.1"
"#
    );
    let loaded = load(&source).unwrap();
    assert_eq!(loaded.warnings.len(), 1);
    assert!(loaded.warnings[0].contains("outside"), "{}", loaded.warnings[0]);
}

#[test]
fn wifi_requires_a_country_code() {
    let source = format!(
        "{MINIMAL}\n[[network.wifi]]\nid = \"home\"\nssid = \"MySSID\"\npsk = {{ env = \"WIFI_PSK\" }}\n"
    );
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("wifi_country"), "{}", err.message);
}

#[test]
fn parses_wifi() {
    let source = format!(
        r#"{MINIMAL}
[network]
wifi_country = "JP"

[[network.wifi]]
id = "home"
ssid = "MySSID"
psk = {{ env = "WIFI_PSK" }}
"#
    );
    let loaded = load(&source).unwrap();
    let wifi = &loaded.spec.network.wifi[0];
    assert_eq!(wifi.ssid, "MySSID");
    assert_eq!(wifi.security, WifiSecurity::WpaPsk);
    assert_eq!(wifi.psk.as_deref(), Some("correct-horse-battery"));
    assert_eq!(wifi.interface, "wlan0");
}

#[test]
fn open_wifi_must_not_carry_a_psk() {
    let source = format!(
        r#"{MINIMAL}
[network]
wifi_country = "JP"
[[network.wifi]]
id = "open"
ssid = "Guest"
security = "open"
psk = {{ env = "WIFI_PSK" }}
"#
    );
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("open network"), "{}", err.message);
}

#[test]
fn parses_usb_gadget_with_derived_addresses() {
    let source = format!(
        r#"{MINIMAL}
[network.usb_gadget]
enabled = true
address = "10.55.0.1/24"
peer_address = "10.55.0.2"
"#
    );
    let loaded = load(&source).unwrap();
    let gadget = loaded.spec.network.usb_gadget.as_ref().unwrap();
    assert_eq!(gadget.function, GadgetFunction::Ecm);
    assert_eq!(gadget.interface, "usb0");
    assert_eq!(gadget.address.to_string(), "10.55.0.1/24");
    assert_eq!(gadget.peer_address, Some(Ipv4Addr::new(10, 55, 0, 2)));
    assert_ne!(gadget.device_mac, gadget.host_mac);
    assert!(gadget.device_mac.is_locally_administered());
    assert_eq!(gadget.vendor_id, 0x1d6b);
    assert_eq!(gadget.serial, "dev-pi-01");

    // Derivation must be stable across loads.
    let again = load(&source).unwrap();
    assert_eq!(gadget.device_mac, again.spec.network.usb_gadget.unwrap().device_mac);
}

#[test]
fn gadget_peer_must_be_in_subnet() {
    let source = format!(
        "{MINIMAL}\n[network.usb_gadget]\nenabled = true\naddress = \"10.55.0.1/24\"\npeer_address = \"10.56.0.2\"\n"
    );
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("is outside"), "{}", err.message);
}

#[test]
fn disabled_gadget_yields_none() {
    let source = format!("{MINIMAL}\n[network.usb_gadget]\nenabled = false\n");
    let loaded = load(&source).unwrap();
    assert!(loaded.spec.network.usb_gadget.is_none());
}

#[test]
fn duplicate_connection_ids_are_rejected() {
    let source = format!(
        r#"{MINIMAL}
[network]
wifi_country = "JP"
[[network.ethernet]]
id = "same"
[[network.wifi]]
id = "same"
ssid = "MySSID"
psk = {{ env = "WIFI_PSK" }}
"#
    );
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("must be unique"), "{}", err.message);
}

#[test]
fn parses_hardware_settings() {
    let source = format!(
        r#"{MINIMAL}
[hardware.uart]
enabled = true
console = true
baudrate = 115200

[hardware.i2c]
enabled = true
baudrate = 400000

[hardware.spi]
enabled = true

[hardware]
pcie_gen = 3
overlays = ["disable-bt"]
"#
    );
    let loaded = load(&source).unwrap();
    let hw = &loaded.spec.hardware;
    assert!(hw.uart.enabled && hw.uart.console);
    assert_eq!(hw.i2c.baudrate, 400_000);
    assert!(hw.spi.enabled);
    assert_eq!(hw.pcie_gen, Some(3));
    assert_eq!(hw.overlays, vec!["disable-bt"]);
}

#[test]
fn uart_console_requires_uart() {
    let source = format!("{MINIMAL}\n[hardware.uart]\nconsole = true\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("requires `enabled`"), "{}", err.message);
}

#[test]
fn rejects_out_of_range_values() {
    let source = format!("{MINIMAL}\n[hardware.i2c]\nenabled = true\nbaudrate = 5\n");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("must be between"), "{}", err.message);
}

#[test]
fn rejects_unsupported_target() {
    let source = MINIMAL.replace("schema_version = 1", "schema_version = 1\ntarget = \"pi4\"");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("must be one of pi5"), "{}", err.message);
}

#[test]
fn rejects_future_schema_version() {
    let source = MINIMAL.replace("schema_version = 1", "schema_version = 2");
    let err = load(&source).unwrap_err();
    assert!(err.message.contains("schema version 1"), "{}", err.message);
}

#[test]
fn rejects_invalid_hostname() {
    for bad in ["-leading", "trailing-", "under_score", "", &"x".repeat(64)] {
        let source = MINIMAL.replace("dev-pi-01", bad);
        assert!(load(&source).is_err(), "hostname `{bad}` should be rejected");
    }
}

#[test]
fn gadget_and_debug_uart_warns_about_power() {
    let source = format!(
        "{MINIMAL}\n[network.usb_gadget]\nenabled = true\n[hardware.uart]\ndebug_connector = true\n"
    );
    let loaded = load(&source).unwrap();
    assert!(loaded.warnings.iter().any(|w| w.contains("GPIO header")), "{:?}", loaded.warnings);
}
