//! Whole-partition snapshots.
//!
//! `revert` undoes the changes this tool made, which is enough when the tool
//! is the only thing that touched the card. A snapshot is the answer to the
//! other case: something else went wrong, or the card was already carrying
//! customisation nobody wrote down, and the alternative is writing the image
//! again from scratch.
//!
//! A snapshot is an ordinary directory holding a byte-for-byte copy of every
//! file on the partition, plus a manifest. Nothing is compressed and nothing
//! is packed into an archive format, so any file can be recovered with `cp`
//! and inspected without this tool.
//!
//! The manifest is written last and is what marks a snapshot complete: an
//! interrupted `backup` leaves a directory that `restore` refuses to use,
//! rather than one that silently restores half a card.

use rpi_provision_spec::sha256::sha256_hex;

use crate::{BootFs, Change, ChangeKind, Error, Result, Summary};

/// Written at the root of a snapshot, once every file is in place.
pub const MANIFEST_NAME: &str = "rpi-provision-backup.tsv";

/// One file in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub digest: String,
    pub bytes: u64,
    pub path: String,
}

/// What a snapshot contains, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub generator: String,
    pub source: String,
    pub created: String,
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.bytes).sum()
    }

    /// The manifest as it is stored: `#`-prefixed headers, then one
    /// tab-separated row per file, in the same shape as the payload manifest
    /// the first-boot runner reads.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# rpi-provision snapshot. Do not edit.\n");
        out.push_str(&format!("# generator: {}\n", self.generator));
        out.push_str(&format!("# source: {}\n", self.source));
        out.push_str(&format!("# created: {}\n", self.created));
        out.push_str(&format!("# files: {}\n", self.entries.len()));
        out.push_str(&format!("# bytes: {}\n", self.total_bytes()));
        out.push_str("#\n# sha256\tbytes\tpath\n");
        for entry in &self.entries {
            out.push_str(&format!("{}\t{}\t{}\n", entry.digest, entry.bytes, entry.path));
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut manifest = Manifest {
            generator: String::new(),
            source: String::new(),
            created: String::new(),
            entries: Vec::new(),
        };
        for (number, line) in text.lines().enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(header) = line.strip_prefix("# ") {
                if let Some((key, value)) = header.split_once(": ") {
                    match key {
                        "generator" => manifest.generator = value.to_string(),
                        "source" => manifest.source = value.to_string(),
                        "created" => manifest.created = value.to_string(),
                        _ => {}
                    }
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [digest, bytes, path] = fields.as_slice() else {
                return Err(Error::new(format!(
                    "{MANIFEST_NAME} line {}: expected three tab-separated fields, got {}",
                    number + 1,
                    fields.len()
                )));
            };
            let bytes: u64 = bytes.parse().map_err(|_| {
                Error::new(format!(
                    "{MANIFEST_NAME} line {}: `{bytes}` is not a byte count",
                    number + 1
                ))
            })?;
            manifest.entries.push(Entry {
                digest: (*digest).to_string(),
                bytes,
                path: (*path).to_string(),
            });
        }
        if manifest.entries.is_empty() {
            return Err(Error::new(format!("{MANIFEST_NAME} lists no files")));
        }
        Ok(manifest)
    }
}

/// A path that cannot be represented in the manifest would come back wrong.
fn check_storable(path: &str) -> Result<()> {
    if path.contains('\t') || path.contains('\n') || path.contains('\r') {
        return Err(Error::new(format!(
            "`{path}` contains a tab or a newline, which the snapshot manifest cannot record. \
             Rename it before taking a snapshot."
        )));
    }
    Ok(())
}

/// Copy every file from `source` into `destination` and record what was
/// copied.
///
/// The destination must be empty: overwriting one snapshot with another
/// would leave files from both, and a snapshot that is a mixture of two
/// cards is worse than no snapshot at all.
pub fn create(
    source: &dyn BootFs,
    destination: &mut dyn BootFs,
    generator: &str,
    created: &str,
) -> Result<Manifest> {
    let existing = destination
        .list()
        .map_err(|err| Error::new(format!("cannot read {}: {err}", destination.describe())))?;
    if !existing.is_empty() {
        return Err(Error::new(format!(
            "{} is not empty ({} file(s) already there); point --out at a new directory",
            destination.describe(),
            existing.len()
        )));
    }

    let paths = source
        .list()
        .map_err(|err| Error::new(format!("cannot read {}: {err}", source.describe())))?;
    if paths.is_empty() {
        return Err(Error::new(format!("{} holds no files", source.describe())));
    }
    if paths.iter().any(|path| path == MANIFEST_NAME) {
        return Err(Error::new(format!(
            "{} already contains a file called `{MANIFEST_NAME}`, which is the name a snapshot \
             manifest uses. Rename it before taking a snapshot.",
            source.describe()
        )));
    }

    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        check_storable(&path)?;
        // One file at a time: a boot partition is far larger than anything
        // this tool would otherwise hold in memory.
        let contents =
            source.read(&path).map_err(|err| Error::new(format!("cannot read `{path}`: {err}")))?;
        destination
            .write(&path, &contents, false)
            .map_err(|err| Error::new(format!("cannot write `{path}`: {err}")))?;
        entries.push(Entry { digest: sha256_hex(&contents), bytes: contents.len() as u64, path });
    }

    let manifest = Manifest {
        generator: generator.to_string(),
        source: source.describe(),
        created: created.to_string(),
        entries,
    };
    // Last, so that an interrupted run cannot be mistaken for a usable one.
    destination
        .write(MANIFEST_NAME, manifest.render().as_bytes(), false)
        .map_err(|err| Error::new(format!("cannot write `{MANIFEST_NAME}`: {err}")))?;
    Ok(manifest)
}

