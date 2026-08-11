//! Command line parsing.
//!
//! Hand written, because the workspace carries no dependencies and the
//! surface is small enough that a parser generator would cost more than it
//! saves.

use std::path::PathBuf;

pub const USAGE: &str = "\
rpi-provision - declarative first-boot provisioning for Raspberry Pi 5 SD cards

USAGE:
    rpi-provision <COMMAND> [OPTIONS]

COMMANDS:
    validate <SPEC>              Parse and validate a specification
    render   <SPEC> --out DIR    Write the generated files into a directory
    diff     <SPEC> --boot PATH  Show what apply would change
    apply    <SPEC> --boot PATH  Write the provisioning payload to a card
    revert   <SPEC> --boot PATH  Undo a previous apply
    detect                       List mounted Raspberry Pi boot partitions
    help                         Show this message
    version                      Show the version

OPTIONS:
    --boot PATH                  Mount point of the FAT boot partition
    --out DIR                    Output directory for `render`
    --set PATH=VALUE             Override a value in the specification.
                                 Repeatable. Note that the value is visible in
                                 the process list; use --set-secret instead for
                                 anything confidential.
    --set-secret PATH=SOURCE     Override a secret's source. SOURCE is one of
                                 env:NAME, file:PATH or value:LITERAL.
                                 Repeatable.
    -y, --yes                    Do not ask for confirmation before writing
    --allow-unverified-boot      Skip the boot-partition sanity check
    --ignore-conflicts           Proceed even if another first-boot mechanism
                                 (custom.toml, userconf.txt, ...) is present
    -q, --quiet                  Only report errors
    -v, --verbose                Show the full content of newly created files
    -h, --help                   Show this message

EXAMPLES:
    rpi-provision validate pi.toml
    rpi-provision diff pi.toml --boot /media/user/bootfs
    RPI_PASSWORD_HASH=\"$(openssl passwd -6)\" \\
        rpi-provision apply pi.toml --boot /media/user/bootfs --yes
";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Validate { spec: PathBuf },
    Render { spec: PathBuf, out: PathBuf },
    Diff { spec: PathBuf, boot: PathBuf },
    Apply { spec: PathBuf, boot: PathBuf },
    Revert { spec: PathBuf, boot: PathBuf },
    Detect,
    Help,
    Version,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub sets: Vec<String>,
    pub set_secrets: Vec<String>,
    pub yes: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub allow_unverified_boot: bool,
    pub ignore_conflicts: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub options: Options,
}

/// A usage error, reported with exit status 2.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn error<T>(message: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(message.into()))
}

