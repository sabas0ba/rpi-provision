//! Token-level editing of `cmdline.txt`.
//!
//! The kernel command line is a single whitespace-separated line. Editing it
//! as tokens rather than with a regular expression keeps the operation
//! idempotent and preserves everything the distribution put there.

use rpi_provision_spec::Spec;

use crate::Layout;

/// Edits to apply, in order: removals first, then appends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ops {
    /// Drop any token starting with one of these prefixes.
    pub remove_prefixes: Vec<String>,
    /// Drop tokens equal to one of these.
    pub remove_tokens: Vec<String>,
    /// Append these tokens if they are not already present.
    pub append: Vec<String>,
}

/// Derive the command line edits for a specification.
pub fn ops(spec: &Spec, layout: &Layout) -> Ops {
    let mut ops = Ops::default();

    // Always take ownership of our own first-boot hooks so that re-applying
    // replaces them instead of accumulating duplicates.
    for prefix in ["systemd.run=", "systemd.run_success_action=", "systemd.unit="] {
        ops.remove_prefixes.push(prefix.to_string());
    }

    let uart = &spec.hardware.uart;
    if uart.console {
        // A console on GPIO 14/15 is /dev/ttyAMA0 on Raspberry Pi 5; `serial0`
        // points at the dedicated debug UART (/dev/ttyAMA10) instead.
        ops.remove_prefixes.push("console=serial0".to_string());
        ops.remove_prefixes.push("console=ttyAMA0".to_string());
        ops.append.push(format!("console=ttyAMA0,{}", uart.baudrate));
    } else if !uart.debug_connector {
        // No serial console was asked for; drop the one the image ships with.
        ops.remove_prefixes.push("console=serial0".to_string());
        ops.remove_prefixes.push("console=ttyAMA0".to_string());
        ops.remove_prefixes.push("console=ttyAMA10".to_string());
    }

    for token in &spec.hardware.cmdline_remove {
        ops.remove_tokens.push(token.clone());
    }

    ops.append.push(format!("systemd.run={}/firstrun.sh", layout.runtime_dir));
    ops.append.push("systemd.run_success_action=reboot".to_string());
    ops.append.push("systemd.unit=kernel-command-line.target".to_string());

    for token in &spec.hardware.cmdline_append {
        if !ops.append.contains(token) {
            ops.append.push(token.clone());
        }
    }

    ops
}

/// Apply `ops` to an existing command line, returning the new file content.
pub fn apply(existing: &str, ops: &Ops) -> Result<String, String> {
    let trimmed = existing.trim();
    if trimmed.lines().count() > 1 {
        return Err("cmdline.txt must contain exactly one line".to_string());
    }

    let mut tokens: Vec<String> = trimmed.split_whitespace().map(str::to_string).collect();

    tokens.retain(|token| {
        !ops.remove_prefixes.iter().any(|prefix| token.starts_with(prefix.as_str()))
            && !ops.remove_tokens.iter().any(|dead| token == dead)
    });

    for token in &ops.append {
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.clone());
        }
    }

    // The firmware requires a single line terminated by a newline.
    Ok(format!("{}\n", tokens.join(" ")))
}

/// Remove the first-boot hooks, as the runner does on the device.
pub fn cleanup_ops() -> Ops {
    Ops {
        remove_prefixes: vec![
            "systemd.run=".to_string(),
            "systemd.run_success_action=".to_string(),
            "systemd.unit=".to_string(),
        ],
        ..Ops::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = "console=serial0,115200 console=tty1 root=PARTUUID=abcd1234-02 \
rootfstype=ext4 fsck.repair=yes rootwait quiet init=/usr/lib/raspberrypi-sys-mods/firstboot\n";

    fn hooks() -> Ops {
        Ops {
            remove_prefixes: vec![
                "systemd.run=".into(),
                "systemd.run_success_action=".into(),
                "systemd.unit=".into(),
                "console=serial0".into(),
                "console=ttyAMA0".into(),
                "console=ttyAMA10".into(),
            ],
            remove_tokens: vec![],
            append: vec![
                "systemd.run=/boot/firmware/rpi-provision/firstrun.sh".into(),
                "systemd.run_success_action=reboot".into(),
                "systemd.unit=kernel-command-line.target".into(),
            ],
        }
    }

    #[test]
    fn appends_hooks_and_drops_serial_console() {
        let result = apply(STOCK, &hooks()).unwrap();
        assert!(!result.contains("console=serial0"));
        assert!(result.contains("console=tty1"), "the virtual console must survive");
        assert!(result.contains("root=PARTUUID=abcd1234-02"));
        assert!(result.contains("init=/usr/lib/raspberrypi-sys-mods/firstboot"));
        assert!(result.ends_with("systemd.unit=kernel-command-line.target\n"));
        assert_eq!(result.lines().count(), 1);
    }

    #[test]
    fn is_idempotent() {
        let once = apply(STOCK, &hooks()).unwrap();
        let twice = apply(&once, &hooks()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn replaces_a_previous_runner_path() {
        let mut previous = hooks();
        previous.append[0] = "systemd.run=/boot/firmware/old-dir/firstrun.sh".into();
        let first = apply(STOCK, &previous).unwrap();
        let second = apply(&first, &hooks()).unwrap();
        assert!(!second.contains("old-dir"));
        assert_eq!(second.matches("systemd.run=").count(), 1, "exactly one runner hook");
        assert!(second.contains("systemd.run=/boot/firmware/rpi-provision/firstrun.sh"));
    }

    #[test]
    fn cleanup_removes_only_the_hooks() {
        let applied = apply(STOCK, &hooks()).unwrap();
        let cleaned = apply(&applied, &cleanup_ops()).unwrap();
        assert!(!cleaned.contains("systemd.run"));
        assert!(!cleaned.contains("systemd.unit"));
        assert!(cleaned.contains("root=PARTUUID=abcd1234-02"));
        assert!(cleaned.contains("rootwait"));
    }

    #[test]
    fn adds_a_gpio_console_when_requested() {
        let ops = Ops {
            remove_prefixes: vec!["console=serial0".into(), "console=ttyAMA0".into()],
            remove_tokens: vec![],
            append: vec!["console=ttyAMA0,115200".into()],
        };
        let result = apply(STOCK, &ops).unwrap();
        assert!(result.contains("console=ttyAMA0,115200"));
        assert!(!result.contains("console=serial0"));
    }

    #[test]
    fn removes_explicit_tokens() {
        let ops = Ops { remove_tokens: vec!["quiet".into()], ..Ops::default() };
        let result = apply(STOCK, &ops).unwrap();
        assert!(!result.contains("quiet"));
        assert!(result.contains("rootwait"));
    }

    #[test]
    fn rejects_multi_line_input() {
        assert!(apply("a=1\nb=2\n", &Ops::default()).is_err());
    }

    #[test]
    fn tolerates_missing_trailing_newline_and_extra_spaces() {
        let result = apply("  a=1   b=2  ", &Ops::default()).unwrap();
        assert_eq!(result, "a=1 b=2\n");
    }
}