/// Read the manifest of a snapshot, failing if there is not one.
pub fn read_manifest(backup: &dyn BootFs) -> Result<Manifest> {
    if !backup.exists(MANIFEST_NAME) {
        return Err(Error::new(format!(
            "{} has no `{MANIFEST_NAME}`, so it is not a complete snapshot. A snapshot writes \
             its manifest last, so an interrupted `backup` looks like this.",
            backup.describe()
        )));
    }
    let bytes = backup
        .read(MANIFEST_NAME)
        .map_err(|err| Error::new(format!("cannot read `{MANIFEST_NAME}`: {err}")))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("`{MANIFEST_NAME}` is not UTF-8")))?;
    Manifest::parse(&text)
}

/// Check every file in the snapshot against the manifest, and work out what
/// putting it back would change on `target`.
///
/// The whole snapshot is verified before a single byte is written, for the
/// same reason `execute` resolves every action first: a restore that fails
/// half way leaves a card that boots neither the old way nor the new one.
pub fn restore_changes(
    backup: &dyn BootFs,
    manifest: &Manifest,
    target: &dyn BootFs,
) -> Result<Vec<Change>> {
    let mut changes = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let stored = backup.read(&entry.path).map_err(|err| {
            Error::new(format!(
                "`{}` is listed in {MANIFEST_NAME} but cannot be read from {}: {err}",
                entry.path,
                backup.describe()
            ))
        })?;
        if stored.len() as u64 != entry.bytes {
            return Err(Error::new(format!(
                "`{}` is {} byte(s) in {} but {} in {MANIFEST_NAME}; the snapshot is damaged",
                entry.path,
                stored.len(),
                backup.describe(),
                entry.bytes
            )));
        }
        let digest = sha256_hex(&stored);
        if digest != entry.digest {
            return Err(Error::new(format!(
                "`{}` does not match its digest in {MANIFEST_NAME}; the snapshot is damaged",
                entry.path
            )));
        }

        let kind = if !target.exists(&entry.path) {
            ChangeKind::Create
        } else {
            let current = target
                .read(&entry.path)
                .map_err(|err| Error::new(format!("cannot read `{}`: {err}", entry.path)))?;
            if current == stored {
                ChangeKind::Unchanged
            } else {
                ChangeKind::Update
            }
        };
        changes.push(Change { path: entry.path.clone(), kind, sensitive: false, diff: None });
    }

    // Anything on the card that the snapshot does not know about postdates
    // it, which is exactly what a restore is meant to undo.
    let kept: Vec<&str> = manifest.entries.iter().map(|entry| entry.path.as_str()).collect();
    let present = target
        .list()
        .map_err(|err| Error::new(format!("cannot read {}: {err}", target.describe())))?;
    for path in present {
        if !kept.contains(&path.as_str()) {
            changes.push(Change { path, kind: ChangeKind::Delete, sensitive: false, diff: None });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Put a verified snapshot back, making the target match it exactly.
///
/// `changes` must come from [`restore_changes`] for this pair of
/// filesystems: it is the verification, and this function trusts it.
pub fn restore(
    backup: &dyn BootFs,
    changes: &[Change],
    target: &mut dyn BootFs,
) -> Result<Summary> {
    let mut summary = Summary::default();
    for change in changes {
        match change.kind {
            ChangeKind::Create | ChangeKind::Update => {
                let contents = backup
                    .read(&change.path)
                    .map_err(|err| Error::new(format!("cannot read `{}`: {err}", change.path)))?;
                target
                    .write(&change.path, &contents, false)
                    .map_err(|err| Error::new(format!("cannot write `{}`: {err}", change.path)))?;
                if change.kind == ChangeKind::Create {
                    summary.created += 1;
                } else {
                    summary.updated += 1;
                }
            }
            ChangeKind::Delete => {
                target
                    .remove(&change.path)
                    .map_err(|err| Error::new(format!("cannot remove `{}`: {err}", change.path)))?;
                summary.deleted += 1;
            }
            ChangeKind::Unchanged | ChangeKind::AlreadyAbsent => summary.unchanged += 1,
        }
    }
    Ok(summary)
}
