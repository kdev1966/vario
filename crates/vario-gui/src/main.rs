//! Vario's graphical interface: the same core as the `vario` command, for people who
//! would rather not use a terminal. The floating gauge follows the quota; the main
//! window installs, checks, repairs and removes Vario, and shows the delegation
//! journal.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};
use vario_core::choices::Choices;
use vario_core::clients::{Env, Integration, by_name};
use vario_core::files::Session;
use vario_core::handoff::State;
use vario_core::install::{self, Outcome};
use vario_core::lang::Lang;
use vario_core::overview::{self, Note};
use vario_core::parts::Kind;
use vario_core::{paths, report, tr};

use instance::Want;
use shell::Shell;

mod dock;
mod gauge;
mod instance;
mod shell;

slint::include_modules!();

/// What the window works on.
struct App {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
    /// The clients found on this machine, with whether each is ticked for install.
    candidates: Vec<(&'static str, bool)>,
}

fn main() -> Result<(), slint::PlatformError> {
    let mut env = Env::current();
    env.exe = command_line_path(&env);
    let lang = Lang::detect(&env);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(answer) = answer_in_terminal(&args, lang) {
        println!("{answer}");
        return Ok(());
    }
    // `vario-gui --gauge`: the floating gauge, as a desktop widget.
    let want = if args.iter().any(|arg| arg == "--gauge") {
        Want::Gauge
    } else {
        Want::Main
    };
    let data_dir = paths::data_dir();
    let Some(_instance) = instance::claim(&data_dir) else {
        instance::request(&data_dir, want);
        return Ok(());
    };
    if want == Want::Gauge {
        gauge::select_widget_backend()?;
    }
    let shell = Shell::new(env, data_dir, lang);
    shell.show(want);
    // Opens on another page, for screenshots: VARIO_GUI_TAB=0..3.
    if let (Some(window), Some(tab)) = (
        shell.main_window(),
        std::env::var("VARIO_GUI_TAB")
            .ok()
            .and_then(|tab| tab.parse().ok()),
    ) {
        window.set_current_tab(tab);
    }
    shell.listen();
    slint::run_event_loop()
}

/// The window, filled and wired to an [`App`] working on `env` and `data_dir`.
pub(crate) fn open(
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
) -> Result<(AppWindow, Rc<RefCell<App>>), slint::PlatformError> {
    let window = AppWindow::new()?;
    select_translation(lang);
    window.set_version(env!("CARGO_PKG_VERSION").into());

    let app = Rc::new(RefCell::new(App {
        env,
        data_dir,
        lang,
        candidates: Vec::new(),
    }));
    app.borrow_mut().reset_candidates();
    app.borrow().refresh(&window);

    let on = |handler: fn(&mut App, &AppWindow)| {
        let app = Rc::clone(&app);
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                handler(&mut app.borrow_mut(), &window);
            }
        }
    };
    window.on_refresh(on(|app, window| app.refresh(window)));
    {
        let data_dir = app.borrow().data_dir.clone();
        window.on_gauge_setting_changed(move |on| {
            let _ = Choices::set_gauge_with_sessions(&data_dir, on);
        });
    }
    window.on_repair(on(App::repair));
    window.on_remove_all(on(App::remove_all));
    window.on_preview(on(App::preview));
    window.on_apply(on(App::apply));
    window.on_mode_changed(on(|app, window| {
        app.reset_candidates();
        app.show_candidates(window);
        window.set_planned(ModelRc::default());
        window.set_can_apply(false);
        window.set_install_message(SharedString::new());
    }));
    {
        let app = Rc::clone(&app);
        let weak = window.as_weak();
        window.on_toggle_choice(move |index, checked| {
            if let Some((_, selected)) = app.borrow_mut().candidates.get_mut(index as usize) {
                *selected = checked;
            }
            if let Some(window) = weak.upgrade() {
                window.set_planned(ModelRc::default());
                window.set_can_apply(false);
            }
        });
    }
    Ok((window, app))
}

