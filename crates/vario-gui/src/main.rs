//! Vario's graphical interface: the same core as the `vario` command, for people who
//! would rather not use a terminal. It installs, checks, repairs and removes Vario,
//! and shows the delegation journal.

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

mod gauge;

slint::include_modules!();

/// What the window works on.
struct App {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
    /// The clients found on this machine, with whether each is ticked for install.
    candidates: Vec<(&'static str, bool)>,
    /// The floating gauge, created the first time it is opened.
    gauge: Option<GaugeWindow>,
}

fn main() -> Result<(), slint::PlatformError> {
    let mut env = Env::current();
    env.exe = command_line_path(&env);
    let lang = Lang::detect(&env);
    // `vario-gui --gauge`: the floating gauge alone, as a desktop widget.
    if std::env::args().any(|arg| arg == "--gauge") {
        let gauge = open_gauge(env.clone(), paths::data_dir(), lang)?;
        gauge.on_close(|| {
            let _ = slint::quit_event_loop();
        });
        let main_window: Rc<RefCell<Option<MainWindow>>> = Rc::default();
        gauge.on_open_main(move || {
            let mut main_window = main_window.borrow_mut();
            if main_window.is_none() {
                *main_window = open(env.clone(), paths::data_dir(), lang).ok();
            }
            if let Some((window, _)) = main_window.as_ref() {
                let _ = window.show();
            }
        });
        gauge.show()?;
        return slint::run_event_loop();
    }
    let (window, _app) = open(env, paths::data_dir(), lang)?;
    // Opens on another tab, for screenshots: VARIO_GUI_TAB=0..3.
    if let Some(tab) = std::env::var("VARIO_GUI_TAB")
        .ok()
        .and_then(|tab| tab.parse().ok())
    {
        window.set_current_tab(tab);
    }
    window.run()
}

/// The window, filled and wired to an [`App`] working on `env` and `data_dir`.
fn open(
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
) -> Result<(AppWindow, Rc<RefCell<App>>), slint::PlatformError> {
    let window = AppWindow::new()?;
    if lang == Lang::Fr {
        let _ = slint::select_bundled_translation("fr");
    }
    window.set_version(env!("CARGO_PKG_VERSION").into());

    let app = Rc::new(RefCell::new(App {
        env,
        data_dir,
        lang,
        candidates: Vec::new(),
        gauge: None,
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
    window.on_open_gauge(on(|app, _| app.show_gauge()));
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

/// The main window and the state behind it.
type MainWindow = (AppWindow, Rc<RefCell<App>>);

/// The floating gauge, fed from a background thread.
fn open_gauge(
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
) -> Result<GaugeWindow, slint::PlatformError> {
    let gauge = GaugeWindow::new()?;
    if lang == Lang::Fr {
        let _ = slint::select_bundled_translation("fr");
    }
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
    /// Open the floating gauge beside the main window; closing it only hides it.
    fn show_gauge(&mut self) {
        if self.gauge.is_none() {
            let Ok(gauge) = open_gauge(self.env.clone(), self.data_dir.clone(), self.lang) else {
                return;
            };
            let weak = gauge.as_weak();
            gauge.on_close(move || {
                if let Some(gauge) = weak.upgrade() {
                    let _ = gauge.hide();
                }
            });
            self.gauge = Some(gauge);
        }
        if let Some(gauge) = &self.gauge {
            let _ = gauge.show();
        }
    }

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
        let cell = |client: &overview::ClientState, kind: Kind| {
            match client.status.iter().find(|(k, _)| *k == kind) {
                Some((_, true)) => "✔",
                Some((_, false)) => "✗",
                None => "—",
            }
            .into()
        };
        let rows: Vec<ClientRow> = clients
            .iter()
            .map(|client| ClientRow {
                label: client.integration.label.into(),
                subagents: cell(client, Kind::Subagents),
                instructions: cell(client, Kind::Instructions),
                hooks: cell(client, Kind::Hooks),
                quota: cell(client, Kind::Quota),
                handoff: match client.handoff {
                    State::Connected => tr!(lang, "connected", "connecté"),
                    State::NotConnected => tr!(lang, "not connected", "non connecté"),
                    State::Absent => tr!(lang, "not installed", "non installé"),
                }
                .into(),
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
        let share = |count: usize, total: usize| -> SharedString {
            if count == 0 || total == 0 {
                count.to_string().into()
            } else {
                format!("{count} ({} %)", (count * 100 + total / 2) / total).into()
            }
        };
        let mut rows: Vec<ReportRow> = summary
            .agents
            .iter()
            .map(|(agent, counts)| (agent.clone(), counts))
            .chain(std::iter::once(("Total".to_string(), &summary.total)))
            .map(|(agent, counts)| ReportRow {
                agent: agent.into(),
                delegations: counts.delegations.to_string().into(),
                escalated: share(counts.escalated, counts.delegations),
                without_footer: share(counts.without_footer, counts.delegations),
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
        assert_eq!(row.subagents, "✔");
        assert_eq!(row.instructions, "✔");
        assert_eq!(row.hooks, "✔");
        assert_eq!(row.quota, "✔");
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
            .find(|row| row.agent == "vario-worker")
            .expect("a vario-worker row");
        assert_eq!(worker.delegations, "2");
        assert_eq!(worker.escalated, "1 (50 %)");
        assert!(rows.iter().any(|row| row.agent == "Total"));
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

    /// Every `msgid` of the catalogue with a non-empty `msgstr`.
    fn translated_msgids(po: &str) -> Vec<String> {
        #[derive(PartialEq)]
        enum In {
            Nothing,
            Id,
            Str,
        }
        let (mut state, mut id, mut string) = (In::Nothing, String::new(), String::new());
        let mut found = Vec::new();
        let mut flush = |id: &mut String, string: &mut String| {
            if !id.is_empty() && !string.is_empty() {
                found.push(std::mem::take(id));
            }
            id.clear();
            string.clear();
        };
        for line in po.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("msgid ") {
                flush(&mut id, &mut string);
                state = In::Id;
                id = first_literal(rest).unwrap_or_default();
            } else if let Some(rest) = line.strip_prefix("msgstr ") {
                state = In::Str;
                string = first_literal(rest).unwrap_or_default();
            } else if line.starts_with('"') {
                let part = first_literal(line).unwrap_or_default();
                match state {
                    In::Id => id += &part,
                    In::Str => string += &part,
                    In::Nothing => {}
                }
            }
        }
        flush(&mut id, &mut string);
        found
    }

    #[test]
    fn every_interface_text_is_translated_to_french() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let slint: String = ["app.slint", "gauge.slint"]
            .iter()
            .map(|file| fs::read_to_string(root.join("ui").join(file)).unwrap())
            .collect();
        let po = fs::read_to_string(
            root.join("lang")
                .join("fr")
                .join("LC_MESSAGES")
                .join("vario-gui.po"),
        )
        .unwrap();
        let translated = translated_msgids(&po);
        let texts: Vec<String> = slint
            .split("@tr(")
            .skip(1)
            .filter_map(first_literal)
            .collect();
        assert!(texts.len() > 20, "found only {} texts", texts.len());
        let missing: Vec<&String> = texts
            .iter()
            .filter(|text| !translated.contains(text))
            .collect();
        assert!(missing.is_empty(), "not translated to French: {missing:#?}");
    }
}
