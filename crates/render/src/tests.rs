use rpi_provision_spec::{load_str, LoadOptions, MapSecrets};

use super::*;

const FULL: &str = r#"
[meta]
schema_version = 1
target = "pi5"

[system]
hostname = "dev-pi-01"
timezone = "Asia/Tokyo"
locale = "en_US.UTF-8"
keymap = "jp"

[user]
name = "engineer"
password_hash = { env = "RPI_PASSWORD_HASH" }
authorized_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host"]
groups = ["gpio", "i2c", "spi", "dialout"]
sudo = "nopasswd"

[ssh]
enabled = true
password_authentication = false
permit_root_login = "no"

[network]
wifi_country = "JP"

[[network.ethernet]]
id = "eth0-static"
method = "manual"
address = "192.168.1.50/24"
gateway = "192.168.1.1"
dns = ["192.168.1.1"]
autoconnect_priority = 100

[[network.wifi]]
id = "home"
ssid = "MySSID"
psk = { env = "WIFI_PSK" }

[network.usb_gadget]
enabled = true
function = "ecm"
address = "10.55.0.1/24"
peer_address = "10.55.0.2"

[hardware.uart]
enabled = true
console = false

[hardware.i2c]
enabled = true
baudrate = 400000

[hardware.spi]
enabled = true

[hardware]
pcie_gen = 3
overlays = ["disable-bt"]
"#;

const MINIMAL: &str = r#"
[meta]
schema_version = 1

[system]
hostname = "pi"

[user]
name = "engineer"
authorized_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host"]
"#;

fn plan_of(source: &str) -> Plan {
    let provider = MapSecrets::default()
        .with_env("RPI_PASSWORD_HASH", "$6$rounds=4096$abcdefgh$0123456789abcdef")
        .with_env("WIFI_PSK", "correct-horse-battery");
    let loaded = load_str(source, &LoadOptions::new(&provider)).expect("specification must load");
    render(&loaded.spec, &loaded.digest)
}

fn written(plan: &Plan, path: &str) -> String {
    plan.actions
        .iter()
        .find_map(|action| match action {
            Action::Write { path: candidate, contents, .. } if candidate == path => {
                Some(contents.clone())
            }
            Action::WriteBytes { path: candidate, contents, .. } if candidate == path => {
                Some(String::from_utf8_lossy(contents).into_owned())
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "no write action for `{path}`; plan contains: {:?}",
                plan.actions.iter().map(Action::path).collect::<Vec<_>>()
            )
        })
}

fn block_of(plan: &Plan, path: &str) -> String {
    plan.actions
        .iter()
        .find_map(|action| match action {
            Action::MergeManagedBlock { path: candidate, block } if candidate == path => {
                Some(block.clone())
            }
            _ => None,
        })
        .expect("managed block")
}

#[test]
fn rendering_is_deterministic() {
    assert_eq!(plan_of(FULL).to_text(), plan_of(FULL).to_text());
}

#[test]
fn config_block_reflects_the_hardware_section() {
    let block = block_of(&plan_of(FULL), "config.txt");
    assert!(block.starts_with(config_txt::BEGIN));
    assert!(block.contains("[all]"));
    // UART0 is the GPIO 14/15 port on Raspberry Pi 5.
    assert!(block.contains("dtparam=uart0=on"));
    assert!(!block.contains("enable_uart=1"), "the debug connector was not requested");
    assert!(block.contains("dtparam=i2c_arm=on"));
    assert!(block.contains("dtparam=i2c_arm_baudrate=400000"));
    assert!(block.contains("dtparam=spi=on"));
    assert!(block.contains("dtparam=pciex1_gen=3"));
    assert!(block.contains("dtoverlay=dwc2,dr_mode=peripheral"));
    assert!(block.contains("dtoverlay=disable-bt"));
    assert!(block.trim_end().ends_with(config_txt::END));
}

#[test]
fn minimal_spec_emits_no_hardware_lines() {
    let block = block_of(&plan_of(MINIMAL), "config.txt");
    assert!(!block.contains("dtparam="));
    assert!(!block.contains("dtoverlay="));
}

#[test]
fn debug_connector_maps_to_enable_uart() {
    let source = format!("{MINIMAL}\n[hardware.uart]\ndebug_connector = true\n");
    let block = block_of(&plan_of(&source), "config.txt");
    assert!(block.contains("enable_uart=1"));
    assert!(!block.contains("dtparam=uart0=on"));
}

