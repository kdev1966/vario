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
use vario_core::lang::Lang;
use vario_core::overview::{self, ClientState, Note};
use vario_core::parts::Kind;
use vario_core::tr;

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
    let lang = ctx.ui.lang;
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
        no_client(&mut ctx.ui);
        return Ok(true);
    }
    if !explicit {
        let found = labels(&candidates);
        ctx.ui.say(tr!(
            lang,
            "AI clients found: {found}",
            "Clients IA trouvés : {found}"
        ));
    }

    let mut chosen = Vec::new();
    let mut declined = Vec::new();
    if manual && ctx.ui.interactive && !yes && !dry_run {
        for integration in candidates {
            let label = integration.label;
            if ctx.ui.confirm(
                &tr!(lang, "Configure {label}?", "Configurer {label} ?"),
                true,
            ) {
                chosen.push(integration);
            } else {
                declined.push(integration.name.to_string());
            }
        }
    } else {
        chosen = candidates;
    }
    if chosen.is_empty() {
        Choices::remember(&ctx.data_dir, &[], &declined)?;
        ctx.ui
            .say(tr!(lang, "Nothing was changed.", "Rien n'a été modifié."));
        return Ok(true);
    }

    let planned = preview(ctx, |session, env| install::apply(session, &chosen, env));
    if is_empty(&planned) {
        ctx.ui.say(tr!(
            lang,
            "Everything is already in place.",
            "Tout est déjà en place."
        ));
        if !dry_run {
            Choices::remember(&ctx.data_dir, &names_of(&chosen), &declined)?;
            suggest_handoff(ctx, &chosen)?;
        }
        return Ok(true);
    }
    let title = tr!(
        lang,
        "Planned changes (each file is backed up first):",
        "Changements prévus (chaque fichier est sauvegardé avant modification) :"
    );
    show(&mut ctx.ui, &ctx.env.home, &title, &planned);
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(tr!(
                lang,
                "No terminal to confirm: nothing was changed. Run 'vario install --yes' to apply.",
                "Pas de terminal pour confirmer : rien n'a été modifié. Lancez « vario install --yes » pour appliquer."
            ));
            return Ok(true);
        }
        if !ctx.ui.confirm(
            &tr!(lang, "Apply these changes?", "Appliquer ces changements ?"),
            true,
        ) {
            cancelled(&mut ctx.ui);
            return Ok(false);
        }
    }

    let mut session = Session::in_data_dir(false, &ctx.data_dir);
    let outcomes = install::apply(&mut session, &chosen, &ctx.env);
    done(&mut ctx.ui, &ctx.env.home, &outcomes, &session);
    Choices::remember(&ctx.data_dir, &names_of(&chosen), &declined)?;
    suggest_handoff(ctx, &chosen)?;
    Ok(succeeded(&outcomes))
}

pub fn remove(ctx: &mut Context, dry_run: bool, yes: bool) -> Result<bool> {
    let lang = ctx.ui.lang;
    let all = integrations();
    let planned = preview(ctx, |session, env| install::remove(session, &all, env));
    if is_empty(&planned) {
        ctx.ui.say(tr!(
            lang,
            "Vario is not installed in any client: nothing to remove.",
            "Vario n'est installé dans aucun client : rien à retirer."
        ));
        if !dry_run {
            Choices::delete(&ctx.data_dir)?;
        }
        return Ok(true);
    }
    let title = tr!(lang, "Vario will remove:", "Vario va retirer :");
    show(&mut ctx.ui, &ctx.env.home, &title, &planned);
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(tr!(
                lang,
                "No terminal to confirm: nothing was removed. Run 'vario remove --yes' to remove.",
                "Pas de terminal pour confirmer : rien n'a été retiré. Lancez « vario remove --yes » pour retirer."
            ));
            return Ok(false);
        }
        if !ctx.ui.confirm(
            &tr!(
                lang,
                "Remove Vario from these clients?",
                "Retirer Vario de ces clients ?"
            ),
            false,
        ) {
            cancelled(&mut ctx.ui);
            return Ok(false);
        }
    }

    let mut session = Session::in_data_dir(false, &ctx.data_dir);
    let outcomes = install::remove(&mut session, &all, &ctx.env);
    done(&mut ctx.ui, &ctx.env.home, &outcomes, &session);
    Choices::delete(&ctx.data_dir)?;
    Ok(succeeded(&outcomes))
}

