//! Command implementations.

use std::io::{IsTerminal, Write};
use std::path::Path;

use rpi_provision_apply::{
    backup as snapshot, conflicting_first_boot_files, execute, plan_changes, revert_plan,
    verify_boot_partition, verify_boot_partition_shape, BootFs, Change, ChangeKind, RealBootFs,
};
use rpi_provision_render::{render, Plan};
use rpi_provision_spec::{load_file, LoadOptions, Loaded, SystemSecrets};

use crate::args::Options;

pub struct Failure(pub String);

impl<E: std::fmt::Display> From<E> for Failure {
    fn from(error: E) -> Self {
        Failure(error.to_string())
    }
}

type Result<T> = std::result::Result<T, Failure>;

fn load(spec: &Path, options: &Options) -> Result<Loaded> {
    let provider = SystemSecrets;
    let mut load_options = LoadOptions::new(&provider);
    for set in &options.sets {
        load_options = load_options.set(set)?;
    }
    for set in &options.set_secrets {
        load_options = load_options.set_secret(set)?;
    }
    Ok(load_file(spec, &load_options)?)
}

fn report_warnings(loaded: &Loaded, options: &Options) {
    if options.quiet {
        return;
    }
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
}

fn summarise(loaded: &Loaded) {
    let spec = &loaded.spec;
    println!("target:      {}", spec.meta.target.as_str());
    println!("hostname:    {}", spec.system.hostname);
    println!("user:        {}", spec.user.name);
    println!(
        "ssh:         {}, password auth {}, {} authorized key(s)",
        if spec.ssh.enabled { "enabled" } else { "disabled" },
        if spec.ssh.password_authentication { "on" } else { "off" },
        spec.user.authorized_keys.len()
    );
    println!(
        "network:     {} wired, {} wireless, usb gadget {}",
        spec.network.ethernet.len(),
        spec.network.wifi.len(),
        match &spec.network.usb_gadget {
            Some(gadget) => format!("{} on {}", gadget.function.as_str(), gadget.address),
            None => "disabled".to_string(),
        }
    );
    let hardware = &spec.hardware;
    let mut enabled: Vec<&str> = Vec::new();
    if hardware.uart.enabled {
        enabled.push("uart0");
    }
    if hardware.uart.debug_connector {
        enabled.push("debug-uart");
    }
    if hardware.i2c.enabled {
        enabled.push("i2c");
    }
    if hardware.spi.enabled {
        enabled.push("spi");
    }
    if hardware.one_wire.enabled {
        enabled.push("1-wire");
    }
    println!(
        "hardware:    {}",
        if enabled.is_empty() { "none".to_string() } else { enabled.join(", ") }
    );
    println!("spec digest: {}", loaded.digest);
}

pub fn validate(spec: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    if !options.quiet {
        summarise(&loaded);
    }
    Ok(())
}

pub fn render_to_directory(spec: &Path, out: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    let plan = render(&loaded.spec, &loaded.digest);

    std::fs::create_dir_all(out)
        .map_err(|err| Failure(format!("cannot create `{}`: {err}", out.display())))?;
    let mut fs = RealBootFs::new(out);
    let summary = execute(&plan, &mut fs)?;

    if !options.quiet {
        println!("wrote the plan to {}", out.display());
        println!("{summary}");
        println!(
            "note: config.txt and cmdline.txt here are rendered as if starting from an empty \
             file, unless `{}` already contained them.",
            out.display()
        );
    }
    Ok(())
}

fn print_changes(changes: &[Change], options: &Options) {
    for change in changes {
        if change.kind == ChangeKind::Unchanged || change.kind == ChangeKind::AlreadyAbsent {
            continue;
        }
        println!("{:>9}  {}", change.kind.label(), change.path);
        if options.quiet {
            continue;
        }
        let Some(diff) = &change.diff else { continue };
        // A newly created file has nothing to compare against, so its whole
        // content would be printed. That is rarely what an operator wants to
        // read; --verbose asks for it explicitly.
        if change.kind == ChangeKind::Create && !options.verbose && !change.sensitive {
            println!("           (new file, {} lines)", diff.lines().count());
            continue;
        }
        for line in diff.lines() {
            println!("           {line}");
        }
    }
}

fn tally(changes: &[Change]) -> (usize, usize, usize, usize) {
    let count = |kind: ChangeKind| changes.iter().filter(|change| change.kind == kind).count();
    (
        count(ChangeKind::Create),
        count(ChangeKind::Update),
        count(ChangeKind::Unchanged),
        count(ChangeKind::Delete),
    )
}

