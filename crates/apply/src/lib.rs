//! Execute a [`Plan`] against a boot partition.
//!
//! The plan itself is produced by `rpi-provision-render` and is independent
//! of any filesystem. This crate is the only place that reads or writes, and
//! it always computes the full resulting content before touching anything, so
//! a dry run and a real run go down the same code path.

pub mod backup;
pub mod detect;
pub mod diff;
pub mod fs;

use std::fmt;

use rpi_provision_render::{cmdline, config_txt, Action, Plan};

pub use fs::{BootFs, MemBootFs, RealBootFs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The concrete outcome of one action, with all merging already done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Write { path: String, contents: Vec<u8>, executable: bool, sensitive: bool },
    Remove { path: String },
}

impl Resolution {
    pub fn path(&self) -> &str {
        match self {
            Resolution::Write { path, .. } | Resolution::Remove { path } => path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Create,
    Update,
    Unchanged,
    Delete,
    /// A removal of something that is not there.
    AlreadyAbsent,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            ChangeKind::Create => "create",
            ChangeKind::Update => "update",
            ChangeKind::Unchanged => "unchanged",
            ChangeKind::Delete => "delete",
            ChangeKind::AlreadyAbsent => "absent",
        }
    }

    pub fn is_write(self) -> bool {
        matches!(self, ChangeKind::Create | ChangeKind::Update)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub sensitive: bool,
    /// `None` when there is nothing to show, or when the content is secret.
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
}

impl Summary {
    pub fn total_changes(&self) -> usize {
        self.created + self.updated + self.deleted
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} created, {} updated, {} unchanged, {} deleted",
            self.created, self.updated, self.unchanged, self.deleted
        )
    }
}