/// What a note asks the user to do, worded for the command line.
fn note_text(note: Note, name: &str, lang: Lang) -> String {
    match note {
        Note::NotFound => tr!(lang, "client not found", "client introuvable"),
        Note::Incomplete => tr!(
            lang,
            "incomplete: run 'vario doctor --fix'",
            "incomplet : lancez « vario doctor --fix »"
        ),
        Note::Declined => tr!(
            lang,
            "declined; 'vario install --client {name}' adds it",
            "écarté ; « vario install --client {name} » l'ajoute"
        ),
        Note::NotConfigured => tr!(
            lang,
            "not configured: run 'vario install'",
            "non configuré : lancez « vario install »"
        ),
        Note::ConnectHandoff => tr!(
            lang,
            "connect Handoff: 'handoff doctor --fix'",
            "connecter Handoff : « handoff doctor --fix »"
        ),
    }
}

pub fn doctor(ctx: &mut Context, fix: bool, dry_run: bool, yes: bool) -> Result<bool> {
    let lang = ctx.ui.lang;
    let choices = Choices::effective(&ctx.data_dir, &ctx.env);
    let rows = overview::clients(&ctx.env, choices.as_ref());
    if !fix {
        if rows.is_empty() {
            no_client(&mut ctx.ui);
            return Ok(true);
        }
        doctor_table(&mut ctx.ui, &rows);
        if choices.is_none() {
            not_set_up(&mut ctx.ui);
        } else if rows.iter().any(ClientState::needs_fix) {
            ctx.ui.say(tr!(
                lang,
                "Something is missing: run 'vario doctor --fix' to repair it.",
                "Il manque quelque chose : lancez « vario doctor --fix » pour réparer."
            ));
        }
        return Ok(true);
    }

    let Some(mut choices) = choices else {
        not_set_up(&mut ctx.ui);
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
                let question = tr!(
                    lang,
                    "New AI client found: {label}. Configure it?",
                    "Nouveau client IA trouvé : {label}. Le configurer ?"
                );
                if ctx.ui.confirm(&question, true) {
                    added.push(row.integration.name.to_string());
                    targets.push(row.integration);
                } else {
                    declined.push(row.integration.name.to_string());
                }
            } else {
                ctx.ui.say(tr!(
                    lang,
                    "New AI client found: {label}. Run 'vario doctor --fix' in a terminal to configure it.",
                    "Nouveau client IA trouvé : {label}. Lancez « vario doctor --fix » dans un terminal pour le configurer."
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
        ctx.ui
            .say(tr!(lang, "Everything is in place.", "Tout est en place."));
        return Ok(true);
    }
    let title = tr!(
        lang,
        "Planned repairs (each file is backed up first):",
        "Réparations prévues (chaque fichier est sauvegardé avant modification) :"
    );
    show(&mut ctx.ui, &ctx.env.home, &title, &planned);
    if dry_run {
        return Ok(true);
    }
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(tr!(
                lang,
                "No terminal to confirm: nothing was changed. Run 'vario doctor --fix --yes' to apply.",
                "Pas de terminal pour confirmer : rien n'a été modifié. Lancez « vario doctor --fix --yes » pour appliquer."
            ));
            return Ok(false);
        }
        if !ctx.ui.confirm(
            &tr!(lang, "Apply these changes?", "Appliquer ces changements ?"),
            true,
        ) {
            cancelled(&mut ctx.ui);
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
    let lang = ctx.ui.lang;
    let backups = files::backups(&ctx.data_dir);
    if list {
        if backups.is_empty() {
            ctx.ui.say(tr!(
                lang,
                "No backups yet.",
                "Aucune sauvegarde pour l'instant."
            ));
        }
        for folder in backups.iter().rev() {
            let name = folder_name(folder);
            let count = files::backed_up(folder).map_or(0, |entries| entries.len());
            ctx.ui.say(tr!(
                lang,
                "{name}  ({count} files)",
                "{name}  ({count} fichiers)"
            ));
        }
        return Ok(true);
    }

    let folder = match backup {
        Some(name) if Path::new(name).is_dir() => PathBuf::from(name),
        Some(name) => files::backups_dir(&ctx.data_dir).join(name),
        None => match backups.last() {
            Some(latest) => latest.clone(),
            None => {
                ctx.ui.say(tr!(
                    lang,
                    "No backups to restore.",
                    "Aucune sauvegarde à restaurer."
                ));
                return Ok(true);
            }
        },
    };
    if !backups.contains(&folder) && !folder.join("manifest.json").is_file() {
        let name = backup.unwrap_or_default();
        ctx.ui.say(tr!(
            lang,
            "No backup named '{name}': see 'vario restore --list'.",
            "Aucune sauvegarde nommée « {name} » : voir « vario restore --list »."
        ));
        return Ok(false);
    }

    let name = folder_name(&folder);
    ctx.ui
        .say(tr!(lang, "Backup {name}:", "Sauvegarde {name} :"));
    for (path, existed) in files::backed_up(&folder)? {
        let shown = shorten(&path.display().to_string(), &ctx.env.home);
        ctx.ui.say(if existed {
            tr!(
                lang,
                "  - put back {shown}",
                "  - remise en place de {shown}"
            )
        } else {
            tr!(
                lang,
                "  - delete {shown} (created by Vario)",
                "  - suppression de {shown} (créé par Vario)"
            )
        });
    }
    ctx.ui.say(tr!(
        lang,
        "The current content of these files is replaced and not backed up.",
        "Le contenu actuel de ces fichiers est remplacé, sans sauvegarde."
    ));
    if !yes {
        if !ctx.ui.interactive {
            ctx.ui.say(tr!(
                lang,
                "No terminal to confirm: nothing was restored. Run 'vario restore --yes' to restore.",
                "Pas de terminal pour confirmer : rien n'a été restauré. Lancez « vario restore --yes » pour restaurer."
            ));
            return Ok(false);
        }
        if !ctx.ui.confirm(
            &tr!(lang, "Restore these files?", "Restaurer ces fichiers ?"),
            false,
        ) {
            cancelled(&mut ctx.ui);
            return Ok(false);
        }
    }
    let count = files::restore(&folder)?.len();
    ctx.ui.say(tr!(
        lang,
        "Restored {count} files. Run 'vario doctor' to check the result.",
        "{count} fichiers restaurés. Lancez « vario doctor » pour vérifier le résultat."
    ));
    Ok(true)
}

pub fn report(ctx: &mut Context) -> Result<bool> {
    let lang = ctx.ui.lang;
    let summary = vario_core::report::summarize(&ctx.data_dir)?;
    if summary.total.delegations == 0 {
        ctx.ui.say(tr!(
            lang,
            "No delegation recorded yet: the journal fills in as Vario's sub-agents finish, through the hooks 'vario install' sets up.",
            "Aucune délégation notée pour l'instant : le journal se remplit à la fin des sous-agents de Vario, grâce aux hooks que pose « vario install »."
        ));
    } else {
        let day = |time: &Option<String>| {
            time.as_deref()
                .map(|time| time.chars().take(10).collect::<String>())
                .unwrap_or_default()
        };
        let (first, last) = (day(&summary.first), day(&summary.last));
        ctx.ui.say(tr!(
            lang,
            "Delegations from {first} to {last}:",
            "Délégations du {first} au {last} :"
        ));
        let mut table = vec![vec![
            tr!(lang, "Sub-agent", "Sous-agent"),
            tr!(lang, "Delegations", "Délégations"),
            tr!(lang, "Escalated", "Escalades"),
            tr!(lang, "No footer", "Sans pied de page"),
        ]];
        let row = |name: String, counts: &vario_core::report::Counts| {
            vec![
                name,
                counts.delegations.to_string(),
                share(counts.escalated, counts.delegations),
                share(counts.without_footer, counts.delegations),
            ]
        };
        for (agent, counts) in &summary.agents {
            table.push(row(agent.clone(), counts));
        }
        table.push(row("Total".to_string(), &summary.total));
        print_table(&mut ctx.ui, &table);
    }
    if summary.unreadable > 0 {
        let count = summary.unreadable;
        ctx.ui.say(tr!(
            lang,
            "{count} unreadable line(s) skipped.",
            "{count} ligne(s) illisible(s) ignorée(s)."
        ));
    }
    let journal = shorten(
        &vario_core::report::journal_path(&ctx.data_dir)
            .display()
            .to_string(),
        &ctx.env.home,
    );
    ctx.ui
        .say(tr!(lang, "Journal: {journal}", "Journal : {journal}"));
    Ok(true)
}

/// `count` and its share of `total`: "1 (33 %)"; just "0" when there is none.
fn share(count: usize, total: usize) -> String {
    if count == 0 || total == 0 {
        return count.to_string();
    }
    let percent = (count * 100 + total / 2) / total;
    format!("{count} ({percent} %)")
}

/// What `run` would change, computed without writing anything.
fn preview(ctx: &Context, run: impl FnOnce(&mut Session, &Env) -> Vec<Outcome>) -> Vec<Outcome> {
    let mut session = Session::in_data_dir(true, &ctx.data_dir);
    run(&mut session, &ctx.env)
}

fn is_empty(outcomes: &[Outcome]) -> bool {
    outcomes
        .iter()
        .all(|outcome| outcome.changes.is_empty() && outcome.ok())
}

fn succeeded(outcomes: &[Outcome]) -> bool {
    outcomes.iter().all(Outcome::ok)
}

/// Each client's changes and errors; clients with neither are left out.
fn show(ui: &mut Ui, home: &Path, title: &str, outcomes: &[Outcome]) {
    let lang = ui.lang;
    ui.say(title);
    for outcome in outcomes {
        if outcome.changes.is_empty() && outcome.ok() {
            continue;
        }
        ui.say(format!("  {}", outcome.label));
        for change in &outcome.changes {
            let path = shorten(&change.path.display().to_string(), home);
            let action = change.action.text(lang);
            ui.say(tr!(
                lang,
                "    - {path}: {action}",
                "    - {path} : {action}"
            ));
        }
        for error in &outcome.errors {
            ui.say(format!("    ! {}", shorten(&error.text(lang), home)));
        }
    }
}

/// After the plan was shown: a summary, and only what differs from the plan, the errors.
fn done(ui: &mut Ui, home: &Path, outcomes: &[Outcome], session: &Session) {
    let lang = ui.lang;
    let count = session.changes().len();
    if succeeded(outcomes) {
        ui.say(tr!(
            lang,
            "Done: {count} change(s).",
            "Terminé : {count} changement(s)."
        ));
    } else {
        ui.say(tr!(
            lang,
            "Done with errors: {count} change(s).",
            "Terminé avec des erreurs : {count} changement(s)."
        ));
        for outcome in outcomes {
            for error in &outcome.errors {
                ui.say(format!(
                    "  {}: {}",
                    outcome.label,
                    shorten(&error.text(lang), home)
                ));
            }
        }
    }
    if count > 0 {
        let folder = shorten(&session.backup_dir().display().to_string(), home);
        ui.say(tr!(lang, "Backups: {folder}", "Sauvegardes : {folder}"));
        ui.say(tr!(
            lang,
            "To undo: vario restore",
            "Pour annuler : vario restore"
        ));
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

fn no_client(ui: &mut Ui) {
    let supported = labels(&integrations());
    ui.say(tr!(
        ui.lang,
        "No supported AI client was found on this machine (supported: {supported}).",
        "Aucun client IA compatible n'a été trouvé sur cette machine (compatibles : {supported})."
    ));
}

fn not_set_up(ui: &mut Ui) {
    ui.say(tr!(
        ui.lang,
        "Vario is not set up yet: run 'vario install'.",
        "Vario n'est pas encore configuré : lancez « vario install »."
    ));
}

fn cancelled(ui: &mut Ui) {
    ui.say(tr!(ui.lang, "Cancelled.", "Annulé."));
}

/// Point out Handoff's state for the configured clients; the suggestion to install it
/// is made only once.
fn suggest_handoff(ctx: &mut Context, chosen: &[Integration]) -> Result<()> {
    let lang = ctx.ui.lang;
    let mut absent = false;
    for integration in chosen {
        let label = integration.label;
        match handoff::state(integration, &ctx.env) {
            State::Connected => {}
            State::NotConnected => ctx.ui.say(tr!(
                lang,
                "Handoff is installed but not connected to {label}: run 'handoff doctor --fix'.",
                "Handoff est installé mais pas connecté à {label} : lancez « handoff doctor --fix »."
            )),
            State::Absent => absent = true,
        }
    }
    let mut choices = Choices::load(&ctx.data_dir).unwrap_or_default();
    if !absent || choices.handoff_suggested {
        return Ok(());
    }
    ctx.ui.say("");
    ctx.ui.say(tr!(
        lang,
        "Tip: Handoff, by the same author, keeps each project's memory when you switch from one AI agent to another.",
        "Conseil : Handoff, du même auteur, garde la mémoire de chaque projet quand vous passez d'un agent IA à un autre."
    ));
    ctx.ui.say(tr!(
        lang,
        "Install it with 'pipx install ai-handoff' (requires Python 3.10 or later). This tip is shown only once.",
        "Installez-le avec « pipx install ai-handoff » (Python 3.10 ou plus récent requis). Ce conseil n'est affiché qu'une fois."
    ));
    choices.handoff_suggested = true;
    choices.save(&ctx.data_dir)
}

fn doctor_table(ui: &mut Ui, rows: &[ClientState]) {
    let lang = ui.lang;
    let kinds: Vec<Kind> = [Kind::Subagents, Kind::Instructions, Kind::Hooks, Kind::Mcp]
        .into_iter()
        .filter(|kind| {
            rows.iter()
                .any(|row| row.status.iter().any(|(k, _)| k == kind))
        })
        .collect();
    let mut header = vec!["Client".to_string()];
    header.extend(kinds.iter().map(|kind| kind_label(*kind, lang)));
    header.extend(["Handoff".to_string(), tr!(lang, "Notes", "Remarques")]);
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
        cells.push(match row.handoff {
            State::Connected => tr!(lang, "connected", "connecté"),
            State::NotConnected => tr!(lang, "not connected", "non connecté"),
            State::Absent => tr!(lang, "not installed", "non installé"),
        });
        let notes: Vec<String> = row
            .notes()
            .into_iter()
            .map(|note| note_text(note, row.integration.name, lang))
            .collect();
        cells.push(notes.join("; "));
        table.push(cells);
    }
    print_table(ui, &table);
}

/// Columns padded to their widest cell; the first row is the header.
fn print_table(ui: &mut Ui, table: &[Vec<String>]) {
    let widths: Vec<usize> = (0..table[0].len())
        .map(|col| {
            table
                .iter()
                .map(|cells| cells[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for cells in table {
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

fn kind_label(kind: Kind, lang: Lang) -> String {
    match kind {
        Kind::Subagents => tr!(lang, "Sub-agents", "Sous-agents"),
        Kind::Instructions => tr!(lang, "Instructions", "Consignes"),
        Kind::Hooks => "Hooks".to_string(),
        Kind::Mcp => "MCP".to_string(),
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
    use crate::ui::testing::{Captured, scripted, scripted_in};

    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        data_dir: PathBuf,
    }

    const USER_RULES: &str = "# My rules\n";

    /// An expected text with the system's path separator (`~\.claude\…` on Windows).
    fn native(text: &str) -> String {
        text.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

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
            self.with_ui(scripted(answers, interactive))
        }

        fn with_ui(&self, (ui, out): (Ui, Captured)) -> (Context, Captured) {
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
        assert!(
            text.contains(&native("- ~/.claude/agents/vario-reader.md: written")),
            "{text}"
        );
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
        assert!(text.contains("Done: 4 change(s)."), "{text}");
        assert!(text.contains("Backups: "), "{text}");
        assert!(fx.claude("agents/vario-worker.md").is_file());
        assert_eq!(Choices::load(&fx.data_dir).unwrap().clients, ["claude"]);

        let (mut ctx, out) = fx.ctx("", false);
        doctor(&mut ctx, false, false, false).unwrap();
        let text = out.text();
        assert!(
            text.contains("Client       Sub-agents  Instructions  Hooks  Handoff"),
            "{text}"
        );
        assert!(
            text.contains("Claude Code  ✔           ✔             ✔      not installed"),
            "{text}"
        );
        assert!(!text.contains("Something is missing"), "{text}");
        assert!(
            !text.contains("OpenCode"),
            "Vario's block in CLAUDE.md, which OpenCode also reads, does not make it installed there: {text}"
        );

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
    fn a_foreign_file_is_reported_as_an_error() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.claude("agents")).unwrap();
        fs::write(fx.claude("agents/vario-reader.md"), "mine").unwrap();
        let (mut ctx, out) = fx.ctx("", false);
        assert!(!install(&mut ctx, false, &[], false, true).unwrap());
        let text = out.text();
        assert!(
            text.contains(&native(
                "! ~/.claude/agents/vario-reader.md is not managed by Vario"
            )),
            "{text}"
        );
        assert!(text.contains("Done with errors"), "{text}");
        assert_eq!(
            fs::read_to_string(fx.claude("agents/vario-reader.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn restore_puts_back_the_latest_backup() {
        let fx = Fixture::new();
        let (mut ctx, _) = fx.ctx("", false);
        assert!(install(&mut ctx, false, &[], false, true).unwrap());
        assert_ne!(fx.rules(), USER_RULES);

        let (mut ctx, out) = fx.ctx("", false);
        assert!(restore(&mut ctx, None, true, false).unwrap());
        assert!(out.text().contains("(4 files)"), "{}", out.text());

        let (mut ctx, out) = fx.ctx("\n", true);
        assert!(
            !restore(&mut ctx, None, false, false).unwrap(),
            "defaults to no"
        );
        let text = out.text();
        assert!(
            text.contains(&native("put back ~/.claude/CLAUDE.md")),
            "{text}"
        );
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
    fn in_french() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.with_ui(scripted_in(Lang::Fr, "oui\n", true));
        assert!(install(&mut ctx, false, &[], false, false).unwrap());
        let text = out.text();
        assert!(text.contains("Clients IA trouvés : Claude Code"), "{text}");
        assert!(
            text.contains(&native("- ~/.claude/agents/vario-reader.md : écriture")),
            "{text}"
        );
        assert!(text.contains("Appliquer ces changements ? [O/n]"), "{text}");
        assert!(text.contains("Terminé : 4 changement(s)."), "{text}");

        let (mut ctx, out) = fx.with_ui(scripted_in(Lang::Fr, "", false));
        doctor(&mut ctx, false, false, false).unwrap();
        let text = out.text();
        assert!(
            text.contains("Client       Sous-agents  Consignes  Hooks  Handoff       Remarques"),
            "{text}"
        );
        assert!(text.contains("non installé"), "{text}");
    }

    #[test]
    fn report_summarizes_the_journal() {
        let fx = Fixture::new();
        let (mut ctx, out) = fx.ctx("", false);
        assert!(report(&mut ctx).unwrap());
        assert!(out.text().contains("No delegation recorded yet"));

        fs::create_dir_all(&fx.data_dir).unwrap();
        let lines = [
            r#"{"time":"2026-10-15T09:00:00+01:00","agent":"vario-worker","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-16T10:00:00+01:00","agent":"vario-worker","result":"stuck","escalate":true}"#,
            r#"{"time":"2026-10-16T11:00:00+01:00","agent":"vario-reader","result":null}"#,
        ];
        fs::write(fx.data_dir.join("delegations.jsonl"), lines.join("\n")).unwrap();
        let (mut ctx, out) = fx.ctx("", false);
        assert!(report(&mut ctx).unwrap());
        let text = out.text();
        assert!(
            text.contains("Delegations from 2026-10-15 to 2026-10-16:"),
            "{text}"
        );
        assert!(
            text.contains("vario-reader  1            0          1 (100 %)"),
            "{text}"
        );
        assert!(
            text.contains("vario-worker  2            1 (50 %)   0"),
            "{text}"
        );
        assert!(
            text.contains("Total         3            1 (33 %)   1 (33 %)"),
            "{text}"
        );
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
