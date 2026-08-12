//! Snapshots against a real directory tree.
//!
//! The unit tests work on an in-memory partition, which never exercises the
//! recursive directory walk, the `/` separator on Windows, or the pruning of
//! directories left empty by a restore. This does all three.

use std::path::{Path, PathBuf};

use rpi_provision_apply::{backup, BootFs, RealBootFs};

fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("rpi-provision-snapshot-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn put(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Everything below `root`, as a sorted list of (relative path, bytes).
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let fs = RealBootFs::new(root);
    let mut found: Vec<(String, Vec<u8>)> = fs
        .list()
        .unwrap()
        .into_iter()
        .map(|path| (path.clone(), fs.read(&path).unwrap()))
        .collect();
    found.sort();
    found
}

/// A card with nested directories and a file large enough not to be a single
/// buffer's worth.
fn card(name: &str) -> PathBuf {
    let root = scratch(name);
    put(&root, "config.txt", b"dtparam=audio=on\narm_64bit=1\n");
    put(&root, "cmdline.txt", b"console=tty1 root=PARTUUID=1c8a4d3f-02 rootwait\n");
    put(&root, "bcm2712-rpi-5-b.dtb", &vec![0x7f; 4096]);
    put(&root, "overlays/dwc2.dtbo", &vec![0xd0; 1024]);
    put(&root, "overlays/nested/deep/thing.dtbo", b"deep\n");
    put(&root, "kernel_2712.img", &(0..=255u8).cycle().take(300_000).collect::<Vec<u8>>());
    root
}

#[test]
fn a_snapshot_of_a_real_directory_round_trips() {
    let source = card("source");
    let store = scratch("store");
    let before = tree(&source);
    assert_eq!(before.len(), 6, "the fixture must have nested files");

    let manifest = backup::create(
        &RealBootFs::new(&source),
        &mut RealBootFs::new(&store),
        "rpi-provision 0.0.0-test",
        "2026-08-12T00:00:00Z",
    )
    .unwrap();
    assert_eq!(manifest.entries.len(), 6);
    assert!(
        manifest.entries.iter().any(|entry| entry.path == "overlays/nested/deep/thing.dtbo"),
        "nested paths must be recorded with `/` separators: {:?}",
        manifest.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    // Now wreck the card: change a file, add one, delete another.
    std::fs::write(source.join("config.txt"), b"dtparam=audio=off\n").unwrap();
    put(&source, "rpi-provision/firstrun.sh", b"#!/bin/sh\nset -eu\n");
    std::fs::remove_file(source.join("overlays/dwc2.dtbo")).unwrap();

    let stored = RealBootFs::new(&store);
    let manifest = backup::read_manifest(&stored).unwrap();
    let mut target = RealBootFs::new(&source);
    let changes = backup::restore_changes(&stored, &manifest, &target).unwrap();
    backup::restore(&stored, &changes, &mut target).unwrap();

    assert_eq!(tree(&source), before, "the card must be byte-for-byte as it was");

    std::fs::remove_dir_all(&source).unwrap();
    std::fs::remove_dir_all(&store).unwrap();
}

#[test]
fn restoring_prunes_a_directory_it_emptied() {
    let source = card("prune");
    let store = scratch("prune-store");
    backup::create(
        &RealBootFs::new(&source),
        &mut RealBootFs::new(&store),
        "rpi-provision 0.0.0-test",
        "2026-08-12T00:00:00Z",
    )
    .unwrap();

    // A whole directory that did not exist when the snapshot was taken.
    put(&source, "rpi-provision/steps/10-hostname.sh", b"#!/bin/sh\n");
    put(&source, "rpi-provision/manifest.tsv", b"0644\ta\tb\n");
    assert!(source.join("rpi-provision/steps").is_dir());

    let stored = RealBootFs::new(&store);
    let manifest = backup::read_manifest(&stored).unwrap();
    let mut target = RealBootFs::new(&source);
    let changes = backup::restore_changes(&stored, &manifest, &target).unwrap();
    backup::restore(&stored, &changes, &mut target).unwrap();

    assert!(!source.join("rpi-provision").exists(), "an emptied directory must not be left behind");

    std::fs::remove_dir_all(&source).unwrap();
    std::fs::remove_dir_all(&store).unwrap();
}

#[test]
fn a_snapshot_will_not_overwrite_another() {
    let source = card("collide");
    let store = scratch("collide-store");
    let take = || {
        backup::create(
            &RealBootFs::new(&source),
            &mut RealBootFs::new(&store),
            "rpi-provision 0.0.0-test",
            "2026-08-12T00:00:00Z",
        )
    };
    take().unwrap();
    let error = take().unwrap_err();
    assert!(error.message.contains("is not empty"), "{}", error.message);

    std::fs::remove_dir_all(&source).unwrap();
    std::fs::remove_dir_all(&store).unwrap();
}