fn open_boot(boot: &Path, plan: &Plan, options: &Options) -> Result<RealBootFs> {
    if !boot.is_dir() {
        return Err(Failure(format!("`{}` is not a directory", boot.display())));
    }
    let fs = RealBootFs::new(boot);
    if options.allow_unverified_boot {
        if !options.quiet {
            eprintln!(
                "warning: skipping the boot-partition check because of --allow-unverified-boot"
            );
        }
    } else {
        verify_boot_partition(&fs, &plan.target_dtb)?;
    }
    Ok(fs)
}

/// Open a partition without a specification to compare it against.
///
/// `backup` and `restore` work on whatever is on the card, so the check is
/// the model-independent one.
fn open_partition(boot: &Path, options: &Options) -> Result<RealBootFs> {
    if !boot.is_dir() {
        return Err(Failure(format!("`{}` is not a directory", boot.display())));
    }
    let fs = RealBootFs::new(boot);
    if options.allow_unverified_boot {
        if !options.quiet {
            eprintln!(
                "warning: skipping the boot-partition check because of --allow-unverified-boot"
            );
        }
    } else {
        verify_boot_partition_shape(&fs)?;
    }
    Ok(fs)
}

/// Refuse to add a second first-boot mechanism to a card that already has one.
fn check_conflicts(fs: &RealBootFs, options: &Options) -> Result<()> {
    let conflicts = conflicting_first_boot_files(fs);
    if conflicts.is_empty() {
        return Ok(());
    }
    let listed: Vec<String> =
        conflicts.iter().map(|(name, what)| format!("  {name}  ({what})")).collect();
    if options.ignore_conflicts {
        eprintln!(
            "warning: another first-boot mechanism is present and will be overridden:\n{}",
            listed.join("\n")
        );
        return Ok(());
    }
    Err(Failure(format!(
        "{} already carries another first-boot mechanism:\n{}\n\
         Applying would rewrite cmdline.txt and silently disable it. Delete the\n\
         file(s) above, or pass --ignore-conflicts if that is what you intend.",
        fs.describe(),
        listed.join("\n")
    )))
}

pub fn diff(spec: &Path, boot: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    let plan = render(&loaded.spec, &loaded.digest);
    let fs = open_boot(boot, &plan, options)?;

    let changes = plan_changes(&plan, &fs)?;
    print_changes(&changes, options);
    let (created, updated, unchanged, deleted) = tally(&changes);
    println!(
        "{created} to create, {updated} to update, {unchanged} unchanged, {deleted} to delete"
    );
    for (name, what) in conflicting_first_boot_files(&fs) {
        eprintln!("warning: `{name}` ({what}) is present and would conflict with apply");
    }
    Ok(())
}

pub fn apply(spec: &Path, boot: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    let plan = render(&loaded.spec, &loaded.digest);
    let mut fs = open_boot(boot, &plan, options)?;
    check_conflicts(&fs, options)?;

    let changes = plan_changes(&plan, &fs)?;
    print_changes(&changes, options);
    let (created, updated, _, deleted) = tally(&changes);
    if created + updated + deleted == 0 {
        println!("{} is already up to date", boot.display());
        return Ok(());
    }

    if !confirm(
        &format!("Write {} change(s) to {}?", created + updated + deleted, fs.describe()),
        options,
    )? {
        return Err(Failure("aborted at the confirmation prompt".into()));
    }

    if let Some(directory) = &options.backup {
        take_snapshot(&fs, directory, options)?;
    }

    let summary = execute(&plan, &mut fs)?;
    println!("{summary}");
    if !options.quiet {
        println!(
            "\nEject the card, boot the Raspberry Pi and wait for it to reboot once.\n\
             The run is logged to {} on the device, and its outcome to\n\
             /var/lib/rpi-provision/status.",
            loaded.spec.provisioning.log_path
        );
        if loaded.spec.provisioning.wipe_payload {
            println!(
                "The payload under {}/{} is deleted on the device after a successful run.",
                loaded.spec.provisioning.boot_mount, loaded.spec.provisioning.runner_dir
            );
        } else {
            println!(
                "warning: `provisioning.wipe_payload` is false, so secrets stay on the card's \
                 FAT partition after the first boot."
            );
        }
    }
    Ok(())
}

