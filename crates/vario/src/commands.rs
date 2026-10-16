//! The subcommands. Each returns `Ok(true)` on success and `Ok(false)` when it was
//! cancelled or a client could not be configured, which exits with status 1.
//!
//! Nothing is changed without a preview first, and without either a confirmation in a
//! terminal or `--yes`.

use std::path::{Path, PathBuf};

use vario_core::Result;
use vario_core::choices::Choices;
use vario_core::clients::{Env, Integration, integrations};
use vario_core::files::{self, Session};
use vario_core::handoff::{self, State};
use vario_core::install::{self, Outcome};
use vario_core::parts::Kind;

use crate::ui::Ui;

pub struct Context {
    pub env: Env,
    pub data_dir: PathBuf,
    pub ui: Ui,
}

pub fn install(
    ctx: &mut Context,
    manual: bool,
    names: &[String],
    dry_run: bool,
    yes: bool,
) -> Result<bool> {
    let explicit = !names.is_empty();
    let candidates: Vec<Integration> = if explicit {
        integrations()
            .into_iter()
            .filter(|integration| names.iter().any(|name| name == integration.name))
            .collect()
    } else {
        install::detected(&ctx.env)
    };
    if candidates.is_empty() {
        let supported = labels(&integrations());
        ctx.ui.say(format!(
            "No supported AI client was found on this machine (supported: {supported})."
        ));
        return Ok(true);
    }
    if !explicit {
        ctx.ui
            .say(format!("AI clients found: {}", labels(&candidates)));
    }

    let mut chosen = Vec::new();
    let mut declined = Vec::new();
    if manual && ctx.ui.interactive && !yes && !dry_run {
        for integration in candidates {
            if ctx
                .ui
                .confirm(&format!("Configure {}?", integration.label), true)
            {
                chosen.push(integration);
            } else {
                declined.push(integration.name.to_string());
            }
        }
    } else {
        chosen = candidates;
    }
    if chosen.is_empty() {
        remember(ctx, &[], &declined)?;
        ctx.ui.say("Nothing was changed.");
        return Ok(true);
    }

    let planned = preview(ctx, |session, env| install::apply(session, &chosen, env));
    if is_empty(&planned) {
        ctx.ui.say("Everything is already in place.");
        if !dry_run {
            remember(ctx, &names_of(&chosen), &declined)?;
            suggest_handoff(ctx, &chosen)?;
        }
        return Ok(true);
    }
    show(
        &mut ctx.ui,
        &ctx.env.home,
        "Planned changes (each file is backed up first):",
        &planned,
    );
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(
                "No terminal to confirm: nothing was changed. Run 'vario install --yes' to apply.",
            );
            return Ok(true);
        }
        if !ctx.ui.confirm("Apply these changes?", true) {
            ctx.ui.say("Cancelled.");
            return Ok(false);
        }
    }

    let mut session = Session::in_data_dir(false, &ctx.data_dir);
    let outcomes = install::apply(&mut session, &chosen, &ctx.env);
    done(&mut ctx.ui, &ctx.env.home, &outcomes, &session);
    remember(ctx, &names_of(&chosen), &declined)?;
    suggest_handoff(ctx, &chosen)?;
    Ok(succeeded(&outcomes))
}

pub fn remove(ctx: &mut Context, dry_run: bool, yes: bool) -> Result<bool> {
    let all = integrations();
    let planned = preview(ctx, |session, env| install::remove(session, &all, env));
    if is_empty(&planned) {
        ctx.ui
            .say("Vario is not installed in any client: nothing to remove.");
        if !dry_run {
            Choices::delete(&ctx.data_dir)?;
        }
        return Ok(true);
    }
    show(&mut ctx.ui, &ctx.env.home, "Vario will remove:", &planned);
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(
                "No terminal to confirm: nothing was removed. Run 'vario remove --yes' to remove.",
            );
            return Ok(false);
        }
        if !ctx.ui.confirm("Remove Vario from these clients?", false) {
            ctx.ui.say("Cancelled.");
            return Ok(false);
        }
    }

    let mut session = Session::in_data_dir(false, &ctx.data_dir);
    let outcomes = install::remove(&mut session, &all, &ctx.env);
    done(&mut ctx.ui, &ctx.env.home, &outcomes, &session);
    Choices::delete(&ctx.data_dir)?;
    Ok(succeeded(&outcomes))
}

