//! The `vario` command: install, remove, check and restore Vario's integrations.

mod commands;
mod ui;

use std::process::ExitCode;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};
use vario_core::clients::{Env, integrations};
use vario_core::paths;

use crate::commands::Context;
use crate::ui::Ui;

#[derive(Parser)]
#[command(
    version,
    about = "Delegate your AI agents' simple tasks to cheaper models."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Configure the AI clients found on this machine.
    Install {
        /// Choose the clients one by one.
        #[arg(long)]
        manual: bool,
        /// Configure this client, even if it is not detected (repeatable).
        #[arg(long = "client", value_name = "NAME", value_parser = client_names())]
        clients: Vec<String>,
        /// Show the changes without making them.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove Vario from every client.
    Remove {
        #[arg(long)]
        dry_run: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Show what is installed, and repair it with --fix.
    Doctor {
        /// Reinstall what is missing in the clients you chose.
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Put back the files as they were before a change by Vario.
    Restore {
        /// Backup folder, by name or path; the most recent one by default.
        backup: Option<String>,
        /// List the backups instead.
        #[arg(long, conflicts_with = "backup")]
        list: bool,
        #[arg(short, long)]
        yes: bool,
    },
}

fn client_names() -> PossibleValuesParser {
    PossibleValuesParser::new(
        integrations()
            .into_iter()
            .map(|integration| integration.name),
    )
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut ctx = Context {
        env: Env::current(),
        data_dir: paths::data_dir(),
        ui: Ui::stdio(),
    };
    let result = match cli.command {
        Command::Install {
            manual,
            clients,
            dry_run,
            yes,
        } => commands::install(&mut ctx, manual, &clients, dry_run, yes),
        Command::Remove { dry_run, yes } => commands::remove(&mut ctx, dry_run, yes),
        Command::Doctor { fix, dry_run, yes } => commands::doctor(&mut ctx, fix, dry_run, yes),
        Command::Restore { backup, list, yes } => {
            commands::restore(&mut ctx, backup.as_deref(), list, yes)
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("vario: {err}");
            ExitCode::FAILURE
        }
    }
}