/// Words the `.slint` text in `lang`. Slint picks a bundle from the system locale on
/// its own, while `lang` may come from `LANG`: an agent that starts the gauge with
/// `LANG=en_US` must get English labels too, not French ones around English values.
fn select_translation(lang: Lang) {
    let _ = slint::select_bundled_translation(match lang {
        Lang::En => "en",
        Lang::Fr => "fr",
    });
}

/// The main window and the state behind it.
type MainWindow = (AppWindow, Rc<RefCell<App>>);

/// The floating gauge, fed from a background thread.
pub(crate) fn open_gauge(
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
) -> Result<GaugeWindow, slint::PlatformError> {
    let gauge = GaugeWindow::new()?;
    select_translation(lang);
    gauge::Reading::read(&data_dir, lang, None, vario_core::quota::now()).show(&gauge);
    gauge::feed(&gauge, env, data_dir, lang);
    Ok(gauge)
}

/// Hooks must run the `vario` command, not this interface: the one the installed hooks
/// already run, so the interface and the command line agree; else the one installed
/// next to the interface (inside Vario.app); else the one on `PATH`; else plain `vario`.
fn command_line_path(env: &Env) -> PathBuf {
    let name = if cfg!(windows) { "vario.exe" } else { "vario" };
    install::hook_program(env)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
                .filter(|path| path.is_file())
        })
        .or_else(|| env.which("vario"))
        .unwrap_or_default()
}

impl App {
    /// Tick every client found; in manual mode the user unticks the ones to leave out.
    fn reset_candidates(&mut self) {
        self.candidates = install::detected(&self.env)
            .iter()
            .map(|integration| (integration.name, true))
            .collect();
    }

    fn chosen(&self) -> Vec<Integration> {
        self.candidates
            .iter()
            .filter(|(_, selected)| *selected)
            .filter_map(|(name, _)| by_name(name))
            .collect()
    }

    fn refresh(&self, window: &AppWindow) {
        self.show_status(window);
        self.show_candidates(window);
        self.show_report(window);
    }

    fn show_candidates(&self, window: &AppWindow) {
        let choices: Vec<Choice> = self
            .candidates
            .iter()
            .filter_map(|(name, selected)| {
                by_name(name).map(|integration| Choice {
                    label: integration.label.into(),
                    selected: *selected,
                })
            })
            .collect();
        window.set_choices(ModelRc::new(VecModel::from(choices)));
    }

    fn show_status(&self, window: &AppWindow) {
        let lang = self.lang;
        let choices = Choices::effective(&self.data_dir, &self.env);
        let clients = overview::clients(&self.env, choices.as_ref());
        let state = |client: &overview::ClientState, kind: Kind| match client
            .status
            .iter()
            .find(|(k, _)| *k == kind)
        {
            Some((_, true)) => 1,
            Some((_, false)) => 0,
            None => -1,
        };
        let rows: Vec<ClientRow> = clients
            .iter()
            .map(|client| ClientRow {
                label: client.integration.label.into(),
                subagents: state(client, Kind::Subagents),
                instructions: state(client, Kind::Instructions),
                hooks: state(client, Kind::Hooks),
                quota: state(client, Kind::Quota),
                handoff: match client.handoff {
                    State::Connected => 1,
                    State::NotConnected => 0,
                    State::Absent => -1,
                },
                notes: client
                    .notes()
                    .into_iter()
                    .map(|note| note_text(note, lang))
                    .collect::<Vec<_>>()
                    .join("; ")
                    .into(),
            })
            .collect();
        window.set_clients(ModelRc::new(VecModel::from(rows)));
        let needs_fix = clients.iter().any(overview::ClientState::needs_fix);
        window.set_needs_fix(needs_fix && choices.is_some());
        let message = if clients.is_empty() {
            tr!(
                lang,
                "No supported AI client was found on this machine.",
                "Aucun client IA compatible n'a été trouvé sur cette machine."
            )
        } else if choices.is_none() {
            tr!(
                lang,
                "Vario is not set up yet: see the Install tab.",
                "Vario n'est pas encore configuré : voir l'onglet Installation."
            )
        } else if needs_fix {
            tr!(
                lang,
                "Something is missing: click Repair.",
                "Il manque quelque chose : cliquez sur Réparer."
            )
        } else {
            tr!(lang, "Everything is in place.", "Tout est en place.")
        };
        window.set_status_message(message.into());
        window.set_gauge_with_sessions(Choices::gauge_with_sessions(&self.data_dir));
        let hint = if clients.iter().any(|client| client.handoff == State::Absent) {
            tr!(
                lang,
                "Tip: Handoff, by the same author, keeps each project's memory when you switch from one AI agent to another (pipx install ai-handoff, Python 3.10 or later).",
                "Conseil : Handoff, du même auteur, garde la mémoire de chaque projet quand vous passez d'un agent IA à un autre (pipx install ai-handoff, Python 3.10 ou plus récent)."
            )
        } else {
            String::new()
        };
        window.set_handoff_hint(hint.into());
    }

