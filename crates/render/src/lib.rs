//! Turn a validated specification into a plan of boot-partition changes.
//!
//! Rendering is a pure function: it reads nothing and writes nothing. The
//! resulting [`Plan`] is what the `apply` crate executes, and what the
//! golden tests compare against.

pub mod cmdline;
pub mod config_txt;
pub mod gadget;
pub mod nm;
pub mod scripts;

use std::fmt::Write as _;

use rpi_provision_spec::model::SudoMode;
use rpi_provision_spec::Spec;

pub const GENERATOR: &str = concat!("rpi-provision ", env!("CARGO_PKG_VERSION"));

/// One change to make on the boot partition. Paths are relative to the
/// partition root and always use `/` as the separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create the file, replacing any previous content.
    Write {
        path: String,
        contents: String,
        executable: bool,
        /// Whether the content is secret and must be redacted in diffs.
        sensitive: bool,
    },
    /// Merge the managed block into an existing text file.
    MergeManagedBlock { path: String, block: String },
    /// Rewrite the kernel command line in place.
    EditCmdline { path: String, ops: cmdline::Ops },
    /// Delete the file if it exists.
    Remove { path: String },
}

impl Action {
    pub fn path(&self) -> &str {
        match self {
            Action::Write { path, .. }
            | Action::MergeManagedBlock { path, .. }
            | Action::EditCmdline { path, .. }
            | Action::Remove { path } => path,
        }
    }

    pub fn is_sensitive(&self) -> bool {
        matches!(self, Action::Write { sensitive: true, .. })
    }
}

/// The full set of changes derived from a specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub generator: String,
    pub digest: String,
    pub target_dtb: String,
    pub actions: Vec<Action>,
}

impl Plan {
    /// A stable textual dump, used by the golden tests and by `--dry-run`.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "generator: {}", self.generator);
        let _ = writeln!(out, "spec-digest: {}", self.digest);
        let _ = writeln!(out, "target-dtb: {}", self.target_dtb);
        for action in &self.actions {
            out.push('\n');
            match action {
                Action::Write { path, contents, executable, sensitive } => {
                    let _ = writeln!(
                        out,
                        "--- write {path}{}{}",
                        if *executable { " (executable)" } else { "" },
                        if *sensitive { " (sensitive)" } else { "" }
                    );
                    out.push_str(contents);
                    if !contents.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Action::MergeManagedBlock { path, block } => {
                    let _ = writeln!(out, "--- merge-managed-block {path}");
                    out.push_str(block);
                    if !block.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Action::EditCmdline { path, ops } => {
                    let _ = writeln!(out, "--- edit-cmdline {path}");
                    for prefix in &ops.remove_prefixes {
                        let _ = writeln!(out, "remove-prefix {prefix}");
                    }
                    for token in &ops.remove_tokens {
                        let _ = writeln!(out, "remove-token {token}");
                    }
                    for token in &ops.append {
                        let _ = writeln!(out, "append {token}");
                    }
                }
                Action::Remove { path } => {
                    let _ = writeln!(out, "--- remove {path}");
                }
            }
        }
        out
    }
}

/// Paths used by the generated artefacts, derived from the specification.
pub struct Layout {
    /// Directory on the boot partition, e.g. `rpi-provision`.
    pub dir: String,
    /// The same directory as seen by the running system,
    /// e.g. `/boot/firmware/rpi-provision`.
    pub runtime_dir: String,
    pub boot_mount: String,
}

impl Layout {
    pub fn new(spec: &Spec) -> Self {
        let dir = spec.provisioning.runner_dir.clone();
        Self {
            runtime_dir: format!("{}/{dir}", spec.provisioning.boot_mount),
            boot_mount: spec.provisioning.boot_mount.clone(),
            dir,
        }
    }

