//! Generation and idempotent merging of the `config.txt` managed block.
//!
//! The block is always placed at the end of the file. `config.txt` conditional
//! filters (`[all]`, `[cm5]`, …) are sticky, so appending is the only position
//! where the block cannot silently change the scope of lines written by
//! somebody else.

use std::fmt::Write as _;

use rpi_provision_spec::Spec;

pub const BEGIN: &str = "# >>> rpi-provision managed block; generated, do not edit >>>";
pub const END: &str = "# <<< rpi-provision managed block <<<";

/// Build the managed block for a specification.
pub fn managed_block(spec: &Spec, digest: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{BEGIN}");
    let _ = writeln!(out, "# generator: {}", crate::GENERATOR);
    let _ = writeln!(out, "# spec-digest: {digest}");
    let _ = writeln!(out, "[all]");

    let hw = &spec.hardware;

    if hw.uart.enabled {
        // UART0 on GPIO 14/15, exposed as /dev/ttyAMA0 on Raspberry Pi 5.
        let _ = writeln!(out, "dtparam=uart0=on");
    }
    if hw.uart.debug_connector {
        // The dedicated three pin debug connector, /dev/ttyAMA10.
        let _ = writeln!(out, "enable_uart=1");
    }
    if hw.i2c.enabled {
        let _ = writeln!(out, "dtparam=i2c_arm=on");
        let _ = writeln!(out, "dtparam=i2c_arm_baudrate={}", hw.i2c.baudrate);
    }
    if hw.spi.enabled {
        let _ = writeln!(out, "dtparam=spi=on");
    }
    if hw.one_wire.enabled {
        let _ = writeln!(out, "dtoverlay=w1-gpio,gpiopin={}", hw.one_wire.gpio);
    }
    if let Some(generation) = hw.pcie_gen {
        let _ = writeln!(out, "dtparam=pciex1_gen={generation}");
    }
    if hw.usb_max_current {
        // Raspberry Pi 5 holds downstream USB to 600mA unless the supply
        // claims 5A, and refuses to boot from USB on a 3A supply. This forces
        // the high limit; the supply has to be able to deliver it.
        let _ = writeln!(out, "usb_max_current_enable=1");
    }
    for (step, threshold) in hw.fan_thresholds.iter().enumerate() {
        let _ = writeln!(out, "dtparam=fan_temp{step}={threshold}");
    }
    if spec.network.usb_gadget.is_some() {
        // Raspberry Pi 5 has no OTG_ID pin, so peripheral mode must be forced.
        let _ = writeln!(out, "dtoverlay=dwc2,dr_mode=peripheral");
    }
    for param in &hw.dtparams {
        let _ = writeln!(out, "dtparam={param}");
    }
    for overlay in &hw.overlays {
        let _ = writeln!(out, "dtoverlay={overlay}");
    }
    for line in &hw.config_extra {
        let _ = writeln!(out, "{line}");
    }

    let _ = writeln!(out, "{END}");
    out
}

/// Merge `block` into `existing`, replacing any previous managed block.
///
/// Returns the new file content. The operation is idempotent: merging the
/// same block twice yields the same result.
pub fn merge(existing: &str, block: &str) -> Result<String, String> {
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();

    let begin = lines.iter().position(|line| line.trim_end() == BEGIN);
    let end = lines.iter().position(|line| line.trim_end() == END);

    let kept: String = match (begin, end) {
        (Some(b), Some(e)) if e >= b => {
            let mut kept = String::new();
            kept.extend(lines[..b].iter().copied());
            kept.extend(lines[e + 1..].iter().copied());
            kept
        }
        (None, None) => existing.to_string(),
        (Some(_), None) => {
            return Err(format!("`{BEGIN}` is present but the closing `{END}` is missing"))
        }
        (None, Some(_)) => {
            return Err(format!("`{END}` is present but the opening `{BEGIN}` is missing"))
        }
        (Some(b), Some(e)) => {
            return Err(format!(
                "the managed block markers are inverted (opening at line {}, closing at line {})",
                b + 1,
                e + 1
            ))
        }
    };

    let mut out = kept.trim_end_matches(['\n', '\r']).to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Remove the managed block without adding a new one.
pub fn strip(existing: &str) -> Result<String, String> {
    let empty = merge(existing, "")?;
    Ok(empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "# original config\ndtparam=audio=on\ncamera_auto_detect=1\n";

    fn block(body: &str) -> String {
        format!("{BEGIN}\n{body}{END}\n")
    }

    #[test]
    fn appends_when_absent() {
        let merged = merge(ORIGINAL, &block("[all]\ndtparam=spi=on\n")).unwrap();
        assert!(merged.starts_with(ORIGINAL));
        assert!(merged.contains("dtparam=spi=on"));
        assert!(merged.ends_with(&format!("{END}\n")));
    }

    #[test]
    fn replaces_previous_block() {
        let first = merge(ORIGINAL, &block("[all]\ndtparam=spi=on\n")).unwrap();
        let second = merge(&first, &block("[all]\ndtparam=i2c_arm=on\n")).unwrap();
        assert!(second.contains("dtparam=i2c_arm=on"));
        assert!(!second.contains("dtparam=spi=on"));
        assert_eq!(second.matches(BEGIN).count(), 1);
        assert!(second.starts_with(ORIGINAL));
    }

    #[test]
    fn is_idempotent() {
        let body = block("[all]\ndtparam=spi=on\n");
        let once = merge(ORIGINAL, &body).unwrap();
        let twice = merge(&once, &body).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_content_after_the_block() {
        let with_tail = format!("{}{}# trailing user content\n", ORIGINAL, block("[all]\nx=1\n"));
        let merged = merge(&with_tail, &block("[all]\ny=2\n")).unwrap();
        assert!(merged.contains("# trailing user content"));
        assert!(merged.contains("y=2"));
        assert!(!merged.contains("x=1"));
        // The block always ends up last.
        let block_start = merged.find(BEGIN).unwrap();
        let tail_start = merged.find("# trailing user content").unwrap();
        assert!(tail_start < block_start);
    }

    #[test]
    fn handles_missing_trailing_newline() {
        let merged = merge("dtparam=audio=on", &block("[all]\nx=1\n")).unwrap();
        assert!(merged.starts_with("dtparam=audio=on\n"));
        assert!(merged.ends_with(&format!("{END}\n")));
    }

    #[test]
    fn handles_crlf_markers() {
        let existing = format!("a=1\r\n{BEGIN}\r\nold=1\r\n{END}\r\nb=2\r\n");
        let merged = merge(&existing, &block("[all]\nnew=1\n")).unwrap();
        assert!(!merged.contains("old=1"));
        assert!(merged.contains("new=1"));
        assert!(merged.contains("b=2"));
    }

    #[test]
    fn rejects_unbalanced_markers() {
        assert!(merge(&format!("a=1\n{BEGIN}\nx=1\n"), "block\n").is_err());
        assert!(merge(&format!("a=1\n{END}\n"), "block\n").is_err());
    }

    #[test]
    fn strip_removes_everything_generated() {
        let merged = merge(ORIGINAL, &block("[all]\nx=1\n")).unwrap();
        assert_eq!(strip(&merged).unwrap(), ORIGINAL);
    }

    #[test]
    fn empty_input_yields_only_the_block() {
        let body = block("[all]\nx=1\n");
        assert_eq!(merge("", &body).unwrap(), body);
    }
}
