//! Command implementations.

use std::io::{IsTerminal, Write};
use std::path::Path;

use rpi_provision_apply::{
    execute, plan_changes, revert_plan, verify_boot_partition, BootFs, Change, ChangeKind,
    RealBootFs,
};
use rpi_provision_render::{render, Plan};
use rpi_provision_spec::{load_file, Loaded, LoadOptions, SystemSecrets};

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
            eprintln!("warning: skipping the boot-partition check because of --allow-unverified-boot");
        }
    } else {
        verify_boot_partition(&fs, &plan.target_dtb)?;
    }
    Ok(fs)
}

pub fn diff(spec: &Path, boot: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    let plan = render(&loaded.spec, &loaded.digest);
    let fs = open_boot(boot, &plan, options)?;

    let changes = plan_changes(&plan, &fs)?;
    print_changes(&changes, options);
    let (created, updated, unchanged, deleted) = tally(&changes);
    println!("{created} to create, {updated} to update, {unchanged} unchanged, {deleted} to delete");
    Ok(())
}

pub fn apply(spec: &Path, boot: &Path, options: &Options) -> Result<()> {
    let loaded = load(spec, options)?;
    report_warnings(&loaded, options);
    let plan = render(&loaded.spec, &loaded.digest);
    let mut fs = open_boot(boot, &plan, options)?;

    let changes = plan_changes(&plan, &fs)?;
    print_changes(&changes, options);
    let (created, updated, _, deleted) = tally(&changes);
    if created + updated + deleted == 0 {
        println!("{} is already up to date", boot.display());
        return Ok(());
    }

    if !confirm(
        &format!(
            "Write {} change(s) to {}?",
            created + updated + deleted,
            fs.describe()
        ),
        options,
    )? {
        return Err(Failure("aborted at the confirmation prompt".into()));
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

    if !confirm(&format!("Revert {} change(s) on {}?", created + updated + deleted, fs.describe()), options)? {
        return Err(Failure("aborted at the confirmation prompt".into()));
    }

    let summary = execute(&reverse, &mut fs)?;
    println!("{summary}");
    Ok(())
}

pub fn detect(options: &Options) -> Result<()> {
    let candidates = crate::detect::candidates();
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
        println!(
            "{}\t{}",
            candidate.path.display(),
            candidate.model.unwrap_or("unknown model")
        );
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
