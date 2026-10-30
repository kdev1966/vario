//! The `vario` command: install, remove, check and restore Vario's integrations.

mod commands;
mod ui;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;

use clap::builder::PossibleValuesParser;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use vario_core::clients::{Env, integrations};
use vario_core::hooks::{self, Event, Via};
use vario_core::lang::Lang;
use vario_core::{parts, paths, statusline};

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
        /// Show the changes without making them.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Show what is installed, and repair it with --fix.
    Doctor {
        /// Reinstall what is missing in the clients you chose.
        #[arg(long)]
        fix: bool,
        /// With --fix: show the changes without making them.
        #[arg(long)]
        dry_run: bool,
        /// With --fix: do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Summarize the delegations journaled by the hooks.
    Report,
    /// Show the quota, the thermostat and the tokens used, from local logs.
    Usage {
        /// Print the last 7 days' tokens as JSON, in all and per working directory, for other tools.
        #[arg(long)]
        json: bool,
    },
    /// Put back the files as they were before a change by Vario.
    Restore {
        /// Backup folder, by name or path; the most recent one by default.
        backup: Option<String>,
        /// List the backups instead.
        #[arg(long, conflicts_with = "backup")]
        list: bool,
        /// Do not ask for confirmation.
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

/// The command line with its help in `lang`. Clap's own words ("Usage", "Options")
/// stay in English.
fn command_line(lang: Lang) -> clap::Command {
    let command = Cli::command();
    if lang == Lang::En {
        return command;
    }
    let dry_run = "Afficher les changements sans les faire.";
    let yes = "Ne pas demander de confirmation.";
    command
        .about("Confiez les tâches simples de vos agents IA à des modèles moins chers.")
        .mut_subcommand("install", |sub| {
            sub.about("Configurer les clients IA trouvés sur cette machine.")
                .mut_arg("manual", |arg| arg.help("Choisir les clients un par un."))
                .mut_arg("clients", |arg| {
                    arg.help("Configurer ce client, même s'il n'est pas détecté (répétable).")
                })
                .mut_arg("dry_run", |arg| arg.help(dry_run))
                .mut_arg("yes", |arg| arg.help(yes))
        })
        .mut_subcommand("remove", |sub| {
            sub.about("Retirer Vario de tous les clients.")
                .mut_arg("dry_run", |arg| arg.help(dry_run))
                .mut_arg("yes", |arg| arg.help(yes))
        })
        .mut_subcommand("doctor", |sub| {
            sub.about("Afficher ce qui est installé, et le réparer avec --fix.")
                .mut_arg("fix", |arg| {
                    arg.help("Réinstaller ce qui manque dans les clients choisis.")
                })
                .mut_arg("dry_run", |arg| {
                    arg.help("Avec --fix : afficher les changements sans les faire.")
                })
                .mut_arg("yes", |arg| {
                    arg.help("Avec --fix : ne pas demander de confirmation.")
                })
        })
        .mut_subcommand("report", |sub| {
            sub.about("Résumer les délégations notées par les hooks.")
        })
        .mut_subcommand("usage", |sub| {
            sub.about(
                "Afficher le quota, le thermostat et les tokens consommés, d'après les journaux locaux.",
            )
            .mut_arg("json", |arg| {
                arg.help("Donner les tokens des 7 derniers jours en JSON, au total et par dossier de travail, pour d'autres outils.")
            })
        })
        .mut_subcommand("restore", |sub| {
            sub.about("Remettre les fichiers comme avant un changement fait par Vario.")
                .mut_arg("backup", |arg| {
                    arg.help(
                        "Dossier de sauvegarde, par nom ou chemin ; le plus récent par défaut.",
                    )
                })
                .mut_arg("list", |arg| arg.help("Lister les sauvegardes."))
                .mut_arg("yes", |arg| arg.help(yes))
        })
}

/// `vario hook <event> --via <client>`, run by the clients. It is handled before the
/// command-line parser: a parse error exits with status 2, which Claude Code reads as
/// "keep the sub-agent running". Whatever happens, a hook exits with status 0.
fn hook(args: &[OsString]) -> ExitCode {
    let arg = |index: usize| args.get(index).and_then(|arg| arg.to_str());
    let event = arg(2).and_then(Event::parse);
    let via = args
        .iter()
        .position(|arg| arg == "--via")
        .and_then(|index| arg(index + 1))
        .and_then(Via::parse);
    let (Some(event), Some(via)) = (event, via) else {
        return ExitCode::SUCCESS;
    };
    let mut input = String::new();
    if !io::stdin().is_terminal() {
        let _ = io::stdin().take(4 << 20).read_to_string(&mut input);
    }
    let output = hooks::run(event, via, &input, &Env::current(), &paths::data_dir());
    let mut stdout = io::stdout();
    let _ = stdout.write_all(output.as_bytes());
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

/// `vario statusline [--then <hex>]`, Claude Code's status line. Like the hooks, it
/// is handled before the parser and always exits with status 0.
fn status_line(args: &[OsString]) -> ExitCode {
    let wrapped = args
        .iter()
        .position(|arg| arg == "--then")
        .and_then(|index| args.get(index + 1))
        .and_then(|hex| hex.to_str())
        .and_then(parts::unwrap_status_line);
    let mut input = String::new();
    if !io::stdin().is_terminal() {
        let _ = io::stdin().take(1 << 20).read_to_string(&mut input);
    }
    let env = Env::current();
    let line = statusline::run(
        &input,
        wrapped.as_deref(),
        &paths::data_dir(),
        Lang::detect(&env),
    );
    let mut stdout = io::stdout();
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "hook") {
        return hook(&args);
    }
    if args.get(1).is_some_and(|arg| arg == "statusline") {
        return status_line(&args);
    }
    let env = Env::current();
    let lang = Lang::detect(&env);
    let cli =
        Cli::from_arg_matches(&command_line(lang).get_matches()).unwrap_or_else(|err| err.exit());
    let mut ctx = Context {
        env,
        data_dir: paths::data_dir(),
        ui: Ui::stdio(lang),
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
        Command::Report => commands::report(&mut ctx),
        Command::Usage { json } => commands::usage(&mut ctx, json),
        Command::Restore { backup, list, yes } => {
            commands::restore(&mut ctx, backup.as_deref(), list, yes)
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("vario: {}", err.text(lang));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_valid_in_every_language() {
        for lang in [Lang::En, Lang::Fr] {
            command_line(lang).debug_assert();
        }
    }
}