    fn join(&self, relative: &str) -> String {
        format!("{}/{relative}", self.dir)
    }
}

/// A file to install onto the root filesystem during the first boot.
#[derive(Debug, Clone)]
pub struct PayloadFile {
    /// Path below `<runner_dir>/payload/`.
    pub source: String,
    /// Absolute destination on the root filesystem.
    pub destination: String,
    /// Octal mode, e.g. `0644`.
    pub mode: &'static str,
    pub contents: String,
    pub sensitive: bool,
}

/// Render a specification into a plan.
pub fn render(spec: &Spec, digest: &str) -> Plan {
    let layout = Layout::new(spec);
    let mut actions = Vec::new();
    let mut payload: Vec<PayloadFile> = Vec::new();
    let mut steps: Vec<scripts::Step> = Vec::new();

    // ---------------------------------------------------------- boot config
    actions.push(Action::MergeManagedBlock {
        path: "config.txt".to_string(),
        block: config_txt::managed_block(spec, digest),
    });
    actions.push(Action::EditCmdline {
        path: "cmdline.txt".to_string(),
        ops: cmdline::ops(spec, &layout),
    });

    // ------------------------------------------------------------- hostname
    steps.push(scripts::hostname_step(spec));

    // ----------------------------------------------------------------- user
    steps.push(scripts::user_step(spec, &layout));
    if let Some(hash) = &spec.user.password_hash {
        actions.push(Action::Write {
            path: layout.join("secrets/password.hash"),
            contents: format!("{hash}\n"),
            executable: false,
            sensitive: true,
        });
    }
    if spec.user.sudo == SudoMode::NoPassword {
        payload.push(PayloadFile {
            source: format!("etc/sudoers.d/010-rpi-provision-{}", spec.user.name),
            destination: format!("/etc/sudoers.d/010-rpi-provision-{}", spec.user.name),
            mode: "0440",
            contents: format!(
                "# Generated by {GENERATOR}. Do not edit.\n{} ALL=(ALL) NOPASSWD: ALL\n",
                spec.user.name
            ),
            sensitive: false,
        });
    }

    // ------------------------------------------------------------------ ssh
    if !spec.user.authorized_keys.is_empty() {
        let mut body = format!("# Generated by {GENERATOR}. Do not edit.\n");
        for key in &spec.user.authorized_keys {
            body.push_str(key);
            body.push('\n');
        }
        actions.push(Action::Write {
            path: layout.join("authorized_keys"),
            contents: body,
            executable: false,
            sensitive: false,
        });
    }
    if spec.ssh.enabled {
        payload.push(PayloadFile {
            source: "etc/ssh/sshd_config.d/10-rpi-provision.conf".to_string(),
            destination: "/etc/ssh/sshd_config.d/10-rpi-provision.conf".to_string(),
            mode: "0644",
            contents: scripts::sshd_config(spec),
            sensitive: false,
        });
    }
    steps.push(scripts::ssh_step(spec, &layout));

    // -------------------------------------------------------------- network
    for connection in &spec.network.ethernet {
        payload.push(nm::ethernet_profile(connection));
    }
    for connection in &spec.network.wifi {
        payload.push(nm::wifi_profile(connection));
    }
    if let Some(gadget) = &spec.network.usb_gadget {
        payload.push(nm::gadget_profile(gadget));
        payload.push(gadget::script(gadget));
        payload.push(gadget::unit(gadget));
        payload.push(gadget::modules_load());
    }
    if !spec.network.ethernet.is_empty()
        || !spec.network.wifi.is_empty()
        || spec.network.wifi_country.is_some()
    {
        steps.push(scripts::network_step(spec));
    }
    if spec.network.usb_gadget.is_some() {
        steps.push(scripts::gadget_step());
    }

    // ------------------------------------------------------------- locale
    if spec.system.timezone.is_some() || spec.system.locale.is_some() || spec.system.keymap.is_some()
    {
        steps.push(scripts::locale_step(spec));
    }

    // --------------------------------------------------- payload + manifest
    payload.sort_by(|a, b| a.destination.cmp(&b.destination));
    let mut manifest = format!(
        "# Generated by {GENERATOR}. Do not edit.\n# mode\tsource\tdestination\n"
    );
    for file in &payload {
        let _ = writeln!(manifest, "{}\tpayload/{}\t{}", file.mode, file.source, file.destination);
        actions.push(Action::Write {
            path: layout.join(&format!("payload/{}", file.source)),
            contents: file.contents.clone(),
            executable: false,
            sensitive: file.sensitive,
        });
    }
    actions.push(Action::Write {
        path: layout.join("manifest.tsv"),
        contents: manifest,
        executable: false,
        sensitive: false,
    });

    // The payload installer sits between account creation (step 20) and every
    // step that consumes an installed file (step 40 onwards).
    steps.push(scripts::payload_step());
    steps.sort_by_key(|step| step.order);
    debug_assert!(
        steps.windows(2).all(|pair| pair[0].order < pair[1].order),
        "step order numbers must be unique"
    );
    for step in &steps {
        actions.push(Action::Write {
            path: layout.join(&format!("steps/{}", step.file_name())),
            contents: step.body.clone(),
            executable: true,
            sensitive: false,
        });
    }

    // ------------------------------------------------------------- runner
    actions.push(Action::Write {
        path: layout.join("firstrun.sh"),
        contents: scripts::firstrun(spec, &layout, digest),
        executable: true,
        sensitive: false,
    });

    actions.sort_by(|a, b| action_rank(a).cmp(&action_rank(b)).then_with(|| a.path().cmp(b.path())));

    Plan {
        generator: GENERATOR.to_string(),
        digest: digest.to_string(),
        target_dtb: spec.meta.target.device_tree_blob().to_string(),
        actions,
    }
}

/// Keep `config.txt` and `cmdline.txt` first so that a dry-run diff reads in
/// the order an operator thinks about the card.
fn action_rank(action: &Action) -> u8 {
    match action.path() {
        "config.txt" => 0,
        "cmdline.txt" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests;
