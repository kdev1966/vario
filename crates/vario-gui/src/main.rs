//! Vario's graphical interface: the same core as the `vario` command, for people who
//! would rather not use a terminal. The floating gauge follows the quota; the main
//! window installs, checks, repairs and removes Vario, and shows the delegation
//! journal.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};
use vario_core::clients::Env;
use vario_core::install;
use vario_core::instance::{self, Want};
use vario_core::lang::Lang;
use vario_core::panel::Panel;
use vario_core::{paths, tr};

use shell::Shell;

mod gauge;
mod shell;

slint::include_modules!();

/// What the window works on: a [`Panel`], whose views it copies into the window.
struct App {
    panel: Panel,
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
        panel: Panel::new(env, data_dir, lang),
    }));
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
        let app = Rc::clone(&app);
        window.on_gauge_setting_changed(move |on| {
            app.borrow().panel.set_gauge_with_sessions(on);
        });
    }
    window.on_repair(on(App::repair));
    window.on_remove_all(on(App::remove_all));
    window.on_preview(on(App::preview));
    window.on_apply(on(App::apply));
    window.on_mode_changed(on(|app, window| {
        app.panel.reset_candidates();
        app.show_candidates(window);
        window.set_planned(ModelRc::default());
        window.set_can_apply(false);
        window.set_install_message(SharedString::new());
    }));
    {
        let app = Rc::clone(&app);
        let weak = window.as_weak();
        window.on_toggle_choice(move |index, checked| {
            app.borrow_mut().panel.toggle(index as usize, checked);
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
    let usage_cache = vario_core::quota::usage_cache(&env);
    gauge::show(
        &vario_core::gauge::Reading::read(
            &data_dir,
            &usage_cache,
            lang,
            None,
            vario_core::quota::now(),
        ),
        &gauge,
    );
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
    fn refresh(&self, window: &AppWindow) {
        self.show_status(window);
        self.show_candidates(window);
        self.show_report(window);
    }

    fn show_candidates(&self, window: &AppWindow) {
        let choices: Vec<Choice> = self
            .panel
            .choices()
            .into_iter()
            .map(|choice| Choice {
                label: choice.label.into(),
                selected: choice.selected,
            })
            .collect();
        window.set_choices(ModelRc::new(VecModel::from(choices)));
    }

    fn show_status(&self, window: &AppWindow) {
        let status = self.panel.status();
        let rows: Vec<ClientRow> = status
            .clients
            .into_iter()
            .map(|client| ClientRow {
                label: client.label.into(),
                subagents: client.subagents,
                instructions: client.instructions,
                hooks: client.hooks,
                quota: client.quota,
                handoff: client.handoff,
                notes: client.notes.into(),
            })
            .collect();
        window.set_clients(ModelRc::new(VecModel::from(rows)));
        window.set_needs_fix(status.needs_fix);
        window.set_status_message(status.message.into());
        window.set_gauge_with_sessions(status.gauge_with_sessions);
        window.set_handoff_hint(status.handoff_hint.into());
    }

    fn show_report(&self, window: &AppWindow) {
        let report = self.panel.report();
        let rows: Vec<ReportRow> = report
            .rows
            .into_iter()
            .map(|row| ReportRow {
                agent: row.agent.into(),
                delegations: row.delegations,
                escalated: row.escalated,
                escalated_share: row.escalated_share,
                without_footer: row.without_footer,
                without_footer_share: row.without_footer_share,
            })
            .collect();
        window.set_report_period(report.period.into());
        window.set_report_rows(ModelRc::new(VecModel::from(rows)));
        window.set_journal(report.journal.into());
    }

    /// Show what installing the ticked clients would change, without writing anything.
    fn preview(&mut self, window: &AppWindow) {
        let plan = self.panel.preview();
        window.set_can_apply(plan.can_apply);
        window.set_install_message(plan.message.into());
        window.set_planned(list(plan.planned));
    }

    fn apply(&mut self, window: &AppWindow) {
        let message = self.panel.apply();
        window.set_install_message(message.into());
        window.set_planned(ModelRc::default());
        window.set_can_apply(false);
        self.show_status(window);
    }

    /// Reinstall what is missing in the clients the user chose.
    fn repair(&mut self, window: &AppWindow) {
        let Some(message) = self.panel.repair() else {
            return;
        };
        self.show_status(window);
        window.set_status_message(message.into());
    }

    fn remove_all(&mut self, window: &AppWindow) {
        let message = self.panel.remove_all();
        self.show_status(window);
        window.set_status_message(message.into());
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

fn list(lines: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = lines
        .into_iter()
        .map(|line| StandardListViewItem::from(SharedString::from(line)))
        .collect();
    ModelRc::new(VecModel::from(items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::fs;
    use std::path::Path;
    use vario_core::report;

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
