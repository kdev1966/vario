//! Prototype: Vario's floating gauge drawn by Tauri (the system's web view) instead of
//! Slint, to compare the two before choosing. The readings come from the same code,
//! `vario_core::gauge`; only the window and its drawing differ.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewWindow};
use vario_core::clients::{CodeGeeXActivity, Env};
use vario_core::gauge::Reading;
use vario_core::lang::Lang;
use vario_core::{paths, quota, usage};

/// The quota is re-read this often; it changes after each answer.
const QUOTA_EVERY: Duration = Duration::from_secs(5);
/// The logs are measured this often.
const USAGE_EVERY: Duration = Duration::from_secs(60);
/// The gauge's margin from the screen's corner, in logical pixels.
const MARGIN: f64 = 24.0;

struct Ctx {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
}

/// The language of the labels: the same as the values' (`LANG` first, else the system).
#[tauri::command]
fn ui_lang(ctx: State<Ctx>) -> &'static str {
    match ctx.lang {
        Lang::En => "en",
        Lang::Fr => "fr",
    }
}

/// The reading of the moment, for the first drawing; the next ones come as events.
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

/// The main window stays the Slint one in this prototype.
#[tauri::command]
fn open_main() {
    let _ = std::process::Command::new("vario-gui").spawn();
}

#[tauri::command]
fn close_gauge(app: AppHandle) {
    app.exit(0);
}

fn main() {
    let env = Env::current();
    let lang = Lang::detect(&env);
    let ctx = Ctx {
        env,
        data_dir: paths::data_dir(),
        lang,
    };
    tauri::Builder::default()
        .manage(ctx)
        .invoke_handler(tauri::generate_handler![ui_lang, reading, open_main, close_gauge])
        .setup(|app| {
            // A desktop widget: no Dock icon and no place in the app switcher.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let gauge = app
                .get_webview_window("gauge")
                .ok_or("the gauge window is missing from tauri.conf.json")?;
            place(&gauge);
            #[cfg(target_os = "macos")]
            {
                macos::join_every_space(&gauge);
                macos::set_on_screen(&gauge, true);
            }
            #[cfg(not(target_os = "macos"))]
            gauge.show()?;
            feed(app.handle().clone());
            follow_agent_apps(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the gauge could not start");
}

/// In the screen's top right corner.
fn place(gauge: &WebviewWindow) {
    let (Ok(Some(monitor)), Ok(size)) = (gauge.current_monitor(), gauge.outer_size()) else {
        return;
    };
    let margin = (MARGIN * monitor.scale_factor()) as i32;
    let area = monitor.work_area();
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let y = area.position.y + margin;
    let _ = gauge.set_position(PhysicalPosition::new(x, y));
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
            let _ = app.emit("reading", reading);
            thread::sleep(QUOTA_EVERY);
        }
    });
}

/// Show the gauge over the agents' applications only. macOS only; elsewhere the gauge
/// stays above every window.
fn follow_agent_apps(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use std::sync::{Arc, Mutex};
        use vario_core::gauge::{FOLLOW_EVERY, Follow};
        let follow = Arc::new(Mutex::new(Follow::default()));
        thread::spawn(move || {
            loop {
                thread::sleep(FOLLOW_EVERY);
                let follow = Arc::clone(&follow);
                let handle = app.clone();
                // AppKit wants its windows and the workspace on the main thread.
                let _ = app.run_on_main_thread(move || {
                    let (Some(gauge), Some(front)) =
                        (handle.get_webview_window("gauge"), macos::front())
                    else {
                        return;
                    };
                    let shown = follow.lock().map(|mut follow| follow.step(&front));
                    if let Ok(Some(shown)) = shown {
                        macos::set_on_screen(&gauge, shown);
                    }
                });
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // one pointer cast, see below; the crate otherwise denies unsafe
mod macos {
    use objc2_app_kit::{NSRunningApplication, NSWindow, NSWindowCollectionBehavior, NSWorkspace};
    use tauri::WebviewWindow;
    use vario_core::gauge::Front;

    /// Shown on every Space and over full-screen apps, like the Slint gauge.
    pub fn join_every_space(gauge: &WebviewWindow) {
        with_ns_window(gauge, |window| {
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        });
    }

    /// On screen or away, without taking the focus from the agent's application: Tauri's
    /// own `show` makes the window key.
    pub fn set_on_screen(gauge: &WebviewWindow, shown: bool) {
        with_ns_window(gauge, |window| {
            if shown {
                window.orderFrontRegardless();
            } else {
                window.orderOut(None);
            }
        });
    }

    /// The application in front, from the main thread.
    pub fn front() -> Option<Front> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        if pid == NSRunningApplication::currentApplication().processIdentifier() {
            return Some(Front::Vario);
        }
        let bundle = app.bundleIdentifier().map(|b| b.to_string());
        Some(Front::App { pid, bundle })
    }

    fn with_ns_window(gauge: &WebviewWindow, job: impl FnOnce(&NSWindow)) {
        let Ok(pointer) = gauge.ns_window() else {
            return;
        };
        // SAFETY: Tauri hands out the NSWindow of a window it owns and keeps alive while
        // `gauge` is borrowed; callers run on the main thread.
        if let Some(window) = unsafe { pointer.cast::<NSWindow>().as_ref() } {
            job(window);
        }
    }
}
