//! The floating gauge: its readings, and the thread that refreshes them without ever
//! blocking the window (measuring the logs takes a fraction of a second).

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use vario_core::clients::Env;
use vario_core::gauge::{FOLLOW_EVERY, Follow, Reading};
use vario_core::lang::Lang;
use vario_core::{quota, usage};

use slint::ComponentHandle;

use crate::GaugeWindow;

/// The quota is re-read this often; it changes after each answer.
const QUOTA_EVERY: Duration = Duration::from_secs(5);
/// The logs are measured this often.
const USAGE_EVERY: Duration = Duration::from_secs(60);

/// Put a reading on the gauge window.
pub fn show(reading: &Reading, gauge: &GaugeWindow) {
    gauge.set_five_hour(reading.five_hour);
    gauge.set_five_hour_reset(reading.five_hour_reset.clone().into());
    gauge.set_seven_day(reading.seven_day);
    gauge.set_level(reading.level);
    gauge.set_delegated(reading.delegated);
    gauge.set_escalated(reading.escalated);
    gauge.set_updated(reading.updated.clone().into());
    gauge.set_stale(reading.stale);
    gauge.set_other(reading.other);
    gauge.set_usage(reading.usage);
    gauge.set_client(reading.client.clone().into());
    gauge.set_model(reading.model.clone().into());
    gauge.set_output(reading.output.clone().into());
    gauge.set_input(reading.input.clone().into());
    gauge.set_cache(reading.cache.clone().into());
    gauge.set_cost(reading.cost.clone().into());
}

/// Keep the gauge up to date from a background thread, until the window is gone.
pub fn feed(gauge: &GaugeWindow, env: Env, data_dir: PathBuf, lang: Lang) {
    let weak = gauge.as_weak();
    thread::spawn(move || {
        let usage_cache = quota::usage_cache(&env);
        let mut codegeex_activity = vario_core::clients::CodeGeeXActivity::new(&env);
        let mut delegated = None;
        let mut measured: Option<Instant> = None;
        loop {
            if measured.is_none_or(|at| at.elapsed() >= USAGE_EVERY) {
                let since = quota::now() - 5 * 3600;
                delegated = usage::claude_code(&env, since).delegated_share();
                measured = Some(Instant::now());
            }
            let reading = Reading::read_with_activity(
                &data_dir,
                &usage_cache,
                lang,
                delegated,
                codegeex_activity.read(),
                quota::now(),
            );
            if weak
                .upgrade_in_event_loop(move |gauge| show(&reading, &gauge))
                .is_err()
            {
                break;
            }
            thread::sleep(QUOTA_EVERY);
        }
    });
}

/// Make the shown gauge a floating system window: above other applications' windows,
/// on every desktop (macOS Spaces, full-screen apps included), and out of the
/// Windows taskbar. Call it after `show()`, once the system window exists.
pub fn float(gauge: &GaugeWindow) {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::window::WindowLevel;
    gauge.window().with_winit_window(|window| {
        window.set_window_level(WindowLevel::AlwaysOnTop);
        #[cfg(target_os = "windows")]
        {
            use slint::winit_030::winit::platform::windows::WindowExtWindows;
            window.set_skip_taskbar(true);
        }
        #[cfg(target_os = "macos")]
        {
            macos::join_every_space(window);
            // Back on screen if following the agent's application had put it away.
            macos::set_on_screen(window, true);
        }
    });
}

