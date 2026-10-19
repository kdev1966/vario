//! The interface's two windows in one process: the floating gauge and the main window.
//!
//! Either opens the other. Released close to each other, they snap side by side, tops
//! aligned, and stay docked: dragging the gauge brings the main window along, and
//! dragging the main window away undocks it (the geometry is `vario_core::dock`'s).
//! Closing the main window leaves the gauge up; the process ends with its last window.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};
use vario_core::dock::{self, Rect, Side};
use vario_core::instance::Want;

pub const GAUGE: &str = "gauge";
pub const MAIN: &str = "main";

/// How close, in logical pixels, a released window must be to snap.
const REACH: f64 = 24.0;
/// The gauge's margin from the screen's corner, in logical pixels.
const MARGIN: f64 = 24.0;
/// A window counts as released when it has not moved for this long.
const SETTLE: Duration = Duration::from_millis(250);

/// The docking state, shared by the window events.
#[derive(Default)]
pub struct Shell(Mutex<Docking>);

#[derive(Default)]
struct Docking {
    /// The main window's side of the gauge, while the two are docked.
    docked: Option<Side>,
    /// Where Vario itself last put each window: the move the system reports back for
    /// it is not the user's.
    placed_gauge: Option<(i32, i32)>,
    placed_main: Option<(i32, i32)>,
    last_moved: Option<&'static str>,
    /// Counts the moves, so only the last one settles.
    moves: u64,
}

impl Shell {
    fn with<T>(&self, job: impl FnOnce(&mut Docking) -> T) -> T {
        let mut docking = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        job(&mut docking)
    }
}

/// Open the gauge or the main window, or bring it back.
pub fn show(app: &AppHandle, want: Want) {
    match want {
        Want::Gauge => show_gauge(app),
        Want::Main => show_main(app),
    }
}

fn show_gauge(app: &AppHandle) {
    let gauge = match app.get_webview_window(GAUGE) {
        Some(gauge) => gauge,
        None => {
            let Ok(gauge) =
                WebviewWindowBuilder::new(app, GAUGE, WebviewUrl::App("index.html".into()))
                    .title("Vario")
                    .inner_size(156.0, 494.0)
                    .resizable(false)
                    .decorations(false)
                    .transparent(true)
                    .shadow(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .visible_on_all_workspaces(true)
                    .focused(false)
                    .visible(false)
                    .build()
            else {
                return;
            };
            place_gauge(app, &gauge);
            #[cfg(target_os = "macos")]
            macos::join_every_space(&gauge);
            gauge
        }
    };
    put_on_screen(&gauge);
}

/// The gauge's first place: beside the main window when it is open, on its roomy side;
/// else in the screen's top right corner.
fn place_gauge(app: &AppHandle, gauge: &WebviewWindow) {
    let Some(gauge_rect) = rect(gauge) else {
        return;
    };
    match app
        .get_webview_window(MAIN)
        .and_then(|main| rect(&main).map(|r| (main, r)))
    {
        Some((main, main_rect)) => {
            let side = match dock::roomy_side(main_rect, screen(&main)) {
                // Room on the main window's left: the gauge goes there.
                Side::Left => Side::Right,
                Side::Right => Side::Left,
            };
            let (x, y) = dock::gauge_beside(main_rect, gauge_rect.width, side);
            place(app, GAUGE, x, y);
            app.state::<Shell>().with(|d| d.docked = Some(side));
        }
        None => {
            if let Some(screen) = screen(gauge) {
                let margin = (MARGIN * gauge.scale_factor().unwrap_or(1.0)) as i32;
                let (x, y) = dock::corner(screen, gauge_rect.width, margin);
                place(app, GAUGE, x, y);
            }
        }
    }
}

fn show_main(app: &AppHandle) {
    // A window with a Dock icon while the main window is open.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let main = match app.get_webview_window(MAIN) {
        Some(main) => main,
        None => {
            let Ok(main) =
                WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("main.html".into()))
                    .title("Vario")
                    .inner_size(640.0, 494.0)
                    .resizable(false)
                    .decorations(false)
                    .transparent(true)
                    .shadow(false)
                    .visible(false)
                    .build()
            else {
                return;
            };
            place_main(app, &main);
            main
        }
    };
    let _ = main.show();
    let _ = main.set_focus();
}

/// Beside the gauge, on its roomy side, and docked; else where the system puts it.
fn place_main(app: &AppHandle, main: &WebviewWindow) {
    let Some(gauge) = app.get_webview_window(GAUGE) else {
        let _ = main.center();
        return;
    };
    let (Some(gauge_rect), Some(main_rect)) = (rect(&gauge), rect(main)) else {
        return;
    };
    let side = dock::roomy_side(gauge_rect, screen(&gauge));
    let (x, y) = dock::main_beside(gauge_rect, main_rect.width, side);
    place(app, MAIN, x, y);
    app.state::<Shell>().with(|d| d.docked = Some(side));
}

