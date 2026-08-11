use rpi_provision_render::{render, Plan};
use rpi_provision_spec::{load_str, LoadOptions, MapSecrets};

use super::*;

const SPEC: &str = r#"
[meta]
schema_version = 1

[system]
hostname = "dev-pi-01"

[user]
name = "engineer"
authorized_keys = ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host"]

[network]
wifi_country = "JP"

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
"#;

const STOCK_CONFIG: &str = "# For more options and information see\n\
# http://rptl.io/configtxt\n\
dtparam=audio=on\n\
camera_auto_detect=1\n\
display_auto_detect=1\n\
auto_initramfs=1\n\
arm_64bit=1\n";

const STOCK_CMDLINE: &str = "console=serial0,115200 console=tty1 root=PARTUUID=1c8a4d3f-02 \
rootfstype=ext4 fsck.repair=yes rootwait quiet init=/usr/lib/raspberrypi-sys-mods/firstboot\n";

fn plan() -> Plan {
    let provider = MapSecrets::default().with_env("WIFI_PSK", "correct-horse-battery");
    let loaded = load_str(SPEC, &LoadOptions::new(&provider)).unwrap();
    render(&loaded.spec, &loaded.digest)
}

fn card() -> MemBootFs {
    MemBootFs::raspberry_pi_os(STOCK_CONFIG, STOCK_CMDLINE)
}

// ------------------------------------------------------------ verification

#[test]
fn accepts_a_raspberry_pi_5_boot_partition() {
    verify_boot_partition(&card(), "bcm2712-rpi-5-b.dtb").unwrap();
}

#[test]
fn rejects_an_unrelated_directory() {
    let mut fs = MemBootFs::new();
    fs.put("notes.txt", "hello");
    let err = verify_boot_partition(&fs, "bcm2712-rpi-5-b.dtb").unwrap_err();
    assert!(err.message.contains("does not look like"), "{}", err.message);
    assert!(err.message.contains("config.txt"), "{}", err.message);
}

#[test]
fn rejects_a_boot_partition_for_another_model() {
    let mut fs = MemBootFs::new();
    fs.put("config.txt", STOCK_CONFIG);
    fs.put("cmdline.txt", STOCK_CMDLINE);
    fs.put("bcm2711-rpi-4-b.dtb", "\0");
    let err = verify_boot_partition(&fs, "bcm2712-rpi-5-b.dtb").unwrap_err();
    assert!(err.message.contains("not a Raspberry Pi 5"), "{}", err.message);
}

// ------------------------------------------------------------------- apply

#[test]
fn applies_cleanly_to_a_fresh_card() {
    let mut fs = card();
    let summary = execute(&plan(), &mut fs).unwrap();
    assert_eq!(summary.updated, 2, "config.txt and cmdline.txt");
    assert!(summary.created > 5);
    assert_eq!(summary.deleted, 0);

    let config = fs.text("config.txt").unwrap();
    assert!(config.starts_with(STOCK_CONFIG), "the stock content must be preserved");
    assert!(config.contains("dtoverlay=dwc2,dr_mode=peripheral"));
    assert!(config.contains("dtparam=uart0=on"));

    let cmdline = fs.text("cmdline.txt").unwrap();
    assert_eq!(cmdline.lines().count(), 1);
    assert!(cmdline.contains("root=PARTUUID=1c8a4d3f-02"));
    assert!(cmdline.contains("systemd.run=/boot/firmware/rpi-provision/firstrun.sh"));
    assert!(!cmdline.contains("console=serial0"));

    assert!(fs.exists("rpi-provision/firstrun.sh"));
    assert!(fs.exists("rpi-provision/manifest.tsv"));
    assert!(fs.exists("rpi-provision/steps/10-hostname.sh"));
    assert_eq!(fs.executable.get("rpi-provision/firstrun.sh"), Some(&true));
    assert_eq!(fs.executable.get("rpi-provision/manifest.tsv"), Some(&false));
}

#[test]
fn applying_twice_changes_nothing_the_second_time() {
    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();
    let after_first = fs.clone();

    let summary = execute(&plan(), &mut fs).unwrap();
    assert_eq!(summary.total_changes(), 0, "the second apply must be a no-op");
    assert_eq!(summary.unchanged, plan().actions.len());
    assert_eq!(fs.files, after_first.files, "the card must be byte-identical");
}

#[test]
fn applying_a_changed_spec_updates_in_place() {
    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();

    let provider = MapSecrets::default().with_env("WIFI_PSK", "correct-horse-battery");
    let changed = SPEC.replace("dev-pi-01", "dev-pi-02");
    let loaded = load_str(&changed, &LoadOptions::new(&provider)).unwrap();
    let summary = execute(&render(&loaded.spec, &loaded.digest), &mut fs).unwrap();

    assert!(summary.updated > 0);
    assert!(fs.text("rpi-provision/steps/10-hostname.sh").unwrap().contains("dev-pi-02"));
    // config.txt keeps exactly one managed block.
    let config = fs.text("config.txt").unwrap();
    assert_eq!(config.matches(rpi_provision_render::config_txt::BEGIN).count(), 1);
    assert!(config.starts_with(STOCK_CONFIG));
}

#[test]
fn foreign_edits_between_runs_survive() {
    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();

    // Somebody appends a line by hand after the managed block.
    let config = fs.text("config.txt").unwrap();
    fs.put("config.txt", &format!("{config}dtoverlay=vc4-kms-v3d\n"));

    execute(&plan(), &mut fs).unwrap();
    let config = fs.text("config.txt").unwrap();
    assert!(config.contains("dtoverlay=vc4-kms-v3d"));
    assert_eq!(config.matches(rpi_provision_render::config_txt::BEGIN).count(), 1);
}