/// Confirm the directory is a Raspberry Pi boot partition of some kind.
///
/// This is the model-independent half of the check. Snapshots are taken and
/// put back without a specification, so there is no target to compare a
/// device tree blob against, but writing a whole partition into the wrong
/// directory is worth refusing all the same.
pub fn verify_boot_partition_shape(fs: &dyn BootFs) -> Result<()> {
    let mut missing = Vec::new();
    for required in ["config.txt", "cmdline.txt"] {
        if !fs.exists(required) {
            missing.push(required.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(Error::new(format!(
            "{} does not look like a Raspberry Pi boot partition: {} not found",
            fs.describe(),
            missing.join(" and ")
        )));
    }
    Ok(())
}

/// Confirm the directory really is a Raspberry Pi boot partition for the
/// target the specification names.
///
/// Writing a provisioning payload into the wrong directory is both easy to do
/// and hard to notice, so this check is not optional in normal operation.
pub fn verify_boot_partition(fs: &dyn BootFs, expected_dtb: &str) -> Result<()> {
    verify_boot_partition_shape(fs)?;
    if !fs.exists(expected_dtb) {
        return Err(Error::new(format!(
            "{} has config.txt and cmdline.txt but no `{expected_dtb}`, so it is not a \
             Raspberry Pi 5 boot partition. Check that the correct card is mounted.",
            fs.describe()
        )));
    }
    Ok(())
}

/// First-boot mechanisms that would fight with this tool.
///
/// Each of these is consumed by something else during the first boot, and the
/// order in which they run relative to `systemd.run=` is not defined. Worse,
/// rewriting `cmdline.txt` drops any foreign `systemd.run=` hook, silently
/// disabling it.
const CONFLICTING_FILES: [(&str, &str); 5] = [
    ("custom.toml", "Raspberry Pi Imager customisation"),
    ("rpi-preseed.toml", "rpi-preseed customisation"),
    ("userconf.txt", "the userconf-pi first-boot account"),
    ("firstrun.sh", "another first-boot script"),
    ("user-data", "a cloud-init datasource"),
];

/// List foreign first-boot files present on the partition.
pub fn conflicting_first_boot_files(fs: &dyn BootFs) -> Vec<(&'static str, &'static str)> {
    CONFLICTING_FILES.into_iter().filter(|(name, _)| fs.exists(name)).collect()
}

/// Compute the exact bytes each action would produce.
pub fn resolve(action: &Action, fs: &dyn BootFs) -> Result<Resolution> {
    match action {
        Action::Write { path, contents, executable, sensitive } => Ok(Resolution::Write {
            path: path.clone(),
            contents: contents.as_bytes().to_vec(),
            executable: *executable,
            sensitive: *sensitive,
        }),
        Action::WriteBytes { path, contents, sensitive } => Ok(Resolution::Write {
            path: path.clone(),
            contents: contents.clone(),
            executable: false,
            sensitive: *sensitive,
        }),
        Action::MergeManagedBlock { path, block } => {
            let existing = read_text(fs, path)?;
            let merged = config_txt::merge(&existing, block)
                .map_err(|err| Error::new(format!("{path}: {err}")))?;
            Ok(Resolution::Write {
                path: path.clone(),
                contents: merged.into_bytes(),
                executable: false,
                sensitive: false,
            })
        }
        Action::EditCmdline { path, ops } => {
            let existing = read_text(fs, path)?;
            let edited = cmdline::apply(&existing, ops)
                .map_err(|err| Error::new(format!("{path}: {err}")))?;
            Ok(Resolution::Write {
                path: path.clone(),
                contents: edited.into_bytes(),
                executable: false,
                sensitive: false,
            })
        }
        Action::Remove { path } => Ok(Resolution::Remove { path: path.clone() }),
    }
}

fn read_text(fs: &dyn BootFs, path: &str) -> Result<String> {
    if !fs.exists(path) {
        return Ok(String::new());
    }
    let bytes = fs.read(path).map_err(|err| Error::new(format!("cannot read {path}: {err}")))?;
    String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{path} is not valid UTF-8; refusing to edit it")))
}

/// Describe what applying the plan would do, without changing anything.
pub fn plan_changes(plan: &Plan, fs: &dyn BootFs) -> Result<Vec<Change>> {
    let mut changes = Vec::with_capacity(plan.actions.len());
    for action in &plan.actions {
        changes.push(describe(&resolve(action, fs)?, fs)?);
    }
    Ok(changes)
}

fn describe(resolution: &Resolution, fs: &dyn BootFs) -> Result<Change> {
    match resolution {
        Resolution::Write { path, contents, sensitive, .. } => {
            let exists = fs.exists(path);
            // Bytes, not text: a payload asset is whatever the user pointed
            // at, so it may not be UTF-8 on either side of the comparison.
            let previous = if exists {
                fs.read(path).map_err(|err| Error::new(format!("cannot read {path}: {err}")))?
            } else {
                Vec::new()
            };

            let kind = if !exists {
                ChangeKind::Create
            } else if previous == *contents {
                ChangeKind::Unchanged
            } else {
                ChangeKind::Update
            };

            let diff = if *sensitive {
                (kind != ChangeKind::Unchanged)
                    .then(|| "  (content withheld: this file holds secret material)\n".to_string())
            } else {
                match (String::from_utf8(previous), std::str::from_utf8(contents)) {
                    (Ok(before), Ok(after)) => diff::unified(&before, after),
                    _ => (kind != ChangeKind::Unchanged)
                        .then(|| format!("  (binary, {} bytes)\n", contents.len())),
                }
            };

            Ok(Change { path: path.clone(), kind, sensitive: *sensitive, diff })
        }
        Resolution::Remove { path } => Ok(Change {
            path: path.clone(),
            kind: if fs.exists(path) { ChangeKind::Delete } else { ChangeKind::AlreadyAbsent },
            sensitive: false,
            diff: None,
        }),
    }
}

/// Apply the plan.
///
/// Every action is resolved against the *original* state first, so a failure
/// part-way through cannot leave a half-merged `config.txt`.
pub fn execute(plan: &Plan, fs: &mut dyn BootFs) -> Result<Summary> {
    let mut resolutions = Vec::with_capacity(plan.actions.len());
    for action in &plan.actions {
        resolutions.push(resolve(action, fs)?);
    }

    let mut summary = Summary::default();
    for resolution in &resolutions {
        let kind = describe(resolution, fs)?.kind;
        match resolution {
            Resolution::Write { path, contents, executable, .. } => match kind {
                ChangeKind::Unchanged => summary.unchanged += 1,
                ChangeKind::Create | ChangeKind::Update => {
                    fs.write(path, contents, *executable)
                        .map_err(|err| Error::new(format!("cannot write {path}: {err}")))?;
                    if kind == ChangeKind::Create {
                        summary.created += 1;
                    } else {
                        summary.updated += 1;
                    }
                }
                other => {
                    return Err(Error::new(format!(
                        "internal error: a write to {path} was classified as `{}`",
                        other.label()
                    )))
                }
            },
            Resolution::Remove { path } => {
                if kind == ChangeKind::Delete {
                    fs.remove(path)
                        .map_err(|err| Error::new(format!("cannot remove {path}: {err}")))?;
                    summary.deleted += 1;
                }
            }
        }
    }
    Ok(summary)
}

/// Remove everything a previous run installed: the managed block, the
/// command line hooks and the payload directory.
pub fn revert_plan(plan: &Plan) -> Plan {
    let mut actions = vec![
        Action::MergeManagedBlock { path: "config.txt".to_string(), block: String::new() },
        Action::EditCmdline { path: "cmdline.txt".to_string(), ops: cmdline::cleanup_ops() },
    ];
    for action in &plan.actions {
        match action {
            Action::Write { path, .. } | Action::WriteBytes { path, .. } => {
                actions.push(Action::Remove { path: path.clone() })
            }
            Action::MergeManagedBlock { .. }
            | Action::EditCmdline { .. }
            | Action::Remove { .. } => {}
        }
    }
    Plan {
        generator: plan.generator.clone(),
        digest: plan.digest.clone(),
        target_dtb: plan.target_dtb.clone(),
        actions,
    }
}

#[cfg(test)]
mod tests;