    fn show_report(&self, window: &AppWindow) {
        let lang = self.lang;
        let summary = match report::summarize(&self.data_dir) {
            Ok(summary) => summary,
            Err(err) => {
                window.set_report_period(err.text(lang).into());
                return;
            }
        };
        let share = |count: usize, total: usize| {
            if total == 0 {
                -1.0
            } else {
                count as f32 * 100.0 / total as f32
            }
        };
        let mut rows: Vec<ReportRow> = summary
            .agents
            .iter()
            .map(|(agent, counts)| (agent.clone(), counts))
            .chain(std::iter::once((
                tr!(lang, "Total", "Total"),
                &summary.total,
            )))
            .map(|(agent, counts)| ReportRow {
                agent: agent.to_uppercase().into(),
                delegations: counts.delegations as i32,
                escalated: counts.escalated as i32,
                escalated_share: share(counts.escalated, counts.delegations),
                without_footer: counts.without_footer as i32,
                without_footer_share: share(counts.without_footer, counts.delegations),
            })
            .collect();
        let period = if summary.total.delegations == 0 {
            rows.clear();
            tr!(
                lang,
                "No delegation recorded yet.",
                "Aucune délégation notée pour l'instant."
            )
        } else {
            let day = |time: &Option<String>| -> String {
                time.as_deref()
                    .map(|time| time.chars().take(10).collect())
                    .unwrap_or_default()
            };
            let (first, last) = (day(&summary.first), day(&summary.last));
            tr!(
                lang,
                "Delegations from {first} to {last}",
                "Délégations du {first} au {last}"
            )
        };
        window.set_report_period(period.into());
        window.set_report_rows(ModelRc::new(VecModel::from(rows)));
        let journal = shorten(&report::journal_path(&self.data_dir), &self.env.home);
        window.set_journal(tr!(lang, "Journal: {journal}", "Journal : {journal}").into());
    }

    /// Show what installing the ticked clients would change, without writing anything.
    fn preview(&mut self, window: &AppWindow) {
        let lang = self.lang;
        let chosen = self.chosen();
        if chosen.is_empty() {
            window.set_planned(ModelRc::default());
            window.set_can_apply(false);
            window.set_install_message(
                tr!(lang, "No client is ticked.", "Aucun client n'est coché.").into(),
            );
            return;
        }
        let mut session = Session::in_data_dir(true, &self.data_dir);
        let outcomes = install::apply(&mut session, &chosen, &self.env);
        let lines = self.lines(&outcomes);
        window.set_can_apply(!lines.is_empty() && outcomes.iter().all(Outcome::ok));
        let message = if lines.is_empty() {
            tr!(
                lang,
                "Everything is already in place.",
                "Tout est déjà en place."
            )
        } else {
            tr!(
                lang,
                "Planned changes; each file is backed up first.",
                "Changements prévus ; chaque fichier est sauvegardé avant modification."
            )
        };
        window.set_install_message(message.into());
        window.set_planned(list(lines));
    }