#[test]
fn a_missing_config_txt_is_created_rather_than_corrupted() {
    let mut fs = MemBootFs::new();
    execute(&plan(), &mut fs).unwrap();
    let config = fs.text("config.txt").unwrap();
    assert!(config.starts_with(rpi_provision_render::config_txt::BEGIN));
}

#[test]
fn refuses_to_edit_a_broken_managed_block() {
    let mut fs = card();
    fs.put(
        "config.txt",
        &format!("{STOCK_CONFIG}{}\ndtparam=spi=on\n", rpi_provision_render::config_txt::BEGIN),
    );
    let err = execute(&plan(), &mut fs).unwrap_err();
    assert!(err.message.contains("closing"), "{}", err.message);
}

#[test]
fn refuses_a_multi_line_cmdline() {
    let mut fs = card();
    fs.put("cmdline.txt", "console=tty1\nroot=PARTUUID=x\n");
    let err = execute(&plan(), &mut fs).unwrap_err();
    assert!(err.message.contains("exactly one line"), "{}", err.message);
}

#[test]
fn a_failure_leaves_nothing_half_written() {
    let mut fs = card();
    fs.put("cmdline.txt", "a\nb\n");
    let before = fs.clone();
    assert!(execute(&plan(), &mut fs).is_err());
    assert_eq!(fs.files, before.files, "no action may run before every action resolves");
}

// -------------------------------------------------------------------- diff

#[test]
fn dry_run_reports_creations_on_a_fresh_card() {
    let fs = card();
    let changes = plan_changes(&plan(), &fs).unwrap();
    let config = changes.iter().find(|change| change.path == "config.txt").unwrap();
    assert_eq!(config.kind, ChangeKind::Update);
    assert!(config.diff.as_ref().unwrap().contains("+ dtoverlay=dwc2,dr_mode=peripheral"));

    let runner = changes
        .iter()
        .find(|change| change.path == "rpi-provision/firstrun.sh")
        .unwrap();
    assert_eq!(runner.kind, ChangeKind::Create);
}

#[test]
fn dry_run_changes_nothing() {
    let fs = card();
    let before = fs.clone();
    plan_changes(&plan(), &fs).unwrap();
    assert_eq!(fs.files, before.files);
}

#[test]
fn dry_run_after_apply_reports_no_changes() {
    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();
    let changes = plan_changes(&plan(), &fs).unwrap();
    assert!(changes.iter().all(|change| change.kind == ChangeKind::Unchanged));
}

#[test]
fn secret_content_is_withheld_from_the_diff() {
    let fs = card();
    let changes = plan_changes(&plan(), &fs).unwrap();
    let profile = changes
        .iter()
        .find(|change| change.path.ends_with("home.nmconnection"))
        .unwrap();
    assert!(profile.sensitive);
    let diff = profile.diff.as_ref().unwrap();
    assert!(diff.contains("withheld"), "{diff}");
    assert!(!diff.contains("correct-horse-battery"), "the pre-shared key leaked into the diff");
}

#[test]
fn no_diff_anywhere_contains_the_pre_shared_key() {
    let fs = card();
    for change in plan_changes(&plan(), &fs).unwrap() {
        if let Some(diff) = &change.diff {
            assert!(
                !diff.contains("correct-horse-battery"),
                "secret leaked in the diff for {}",
                change.path
            );
        }
    }
}

// ------------------------------------------------------------------ revert

#[test]
fn revert_restores_the_original_card() {
    let mut fs = card();
    let before = fs.clone();
    let plan = plan();
    execute(&plan, &mut fs).unwrap();
    assert_ne!(fs.files, before.files);

    execute(&revert_plan(&plan), &mut fs).unwrap();
    assert_eq!(
        fs.text("config.txt").unwrap(),
        STOCK_CONFIG,
        "config.txt must return to its original content"
    );
    assert!(!fs.text("cmdline.txt").unwrap().contains("systemd.run"));
    assert!(fs.paths().iter().all(|path| !path.starts_with("rpi-provision/")));
}

#[test]
fn revert_on_an_untouched_card_is_a_no_op_for_the_payload() {
    let mut fs = card();
    let summary = execute(&revert_plan(&plan()), &mut fs).unwrap();
    assert_eq!(summary.deleted, 0);
    assert_eq!(fs.text("config.txt").unwrap(), STOCK_CONFIG);
}

// --------------------------------------------------------------- real disk

#[test]
fn writes_to_a_real_directory() {
    let root = std::env::temp_dir().join(format!("rpi-provision-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.txt"), STOCK_CONFIG).unwrap();
    std::fs::write(root.join("cmdline.txt"), STOCK_CMDLINE).unwrap();
    std::fs::write(root.join("bcm2712-rpi-5-b.dtb"), [0u8; 4]).unwrap();

    let mut fs = RealBootFs::new(&root);
    verify_boot_partition(&fs, "bcm2712-rpi-5-b.dtb").unwrap();
    let plan = plan();
    execute(&plan, &mut fs).unwrap();

    let runner = root.join("rpi-provision/firstrun.sh");
    assert!(runner.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&runner).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the runner must be executable");
    }

    execute(&revert_plan(&plan), &mut fs).unwrap();
    assert!(!root.join("rpi-provision").exists(), "empty directories must be pruned");
    assert_eq!(std::fs::read_to_string(root.join("config.txt")).unwrap(), STOCK_CONFIG);

    std::fs::remove_dir_all(&root).unwrap();
}
