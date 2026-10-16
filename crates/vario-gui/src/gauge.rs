//! The floating gauge: its readings, and the thread that refreshes them without ever
//! blocking the window (measuring the logs takes a fraction of a second).

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use vario_core::clients::Env;
use vario_core::lang::Lang;
use vario_core::quota::{self, Level, Quota};
use vario_core::{report, tr, usage};

use slint::ComponentHandle;

use crate::GaugeWindow;

/// The quota is re-read this often; it changes after each answer.
const QUOTA_EVERY: Duration = Duration::from_secs(5);
/// The logs are measured this often.
const USAGE_EVERY: Duration = Duration::from_secs(60);

/// What the gauge shows; values below 0 are not known yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub five_hour: f32,
    pub five_hour_reset: String,
    pub seven_day: f32,
    pub level: i32,
    pub delegated: f32,
    pub escalated: f32,
    pub updated: String,
}

impl Reading {
    /// Read the quota and the journal now; `delegated` comes from the last
    /// measurement of the logs.
    pub fn read(data_dir: &Path, lang: Lang, delegated: Option<f64>, now: i64) -> Reading {
        let quota = Quota::load(data_dir);
        let (five, seven) = quota
            .as_ref()
            .map_or((None, None), |quota| quota.current(now));
        let five_hour_reset = match five {
            Some(window) => {
                let left = quota::time_left(window.resets_at, now);
                tr!(lang, "resets in {left}", "remise à zéro dans {left}")
            }
            None => tr!(lang, "no quota yet", "pas encore de quota"),
        };
        // No current window (none seen yet, or all past their reset): unknown.
        let level = match quota.as_ref().map(|quota| quota.level(now)) {
            _ if five.is_none() && seven.is_none() => -1,
            None | Some(Level::Normal) => 0,
            Some(Level::Reinforced) => 1,
            Some(Level::Alert) => 2,
            Some(Level::Limit) => 3,
        };
        let journal = report::summarize(data_dir).unwrap_or_default();
        let escalated = (journal.total.delegations > 0)
            .then(|| journal.total.escalated as f64 * 100.0 / journal.total.delegations as f64);
        let time = chrono::Local::now().format("%H:%M").to_string();
        let updated = match quota.map(|quota| quota.captured_at) {
            Some(captured) => {
                let seen = chrono::DateTime::from_timestamp(captured, 0)
                    .map(|time| {
                        time.with_timezone(&chrono::Local)
                            .format("%H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                tr!(
                    lang,
                    "quota seen at {seen} · read at {time} · open Vario",
                    "quota relevé à {seen} · lu à {time} · ouvrir Vario"
                )
            }
            None => tr!(
                lang,
                "read at {time} · open Vario",
                "lu à {time} · ouvrir Vario"
            ),
        };
        let known = |value: Option<f64>| value.map_or(-1.0, |value| value as f32);
        Reading {
            five_hour: known(five.map(|w| w.used_percentage)),
            five_hour_reset,
            seven_day: known(seven.map(|w| w.used_percentage)),
            level,
            delegated: known(delegated),
            escalated: known(escalated),
            updated,
        }
    }

    pub fn show(&self, gauge: &GaugeWindow) {
        gauge.set_five_hour(self.five_hour);
        gauge.set_five_hour_reset(self.five_hour_reset.clone().into());
        gauge.set_seven_day(self.seven_day);
        gauge.set_level(self.level);
        gauge.set_delegated(self.delegated);
        gauge.set_escalated(self.escalated);
        gauge.set_updated(self.updated.clone().into());
    }
}

/// Keep the gauge up to date from a background thread, until the window is gone.
pub fn feed(gauge: &GaugeWindow, env: Env, data_dir: PathBuf, lang: Lang) {
    let weak = gauge.as_weak();
    thread::spawn(move || {
        let mut delegated = None;
        let mut measured: Option<Instant> = None;
        loop {
            if measured.is_none_or(|at| at.elapsed() >= USAGE_EVERY) {
                let since = quota::now() - 5 * 3600;
                delegated = usage::claude_code(&env, since).delegated_share();
                measured = Some(Instant::now());
            }
            let reading = Reading::read(&data_dir, lang, delegated, quota::now());
            if weak
                .upgrade_in_event_loop(move |gauge| reading.show(&gauge))
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
        macos::join_every_space(window);
    });
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
    use objc2_app_kit::{NSView, NSWindowCollectionBehavior};
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::winit::window::Window;

    /// NSWindow.collectionBehavior: shown on every Space and over full-screen apps.
    pub fn join_every_space(window: &Window) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: winit hands out the NSView of a window it owns, alive for as long as
        // `window` is borrowed, and this runs on the event loop's (main) thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        if let Some(ns_window) = view.window() {
            ns_window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        }
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
        };
        reading.show(&gauge);
        assert_eq!(gauge.get_five_hour(), 85.0);
        assert_eq!(gauge.get_level(), 2);
        assert_eq!(gauge.get_five_hour_reset(), "resets in 40 min");
        assert_eq!(gauge.get_updated(), "read at 10:00");
    }

    #[test]
    fn without_data_everything_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let reading = Reading::read(dir.path(), Lang::En, None, 1_800_000_000);
        assert_eq!(reading.five_hour, -1.0);
        assert_eq!(reading.seven_day, -1.0);
        assert_eq!(reading.level, -1);
        assert_eq!(reading.delegated, -1.0);
        assert_eq!(reading.escalated, -1.0);
        assert_eq!(reading.five_hour_reset, "no quota yet");
        assert!(reading.updated.contains("open Vario"));
    }

    #[test]
    fn reads_the_quota_the_level_and_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_800_000_000;
        Quota {
            captured_at: now - 30,
            five_hour: Some(quota::Window {
                used_percentage: 62.4,
                resets_at: now + 80 * 60,
            }),
            seven_day: Some(quota::Window {
                used_percentage: 41.0,
                resets_at: now + 86_400,
            }),
        }
        .save(dir.path())
        .unwrap();
        let journal = [
            r#"{"time":"2026-10-16T10:00:00+01:00","agent":"vario-worker","result":"ok","escalate":false}"#,
            r#"{"time":"2026-10-16T11:00:00+01:00","agent":"vario-worker","result":"stuck","escalate":true}"#,
        ];
        std::fs::write(report::journal_path(dir.path()), journal.join("\n")).unwrap();

        let reading = Reading::read(dir.path(), Lang::Fr, Some(12.0), now);
        assert_eq!(reading.five_hour, 62.4);
        assert_eq!(reading.seven_day, 41.0);
        assert_eq!(
            reading.level, 1,
            "62 % of the most used window: delegating more"
        );
        assert_eq!(reading.five_hour_reset, "remise à zéro dans 1 h 20");
        assert_eq!(reading.delegated, 12.0);
        assert_eq!(reading.escalated, 50.0);
        assert!(reading.updated.starts_with("quota relevé à"));
    }
}