#[test]
fn cmdline_ops_install_the_runner_hook() {
    let plan = plan_of(FULL);
    let ops = plan
        .actions
        .iter()
        .find_map(|action| match action {
            Action::EditCmdline { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .expect("cmdline edit");
    assert!(ops
        .append
        .contains(&"systemd.run=/boot/firmware/rpi-provision/firstrun.sh".to_string()));
    assert!(ops.append.contains(&"systemd.run_success_action=reboot".to_string()));
    assert!(ops.remove_prefixes.iter().any(|prefix| prefix == "console=serial0"));
}

#[test]
fn gpio_console_is_ttyama0_not_serial0() {
    let source =
        format!("{MINIMAL}\n[hardware.uart]\nenabled = true\nconsole = true\nbaudrate = 115200\n");
    let plan = plan_of(&source);
    let ops = plan
        .actions
        .iter()
        .find_map(|action| match action {
            Action::EditCmdline { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .unwrap();
    assert!(ops.append.contains(&"console=ttyAMA0,115200".to_string()));
    assert!(ops.remove_prefixes.iter().any(|prefix| prefix == "console=serial0"));
}

#[test]
fn debug_connector_keeps_the_stock_console() {
    let source = format!("{MINIMAL}\n[hardware.uart]\ndebug_connector = true\n");
    let plan = plan_of(&source);
    let ops = plan
        .actions
        .iter()
        .find_map(|action| match action {
            Action::EditCmdline { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        !ops.remove_prefixes.iter().any(|prefix| prefix.starts_with("console=")),
        "the debug UART console must be left alone"
    );
}

#[test]
fn manifest_lists_every_payload_file() {
    let plan = plan_of(FULL);
    let manifest = written(&plan, "rpi-provision/manifest.tsv");
    let entries: Vec<&str> = manifest
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter(|line| !line.trim().is_empty())
        .collect();

    for entry in &entries {
        let fields: Vec<&str> = entry.split('\t').collect();
        assert_eq!(fields.len(), 5, "malformed manifest entry: {entry:?}");
        let (mode, owner, group, source, destination) =
            (fields[0], fields[1], fields[2], fields[3], fields[4]);
        assert_eq!((owner, group), ("root", "root"), "generated files are root owned");
        assert_eq!(mode.len(), 4, "mode must be four octal digits: {mode}");
        assert!(mode.bytes().all(|b| (b'0'..=b'7').contains(&b)), "mode {mode}");
        assert!(destination.starts_with('/'), "destination must be absolute: {destination}");
        assert!(source.starts_with("payload/"), "source must live under payload/: {source}");
        // Every manifest entry must have a corresponding write action.
        written(&plan, &format!("rpi-provision/{source}"));
    }

    let destinations: Vec<&str> =
        entries.iter().map(|line| line.split('\t').next_back().unwrap()).collect();
    for expected in [
        "/etc/NetworkManager/system-connections/eth0-static.nmconnection",
        "/etc/NetworkManager/system-connections/home.nmconnection",
        "/etc/NetworkManager/system-connections/usb0-gadget.nmconnection",
        "/etc/ssh/sshd_config.d/10-rpi-provision.conf",
        "/etc/sudoers.d/010-rpi-provision-engineer",
        "/etc/systemd/system/rpi-provision-gadget.service",
        "/etc/modules-load.d/rpi-provision-gadget.conf",
        "/usr/local/sbin/rpi-provision-gadget",
    ] {
        assert!(destinations.contains(&expected), "missing {expected} in {destinations:?}");
    }
}

#[test]
fn manifest_destinations_are_unique() {
    let plan = plan_of(FULL);
    let manifest = written(&plan, "rpi-provision/manifest.tsv");
    let mut destinations: Vec<&str> = manifest
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| line.split('\t').next_back().unwrap())
        .collect();
    let total = destinations.len();
    destinations.sort_unstable();
    destinations.dedup();
    assert_eq!(destinations.len(), total);
}

#[test]
fn secrets_are_marked_sensitive() {
    let plan = plan_of(FULL);
    let sensitive: Vec<&str> =
        plan.actions.iter().filter(|action| action.is_sensitive()).map(Action::path).collect();
    assert!(sensitive.contains(&"rpi-provision/secrets/password.hash"));
    assert!(sensitive.iter().any(|path| path.ends_with("home.nmconnection")));
    assert!(
        !sensitive.iter().any(|path| path.ends_with("eth0-static.nmconnection")),
        "a wired profile carries no secret"
    );
}

#[test]
fn password_hash_never_appears_in_a_step_script() {
    let plan = plan_of(FULL);
    for action in &plan.actions {
        if let Action::Write { path, contents, .. } = action {
            if path.contains("/steps/") || path.ends_with("firstrun.sh") {
                assert!(!contents.contains("$6$rounds=4096"), "{path} embeds the password hash");
            }
        }
    }
}

#[test]
fn steps_are_numbered_in_dependency_order() {
    let plan = plan_of(FULL);
    let mut steps: Vec<&str> =
        plan.actions.iter().map(Action::path).filter(|path| path.contains("/steps/")).collect();
    steps.sort_unstable();
    let names: Vec<&str> = steps.iter().map(|path| path.rsplit('/').next().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "10-hostname.sh",
            "20-user.sh",
            "30-payload.sh",
            "40-ssh.sh",
            "50-network.sh",
            "60-usb-gadget.sh",
            "70-locale.sh",
        ]
    );
}

#[test]
fn minimal_spec_omits_optional_steps() {
    let plan = plan_of(MINIMAL);
    let names: Vec<String> = plan
        .actions
        .iter()
        .map(Action::path)
        .filter(|path| path.contains("/steps/"))
        .map(|path| path.rsplit('/').next().unwrap().to_string())
        .collect();
    assert!(!names.iter().any(|name| name.contains("network")));
    assert!(!names.iter().any(|name| name.contains("gadget")));
    assert!(!names.iter().any(|name| name.contains("locale")));
}

#[test]
fn runner_stages_into_tmpfs_and_cleans_the_command_line() {
    let firstrun = written(&plan_of(FULL), "rpi-provision/firstrun.sh");
    assert!(firstrun.starts_with("#!/bin/sh\n"));
    assert!(firstrun.contains("set -eu"));
    assert!(firstrun.contains("exec /bin/sh \"$STAGE_DIR/firstrun.sh\""));
    assert!(firstrun.contains("cleanup_cmdline"));
    assert!(firstrun.contains("rm -rf \"$BOOT_BASE\""), "the payload must be wiped by default");
    assert!(firstrun.trim_end().ends_with("exit 0"));
}

#[test]
fn wipe_can_be_disabled() {
    let source = format!("{MINIMAL}\n[provisioning]\nwipe_payload = false\n");
    let firstrun = written(&plan_of(&source), "rpi-provision/firstrun.sh");
    assert!(firstrun.contains("WIPE_PAYLOAD=0"));
}

#[test]
fn runner_path_follows_the_configured_directory() {
    let source = format!("{MINIMAL}\n[provisioning]\nrunner_dir = \"provisioning\"\n");
    let plan = plan_of(&source);
    written(&plan, "provisioning/firstrun.sh");
    let ops = plan
        .actions
        .iter()
        .find_map(|action| match action {
            Action::EditCmdline { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .unwrap();
    assert!(ops
        .append
        .contains(&"systemd.run=/boot/firmware/provisioning/firstrun.sh".to_string()));
}

#[test]
fn sshd_config_disables_passwords_by_default() {
    let config = written(
        &plan_of(FULL),
        "rpi-provision/payload/etc/ssh/sshd_config.d/10-rpi-provision.conf",
    );
    assert!(config.contains("PasswordAuthentication no"));
    assert!(config.contains("KbdInteractiveAuthentication no"));
    assert!(config.contains("PermitRootLogin no"));
    assert!(config.contains("Port 22"));
}

#[test]
fn authorized_keys_are_staged_next_to_the_runner() {
    let keys = written(&plan_of(FULL), "rpi-provision/authorized_keys");
    assert!(keys.contains("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5"));
    // Not part of the manifest: the SSH step installs it with user ownership.
    let manifest = written(&plan_of(FULL), "rpi-provision/manifest.tsv");
    assert!(!manifest.contains("authorized_keys"));
}

#[test]
fn user_step_adds_sudo_group_for_nopasswd() {
    let step = written(&plan_of(FULL), "rpi-provision/steps/20-user.sh");
    assert!(step.contains("'sudo'"));
    assert!(step.contains("'gpio'"));
    assert!(step.contains("chpasswd --encrypted"));
    assert!(step.contains("$BASE/secrets/password.hash"));
}

#[test]
fn user_step_locks_the_account_without_a_password() {
    let step = written(&plan_of(MINIMAL), "rpi-provision/steps/20-user.sh");
    assert!(step.contains("passwd --lock"));
    assert!(!step.contains("chpasswd"));
}

#[test]
fn hostname_is_shell_quoted() {
    let step = written(&plan_of(FULL), "rpi-provision/steps/10-hostname.sh");
    assert!(step.contains("HOSTNAME='dev-pi-01'"));
    assert!(step.contains("/etc/hostname"));
    assert!(step.contains("127.0.1.1"));
}

#[test]
fn plan_orders_boot_files_first() {
    let plan = plan_of(FULL);
    assert_eq!(plan.actions[0].path(), "config.txt");
    assert_eq!(plan.actions[1].path(), "cmdline.txt");
}

#[test]
fn plan_records_the_expected_device_tree_blob() {
    assert_eq!(plan_of(MINIMAL).target_dtb, "bcm2712-rpi-5-b.dtb");
}

#[test]
fn every_action_path_is_relative_and_normalised() {
    for action in &plan_of(FULL).actions {
        let path = action.path();
        assert!(!path.starts_with('/'), "{path} must be relative to the boot partition");
        assert!(!path.contains(".."), "{path} must not escape the boot partition");
        assert!(!path.contains('\\'), "{path} must use forward slashes");
    }
}

// ------------------------------------------------------- declared transfers

/// `spec::reserved_destination` exists so that validation can reject a
/// `[[files]]` entry that would fight with a generated one. It is a hand
/// written list, so this keeps it honest.
#[test]
fn every_generated_destination_is_reserved() {
    let plan = plan_of(FULL);
    let manifest = written(&plan, "rpi-provision/manifest.tsv");
    let mut checked = 0;
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let destination = line.split('\t').next_back().expect("a destination column");
        assert!(
            rpi_provision_spec::model::reserved_destination(destination, "engineer").is_some(),
            "{destination} is generated but not reserved; add it to reserved_destination"
        );
        checked += 1;
    }
    assert!(checked >= 4, "the fixture must generate several payload files, saw {checked}");
}

#[test]
fn a_declared_file_reaches_the_manifest_with_its_mode_and_owner() {
    let provider = MapSecrets::default().with_file("./files/motd", "welcome\n");
    let source = format!(
        "{MINIMAL}\n[[files]]\nsource = \"files/motd\"\ndestination = \"/etc/motd\"\n\
         mode = \"0640\"\nowner = \"engineer\"\ngroup = \"engineer\"\n"
    );
    let loaded = load_str(&source, &LoadOptions::new(&provider)).unwrap();
    let plan = render(&loaded.spec, &loaded.digest);

    let manifest = written(&plan, "rpi-provision/manifest.tsv");
    let entry = manifest
        .lines()
        .find(|line| line.ends_with("/etc/motd"))
        .unwrap_or_else(|| panic!("no entry for /etc/motd in {manifest}"));
    let fields: Vec<&str> = entry.split('\t').collect();
    assert_eq!(fields[0], "0640");
    assert_eq!((fields[1], fields[2]), ("engineer", "engineer"));
    // Staged under files/ so a declared name can never collide with a
    // generated one.
    assert!(fields[3].starts_with("payload/files/"), "{}", fields[3]);
    assert_eq!(written(&plan, &format!("rpi-provision/{}", fields[3])), "welcome\n");
}

#[test]
fn a_declared_file_may_be_binary() {
    let provider = MapSecrets::default().with_file("./files/blob", "\u{0}\u{1}\u{2}");
    let source =
        format!("{MINIMAL}\n[[files]]\nsource = \"files/blob\"\ndestination = \"/opt/blob\"\n");
    let loaded = load_str(&source, &LoadOptions::new(&provider)).unwrap();
    let plan = render(&loaded.spec, &loaded.digest);
    let staged = plan
        .actions
        .iter()
        .any(|action| matches!(action, Action::WriteBytes { path, .. } if path.contains("blob")));
    assert!(staged, "a declared file must be staged as bytes, not text");
}

#[test]
fn run_commands_become_the_last_step() {
    let source = format!(
        "{MINIMAL}\n[[run]]\ndescription = \"say hello\"\ncommand = \"echo hello\"\n\
         \n[[run]]\ncommand = \"apt-get update\"\nignore_failure = true\n"
    );
    let plan = plan_of(&source);
    let step = written(&plan, "rpi-provision/steps/80-run.sh");

    // The description is a value and is quoted; the command is code and is not.
    assert!(step.contains("printf 'run: %s\\n' 'say hello'"), "{step}");
    assert!(step.contains("\necho hello\n"), "{step}");
    // A tolerated failure is reported rather than aborting the step.
    assert!(step.contains("if ! apt-get update; then"), "{step}");
    assert!(step.starts_with("#!/bin/sh\n"));
    assert!(step.contains("\nset -eu\n"));

    // 80 puts it after every step that configures the machine.
    let steps: Vec<&str> = plan
        .actions
        .iter()
        .map(Action::path)
        .filter(|path| path.starts_with("rpi-provision/steps/"))
        .collect();
    assert_eq!(steps.last().copied(), Some("rpi-provision/steps/80-run.sh"), "{steps:?}");
}

#[test]
fn no_run_commands_means_no_run_step() {
    let plan = plan_of(MINIMAL);
    assert!(
        !plan.actions.iter().any(|action| action.path().ends_with("80-run.sh")),
        "an empty `run` must not produce a step"
    );
}