/// One client in the `doctor` report.
struct Row {
    integration: Integration,
    detected: bool,
    status: Vec<(Kind, bool)>,
    chosen: bool,
    declined: bool,
    handoff: State,
}

impl Row {
    fn configured(&self) -> bool {
        self.status.iter().any(|(_, present)| *present)
    }

    fn complete(&self) -> bool {
        self.status.iter().all(|(_, present)| *present)
    }

    fn settled(&self) -> bool {
        self.chosen || self.declined
    }

    fn notes(&self) -> String {
        let name = self.integration.name;
        let mut notes = Vec::new();
        if self.chosen && !self.detected {
            notes.push("client not found".to_string());
        } else if self.chosen && !self.complete() {
            notes.push("incomplete: run 'vario doctor --fix'".to_string());
        } else if self.declined {
            notes.push(format!("declined; 'vario install --client {name}' adds it"));
        } else if !self.chosen {
            notes.push("not configured: run 'vario install'".to_string());
        }
        if self.handoff == State::NotConnected {
            notes.push("connect Handoff: 'handoff doctor --fix'".to_string());
        }
        notes.join("; ")
    }
}

fn rows(env: &Env, choices: Option<&Choices>) -> Vec<Row> {
    let has = |list: fn(&Choices) -> &Vec<String>, name: &str| {
        choices.is_some_and(|choices| list(choices).iter().any(|n| n == name))
    };
    integrations()
        .into_iter()
        .map(|integration| Row {
            detected: integration.detect(env),
            status: install::status(&integration, env),
            chosen: has(|c| &c.clients, integration.name),
            declined: has(|c| &c.declined, integration.name),
            handoff: handoff::state(&integration, env),
            integration,
        })
        .filter(|row| row.detected || row.chosen || row.configured())
        .collect()
}

pub fn doctor(ctx: &mut Context, fix: bool, dry_run: bool, yes: bool) -> Result<bool> {
    let choices = Choices::effective(&ctx.data_dir, &ctx.env);
    let rows = rows(&ctx.env, choices.as_ref());
    if !fix {
        if rows.is_empty() {
            let supported = labels(&integrations());
            ctx.ui.say(format!(
                "No supported AI client was found on this machine (supported: {supported})."
            ));
            return Ok(true);
        }
        report(&mut ctx.ui, &rows);
        if choices.is_none() {
            ctx.ui.say("Vario is not set up yet: run 'vario install'.");
        } else if rows.iter().any(|row| {
            (row.chosen && row.detected && !row.complete()) || (row.detected && !row.settled())
        }) {
            ctx.ui
                .say("Something is missing: run 'vario doctor --fix' to repair it.");
        }
        return Ok(true);
    }

    let Some(mut choices) = choices else {
        ctx.ui.say("Vario is not set up yet: run 'vario install'.");
        return Ok(true);
    };
    let mut targets = Vec::new();
    let mut added = Vec::new();
    let mut declined = Vec::new();
    for row in rows {
        if !row.detected {
            continue;
        }
        if row.chosen {
            targets.push(row.integration);
        } else if !row.settled() {
            let label = row.integration.label;
            if ctx.ui.interactive && !yes && !dry_run {
                if ctx.ui.confirm(
                    &format!("New AI client found: {label}. Configure it?"),
                    true,
                ) {
                    added.push(row.integration.name.to_string());
                    targets.push(row.integration);
                } else {
                    declined.push(row.integration.name.to_string());
                }
            } else {
                ctx.ui.say(format!(
                    "New AI client found: {label}. Run 'vario doctor --fix' in a terminal to configure it."
                ));
            }
        }
    }

    let planned = preview(ctx, |session, env| install::apply(session, &targets, env));
    if is_empty(&planned) {
        if !dry_run && !declined.is_empty() {
            choices.decline(&declined);
            choices.save(&ctx.data_dir)?;
        }
        ctx.ui.say("Everything is in place.");
        return Ok(true);
    }
    show(
        &mut ctx.ui,
        &ctx.env.home,
        "Planned repairs (each file is backed up first):",
        &planned,
    );
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(
                "No terminal to confirm: nothing was changed. Run 'vario doctor --fix --yes' to apply.",
            );
            return Ok(false);
        }
        if !ctx.ui.confirm("Apply these changes?", true) {
            ctx.ui.say("Cancelled.");
            return Ok(false);
        }
    }

    let mut session = Session::in_data_dir(false, &ctx.data_dir);
    let outcomes = install::apply(&mut session, &targets, &ctx.env);
    done(&mut ctx.ui, &ctx.env.home, &outcomes, &session);
    choices.choose(&added);
    choices.decline(&declined);
    choices.save(&ctx.data_dir)?;
    Ok(succeeded(&outcomes))
}

