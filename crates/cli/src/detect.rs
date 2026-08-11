//! Locate mounted Raspberry Pi boot partitions.
//!
//! This is a convenience for `rpi-provision detect`; every command that
//! writes still takes an explicit `--boot` path, because guessing which card
//! to modify is not a decision a tool should make.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    /// Present when the device tree blob identifies the model.
    pub model: Option<&'static str>,
}

const MODELS: [(&str, &str); 4] = [
    ("bcm2712-rpi-5-b.dtb", "Raspberry Pi 5"),
    ("bcm2711-rpi-4-b.dtb", "Raspberry Pi 4"),
    ("bcm2710-rpi-3-b-plus.dtb", "Raspberry Pi 3 B+"),
    ("bcm2710-rpi-zero-2-w.dtb", "Raspberry Pi Zero 2 W"),
];

/// Classify a directory: `None` when it is not a boot partition.
pub fn inspect(path: &Path) -> Option<Candidate> {
    if !path.join("config.txt").is_file() || !path.join("cmdline.txt").is_file() {
        return None;
    }
    let model = MODELS
        .iter()
        .find(|(blob, _)| path.join(blob).exists())
        .map(|(_, name)| *name);
    Some(Candidate { path: path.to_path_buf(), model })
}

/// Enumerate plausible boot partitions on this host.
pub fn candidates() -> Vec<Candidate> {
    let mut found: Vec<Candidate> = mount_points()
        .iter()
        .filter_map(|path| inspect(path))
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

#[cfg(target_os = "linux")]
fn mount_points() -> Vec<PathBuf> {
    // /proc/mounts escapes spaces and tabs as octal sequences.
    let Ok(contents) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let mount = fields.next()?;
            let filesystem = fields.next()?;
            matches!(filesystem, "vfat" | "msdos" | "exfat").then(|| PathBuf::from(unescape(mount)))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &text[index + 1..index + 4];
            if let Ok(code) = u8::from_str_radix(digits, 8) {
                out.push(code as char);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

#[cfg(target_os = "windows")]
fn mount_points() -> Vec<PathBuf> {
    ('C'..='Z')
        .map(|letter| PathBuf::from(format!("{letter}:\\")))
        .filter(|path| path.join("config.txt").exists())
        .collect()
}

#[cfg(target_os = "macos")]
fn mount_points() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    entries.filter_map(|entry| Some(entry.ok()?.path())).collect()
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn mount_points() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rpi-provision-detect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn recognises_a_pi5_boot_partition() {
        let path = scratch("pi5");
        std::fs::write(path.join("config.txt"), "").unwrap();
        std::fs::write(path.join("cmdline.txt"), "").unwrap();
        std::fs::write(path.join("bcm2712-rpi-5-b.dtb"), "").unwrap();

        let candidate = inspect(&path).unwrap();
        assert_eq!(candidate.model, Some("Raspberry Pi 5"));
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn reports_an_unknown_model_without_a_blob() {
        let path = scratch("unknown");
        std::fs::write(path.join("config.txt"), "").unwrap();
        std::fs::write(path.join("cmdline.txt"), "").unwrap();

        assert_eq!(inspect(&path).unwrap().model, None);
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn ignores_an_unrelated_directory() {
        let path = scratch("unrelated");
        std::fs::write(path.join("notes.txt"), "").unwrap();
        assert!(inspect(&path).is_none());
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unescapes_octal_sequences_in_mount_points() {
        assert_eq!(unescape("/media/user/BOOT"), "/media/user/BOOT");
        assert_eq!(unescape("/media/my\\040card"), "/media/my card");
    }

    #[test]
    fn enumeration_does_not_panic() {
        let _ = candidates();
    }
}