    fn apply(&mut self, window: &AppWindow) {
        let lang = self.lang;
        let chosen = self.chosen();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::apply(&mut session, &chosen, &self.env);
        let names: Vec<String> = chosen.iter().map(|i| i.name.to_string()).collect();
        let declined: Vec<String> = self
            .candidates
            .iter()
            .filter(|(_, selected)| !*selected)
            .map(|(name, _)| name.to_string())
            .collect();
        let mut message = self.done(&outcomes, &session);
        if let Err(err) = Choices::remember(&self.data_dir, &names, &declined) {
            message = format!("{message}\n{}", err.text(lang));
        }
        window.set_install_message(message.into());
        window.set_planned(ModelRc::default());
        window.set_can_apply(false);
        self.show_status(window);
    }

    /// Reinstall what is missing in the clients the user chose.
    fn repair(&mut self, window: &AppWindow) {
        let Some(choices) = Choices::effective(&self.data_dir, &self.env) else {
            return;
        };
        let targets: Vec<Integration> = overview::clients(&self.env, Some(&choices))
            .into_iter()
            .filter(|client| client.chosen && client.detected)
            .map(|client| client.integration)
            .collect();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::apply(&mut session, &targets, &self.env);
        let message = self.done(&outcomes, &session);
        self.show_status(window);
        window.set_status_message(message.into());
    }

    fn remove_all(&mut self, window: &AppWindow) {
        let lang = self.lang;
        let all = vario_core::clients::integrations();
        let mut session = Session::in_data_dir(false, &self.data_dir);
        let outcomes = install::remove(&mut session, &all, &self.env);
        let mut message = self.done(&outcomes, &session);
        if let Err(err) = Choices::delete(&self.data_dir) {
            message = format!("{message}\n{}", err.text(lang));
        }
        self.show_status(window);
        window.set_status_message(message.into());
    }

    /// One line per change or error, with the client's name.
    fn lines(&self, outcomes: &[Outcome]) -> Vec<String> {
        let lang = self.lang;
        let mut lines = Vec::new();
        for outcome in outcomes {
            for change in &outcome.changes {
                let path = shorten(&change.path, &self.env.home);
                let action = change.action.text(lang);
                lines.push(format!("{} — {path} : {action}", outcome.label));
            }
            for error in &outcome.errors {
                lines.push(format!(
                    "{} — ⚠ {}",
                    outcome.label,
                    error
                        .text(lang)
                        .replace(&self.env.home.display().to_string(), "~")
                ));
            }
        }
        lines
    }

    fn done(&self, outcomes: &[Outcome], session: &Session) -> String {
        let lang = self.lang;
        let count = session.changes().len();
        let mut message = if outcomes.iter().all(Outcome::ok) {
            tr!(
                lang,
                "Done: {count} change(s).",
                "Terminé : {count} changement(s)."
            )
        } else {
            let errors: Vec<String> = self
                .lines(outcomes)
                .into_iter()
                .filter(|line| line.contains('⚠'))
                .collect();
            let errors = errors.join("\n");
            tr!(
                lang,
                "Done with errors: {count} change(s).\n{errors}",
                "Terminé avec des erreurs : {count} changement(s).\n{errors}"
            )
        };
        if count > 0 {
            let folder = shorten(session.backup_dir(), &self.env.home);
            message += &tr!(lang, "\nBackups: {folder}", "\nSauvegardes : {folder}");
        }
        message
    }
}

/// `--version` and `--help` answer in the terminal instead of opening a window.
fn answer_in_terminal(args: &[String], lang: Lang) -> Option<String> {
    let has = |names: [&str; 2]| args.iter().any(|arg| names.contains(&arg.as_str()));
    if has(["--version", "-V"]) {
        return Some(format!("vario-gui {}", env!("CARGO_PKG_VERSION")));
    }
    has(["--help", "-h"]).then(|| {
        tr!(
            lang,
            "Vario's window: install, check, repair and remove Vario.\n\n\
             Usage: vario-gui [--gauge]\n\n\
             Options:\n  \
             --gauge        the floating quota gauge alone\n  \
             -h, --help     this help\n  \
             -V, --version  the version",
            "La fenêtre de Vario : installer, vérifier, réparer et retirer Vario.\n\n\
             Utilisation : vario-gui [--gauge]\n\n\
             Options :\n  \
             --gauge        la jauge de quota flottante seule\n  \
             -h, --help     cette aide\n  \
             -V, --version  la version"
        )
    })
}