pub fn restore(ctx: &mut Context, backup: Option<&str>, list: bool, yes: bool) -> Result<bool> {
    let backups = files::backups(&ctx.data_dir);
    if list {
        if backups.is_empty() {
            ctx.ui.say("No backups yet.");
        }
        for folder in backups.iter().rev() {
            let count = files::backed_up(folder).map_or(0, |entries| entries.len());
            ctx.ui
                .say(format!("{}  ({count} files)", folder_name(folder)));
        }
        return Ok(true);
    }

    let folder = match backup {
        Some(name) if Path::new(name).is_dir() => PathBuf::from(name),
        Some(name) => files::backups_dir(&ctx.data_dir).join(name),
        None => match backups.last() {
            Some(latest) => latest.clone(),
            None => {
                ctx.ui.say("No backups to restore.");
                return Ok(true);
            }
        },
    };
    if !backups.contains(&folder) && !folder.join("manifest.json").is_file() {
        ctx.ui.say(format!(
            "No backup named '{}': see 'vario restore --list'.",
            backup.unwrap_or_default()
        ));
        return Ok(false);
    }

    ctx.ui.say(format!("Backup {}:", folder_name(&folder)));
    for (path, existed) in files::backed_up(&folder)? {
        let shown = shorten(&path.display().to_string(), &ctx.env.home);
        if existed {
            ctx.ui.say(format!("  - put back {shown}"));
        } else {
            ctx.ui.say(format!("  - delete {shown} (created by Vario)"));
        }
    }
    ctx.ui
        .say("The current content of these files is replaced and not backed up.");
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(
                "No terminal to confirm: nothing was restored. Run 'vario restore --yes' to restore.",
            );
            return Ok(false);
        }
        if !ctx.ui.confirm("Restore these files?", false) {
            ctx.ui.say("Cancelled.");
            return Ok(false);
        }
    }
    let restored = files::restore(&folder)?;
    ctx.ui.say(format!(
        "Restored {} files. Run 'vario doctor' to check the result.",
        restored.len()
    ));
    Ok(true)
}

/// What `run` would change, computed without writing anything.
fn preview(ctx: &Context, run: impl FnOnce(&mut Session, &Env) -> Vec<Outcome>) -> Vec<Outcome> {
    let mut session = Session::in_data_dir(true, &ctx.data_dir);
    run(&mut session, &ctx.env)
}

fn is_empty(outcomes: &[Outcome]) -> bool {
    outcomes
        .iter()
        .all(|outcome| outcome.changes.is_empty() && outcome.error.is_none())
}

fn succeeded(outcomes: &[Outcome]) -> bool {
    outcomes.iter().all(|outcome| outcome.error.is_none())
}

