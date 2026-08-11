//! Behavioural tests for the shell that `rpi-provision` generates.
//!
//! Raspberry Pi OS uses dash as `/bin/sh`, which is also what this container
//! provides, so the generated scripts are checked with the same shell that
//! will run them on the device.

use std::path::{Path, PathBuf};
use std::process::Command;

use rpi_provision_apply::{execute, RealBootFs};
use rpi_provision_render::render;
use rpi_provision_spec::{load_str, LoadOptions, MapSecrets};

const SPEC: &str = r#"
[meta]
schema_version = 1

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

[network]
wifi_country = "JP"

[[network.ethernet]]
id = "eth0-static"
method = "manual"
address = "192.168.1.50/24"
gateway = "192.168.1.1"
dns = ["192.168.1.1"]

[[network.wifi]]
id = "home"
ssid = "MySSID"
psk = { env = "WIFI_PSK" }

[network.usb_gadget]
enabled = true

[hardware.uart]
enabled = true

[hardware.i2c]
enabled = true

[hardware.spi]
enabled = true
"#;

fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("rpi-provision-shell-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Render the specification into a directory and return it.
fn rendered(name: &str) -> PathBuf {
    let provider = MapSecrets::default()
        .with_env("RPI_PASSWORD_HASH", "$6$rounds=4096$abcdefgh$0123456789abcdef")
        .with_env("WIFI_PSK", "correct-horse-battery");
    let loaded = load_str(SPEC, &LoadOptions::new(&provider)).expect("specification must load");
    let plan = render(&loaded.spec, &loaded.digest);

    let root = scratch(name);
    let mut fs = RealBootFs::new(&root);
    execute(&plan, &mut fs).expect("plan must apply");
    root
}

fn shell_scripts(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "sh")
                || path.file_name().is_some_and(|name| name == "rpi-provision-gadget")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    assert!(!found.is_empty(), "no generated shell scripts were found");
    found
}

