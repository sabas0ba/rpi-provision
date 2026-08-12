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

    let runner = changes.iter().find(|change| change.path == "rpi-provision/firstrun.sh").unwrap();
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
    let profile = changes.iter().find(|change| change.path.ends_with("home.nmconnection")).unwrap();
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

// --------------------------------------------------------------- conflicts

#[test]
fn a_clean_card_has_no_conflicting_first_boot_files() {
    assert!(conflicting_first_boot_files(&card()).is_empty());
}

#[test]
fn an_imager_customisation_is_reported() {
    let mut fs = card();
    fs.put("custom.toml", "[user]\n");
    fs.put("userconf.txt", "pi:$6$x\n");
    let found: Vec<&str> =
        conflicting_first_boot_files(&fs).into_iter().map(|(name, _)| name).collect();
    assert_eq!(found, vec!["custom.toml", "userconf.txt"]);
}

#[test]
fn our_own_runner_is_not_mistaken_for_a_foreign_one() {
    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();
    assert!(
        conflicting_first_boot_files(&fs).is_empty(),
        "rpi-provision/firstrun.sh lives below the runner directory, not at the root"
    );
}

// ---------------------------------------------------------------- snapshots

use crate::backup::{self, Manifest, MANIFEST_NAME};

/// Take a snapshot of `source` into a fresh in-memory directory.
fn snapshot_of(source: &dyn BootFs) -> (MemBootFs, Manifest) {
    let mut destination = MemBootFs::new();
    let manifest = backup::create(
        source,
        &mut destination,
        "rpi-provision 0.0.0-test",
        "2026-08-12T00:00:00Z",
    )
    .unwrap();
    (destination, manifest)
}

#[test]
fn a_snapshot_records_every_file() {
    let card = card();
    let (stored, manifest) = snapshot_of(&card);

    assert_eq!(manifest.entries.len(), card.paths().len());
    assert_eq!(manifest.total_bytes(), card.files.values().map(|v| v.len() as u64).sum::<u64>());
    for path in card.paths() {
        assert_eq!(stored.files.get(path), card.files.get(path), "{path} was not copied verbatim");
    }
    // The manifest itself is in the snapshot but not one of its entries.
    assert!(stored.exists(MANIFEST_NAME));
    assert!(!manifest.entries.iter().any(|entry| entry.path == MANIFEST_NAME));
}

#[test]
fn a_snapshot_survives_a_round_trip_through_its_manifest() {
    let (_, manifest) = snapshot_of(&card());
    let reparsed = Manifest::parse(&manifest.render()).unwrap();
    assert_eq!(reparsed, manifest);
}

#[test]
fn restoring_undoes_an_apply() {
    let original = card();
    let (stored, manifest) = snapshot_of(&original);

    let mut fs = card();
    execute(&plan(), &mut fs).unwrap();
    assert_ne!(fs.files, original.files, "the apply must have changed something");

    let changes = backup::restore_changes(&stored, &manifest, &fs).unwrap();
    backup::restore(&stored, &changes, &mut fs).unwrap();
    assert_eq!(fs.files, original.files, "the card must be byte-for-byte as it was");
}

#[test]
fn restoring_removes_files_that_postdate_the_snapshot() {
    let (stored, manifest) = snapshot_of(&card());
    let mut fs = card();
    fs.put("rpi-provision/firstrun.sh", "#!/bin/sh\n");
    fs.put("stray.txt", "written later\n");

    let changes = backup::restore_changes(&stored, &manifest, &fs).unwrap();
    let deleted: Vec<&str> = changes
        .iter()
        .filter(|change| change.kind == ChangeKind::Delete)
        .map(|change| change.path.as_str())
        .collect();
    assert_eq!(deleted, vec!["rpi-provision/firstrun.sh", "stray.txt"]);

    let summary = backup::restore(&stored, &changes, &mut fs).unwrap();
    assert_eq!(summary.deleted, 2);
    assert!(!fs.exists("stray.txt"));
}

#[test]
fn restoring_an_untouched_card_changes_nothing() {
    let (stored, manifest) = snapshot_of(&card());
    let fs = card();
    let changes = backup::restore_changes(&stored, &manifest, &fs).unwrap();
    assert!(changes.iter().all(|change| change.kind == ChangeKind::Unchanged), "{changes:?}");
}

#[test]
fn a_snapshot_refuses_a_destination_that_is_not_empty() {
    let mut destination = MemBootFs::new();
    destination.put("something.txt", "already here\n");
    let error = backup::create(&card(), &mut destination, "g", "t").unwrap_err();
    assert!(error.message.contains("is not empty"), "{}", error.message);
}

#[test]
fn a_snapshot_refuses_a_card_that_would_collide_with_the_manifest() {
    let mut card = card();
    card.put(MANIFEST_NAME, "not ours\n");
    let error = backup::create(&card, &mut MemBootFs::new(), "g", "t").unwrap_err();
    assert!(error.message.contains("already contains"), "{}", error.message);
}

#[test]
fn an_incomplete_snapshot_is_refused() {
    // A snapshot writes its manifest last, so this is what an interrupted
    // run leaves behind.
    let mut stored = MemBootFs::new();
    stored.put("config.txt", STOCK_CONFIG);
    let error = backup::read_manifest(&stored).unwrap_err();
    assert!(error.message.contains("not a complete snapshot"), "{}", error.message);
}

#[test]
fn a_damaged_snapshot_is_refused_before_anything_is_written() {
    let (mut stored, manifest) = snapshot_of(&card());
    // Same length, different content: the kind of damage a size check alone
    // would wave through.
    let mut damaged = stored.files["config.txt"].clone();
    damaged[0] ^= 0xff;
    stored.files.insert("config.txt".to_string(), damaged);

    let error = backup::restore_changes(&stored, &manifest, &card()).unwrap_err();
    assert!(error.message.contains("does not match its digest"), "{}", error.message);
}

#[test]
fn a_truncated_file_is_refused() {
    let (mut stored, mut manifest) = snapshot_of(&card());
    stored.put("config.txt", STOCK_CONFIG);
    // Claim a size the file does not have.
    for entry in &mut manifest.entries {
        if entry.path == "config.txt" {
            entry.bytes += 1;
        }
    }
    let error = backup::restore_changes(&stored, &manifest, &card()).unwrap_err();
    assert!(error.message.contains("the snapshot is damaged"), "{}", error.message);
}

#[test]
fn a_snapshot_of_a_snapshot_is_identical() {
    // Snapshots are plain directories, so taking one of a restored card must
    // produce the same manifest entries.
    let (first, manifest) = snapshot_of(&card());
    let mut card = card();
    let changes = backup::restore_changes(&first, &manifest, &card).unwrap();
    backup::restore(&first, &changes, &mut card).unwrap();
    let (_, again) = snapshot_of(&card);
    assert_eq!(again.entries, manifest.entries);
}