/// Show the gauge over the agents' applications only, from now on. macOS only; elsewhere
/// the gauge stays above every window.
pub fn follow_agent_apps(gauge: &GaugeWindow, timer: &slint::Timer) {
    #[cfg(target_os = "macos")]
    {
        use slint::winit_030::WinitWindowAccessor;
        let weak = gauge.as_weak();
        let mut follow = Follow::default();
        timer.start(slint::TimerMode::Repeated, FOLLOW_EVERY, move || {
            let (Some(gauge), Some(front)) = (weak.upgrade(), macos::front()) else {
                return;
            };
            if let Some(shown) = follow.step(&front) {
                gauge
                    .window()
                    .with_winit_window(|window| macos::set_on_screen(window, shown));
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (gauge, timer);
}

/// On macOS, show the Dock icon (and a place in the app switcher) while the main
/// window is open; without it, the gauge stays a desktop widget.
pub fn show_in_dock(show: bool) {
    #[cfg(target_os = "macos")]
    macos::show_in_dock(show);
    #[cfg(not(target_os = "macos"))]
    let _ = show;
}

/// Bring the window forward with the focus, also when the gauge started Vario as a
/// background widget.
pub fn bring_to_front(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    #[cfg(target_os = "macos")]
    macos::activate();
    window.with_winit_window(|window| window.focus_window());
}

/// Run as a desktop widget: on macOS, no Dock icon and no place in the app switcher.
/// Must run before any window is created.
pub fn select_widget_backend() -> Result<(), slint::PlatformError> {
    #[cfg(target_os = "macos")]
    {
        use slint::winit_030::winit::event_loop::EventLoop;
        use slint::winit_030::winit::platform::macos::{
            ActivationPolicy, EventLoopBuilderExtMacOS,
        };
        let mut builder = EventLoop::with_user_event();
        builder.with_activation_policy(ActivationPolicy::Accessory);
        slint::BackendSelector::new()
            .with_winit_event_loop_builder(builder)
            .select()?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // one pointer cast, see below; the crate otherwise denies unsafe
mod macos {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSRunningApplication, NSView, NSWindow,
        NSWindowCollectionBehavior, NSWorkspace,
    };
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::winit::window::Window;
    use vario_core::gauge::Front;

    pub fn show_in_dock(show: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let policy = if show {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        };
        NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
    }

    /// `activate` replaces it from macOS 14 only; Vario runs on earlier versions too.
    #[allow(deprecated)]
    pub fn activate() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
    }

    /// NSWindow.collectionBehavior: shown on every Space and over full-screen apps.
    pub fn join_every_space(window: &Window) {
        if let Some(ns_window) = ns_window(window) {
            ns_window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        }
    }

    /// Put the window on screen or away, without closing it: Slint would end the event
    /// loop with its last window hidden.
    pub fn set_on_screen(window: &Window, shown: bool) {
        if let Some(ns_window) = ns_window(window) {
            if shown {
                ns_window.orderFrontRegardless();
            } else {
                ns_window.orderOut(None);
            }
        }
    }

    /// The application in front, from the event loop's (main) thread.
    pub fn front() -> Option<Front> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        if pid == NSRunningApplication::currentApplication().processIdentifier() {
            return Some(Front::Vario);
        }
        let bundle = app.bundleIdentifier().map(|b| b.to_string());
        Some(Front::App { pid, bundle })
    }

    fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
        let Ok(handle) = window.window_handle() else {
            return None;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: winit hands out the NSView of a window it owns, alive for as long as
        // `window` is borrowed, and this runs on the event loop's (main) thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        view.window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_window_shows_a_reading() {
        i_slint_backend_testing::init_no_event_loop();
        let gauge = GaugeWindow::new().unwrap();
        let reading = Reading {
            five_hour: 85.0,
            five_hour_reset: "resets in 40 min".into(),
            seven_day: 30.0,
            level: 2,
            delegated: 22.0,
            escalated: 5.0,
            updated: "read at 10:00".into(),
            stale: true,
            other: false,
            usage: false,
            client: String::new(),
            model: String::new(),
            output: String::new(),
            input: String::new(),
            cache: String::new(),
            cost: String::new(),
        };
        show(&reading, &gauge);
        assert_eq!(gauge.get_five_hour(), 85.0);
        assert_eq!(gauge.get_level(), 2);
        assert_eq!(gauge.get_five_hour_reset(), "resets in 40 min");
        assert_eq!(gauge.get_updated(), "read at 10:00");
        assert!(gauge.get_stale());
        assert!(!gauge.get_other());
    }
}