pub fn parse<I, S>(arguments: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let arguments: Vec<String> = arguments.into_iter().map(Into::into).collect();
    let mut options = Options::default();
    let mut positional: Vec<String> = Vec::new();
    let mut boot: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;

    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        let mut take_value = |name: &str| -> Result<String, UsageError> {
            index += 1;
            match arguments.get(index) {
                Some(value) if !value.starts_with('-') || value.len() == 1 => Ok(value.clone()),
                _ => Err(UsageError(format!("`{name}` needs a value"))),
            }
        };

        match argument {
            "--boot" => boot = Some(PathBuf::from(take_value("--boot")?)),
            "--out" => out = Some(PathBuf::from(take_value("--out")?)),
            "--set" => options.sets.push(take_value("--set")?),
            "--set-secret" => options.set_secrets.push(take_value("--set-secret")?),
            "-y" | "--yes" => options.yes = true,
            "-q" | "--quiet" => options.quiet = true,
            "-v" | "--verbose" => options.verbose = true,
            "--allow-unverified-boot" => options.allow_unverified_boot = true,
            "--ignore-conflicts" => options.ignore_conflicts = true,
            "-h" | "--help" => {
                return Ok(Invocation { command: Command::Help, options: Options::default() })
            }
            "--version" | "-V" => {
                return Ok(Invocation { command: Command::Version, options: Options::default() })
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return error(format!("unknown option `{other}`"))
            }
            other => positional.push(other.to_string()),
        }
        index += 1;
    }

    let (verb, rest) = match positional.split_first() {
        Some((verb, rest)) => (verb.as_str(), rest),
        None => return Ok(Invocation { command: Command::Help, options }),
    };

    let spec = |rest: &[String]| -> Result<PathBuf, UsageError> {
        match rest {
            [one] => Ok(PathBuf::from(one)),
            [] => Err(UsageError(format!("`{verb}` needs a specification file"))),
            _ => Err(UsageError(format!(
                "`{verb}` takes one specification file, got {}",
                rest.len()
            ))),
        }
    };

    let command = match verb {
        "validate" => Command::Validate { spec: spec(rest)? },
        "render" => Command::Render {
            spec: spec(rest)?,
            out: out.ok_or_else(|| UsageError("`render` needs --out DIR".into()))?,
        },
        "diff" => Command::Diff {
            spec: spec(rest)?,
            boot: boot.ok_or_else(|| UsageError("`diff` needs --boot PATH".into()))?,
        },
        "apply" => Command::Apply {
            spec: spec(rest)?,
            boot: boot.ok_or_else(|| UsageError("`apply` needs --boot PATH".into()))?,
        },
        "revert" => Command::Revert {
            spec: spec(rest)?,
            boot: boot.ok_or_else(|| UsageError("`revert` needs --boot PATH".into()))?,
        },
        "detect" => {
            if !rest.is_empty() {
                return error("`detect` takes no arguments");
            }
            Command::Detect
        }
        "help" => Command::Help,
        "version" => Command::Version,
        other => return error(format!("unknown command `{other}`; run `rpi-provision help`")),
    };

    Ok(Invocation { command, options })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Invocation {
        parse(arguments.iter().copied()).unwrap()
    }

    #[test]
    fn no_arguments_shows_help() {
        assert_eq!(parsed(&[]).command, Command::Help);
    }

    #[test]
    fn parses_validate() {
        assert_eq!(
            parsed(&["validate", "pi.toml"]).command,
            Command::Validate { spec: PathBuf::from("pi.toml") }
        );
    }

    #[test]
    fn parses_apply_with_options() {
        let invocation = parsed(&[
            "apply",
            "pi.toml",
            "--boot",
            "/media/bootfs",
            "--yes",
            "--set",
            "system.hostname=other",
            "--set-secret",
            "user.password_hash=env:HASH",
        ]);
        assert_eq!(
            invocation.command,
            Command::Apply { spec: "pi.toml".into(), boot: "/media/bootfs".into() }
        );
        assert!(invocation.options.yes);
        assert_eq!(invocation.options.sets, vec!["system.hostname=other"]);
        assert_eq!(invocation.options.set_secrets, vec!["user.password_hash=env:HASH"]);
    }

    #[test]
    fn options_may_precede_the_command() {
        let invocation = parsed(&["--quiet", "validate", "pi.toml"]);
        assert!(invocation.options.quiet);
        assert_eq!(invocation.command, Command::Validate { spec: "pi.toml".into() });
    }

    #[test]
    fn repeated_sets_accumulate() {
        let invocation = parsed(&["validate", "pi.toml", "--set", "a=1", "--set", "b=2"]);
        assert_eq!(invocation.options.sets, vec!["a=1", "b=2"]);
    }

    #[test]
    fn apply_requires_boot() {
        assert!(parse(["apply", "pi.toml"]).is_err());
    }

    #[test]
    fn render_requires_out() {
        assert!(parse(["render", "pi.toml"]).is_err());
    }

    #[test]
    fn rejects_unknown_option_and_command() {
        assert!(parse(["validate", "pi.toml", "--nope"]).is_err());
        assert!(parse(["frobnicate"]).is_err());
    }

    #[test]
    fn rejects_a_missing_option_value() {
        let err = parse(["apply", "pi.toml", "--boot"]).unwrap_err();
        assert!(err.0.contains("needs a value"));
    }

    #[test]
    fn rejects_extra_positionals() {
        let err = parse(["validate", "a.toml", "b.toml"]).unwrap_err();
        assert!(err.0.contains("takes one specification file"));
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parsed(&["apply", "--help"]).command, Command::Help);
        assert_eq!(parsed(&["--version"]).command, Command::Version);
        assert_eq!(parsed(&["version"]).command, Command::Version);
    }
}