/// Each client's changes and error; clients with neither are left out.
fn show(ui: &mut Ui, home: &Path, title: &str, outcomes: &[Outcome]) {
    ui.say(title);
    for outcome in outcomes {
        if outcome.changes.is_empty() && outcome.error.is_none() {
            continue;
        }
        ui.say(format!("  {}", outcome.label));
        for change in &outcome.changes {
            ui.say(format!("    - {}", shorten(change, home)));
        }
        if let Some(error) = &outcome.error {
            ui.say(format!("    ! {}", shorten(error, home)));
        }
    }
}

/// After the plan was shown: a summary, and only what differs from the plan, the errors.
fn done(ui: &mut Ui, home: &Path, outcomes: &[Outcome], session: &Session) {
    let count = session.changes().len();
    if succeeded(outcomes) {
        ui.say(format!("Done: {count} change(s)."));
    } else {
        ui.say(format!("Done with errors: {count} change(s)."));
        for outcome in outcomes {
            if let Some(error) = &outcome.error {
                ui.say(format!("  {}: {}", outcome.label, shorten(error, home)));
            }
        }
    }
    if count > 0 {
        ui.say(format!(
            "Backups: {}",
            shorten(&session.backup_dir().display().to_string(), home)
        ));
        ui.say("To undo: vario restore");
    }
}

/// Paths under the home directory are shown with `~`.
fn shorten(text: &str, home: &Path) -> String {
    let home = home.display().to_string();
    if home.len() <= 1 {
        return text.to_string();
    }
    text.replace(&home, "~")
}

fn remember(ctx: &Context, chosen: &[String], declined: &[String]) -> Result<()> {
    let mut choices = Choices::load(&ctx.data_dir).unwrap_or_default();
    choices.choose(chosen);
    choices.decline(declined);
    choices.save(&ctx.data_dir)
}

/// Point out Handoff's state for the configured clients; the suggestion to install it
/// is made only once.
fn suggest_handoff(ctx: &mut Context, chosen: &[Integration]) -> Result<()> {
    let mut absent = false;
    for integration in chosen {
        match handoff::state(integration, &ctx.env) {
            State::Connected => {}
            State::NotConnected => ctx.ui.say(format!(
                "Handoff is installed but not connected to {}: run 'handoff doctor --fix'.",
                integration.label
            )),
            State::Absent => absent = true,
        }
    }
    let mut choices = Choices::load(&ctx.data_dir).unwrap_or_default();
    if !absent || choices.handoff_suggested {
        return Ok(());
    }
    ctx.ui.say("");
    ctx.ui.say(
        "Tip: Handoff, by the same author, keeps each project's memory when you switch \
         from one AI agent to another.",
    );
    ctx.ui.say(
        "Install it with 'pipx install ai-handoff' (requires Python 3.10 or later). \
         This tip is shown only once.",
    );
    choices.handoff_suggested = true;
    choices.save(&ctx.data_dir)
}

