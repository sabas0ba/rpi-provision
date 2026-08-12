#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop front end for `rpi-provision`.
//!
//! Every operation here goes through the same three crates the command line
//! uses, so the two cannot disagree about what a specification means. The
//! window is granted no filesystem or shell plugin: the commands below are
//! the whole of what the page can ask the host to do.
//!
//! The specification text is the single source of truth. The form does not
//! hold a parallel model of it — each control edits the document through
//! `toml_edit`, which preserves the comments and layout of a file that is
//! meant to stay in version control.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rpi_provision_apply::{
    backup, conflicting_first_boot_files, detect, execute, plan_changes, revert_plan,
    verify_boot_partition, verify_boot_partition_shape, RealBootFs,
};
use rpi_provision_render::{render, Plan, GENERATOR};
use rpi_provision_spec::{load_str, LoadOptions, Loaded, SecretProvider, SystemSecrets};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ secrets

/// Secrets typed into the window, falling back to the real environment.
///
/// Everything that is not a secret — `[[files]]` assets, `file =` sources —
/// is read from the real filesystem, exactly as the command line reads it.
struct GuiSecrets {
    overrides: BTreeMap<String, String>,
}

impl SecretProvider for GuiSecrets {
    fn env(&self, name: &str) -> Option<String> {
        match self.overrides.get(name) {
            Some(value) if !value.is_empty() => Some(value.clone()),
            _ => SystemSecrets.env(name),
        }
    }

    fn read_file(&self, path: &Path) -> std::io::Result<String> {
        SystemSecrets.read_file(path)
    }

    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        SystemSecrets.read_bytes(path)
    }

    fn list_dir(&self, path: &Path) -> std::io::Result<Option<Vec<PathBuf>>> {
        SystemSecrets.list_dir(path)
    }
}

// ------------------------------------------------------------------- shapes

#[derive(Serialize)]
struct Card {
    path: String,
    model: String,
}

#[derive(Serialize, Default)]
struct Summary {
    hostname: String,
    user: String,
    target: String,
    digest: String,
    ssh: String,
    network: String,
    hardware: String,
    files: usize,
    run: usize,
}

#[derive(Serialize, Default)]
struct Validation {
    ok: bool,
    error: Option<String>,
    warnings: Vec<String>,
    summary: Option<Summary>,
}

#[derive(Serialize)]
struct ChangeRow {
    kind: String,
    path: String,
    diff: Option<String>,
    sensitive: bool,
}

#[derive(Serialize, Default)]
struct Preview {
    changes: Vec<ChangeRow>,
    created: usize,
    updated: usize,
    unchanged: usize,
    deleted: usize,
    conflicts: Vec<String>,
}

/// What the window sends for anything that needs a loaded specification.
#[derive(Deserialize)]
struct SpecInput {
    text: String,
    /// Directory relative paths in the specification resolve against, which
    /// is the directory the file was opened from.
    base_dir: String,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

type Reply<T> = std::result::Result<T, String>;

impl SpecInput {
    fn load(&self) -> Reply<Loaded> {
        let provider = GuiSecrets { overrides: self.secrets.clone() };
        let base = if self.base_dir.is_empty() { "." } else { self.base_dir.as_str() };
        let options = LoadOptions::new(&provider).base_dir(base);
        load_str(&self.text, &options).map_err(|error| error.to_string())
    }

