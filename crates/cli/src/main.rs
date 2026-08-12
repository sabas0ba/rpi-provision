//! `rpi-provision` command line entry point.
//!
//! Exit status: 0 success, 1 failure, 2 usage error.

mod args;
mod commands;

use args::{Command, Invocation, USAGE};

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let Invocation { command, options } = match args::parse(arguments) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("rpi-provision: {error}");
            eprintln!("run `rpi-provision help` for usage");
            return std::process::ExitCode::from(2);
        }
    };

    let outcome = match &command {
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("{}", rpi_provision_render::GENERATOR);
            Ok(())
        }
        Command::Validate { spec } => commands::validate(spec, &options),
        Command::Render { spec, out } => commands::render_to_directory(spec, out, &options),
        Command::Diff { spec, boot } => commands::diff(spec, boot, &options),
        Command::Apply { spec, boot } => commands::apply(spec, boot, &options),
        Command::Revert { spec, boot } => commands::revert(spec, boot, &options),
        Command::Backup { boot, out } => commands::backup(boot, out, &options),
        Command::Restore { boot, from } => commands::restore(boot, from, &options),
        Command::Detect => commands::detect(&options),
    };

    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(commands::Failure(message)) => {
            eprintln!("rpi-provision: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}