fn note_text(note: Note, lang: Lang) -> String {
    match note {
        Note::NotFound => tr!(lang, "client not found", "client introuvable"),
        Note::Incomplete => tr!(
            lang,
            "incomplete: click Repair",
            "incomplet : cliquez sur Réparer"
        ),
        Note::Declined => tr!(
            lang,
            "set aside: tick it in Install to add it",
            "écarté : cochez-le dans Installation pour l'ajouter"
        ),
        Note::NotConfigured => tr!(
            lang,
            "not configured: see Install",
            "non configuré : voir Installation"
        ),
        Note::ConnectHandoff => tr!(
            lang,
            "connect Handoff: handoff doctor --fix",
            "connecter Handoff : handoff doctor --fix"
        ),
    }
}

fn list(lines: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = lines
        .into_iter()
        .map(|line| StandardListViewItem::from(SharedString::from(line)))
        .collect();
    ModelRc::new(VecModel::from(items))
}

/// Paths under the home directory are shown with `~`.
fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        }
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::fs;

    const USER_RULES: &str = "# My rules\n";

    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        data_dir: PathBuf,
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

        fn open(&self) -> (AppWindow, Rc<RefCell<App>>) {
            i_slint_backend_testing::init_no_event_loop();
            open(self.env.clone(), self.data_dir.clone(), Lang::En).unwrap()
        }

        fn claude(&self) -> PathBuf {
            self.env.home.join(".claude")
        }
    }

    #[test]
    fn version_and_help_answer_without_a_window() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(
            answer_in_terminal(&args(&["--version"]), Lang::En).as_deref(),
            Some(concat!("vario-gui ", env!("CARGO_PKG_VERSION")))
        );
        let help = answer_in_terminal(&args(&["-h"]), Lang::Fr).unwrap();
        assert!(help.starts_with("La fenêtre de Vario"));
        assert!(help.contains("--gauge"));
        assert_eq!(answer_in_terminal(&args(&["--gauge"]), Lang::En), None);
        assert_eq!(answer_in_terminal(&[], Lang::En), None);
    }

    #[test]
    fn a_fresh_machine_installs_from_the_window() {
        let fixture = Fixture::new();
        let (window, _app) = fixture.open();
        assert!(window.get_status_message().contains("not set up yet"));
        let choices: Vec<(String, bool)> = window
            .get_choices()
            .iter()
            .map(|choice| (choice.label.to_string(), choice.selected))
            .collect();
        assert_eq!(choices, vec![("Claude Code".to_string(), true)]);

        window.invoke_preview();
        assert_eq!(
            window.get_planned().row_count(),
            5,
            "two agents, CLAUDE.md, then settings.json for the hooks and the status line"
        );
        assert!(window.get_can_apply());
        assert!(!fixture.claude().join("agents").exists());
        assert!(!fixture.claude().join("settings.json").exists());

        window.invoke_apply();
        assert!(
            fixture
                .claude()
                .join("agents")
                .join("vario-reader.md")
                .exists()
        );
        assert!(
            fixture
                .claude()
                .join("agents")
                .join("vario-worker.md")
                .exists()
        );
        assert!(
            window
                .get_install_message()
                .starts_with("Done: 5 change(s)."),
            "{}",
            window.get_install_message()
        );
        let row = window
            .get_clients()
            .iter()
            .find(|row| row.label == "Claude Code")
            .expect("a Claude Code row");
        assert_eq!(
            (row.subagents, row.instructions, row.hooks, row.quota),
            (1, 1, 1, 1)
        );
        assert_eq!(row.handoff, -1, "Handoff is not installed here");
        assert_eq!(window.get_status_message(), "Everything is in place.");
        let setup = fs::read_to_string(fixture.data_dir.join("setup.json")).unwrap();
        assert!(setup.contains("claude"), "{setup}");
    }

    #[test]
    fn manual_mode_declines_an_unticked_client() {
        let fixture = Fixture::new();
        let (window, _app) = fixture.open();
        window.set_manual(true);
        window.invoke_mode_changed();
        window.invoke_toggle_choice(0, false);
        window.invoke_preview();
        assert!(
            window
                .get_install_message()
                .contains("No client is ticked.")
        );
        assert!(!window.get_can_apply());
    }

    #[test]
    fn remove_all_puts_the_files_back() {
        let fixture = Fixture::new();
        let (window, _app) = fixture.open();
        window.invoke_apply();
        assert!(
            fixture
                .claude()
                .join("agents")
                .join("vario-reader.md")
                .exists()
        );

        window.invoke_remove_all();
        let rules = fs::read_to_string(fixture.claude().join("CLAUDE.md")).unwrap();
        assert_eq!(rules, USER_RULES);
        assert!(
            !fixture
                .claude()
                .join("agents")
                .join("vario-reader.md")
                .exists()
        );
        assert!(
            !fixture
                .claude()
                .join("agents")
                .join("vario-worker.md")
                .exists()
        );
        assert!(!fixture.claude().join("settings.json").exists());
        assert!(!fixture.data_dir.join("setup.json").exists());
    }

    #[test]
    fn the_delegations_tab_reads_the_journal() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.data_dir).unwrap();
        let lines = [
            r#"{"time":"2026-10-16T10:00:00+01:00","client":"claude","agent":"vario-worker","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-16T11:00:00+01:00","client":"claude","agent":"vario-worker","result":"stuck","escalate":true}"#,
        ];
        fs::write(
            report::journal_path(&fixture.data_dir),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let (window, _app) = fixture.open();
        let rows: Vec<ReportRow> = window.get_report_rows().iter().collect();
        let worker = rows
            .iter()
            .find(|row| row.agent == "VARIO-WORKER")
            .expect("a vario-worker channel");
        assert_eq!(worker.delegations, 2);
        assert_eq!(worker.escalated, 1);
        assert_eq!(worker.escalated_share, 50.0);
        assert_eq!(worker.without_footer_share, 0.0);
        assert!(rows.iter().any(|row| row.agent == "TOTAL"));
        assert!(window.get_report_period().contains("Delegations from"));
    }

    /// The text of the first string literal in `text`, with its escapes resolved.
    fn first_literal(text: &str) -> Option<String> {
        let mut chars = text.chars();
        if chars.next()? != '"' {
            return None;
        }
        let mut out = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Every (context, text) of the catalogue with a non-empty translation.
    fn translated(po: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let (mut context, mut id) = (String::new(), String::new());
        for line in po.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("msgctxt ") {
                context = first_literal(rest).unwrap_or_default();
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                id = first_literal(rest).unwrap_or_default();
            } else if let Some(rest) = line.strip_prefix("msgstr ")
                && !id.is_empty()
                && !first_literal(rest).unwrap_or_default().is_empty()
            {
                found.push((std::mem::take(&mut context), std::mem::take(&mut id)));
            }
        }
        found
    }

    /// Every (component, text) marked with `@tr`: Slint looks a text up in the context
    /// of the component that holds it.
    fn marked(slint: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut component = String::new();
        for line in slint
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
        {
            let words: Vec<&str> = line.split_whitespace().collect();
            if let Some(at) = words.iter().position(|word| *word == "component") {
                component = words.get(at + 1).unwrap_or(&"").to_string();
            }
            for rest in line.split("@tr(").skip(1) {
                if let Some(text) = first_literal(rest) {
                    found.push((component.clone(), text));
                }
            }
        }
        found
    }

    #[test]
    fn every_interface_text_is_translated_to_french() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let marked: Vec<(String, String)> = ["app.slint", "gauge.slint", "theme.slint"]
            .iter()
            .flat_map(|file| marked(&fs::read_to_string(root.join("ui").join(file)).unwrap()))
            .collect();
        let po = fs::read_to_string(
            root.join("lang")
                .join("fr")
                .join("LC_MESSAGES")
                .join("vario-gui.po"),
        )
        .unwrap();
        let translated = translated(&po);
        assert!(marked.len() > 30, "found only {} texts", marked.len());
        let missing: Vec<&(String, String)> = marked
            .iter()
            .filter(|text| !translated.contains(text))
            .collect();
        assert!(missing.is_empty(), "not translated to French: {missing:#?}");
    }
}