    fn plan(&self) -> Reply<Plan> {
        let loaded = self.load()?;
        Ok(render(&loaded.spec, &loaded.digest))
    }
}

fn summarise(loaded: &Loaded) -> Summary {
    let spec = &loaded.spec;
    Summary {
        hostname: spec.system.hostname.clone(),
        user: spec.user.name.clone(),
        target: spec.meta.target.as_str().to_string(),
        digest: loaded.digest.clone(),
        ssh: format!(
            "{}, password auth {}, {} key(s)",
            if spec.ssh.enabled { "enabled" } else { "disabled" },
            if spec.ssh.password_authentication { "on" } else { "off" },
            spec.user.authorized_keys.len()
        ),
        network: format!(
            "{} wired, {} wireless, gadget {}",
            spec.network.ethernet.len(),
            spec.network.wifi.len(),
            if spec.network.usb_gadget.is_some() { "on" } else { "off" }
        ),
        hardware: {
            let mut on = Vec::new();
            if spec.hardware.uart.enabled {
                on.push("uart");
            }
            if spec.hardware.i2c.enabled {
                on.push("i2c");
            }
            if spec.hardware.spi.enabled {
                on.push("spi");
            }
            if spec.hardware.one_wire.enabled {
                on.push("1-wire");
            }
            if on.is_empty() {
                "none".to_string()
            } else {
                on.join(", ")
            }
        },
        files: spec.files.len(),
        run: spec.run.len(),
    }
}

// ----------------------------------------------------------------- commands

#[tauri::command]
fn generator() -> String {
    GENERATOR.to_string()
}

#[tauri::command]
fn detect_cards() -> Vec<Card> {
    detect::candidates()
        .into_iter()
        .map(|candidate| Card {
            path: candidate.path.display().to_string(),
            model: candidate.model.unwrap_or("unknown model").to_string(),
        })
        .collect()
}

#[tauri::command]
fn read_spec(path: String) -> Reply<String> {
    std::fs::read_to_string(&path).map_err(|err| format!("cannot read `{path}`: {err}"))
}

#[tauri::command]
fn write_spec(path: String, text: String) -> Reply<()> {
    std::fs::write(&path, text).map_err(|err| format!("cannot write `{path}`: {err}"))
}

#[tauri::command]
fn validate(input: SpecInput) -> Validation {
    match input.load() {
        Ok(loaded) => Validation {
            ok: true,
            error: None,
            warnings: loaded.warnings.clone(),
            summary: Some(summarise(&loaded)),
        },
        Err(error) => Validation { ok: false, error: Some(error), ..Validation::default() },
    }
}

/// Set or clear one key, keeping the rest of the document as it was written.
#[tauri::command]
fn set_value(text: String, path: String, value: serde_json::Value) -> Reply<String> {
    use toml_edit::{DocumentMut, Item, Value as TomlValue};

    let mut document: DocumentMut =
        text.parse().map_err(|err| format!("the specification does not parse: {err}"))?;

    let keys: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = keys.split_last() else {
        return Err("an empty key path".to_string());
    };

    // Clearing a key removes it rather than writing an empty string, so that
    // the default documented in the reference is what applies.
    let clearing = matches!(&value, serde_json::Value::String(text) if text.is_empty());

    let mut table = document.as_table_mut();
    for key in parents {
        if clearing && !table.contains_key(key) {
            return Ok(document.to_string());
        }
        let entry = table.entry(key).or_insert_with(|| Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_mut()
            .ok_or_else(|| format!("`{key}` is not a table, so `{path}` cannot be set"))?;
    }

    if clearing {
        table.remove(last);
        return Ok(document.to_string());
    }

    let assigned: TomlValue = match value {
        serde_json::Value::String(text) => text.into(),
        serde_json::Value::Bool(flag) => flag.into(),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(integer) => integer.into(),
            None => return Err(format!("`{path}`: only whole numbers are supported")),
        },
        serde_json::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                match item {
                    serde_json::Value::String(text) => array.push(text),
                    other => return Err(format!("`{path}`: cannot store {other} in a list")),
                }
            }
            TomlValue::Array(array)
        }
        other => return Err(format!("`{path}`: unsupported value {other}")),
    };
    // Replacing the item outright would take the whitespace and any trailing
    // comment with it, so an existing value is overwritten in place and keeps
    // its decoration.
    match table.get_mut(last).and_then(Item::as_value_mut) {
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = assigned;
            *existing.decor_mut() = decor;
        }
        None => {
            table.insert(last, Item::Value(assigned));
        }
    }
    Ok(document.to_string())
}

/// Read a set of keys out of a document, for populating the form.
#[tauri::command]
fn get_values(text: String, paths: Vec<String>) -> Reply<BTreeMap<String, serde_json::Value>> {
    use toml_edit::{DocumentMut, Value as TomlValue};

    let document: DocumentMut =
        text.parse().map_err(|err| format!("the specification does not parse: {err}"))?;

    let mut found = BTreeMap::new();
    for path in paths {
        let mut item = document.as_item();
        for key in path.split('.') {
            match item.get(key) {
                Some(next) => item = next,
                None => {
                    item = &toml_edit::Item::None;
                    break;
                }
            }
        }
        let value = match item.as_value() {
            Some(TomlValue::String(text)) => serde_json::Value::from(text.value().as_str()),
            Some(TomlValue::Integer(number)) => serde_json::Value::from(*number.value()),
            Some(TomlValue::Boolean(flag)) => serde_json::Value::from(*flag.value()),
            Some(TomlValue::Array(items)) => serde_json::Value::Array(
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(serde_json::Value::from)
                    .collect(),
            ),
            _ => serde_json::Value::Null,
        };
        found.insert(path, value);
    }
    Ok(found)
}

#[derive(Deserialize)]
struct CardInput {
    #[serde(flatten)]
    spec: SpecInput,
    boot: String,
}

