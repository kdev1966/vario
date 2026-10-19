//! Vario's interface: the floating gauge and the main window, drawn by the system's web
//! view through Tauri. All logic lives in vario-core (`vario_core::gauge`,
//! `vario_core::panel`); the pages in `ui/` only lay out what the commands below return.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;

use std::path::PathBuf;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use vario_core::clients::{CodeGeeXActivity, Env};
use vario_core::gauge::Reading;
use vario_core::install;
use vario_core::instance::{self, Want};
use vario_core::lang::Lang;
use vario_core::panel::{Choice, Panel, Plan, Report, Status};
use vario_core::{paths, quota, tr, usage};

/// The quota is re-read this often; it changes after each answer.
const QUOTA_EVERY: Duration = Duration::from_secs(5);
/// The logs are measured this often.
const USAGE_EVERY: Duration = Duration::from_secs(60);
/// How often requests from later runs are read.
const POLL: Duration = Duration::from_millis(300);

/// The commands that detect the clients or write files are `async`: Tauri then runs
/// them off the main thread, which would otherwise freeze both windows meanwhile
/// (detecting the clients takes seconds).
struct Ctx {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
    /// Read by the pages' looks (status, choices, report) side by side; written by the
    /// ticks and the installs.
    panel: RwLock<Panel>,
}

impl Ctx {
    fn panel(&self) -> RwLockReadGuard<'_, Panel> {
        self.panel
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn panel_mut(&self) -> RwLockWriteGuard<'_, Panel> {
        self.panel
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The language of the labels: the same as the values' (`LANG` first, else the system).
#[tauri::command]
fn ui_lang(ctx: State<Ctx>) -> &'static str {
    match ctx.lang {
        Lang::En => "en",
        Lang::Fr => "fr",
    }
}

/// A page's error, on the program's output.
#[tauri::command]
fn js_error(page: String, message: String) {
    eprintln!("vario-gui: {page}: {message}");
}

#[tauri::command]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The page the main window opens on: `VARIO_GUI_TAB=0..3`, for screenshots.
#[tauri::command]
fn first_page() -> u8 {
    std::env::var("VARIO_GUI_TAB")
        .ok()
        .and_then(|tab| tab.parse().ok())
        .filter(|tab| *tab < 4)
        .unwrap_or(0)
}

/// The reading of the moment, for the gauge's first drawing; the next ones are events.
#[tauri::command]
fn reading(ctx: State<Ctx>) -> Reading {
    Reading::read(
        &ctx.data_dir,
        &quota::usage_cache(&ctx.env),
        ctx.lang,
        None,
        quota::now(),
    )
}

#[tauri::command]
async fn status(ctx: State<'_, Ctx>) -> Result<Status, String> {
    Ok(ctx.panel().status())
}

#[tauri::command]
async fn choices(ctx: State<'_, Ctx>) -> Result<Vec<Choice>, String> {
    Ok(ctx.panel().choices())
}

#[tauri::command]
async fn report(ctx: State<'_, Ctx>) -> Result<Report, String> {
    Ok(ctx.panel().report())
}

/// Every detected client ticked again, as when the mode changes.
#[tauri::command]
async fn reset_candidates(ctx: State<'_, Ctx>) -> Result<Vec<Choice>, String> {
    Ok({
        let mut panel = ctx.panel_mut();
        panel.reset_candidates();
        panel.choices()
    })
}

#[tauri::command]
async fn toggle(ctx: State<'_, Ctx>, index: usize, checked: bool) -> Result<Vec<Choice>, String> {
    Ok({
        let mut panel = ctx.panel_mut();
        panel.toggle(index, checked);
        panel.choices()
    })
}

#[tauri::command]
fn set_gauge_with_sessions(ctx: State<Ctx>, on: bool) {
    ctx.panel().set_gauge_with_sessions(on);
}

#[tauri::command]
async fn preview(ctx: State<'_, Ctx>) -> Result<Plan, String> {
    Ok(ctx.panel().preview())
}

#[tauri::command]
async fn apply(ctx: State<'_, Ctx>) -> Result<String, String> {
    Ok(ctx.panel_mut().apply())
}

#[tauri::command]
async fn repair(ctx: State<'_, Ctx>) -> Result<Option<String>, String> {
    Ok(ctx.panel_mut().repair())
}

#[tauri::command]
async fn remove_all(ctx: State<'_, Ctx>) -> Result<String, String> {
    Ok(ctx.panel_mut().remove_all())
}

#[tauri::command]
fn open_gauge(app: AppHandle) {
    shell::show(&app, Want::Gauge);
}

#[tauri::command]
fn open_main(app: AppHandle) {
    shell::show(&app, Want::Main);
}

#[tauri::command]
fn close_gauge(app: AppHandle) {
    shell::close(&app, shell::GAUGE);
}

