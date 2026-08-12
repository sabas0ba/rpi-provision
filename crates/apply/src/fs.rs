//! The boot partition abstraction.
//!
//! Everything the tool writes lives below a single directory: the mount point
//! of the FAT boot partition. Modelling that as a trait keeps the merge and
//! diff logic testable without touching a real card.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

/// A writable view of a boot partition.
pub trait BootFs {
    fn exists(&self, relative: &str) -> bool;
    fn read(&self, relative: &str) -> io::Result<Vec<u8>>;
    fn write(&mut self, relative: &str, contents: &[u8], executable: bool) -> io::Result<()>;
    fn remove(&mut self, relative: &str) -> io::Result<()>;
    /// Every file below the root, as relative `/`-separated paths, sorted.
    ///
    /// Directories are not listed in their own right: an empty one carries no
    /// information that a snapshot needs to preserve.
    fn list(&self) -> io::Result<Vec<String>>;
    /// A description used in messages, e.g. the mount point.
    fn describe(&self) -> String;
}

/// Reject anything that could escape the boot partition.
pub fn validate_relative(relative: &str) -> io::Result<PathBuf> {
    if relative.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty path"));
    }
    if relative.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{relative}` must use `/` as the separator"),
        ));
    }
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{relative}` must be relative"),
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{relative}` must not contain `.` or `..`"),
                ))
            }
        }
    }
    Ok(path.to_path_buf())
}

/// A real directory, normally the mounted boot partition.
pub struct RealBootFs {
    root: PathBuf,
}

impl RealBootFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, relative: &str) -> io::Result<PathBuf> {
        Ok(self.root.join(validate_relative(relative)?))
    }
}

impl BootFs for RealBootFs {
    fn exists(&self, relative: &str) -> bool {
        self.resolve(relative).map(|path| path.exists()).unwrap_or(false)
    }

    fn read(&self, relative: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.resolve(relative)?)
    }

    fn write(&mut self, relative: &str, contents: &[u8], executable: bool) -> io::Result<()> {
        let path = self.resolve(relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
        if executable {
            set_executable(&path)?;
        }
        Ok(())
    }

    fn remove(&mut self, relative: &str) -> io::Result<()> {
        let path = self.resolve(relative)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        }
        // Prune directories the tool created, so that reverting leaves the
        // boot partition as it was found. Stops at the first non-empty one.
        let mut parent = path.parent().map(Path::to_path_buf);
        while let Some(directory) = parent {
            if directory == self.root || !directory.starts_with(&self.root) {
                break;
            }
            if std::fs::remove_dir(&directory).is_err() {
                break;
            }
            parent = directory.parent().map(Path::to_path_buf);
        }
        Ok(())
    }

    fn list(&self) -> io::Result<Vec<String>> {
        let mut found = Vec::new();
        if self.root.is_dir() {
            walk(&self.root, &mut String::new(), &mut found)?;
        }
        found.sort();
        Ok(found)
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }
}

/// Recursive listing, depth first, with `/` as the separator on every
/// platform so that a snapshot taken on Windows restores on Linux.
fn walk(directory: &Path, prefix: &mut String, found: &mut Vec<String>) -> io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} has a name that is not UTF-8", entry.path().display()),
            ));
        };
        let relative =
            if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        // `file_type` does not follow symlinks; a link is copied as whatever
        // it points at rather than being skipped, which is what a snapshot of
        // a FAT partition wants (FAT has no links, but a rendered directory
        // on an ext4 host might).
        if entry.metadata()?.is_dir() {
            let mut nested = relative;
            walk(&entry.path(), &mut nested, found)?;
        } else {
            found.push(relative);
        }
    }
    Ok(())
}

/// FAT has no permission bits, so this is a no-op there; it matters when the
/// plan is materialised into an ordinary directory for inspection.
#[cfg(unix)]
fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// An in-memory boot partition, used by the tests.
#[derive(Debug, Default, Clone)]
pub struct MemBootFs {
    pub files: BTreeMap<String, Vec<u8>>,
    pub executable: BTreeMap<String, bool>,
}

impl MemBootFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Populate with the files a freshly written Raspberry Pi OS card has.
    pub fn raspberry_pi_os(config_txt: &str, cmdline_txt: &str) -> Self {
        let mut fs = Self::new();
        fs.put("config.txt", config_txt);
        fs.put("cmdline.txt", cmdline_txt);
        fs.put("bcm2712-rpi-5-b.dtb", "\0\0\0\0");
        fs.put("kernel_2712.img", "\0\0\0\0");
        fs
    }

    pub fn put(&mut self, relative: &str, contents: &str) {
        self.files.insert(relative.to_string(), contents.as_bytes().to_vec());
    }

    pub fn text(&self, relative: &str) -> Option<String> {
        self.files.get(relative).map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn paths(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }
}

impl BootFs for MemBootFs {
    fn exists(&self, relative: &str) -> bool {
        self.files.contains_key(relative)
    }

    fn read(&self, relative: &str) -> io::Result<Vec<u8>> {
        validate_relative(relative)?;
        self.files
            .get(relative)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{relative} not found")))
    }

    fn write(&mut self, relative: &str, contents: &[u8], executable: bool) -> io::Result<()> {
        validate_relative(relative)?;
        self.files.insert(relative.to_string(), contents.to_vec());
        self.executable.insert(relative.to_string(), executable);
        Ok(())
    }

    fn remove(&mut self, relative: &str) -> io::Result<()> {
        validate_relative(relative)?;
        self.files.remove(relative);
        self.executable.remove(relative);
        Ok(())
    }

    fn list(&self) -> io::Result<Vec<String>> {
        // A BTreeMap is already in sorted order.
        Ok(self.files.keys().cloned().collect())
    }

    fn describe(&self) -> String {
        "<in-memory boot partition>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        assert!(validate_relative("../etc/passwd").is_err());
        assert!(validate_relative("/etc/passwd").is_err());
        assert!(validate_relative("a/../../b").is_err());
        assert!(validate_relative("a\\b").is_err());
        assert!(validate_relative("").is_err());
        assert!(validate_relative("rpi-provision/steps/10-hostname.sh").is_ok());
    }

    #[test]
    fn memory_filesystem_round_trips() {
        let mut fs = MemBootFs::new();
        assert!(!fs.exists("a.txt"));
        fs.write("a.txt", b"hello", false).unwrap();
        assert!(fs.exists("a.txt"));
        assert_eq!(fs.read("a.txt").unwrap(), b"hello");
        fs.remove("a.txt").unwrap();
        assert!(!fs.exists("a.txt"));
        // Removing something absent is not an error.
        fs.remove("a.txt").unwrap();
    }

    #[test]
    fn memory_filesystem_enforces_path_rules() {
        let mut fs = MemBootFs::new();
        assert!(fs.write("../escape", b"x", false).is_err());
    }
}