fn open_card(boot: &str, plan: Option<&Plan>) -> Reply<RealBootFs> {
    let path = PathBuf::from(boot);
    if !path.is_dir() {
        return Err(format!("`{boot}` is not a directory"));
    }
    let fs = RealBootFs::new(&path);
    let checked = match plan {
        Some(plan) => verify_boot_partition(&fs, &plan.target_dtb),
        None => verify_boot_partition_shape(&fs),
    };
    checked.map_err(|error| error.to_string())?;
    Ok(fs)
}

#[tauri::command]
fn preview(input: CardInput) -> Reply<Preview> {
    let plan = input.spec.plan()?;
    let fs = open_card(&input.boot, Some(&plan))?;
    let changes = plan_changes(&plan, &fs).map_err(|error| error.to_string())?;

    let mut preview = Preview {
        conflicts: conflicting_first_boot_files(&fs)
            .into_iter()
            .map(|(name, what)| format!("{name} ({what})"))
            .collect(),
        ..Preview::default()
    };
    for change in changes {
        use rpi_provision_apply::ChangeKind::*;
        match change.kind {
            Create => preview.created += 1,
            Update => preview.updated += 1,
            Unchanged | AlreadyAbsent => preview.unchanged += 1,
            Delete => preview.deleted += 1,
        }
        preview.changes.push(ChangeRow {
            kind: change.kind.label().to_string(),
            path: change.path,
            diff: change.diff,
            sensitive: change.sensitive,
        });
    }
    Ok(preview)
}

#[derive(Deserialize)]
struct ApplyInput {
    #[serde(flatten)]
    card: CardInput,
    /// Snapshot the partition into this directory first. Empty to skip.
    #[serde(default)]
    backup_into: String,
}

#[tauri::command]
fn apply_spec(input: ApplyInput) -> Reply<String> {
    let plan = input.card.spec.plan()?;
    let mut fs = open_card(&input.card.boot, Some(&plan))?;

    if !input.backup_into.is_empty() {
        snapshot(&fs, &input.backup_into)?;
    }

    let summary = execute(&plan, &mut fs).map_err(|error| error.to_string())?;
    Ok(summary.to_string())
}

#[tauri::command]
fn revert_spec(input: CardInput) -> Reply<String> {
    let plan = input.spec.plan()?;
    let mut fs = open_card(&input.boot, Some(&plan))?;
    let summary = execute(&revert_plan(&plan), &mut fs).map_err(|error| error.to_string())?;
    Ok(summary.to_string())
}

fn snapshot(source: &RealBootFs, out: &str) -> Reply<String> {
    let destination = PathBuf::from(out);
    if destination.exists() && !destination.is_dir() {
        return Err(format!("`{out}` exists and is not a directory"));
    }
    std::fs::create_dir_all(&destination).map_err(|err| format!("cannot create `{out}`: {err}"))?;
    let manifest = backup::create(
        source,
        &mut RealBootFs::new(&destination),
        GENERATOR,
        // The window has no clock of its own to offer; the host's is fine.
        &now_utc(),
    )
    .map_err(|error| error.to_string())?;
    Ok(format!("{} file(s), {} bytes", manifest.entries.len(), manifest.total_bytes()))
}

#[tauri::command]
fn backup_card(boot: String, out: String) -> Reply<String> {
    let fs = open_card(&boot, None)?;
    snapshot(&fs, &out)
}

#[tauri::command]
fn restore_card(boot: String, from: String) -> Reply<String> {
    let source = RealBootFs::new(PathBuf::from(&from));
    let manifest = backup::read_manifest(&source).map_err(|error| error.to_string())?;
    let mut target = open_card(&boot, None)?;
    let changes =
        backup::restore_changes(&source, &manifest, &target).map_err(|error| error.to_string())?;
    let summary =
        backup::restore(&source, &changes, &mut target).map_err(|error| error.to_string())?;
    Ok(summary.to_string())
}

/// Describe a snapshot without putting it back.
#[tauri::command]
fn inspect_snapshot(from: String) -> Reply<String> {
    let source = RealBootFs::new(PathBuf::from(&from));
    let manifest = backup::read_manifest(&source).map_err(|error| error.to_string())?;
    Ok(format!(
        "{} file(s), {} bytes, taken {} from {}",
        manifest.entries.len(),
        manifest.total_bytes(),
        manifest.created,
        manifest.source
    ))
}

fn now_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            generator,
            detect_cards,
            read_spec,
            write_spec,
            validate,
            set_value,
            get_values,
            preview,
            apply_spec,
            revert_spec,
            backup_card,
            restore_card,
            inspect_snapshot,
        ])
        .run(tauri::generate_context!())
        .expect("the application must start");
}

#[cfg(test)]
mod tests;