#[tauri::command]
fn close_main(app: AppHandle) {
    shell::close(&app, shell::MAIN);
}

fn main() {
    let mut env = Env::current();
    env.exe = command_line_path(&env);
    let lang = Lang::detect(&env);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(answer) = answer_in_terminal(&args, lang) {
        println!("{answer}");
        return;
    }
    // `vario-gui --gauge`: the floating gauge, as a desktop widget.
    let want = if args.iter().any(|arg| arg == "--gauge") {
        Want::Gauge
    } else {
        Want::Main
    };
    let data_dir = paths::data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let Some(_instance) = instance::claim(&data_dir) else {
        instance::request(&data_dir, want);
        return;
    };
    let data_dir_for_requests = data_dir.clone();
    let ctx = Ctx {
        panel: RwLock::new(Panel::new(env.clone(), data_dir.clone(), lang)),
        env,
        data_dir,
        lang,
    };
    tauri::Builder::default()
        .manage(ctx)
        .manage(shell::Shell::default())
        .invoke_handler(tauri::generate_handler![
            js_error,
            ui_lang,
            version,
            first_page,
            reading,
            status,
            choices,
            report,
            reset_candidates,
            toggle,
            set_gauge_with_sessions,
            preview,
            apply,
            repair,
            remove_all,
            open_gauge,
            open_main,
            close_gauge,
            close_main,
        ])
        .on_window_event(shell::on_window_event)
        .setup(move |app| {
            // A desktop widget: no Dock icon until the main window opens.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            shell::show(&handle, want);
            // Ready before it is asked for: the web view and the clients' state take
            // seconds to come up.
            shell::prepare_main(&handle);
            feed(handle.clone());
            shell::follow_agent_apps(handle.clone());
            listen(handle, data_dir_for_requests);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Vario's interface could not start");
}

/// Hooks must run the `vario` command, not this interface: the one the installed hooks
/// already run, so the interface and the command line agree; else the one installed
/// next to the interface; else the one on `PATH`; else plain `vario`.
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

/// Answer the requests of the runs started after this one.
fn listen(app: AppHandle, data_dir: PathBuf) {
    thread::spawn(move || {
        loop {
            thread::sleep(POLL);
            for want in instance::take_requests(&data_dir) {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || shell::show(&handle, want));
            }
        }
    });
}

/// Keep the gauge up to date from a background thread, until the application ends.
fn feed(app: AppHandle) {
    thread::spawn(move || {
        let ctx = app.state::<Ctx>();
        let usage_cache = quota::usage_cache(&ctx.env);
        let mut codegeex_activity = CodeGeeXActivity::new(&ctx.env);
        let mut delegated = None;
        let mut measured: Option<Instant> = None;
        loop {
            if measured.is_none_or(|at| at.elapsed() >= USAGE_EVERY) {
                let since = quota::now() - 5 * 3600;
                delegated = usage::claude_code(&ctx.env, since).delegated_share();
                measured = Some(Instant::now());
            }
            let reading = Reading::read_with_activity(
                &ctx.data_dir,
                &usage_cache,
                ctx.lang,
                delegated,
                codegeex_activity.read(),
                quota::now(),
            );
            let _ = app.emit_to(shell::GAUGE, "reading", reading);
            thread::sleep(QUOTA_EVERY);
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

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

    /// The quoted texts that follow `start` in `page`, up to their closing quote.
    fn quoted_after(page: &str, start: &str) -> Vec<String> {
        page.split(start)
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(str::to_string)
            .collect()
    }

    /// The texts given to `t("…")`, not to calls whose name ends in t (`closest("…")`).
    fn calls_of_t(page: &str) -> Vec<String> {
        page.match_indices("t(\"")
            .filter(|(at, _)| {
                page[..*at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || "_$.".contains(before)))
            })
            .filter_map(|(at, _)| page[at + 3..].split('"').next())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn every_interface_text_is_translated_to_french() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut checked = 0;
        for file in ["index.html", "main.html"] {
            let page = std::fs::read_to_string(ui.join(file)).unwrap();
            // The texts to translate: `data-t="…"` attributes and `t("…")` calls.
            let marked: BTreeSet<String> = quoted_after(&page, "data-t=\"")
                .into_iter()
                .chain(calls_of_t(&page))
                .collect();
            // The page's French words: the `"…": "…"` keys of its `FR` table.
            let table = page
                .split("const FR = {")
                .nth(1)
                .unwrap()
                .split("};")
                .next()
                .unwrap();
            let french: BTreeSet<String> = table
                .split("\":")
                .filter_map(|before| before.rsplit('"').next())
                .map(str::to_string)
                .collect();
            let missing: Vec<&String> = marked.difference(&french).collect();
            assert!(
                missing.is_empty(),
                "{file}: not translated to French: {missing:#?}"
            );
            checked += marked.len();
        }
        assert!(checked > 40, "found only {checked} texts");
    }
}