// Unix only: on the Windows runner `bash` resolves to the WSL stub, which
// exits non-zero without a distribution installed and so rejects every script
// it is handed. The Linux job is what actually vets the generated shell; the
// Windows job still checks the scripts as text, line endings included.
#[cfg(unix)]
#[test]
fn every_generated_script_parses_under_dash_and_bash() {
    let root = rendered("syntax");
    let scripts = shell_scripts(&root);
    assert!(scripts.len() >= 8, "expected the runner plus its steps, found {}", scripts.len());

    for script in &scripts {
        for shell in ["dash", "bash"] {
            let output = Command::new(shell)
                .arg("-n")
                .arg(script)
                .output()
                .unwrap_or_else(|err| panic!("cannot run {shell}: {err}"));
            assert!(
                output.status.success(),
                "{shell} rejected {}:\n{}",
                script.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn every_generated_script_is_a_strict_posix_script() {
    let root = rendered("strict");
    for script in shell_scripts(&root) {
        let body = std::fs::read_to_string(&script).unwrap();
        assert!(
            body.starts_with("#!/bin/sh\n"),
            "{} must be a POSIX shell script",
            script.display()
        );
        assert!(
            body.contains("\nset -eu\n"),
            "{} must fail on errors and unset variables",
            script.display()
        );
        assert!(
            !body.contains("\r"),
            "{} must use Unix line endings even when generated on Windows",
            script.display()
        );
        assert!(body.ends_with('\n'), "{} must end with a newline", script.display());
    }
    std::fs::remove_dir_all(&root).unwrap();
}

// Unix only: the step is asserted on by the permission bits it leaves behind,
// which have no meaning on Windows.
#[cfg(unix)]
#[test]
fn the_payload_step_installs_files_with_the_declared_modes() {
    let root = rendered("payload");
    let base = root.join("rpi-provision");
    let destination_root = scratch("payload-dest");

    // Rewrite the manifest so that its absolute destinations land in a
    // scratch directory instead of the real root filesystem.
    let manifest = std::fs::read_to_string(base.join("manifest.tsv")).unwrap();
    let redirected: String = manifest
        .lines()
        .map(|line| {
            if line.starts_with('#') || line.trim().is_empty() {
                return format!("{line}\n");
            }
            let fields: Vec<&str> = line.split('\t').collect();
            format!("{}\t{}\t{}{}\n", fields[0], fields[1], destination_root.display(), fields[2])
        })
        .collect();
    std::fs::write(base.join("manifest.tsv"), &redirected).unwrap();

    let output = Command::new("dash")
        .arg(base.join("steps/30-payload.sh"))
        .env("RPI_PROVISION_BASE", &base)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("the payload step must run");
    assert!(
        output.status.success(),
        "the payload step failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let installed =
        destination_root.join("etc/NetworkManager/system-connections/home.nmconnection");
    assert!(installed.exists(), "the wireless profile was not installed");

    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "NetworkManager refuses world-readable profiles");

        let script = destination_root.join("usr/local/sbin/rpi-provision-gadget");
        let mode = std::fs::metadata(&script).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    // The step reports each installation, which is what ends up in the log.
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(log.contains("installed"), "{log}");

    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&destination_root).unwrap();
}

#[test]
fn the_payload_step_stops_on_a_missing_source_file() {
    let root = rendered("payload-missing");
    let base = root.join("rpi-provision");
    let destination = scratch("payload-missing-dest");

    std::fs::write(
        base.join("manifest.tsv"),
        format!("0644\tpayload/does/not/exist\t{}/target\n", destination.display()),
    )
    .unwrap();

    let output = Command::new("dash")
        .arg(base.join("steps/30-payload.sh"))
        .env("RPI_PROVISION_BASE", &base)
        .output()
        .unwrap();
    assert!(!output.status.success(), "a missing payload file must abort the step");

    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&destination).unwrap();
}

/// Extract a shell function from a generated script so it can be exercised
/// on its own.
fn extract_function(script: &str, name: &str) -> String {
    let start = script
        .find(&format!("{name}() {{"))
        .unwrap_or_else(|| panic!("`{name}` is not defined in the runner"));
    let rest = &script[start..];
    let end = rest.find("\n}\n").expect("unterminated function") + 3;
    rest[..end].to_string()
}

#[test]
fn the_runner_removes_its_own_hooks_from_the_command_line() {
    let root = rendered("cleanup");
    let runner = std::fs::read_to_string(root.join("rpi-provision/firstrun.sh")).unwrap();
    let function = extract_function(&runner, "cleanup_cmdline");

    let boot = scratch("cleanup-boot");
    let stock = "console=tty1 root=PARTUUID=1c8a4d3f-02 rootfstype=ext4 fsck.repair=yes \
rootwait quiet init=/usr/lib/raspberrypi-sys-mods/firstboot \
systemd.run=/boot/firmware/rpi-provision/firstrun.sh \
systemd.run_success_action=reboot systemd.unit=kernel-command-line.target\n";
    std::fs::write(boot.join("cmdline.txt"), stock).unwrap();

    let program =
        format!("set -eu\nBOOT_MOUNT='{}'\n{function}\ncleanup_cmdline\n", boot.display());
    let output = Command::new("dash").arg("-c").arg(&program).output().unwrap();
    assert!(output.status.success(), "cleanup failed: {}", String::from_utf8_lossy(&output.stderr));

    let cleaned = std::fs::read_to_string(boot.join("cmdline.txt")).unwrap();
    assert!(!cleaned.contains("systemd.run"), "{cleaned}");
    assert!(!cleaned.contains("systemd.unit"), "{cleaned}");
    assert!(cleaned.contains("root=PARTUUID=1c8a4d3f-02"), "{cleaned}");
    assert!(cleaned.contains("init=/usr/lib/raspberrypi-sys-mods/firstboot"), "{cleaned}");
    assert!(cleaned.contains("console=tty1"), "{cleaned}");
    assert_eq!(cleaned.lines().count(), 1, "cmdline.txt must stay a single line");
    assert!(!cleaned.contains("  "), "no double spaces may be left behind: {cleaned:?}");

    // Running it again must be harmless.
    let output = Command::new("dash").arg("-c").arg(&program).output().unwrap();
    assert!(output.status.success());
    assert_eq!(std::fs::read_to_string(boot.join("cmdline.txt")).unwrap(), cleaned);

    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&boot).unwrap();
}

#[test]
fn the_runner_re_executes_from_the_staging_directory() {
    let root = rendered("stage");
    let runner = std::fs::read_to_string(root.join("rpi-provision/firstrun.sh")).unwrap();

    // The staging guard must come before anything that could be destroyed by
    // wiping the boot partition.
    let guard = runner.find("RPI_PROVISION_STAGED:-0").expect("staging guard");
    let wipe = runner.find("rm -rf \"$BOOT_BASE\"").expect("payload wipe");
    assert!(guard < wipe, "the runner must stage itself before it wipes the payload");
    assert!(runner.contains("exec /bin/sh \"$STAGE_DIR/firstrun.sh\""));
    assert!(
        runner.contains("chmod 0700 \"$STAGE_DIR\""),
        "staged secrets must not be world-readable"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn secrets_never_reach_a_world_readable_generated_file() {
    let root = rendered("secrets");
    let mut offenders = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else { continue };
            if body.contains("correct-horse-battery") || body.contains("$6$rounds=4096") {
                offenders.push(path.strip_prefix(&root).unwrap().to_path_buf());
            }
        }
    }
    offenders.sort();
    // Exactly two files may hold secret material: the NetworkManager profile
    // that needs the pre-shared key, and the staged password hash.
    assert_eq!(
        offenders,
        vec![
            PathBuf::from(
                "rpi-provision/payload/etc/NetworkManager/system-connections/home.nmconnection"
            ),
            PathBuf::from("rpi-provision/secrets/password.hash"),
        ],
        "unexpected files contain secret material"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