fn report(ui: &mut Ui, rows: &[Row]) {
    let kinds: Vec<Kind> = [Kind::Subagents, Kind::Instructions, Kind::Hooks, Kind::Mcp]
        .into_iter()
        .filter(|kind| {
            rows.iter()
                .any(|row| row.status.iter().any(|(k, _)| k == kind))
        })
        .collect();
    let mut header = vec!["Client".to_string()];
    header.extend(kinds.iter().map(|kind| kind_label(*kind).to_string()));
    header.extend(["Handoff".to_string(), "Notes".to_string()]);
    let mut table = vec![header];
    for row in rows {
        let mut cells = vec![row.integration.label.to_string()];
        cells.extend(kinds.iter().map(|kind| {
            match row.status.iter().find(|(k, _)| k == kind) {
                Some((_, true)) => "✔",
                Some((_, false)) => "✗",
                None => "—",
            }
            .to_string()
        }));
        cells.push(
            match row.handoff {
                State::Connected => "connected",
                State::NotConnected => "not connected",
                State::Absent => "not installed",
            }
            .to_string(),
        );
        cells.push(row.notes());
        table.push(cells);
    }
    let widths: Vec<usize> = (0..table[0].len())
        .map(|col| {
            table
                .iter()
                .map(|cells| cells[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for cells in &table {
        let line: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| {
                let padding = width - cell.chars().count();
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect();
        ui.say(line.join("  ").trim_end());
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Subagents => "Sub-agents",
        Kind::Instructions => "Instructions",
        Kind::Hooks => "Hooks",
        Kind::Mcp => "MCP",
    }
}

fn labels(list: &[Integration]) -> String {
    let labels: Vec<&str> = list.iter().map(|integration| integration.label).collect();
    labels.join(", ")
}

fn names_of(list: &[Integration]) -> Vec<String> {
    list.iter()
        .map(|integration| integration.name.to_string())
        .collect()
}

fn folder_name(folder: &Path) -> String {
    folder.file_name().map_or_else(
        || folder.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::ui::testing::{Captured, scripted};

    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        data_dir: PathBuf,
    }

    const USER_RULES: &str = "# My rules\n";

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            fs::create_dir_all(home.join(".claude")).unwrap();
            fs::write(home.join(".claude").join("CLAUDE.md"), USER_RULES).unwrap();
            Fixture {
                data_dir: dir.path().join("data"),
                env: Env {
                    home,
                    ..Env::default()
                },
                _dir: dir,
            }
        }

        fn ctx(&self, answers: &str, interactive: bool) -> (Context, Captured) {
            let (ui, out) = scripted(answers, interactive);
            let ctx = Context {
                env: self.env.clone(),
                data_dir: self.data_dir.clone(),
                ui,
            };
            (ctx, out)
        }

        fn claude(&self, file: &str) -> PathBuf {
            self.env.home.join(".claude").join(file)
        }

        fn rules(&self) -> String {
            fs::read_to_string(self.claude("CLAUDE.md")).unwrap()
        }
    }

    #[test]
    fn without_a_terminal_or_yes_nothing_changes() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("", false);
        assert!(install(&mut ctx, false, &[], false, false).unwrap());
        let text = out.text();
        assert!(text.contains("AI clients found: Claude Code"), "{text}");
        assert!(text.contains("vario-reader.md: written"), "{text}");
        assert!(text.contains("nothing was changed"), "{text}");
        assert_eq!(fx.rules(), USER_RULES);
        assert!(!fx.data_dir.exists(), "no backup and no setup.json");
    }

    #[test]
    fn dry_run_shows_the_plan_and_writes_nothing() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("", true);
        assert!(install(&mut ctx, false, &[], true, false).unwrap());
        assert!(out.text().contains("Planned changes"));
        assert_eq!(fx.rules(), USER_RULES);
        assert!(!fx.data_dir.exists());
    }

    #[test]
    fn refusing_the_confirmation_cancels() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("n\n", true);
        assert!(!install(&mut ctx, false, &[], false, false).unwrap());
        assert!(out.text().contains("Cancelled."));
        assert_eq!(fx.rules(), USER_RULES);
    }

    #[test]
    fn manual_mode_remembers_a_declined_client() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("n\n", true);
        assert!(install(&mut ctx, true, &[], false, false).unwrap());
        assert!(out.text().contains("Configure Claude Code? [Y/n]"));
        assert_eq!(fx.rules(), USER_RULES);
        let choices = Choices::load(&fx.data_dir).unwrap();
        assert_eq!(choices.declined, ["claude"]);

        let (mut ctx, out) = fx.ctx("", true);
        doctor(&mut ctx, false, false, false).unwrap();
        let text = out.text();
        assert!(text.contains("declined"), "{text}");
        assert!(!text.contains("Something is missing"), "{text}");
    }

    #[test]
    fn install_doctor_fix_then_remove() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("\n", true);
        assert!(install(&mut ctx, false, &[], false, false).unwrap());
        let text = out.text();
        assert!(text.contains("Apply these changes? [Y/n]"), "{text}");
        assert!(text.contains("Backups: "), "{text}");
        assert!(fx.claude("agents/vario-worker.md").is_file());
        assert_eq!(Choices::load(&fx.data_dir).unwrap().clients, ["claude"]);

        let (mut ctx, out) = fx.ctx("", false);
        doctor(&mut ctx, false, false, false).unwrap();
        let text = out.text();
        assert!(
            text.contains("Client       Sub-agents  Instructions  Handoff"),
            "{text}"
        );
        assert!(
            text.contains("Claude Code  ✔           ✔             not installed"),
            "{text}"
        );
        assert!(!text.contains("Something is missing"), "{text}");

        fs::remove_file(fx.claude("agents/vario-reader.md")).unwrap();
        let (mut ctx, out) = fx.ctx("", false);
        doctor(&mut ctx, false, false, false).unwrap();
        assert!(out.text().contains("incomplete"));
        let (mut ctx, _) = fx.ctx("", false);
        assert!(doctor(&mut ctx, true, false, true).unwrap());
        assert!(fx.claude("agents/vario-reader.md").is_file());
        let (mut ctx, out) = fx.ctx("", false);
        assert!(doctor(&mut ctx, true, false, false).unwrap());
        assert!(out.text().contains("Everything is in place."));

        let (mut ctx, out) = fx.ctx("", false);
        assert!(
            !remove(&mut ctx, false, false).unwrap(),
            "no terminal, no --yes"
        );
        assert!(out.text().contains("nothing was removed"));
        let (mut ctx, _) = fx.ctx("", false);
        assert!(remove(&mut ctx, false, true).unwrap());
        assert_eq!(fx.rules(), USER_RULES);
        assert!(!fx.claude("agents/vario-worker.md").exists());
        assert!(Choices::load(&fx.data_dir).is_none());
    }

    #[test]
    fn restore_puts_back_the_latest_backup() {
        let fx = Fixture::new();
        let (mut ctx, _) = fx.ctx("", false);
        assert!(install(&mut ctx, false, &[], false, true).unwrap());
        assert_ne!(fx.rules(), USER_RULES);

        let (mut ctx, out) = fx.ctx("", false);
        assert!(restore(&mut ctx, None, true, false).unwrap());
        assert!(out.text().contains("(3 files)"), "{}", out.text());

        let (mut ctx, out) = fx.ctx("\n", true);
        assert!(
            !restore(&mut ctx, None, false, false).unwrap(),
            "defaults to no"
        );
        let text = out.text();
        assert!(text.contains("put back "), "{text}");
        assert!(
            text.contains("vario-reader.md (created by Vario)"),
            "{text}"
        );

        let (mut ctx, _) = fx.ctx("y\n", true);
        assert!(restore(&mut ctx, None, false, false).unwrap());
        assert_eq!(fx.rules(), USER_RULES);
        assert!(!fx.claude("agents/vario-reader.md").exists());

        let (mut ctx, out) = fx.ctx("", false);
        assert!(!restore(&mut ctx, Some("nope"), false, true).unwrap());
        assert!(out.text().contains("No backup named 'nope'"));
    }

    #[test]
    fn the_handoff_tip_is_shown_once() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("", false);
        install(&mut ctx, false, &[], false, true).unwrap();
        assert!(out.text().contains("pipx install ai-handoff"));
        let (mut ctx, out) = fx.ctx("", false);
        install(&mut ctx, false, &[], false, true).unwrap();
        let text = out.text();
        assert!(text.contains("Everything is already in place."), "{text}");
        assert!(!text.contains("pipx install ai-handoff"), "{text}");
    }

    #[test]
    fn no_client_found() {
        let dir = tempfile::tempdir().unwrap();
        let (ui, out) = scripted("", false);
        let mut ctx = Context {
            env: Env {
                home: dir.path().to_path_buf(),
                ..Env::default()
            },
            data_dir: dir.path().join("data"),
            ui,
        };
        assert!(install(&mut ctx, false, &[], false, true).unwrap());
        assert!(out.text().contains("No supported AI client was found"));
    }
}