pub fn revert(spec: &Path, boot: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    let plan = render(&loaded.spec, &loaded.digest);
    let reverse = revert_plan(&plan);
    let mut fs = open_boot(boot, &plan, options)?;

    let changes = plan_changes(&reverse, &fs)?;
    print_changes(&changes, options);
    let (created, updated, _, deleted) = tally(&changes);
    if created + updated + deleted == 0 {
        println!("{} has nothing to revert", boot.display());
        return Ok(());
    }

    if !confirm(
        &format!("Revert {} change(s) on {}?", created + updated + deleted, fs.describe()),
        options,
    )? {
        return Err(Failure("aborted at the confirmation prompt".into()));
    }

    let summary = execute(&reverse, &mut fs)?;
    println!("{summary}");
    Ok(())
}

/// Copy a whole boot partition into a new directory.
fn take_snapshot(source: &RealBootFs, out: &Path, options: &Options) -> Result<()> {
    if out.exists() && !out.is_dir() {
        return Err(Failure(format!("`{}` exists and is not a directory", out.display())));
    }
    std::fs::create_dir_all(out)
        .map_err(|err| Failure(format!("cannot create `{}`: {err}", out.display())))?;
    let mut destination = RealBootFs::new(out);
    let manifest = snapshot::create(
        source,
        &mut destination,
        rpi_provision_render::GENERATOR,
        &utc_timestamp(now_seconds()),
    )?;
    if !options.quiet {
        println!(
            "snapshot: {} file(s), {} to {}",
            manifest.entries.len(),
            human_bytes(manifest.total_bytes()),
            out.display()
        );
    }
    Ok(())
}

pub fn backup(boot: &Path, out: &Path, options: &Options) -> Result<()> {
    let source = open_partition(boot, options)?;
    take_snapshot(&source, out, options)?;
    if !options.quiet {
        println!(
            "\nPut it back with:\n    rpi-provision restore --boot {} --from {}",
            boot.display(),
            out.display()
        );
    }
    Ok(())
}

pub fn restore(boot: &Path, from: &Path, options: &Options) -> Result<()> {
    if !from.is_dir() {
        return Err(Failure(format!("`{}` is not a directory", from.display())));
    }
    let source = RealBootFs::new(from);
    let manifest = snapshot::read_manifest(&source)?;
    let mut target = open_partition(boot, options)?;

    if !options.quiet {
        println!(
            "snapshot of {}, taken {} by {}",
            manifest.source, manifest.created, manifest.generator
        );
    }

    // Verifies every file in the snapshot before anything is written.
    let changes = snapshot::restore_changes(&source, &manifest, &target)?;
    print_changes(&changes, options);
    let (created, updated, unchanged, deleted) = tally(&changes);
    if created + updated + deleted == 0 {
        println!("{} already matches the snapshot ({unchanged} file(s))", boot.display());
        return Ok(());
    }

    println!(
        "{created} to restore, {updated} to overwrite, {unchanged} unchanged, \
         {deleted} to delete"
    );
    if deleted > 0 && !options.quiet {
        println!(
            "The {deleted} file(s) marked `delete` are on the card but not in the snapshot; \
             restoring removes them."
        );
    }
    if !confirm(
        &format!(
            "Make {} match the snapshot ({} change(s))?",
            target.describe(),
            created + updated + deleted
        ),
        options,
    )? {
        return Err(Failure("aborted at the confirmation prompt".into()));
    }

    let summary = snapshot::restore(&source, &changes, &mut target)?;
    println!("{summary}");
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Format a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_timestamp(seconds: u64) -> String {
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time / 60) % 60,
        time % 60
    )
}

/// Days since 1970-01-01 to a civil date, by Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

pub fn detect(options: &Options) -> Result<()> {
    let candidates = rpi_provision_apply::detect::candidates();
    if candidates.is_empty() {
        if !options.quiet {
            eprintln!(
                "no mounted Raspberry Pi boot partition found; insert the card, or pass \
                 --boot explicitly"
            );
        }
        return Ok(());
    }
    for candidate in candidates {
        println!("{}\t{}", candidate.path.display(), candidate.model.unwrap_or("unknown model"));
    }
    Ok(())
}

/// Ask before writing. Without a terminal, `--yes` is mandatory: a
/// provisioning run started from a script should be explicit about it.
fn confirm(question: &str, options: &Options) -> Result<bool> {
    if options.yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Failure(
            "standard input is not a terminal; pass --yes to confirm non-interactively".into(),
        ));
    }
    print!("{question} [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|err| Failure(format!("cannot write to standard output: {err}")))?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|err| Failure(format!("cannot read the answer: {err}")))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}