pub fn close(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.close();
    }
}

/// Follow the windows' moves for the magnetism, and their closing.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    let label: &'static str = match window.label() {
        GAUGE => GAUGE,
        MAIN => MAIN,
        _ => return,
    };
    match event {
        WindowEvent::Moved(at) => moved(app, label, at.x, at.y),
        WindowEvent::Destroyed => {
            app.state::<Shell>().with(|d| d.docked = None);
            // Back to a desktop widget: no Dock icon while only the gauge shows.
            #[cfg(target_os = "macos")]
            if label == MAIN {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
        }
        _ => {}
    }
}

fn moved(app: &AppHandle, label: &'static str, x: i32, y: i32) {
    let shell = app.state::<Shell>();
    let ours = shell.with(|d| {
        let placed = match label {
            GAUGE => d.placed_gauge.take(),
            _ => d.placed_main.take(),
        };
        placed.is_some_and(|(px, py)| (px - x).abs() <= 2 && (py - y).abs() <= 2)
    });
    if ours {
        return;
    }
    if label == GAUGE {
        // The docked main window follows the gauge.
        let docked = shell.with(|d| d.docked);
        if let (Some(side), Some(gauge), Some(main)) = (
            docked,
            app.get_webview_window(GAUGE),
            app.get_webview_window(MAIN),
        ) && let (Some(gauge_rect), Some(main_rect)) = (rect(&gauge), rect(&main))
        {
            let (x, y) = dock::main_beside(gauge_rect, main_rect.width, side);
            place(app, MAIN, x, y);
        }
    } else {
        // Dragging the main window takes it off the gauge.
        shell.with(|d| d.docked = None);
    }
    let move_number = shell.with(|d| {
        d.last_moved = Some(label);
        d.moves += 1;
        d.moves
    });
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(SETTLE);
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if app.state::<Shell>().with(|d| d.moves) == move_number {
                settled(&app);
            }
        });
    });
}

/// A released window snaps to the other when their facing edges are close.
fn settled(app: &AppHandle) {
    let shell = app.state::<Shell>();
    let Some(which) = shell.with(|d| d.last_moved.take()) else {
        return;
    };
    let (Some(gauge), Some(main)) = (app.get_webview_window(GAUGE), app.get_webview_window(MAIN))
    else {
        return;
    };
    let shown = gauge.is_visible().unwrap_or(false) && main.is_visible().unwrap_or(false);
    if shell.with(|d| d.docked.is_some()) || !shown {
        return;
    }
    let (Some(gauge_rect), Some(main_rect)) = (rect(&gauge), rect(&main)) else {
        return;
    };
    let reach = (REACH * gauge.scale_factor().unwrap_or(1.0)) as i32;
    if let Some(side) = dock::snap(main_rect, gauge_rect, reach) {
        if which == MAIN {
            let (x, y) = dock::main_beside(gauge_rect, main_rect.width, side);
            place(app, MAIN, x, y);
        } else {
            let (x, y) = dock::gauge_beside(main_rect, gauge_rect.width, side);
            place(app, GAUGE, x, y);
        }
        shell.with(|d| d.docked = Some(side));
    }
}

/// Move a window, remembering that the move is Vario's own.
fn place(app: &AppHandle, label: &str, x: i32, y: i32) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    app.state::<Shell>().with(|d| match label {
        GAUGE => d.placed_gauge = Some((x, y)),
        _ => d.placed_main = Some((x, y)),
    });
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// A window's outer rectangle on the desktop, in physical pixels.
fn rect(window: &WebviewWindow) -> Option<Rect> {
    let at = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some(Rect {
        x: at.x,
        y: at.y,
        width: size.width as i32,
        height: size.height as i32,
    })
}

/// The usable area of the window's screen, in physical pixels.
fn screen(window: &WebviewWindow) -> Option<Rect> {
    let monitor = window.current_monitor().ok()??;
    let area = monitor.work_area();
    Some(Rect {
        x: area.position.x,
        y: area.position.y,
        width: area.size.width as i32,
        height: area.size.height as i32,
    })
}

/// On screen without taking the focus from the agent's application on macOS, where
/// Tauri's own `show` makes the window key.
fn put_on_screen(gauge: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    macos::set_on_screen(gauge, true);
    #[cfg(not(target_os = "macos"))]
    let _ = gauge.show();
}

/// Show the gauge over the agents' applications only. macOS only; elsewhere the gauge
/// stays above every window.
pub fn follow_agent_apps(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        use std::sync::Arc;
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
                        (handle.get_webview_window(GAUGE), macos::front())
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

    /// On screen or away, without taking the focus and without closing the window.
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
