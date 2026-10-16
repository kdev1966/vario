//! The interface's two windows in one process: the floating gauge and the main window.
//!
//! Either opens the other. Released close to each other, they snap side by side, tops
//! aligned, and stay docked: dragging the gauge brings the main window along, and
//! dragging the main window away undocks it. Closing the main window leaves the gauge
//! up; the process ends with its last window.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, PhysicalPosition, Timer, TimerMode};
use vario_core::clients::Env;
use vario_core::lang::Lang;

use crate::dock::{self, Rect, Side};
use crate::instance::{self, Want};
use crate::{AppWindow, GaugeWindow, MainWindow, gauge};

/// How close, in logical pixels, a released window must be to snap.
const REACH: f32 = 24.0;
/// The gauge's margin from the screen's corner, in logical pixels.
const MARGIN: f32 = 24.0;
/// A window counts as released when it has not moved for this long.
const SETTLE: Duration = Duration::from_millis(250);
/// A new main window ignores clicks this long: it may open under the pointer.
const ARMING: Duration = Duration::from_millis(600);
/// How often requests from later `vario-gui` runs are read.
const POLL: Duration = Duration::from_millis(300);
/// Where the gauge was left, in physical pixels.
const PLACES: &str = "gui.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Gauge,
    Main,
}

pub struct Shell {
    env: Env,
    data_dir: PathBuf,
    lang: Lang,
    main: RefCell<Option<MainWindow>>,
    gauge: RefCell<Option<GaugeWindow>>,
    main_shown: Cell<bool>,
    gauge_shown: Cell<bool>,
    /// The main window's side of the gauge, while the two are docked.
    docked: Cell<Option<Side>>,
    /// Where Vario itself last put each window: the move the system reports back
    /// for it is not the user's.
    placed_gauge: Cell<Option<(i32, i32)>>,
    placed_main: Cell<Option<(i32, i32)>>,
    last_moved: Cell<Option<Which>>,
    settle: Timer,
    poll: Timer,
}

impl Shell {
    pub fn new(env: Env, data_dir: PathBuf, lang: Lang) -> Rc<Shell> {
        Rc::new(Shell {
            env,
            data_dir,
            lang,
            main: RefCell::default(),
            gauge: RefCell::default(),
            main_shown: Cell::new(false),
            gauge_shown: Cell::new(false),
            docked: Cell::new(None),
            placed_gauge: Cell::new(None),
            placed_main: Cell::new(None),
            last_moved: Cell::new(None),
            settle: Timer::default(),
            poll: Timer::default(),
        })
    }

    pub fn show(self: &Rc<Self>, want: Want) {
        match want {
            Want::Gauge => self.show_gauge(),
            Want::Main => self.show_main(),
        }
    }

    /// Answer the requests of the `vario-gui` runs started after this one.
    pub fn listen(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.poll.start(TimerMode::Repeated, POLL, move || {
            if let Some(shell) = weak.upgrade() {
                for want in instance::take_requests(&shell.data_dir) {
                    shell.show(want);
                }
            }
        });
    }

    /// The main window, opened if needed (for screenshots and tests).
    pub fn main_window(&self) -> Option<AppWindow> {
        self.main
            .borrow()
            .as_ref()
            .map(|(window, _)| window.clone_strong())
    }

    fn gauge_window(&self) -> Option<GaugeWindow> {
        self.gauge
            .borrow()
            .as_ref()
            .map(ComponentHandle::clone_strong)
    }

    fn show_gauge(self: &Rc<Self>) {
        if self.gauge.borrow().is_none() {
            let Ok(gauge) = crate::open_gauge(self.env.clone(), self.data_dir.clone(), self.lang)
            else {
                return;
            };
            let weak = Rc::downgrade(self);
            gauge.on_close_gauge(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.hide(Which::Gauge);
                }
            });
            let weak = Rc::downgrade(self);
            gauge.on_open_main(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.show_main();
                }
            });
            self.watch(gauge.window(), Which::Gauge);
            *self.gauge.borrow_mut() = Some(gauge);
        }
        let Some(gauge) = self.gauge_window() else {
            return;
        };
        let first = !self.gauge_shown.get();
        if gauge.show().is_err() {
            return;
        }
        self.gauge_shown.set(true);
        // The system window exists once the event loop has run: float and place it then.
        self.later(move |shell| {
            gauge::float(&gauge);
            if first {
                shell.place_gauge(&gauge);
            }
        });
    }

    /// The gauge's first place: beside the main window when it is open, on its roomy
    /// side; else where it was left, or in the screen's top right corner.
    fn place_gauge(&self, gauge: &GaugeWindow) {
        let scale = gauge.window().scale_factor();
        let size = gauge.window().size();
        match self.main_window().filter(|_| self.main_shown.get()) {
            Some(main) => {
                let main = rect(main.window());
                let side = match dock::roomy_side(main, screen(gauge.window())) {
                    // Room on the main window's left: the gauge goes there.
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                };
                let (x, y) = dock::gauge_beside(main, size.width as i32, side);
                self.place(Which::Gauge, x, y);
                self.docked.set(Some(side));
            }
            None => {
                let saved =
                    load_place(&self.data_dir).filter(|&(x, y)| on_a_screen(gauge.window(), x, y));
                let place = saved.or_else(|| {
                    screen(gauge.window()).map(|screen| {
                        dock::corner(screen, size.width as i32, (MARGIN * scale) as i32)
                    })
                });
                if let Some((x, y)) = place {
                    self.place(Which::Gauge, x, y);
                }
            }
        }
    }

    fn show_main(self: &Rc<Self>) {
        if self.main.borrow().is_none() {
            let Ok((window, app)) = crate::open(self.env.clone(), self.data_dir.clone(), self.lang)
            else {
                return;
            };
            let weak = Rc::downgrade(self);
            window.on_open_gauge(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.show_gauge();
                }
            });
            let weak = Rc::downgrade(self);
            window.on_close_main(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.hide(Which::Main);
                }
            });
            self.watch(window.window(), Which::Main);
            *self.main.borrow_mut() = Some((window, app));
        }
        let Some(main) = self.main_window() else {
            return;
        };
        let reopened = self.main_shown.get();
        gauge::show_in_dock(true);
        if main.show().is_err() {
            return;
        }
        self.main_shown.set(true);
        if !reopened {
            main.set_ready(false);
            let weak = main.as_weak();
            Timer::single_shot(ARMING, move || {
                if let Some(main) = weak.upgrade() {
                    main.set_ready(true);
                }
            });
        }
        self.later(move |shell| {
            gauge::bring_to_front(main.window());
            if !reopened {
                shell.place_main(&main);
            }
        });
    }

    /// The main window's first place: beside the gauge when it is up, on the side
    /// with room.
    fn place_main(&self, main: &AppWindow) {
        if let Some(gauge) = self.gauge_window().filter(|_| self.gauge_shown.get()) {
            let gauge_rect = rect(gauge.window());
            let side = dock::roomy_side(gauge_rect, screen(gauge.window()));
            let width = main.window().size().width as i32;
            let (x, y) = dock::main_beside(gauge_rect, width, side);
            self.place(Which::Main, x, y);
            self.docked.set(Some(side));
        }
    }

    /// Run once the event loop has handled what is pending, the new windows included.
    fn later(self: &Rc<Self>, job: impl FnOnce(&Rc<Shell>) + 'static) {
        let weak = Rc::downgrade(self);
        Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(shell) = weak.upgrade() {
                job(&shell);
            }
        });
    }

    fn hide(&self, which: Which) {
        self.docked.set(None);
        match which {
            Which::Gauge => {
                if let Some(gauge) = self.gauge_window() {
                    let _ = gauge.hide();
                }
                self.gauge_shown.set(false);
            }
            Which::Main => {
                if let Some(main) = self.main_window() {
                    let _ = main.hide();
                }
                self.main_shown.set(false);
                // Back to a desktop widget: no Dock icon while only the gauge shows.
                if self.gauge_shown.get() {
                    gauge::show_in_dock(false);
                }
            }
        }
        // With no window left, the event loop ends, and the process with it.
    }

    /// Follow the window's moves, for the magnetism.
    fn watch(self: &Rc<Self>, window: &slint::Window, which: Which) {
        let weak = Rc::downgrade(self);
        window.on_winit_window_event(move |_, event| {
            if let winit::event::WindowEvent::Moved(at) = event
                && let Some(shell) = weak.upgrade()
            {
                shell.moved(which, at.x, at.y);
            }
            EventResult::Propagate
        });
    }

    fn moved(self: &Rc<Self>, which: Which, x: i32, y: i32) {
        let placed = match which {
            Which::Gauge => self.placed_gauge.take(),
            Which::Main => self.placed_main.take(),
        };
        if placed.is_some_and(|(px, py)| (px - x).abs() <= 2 && (py - y).abs() <= 2) {
            return;
        }
        match which {
            // The docked main window follows the gauge.
            Which::Gauge => {
                if let (Some(side), Some(gauge), Some(main)) =
                    (self.docked.get(), self.gauge_window(), self.main_window())
                {
                    let width = main.window().size().width as i32;
                    let (x, y) = dock::main_beside(rect(gauge.window()), width, side);
                    self.place(Which::Main, x, y);
                }
            }
            // Dragging the main window takes it off the gauge.
            Which::Main => self.docked.set(None),
        }
        self.last_moved.set(Some(which));
        let weak: Weak<Shell> = Rc::downgrade(self);
        self.settle.start(TimerMode::SingleShot, SETTLE, move || {
            if let Some(shell) = weak.upgrade() {
                shell.settled();
            }
        });
    }

    /// A window was released: snap it beside the other when close enough.
    fn settled(&self) {
        let Some(which) = self.last_moved.take() else {
            return;
        };
        let (Some(gauge), Some(main)) = (self.gauge_window(), self.main_window()) else {
            self.remember_gauge();
            return;
        };
        if self.docked.get().is_none() && self.gauge_shown.get() && self.main_shown.get() {
            let reach = (REACH * gauge.window().scale_factor()) as i32;
            let (gauge_rect, main_rect) = (rect(gauge.window()), rect(main.window()));
            if let Some(side) = dock::snap(main_rect, gauge_rect, reach) {
                match which {
                    Which::Main => {
                        let (x, y) = dock::main_beside(gauge_rect, main_rect.width, side);
                        self.place(Which::Main, x, y);
                    }
                    Which::Gauge => {
                        let (x, y) = dock::gauge_beside(main_rect, gauge_rect.width, side);
                        self.place(Which::Gauge, x, y);
                    }
                }
                self.docked.set(Some(side));
            }
        }
        self.remember_gauge();
    }

    /// Move a window, noting that the move is Vario's own.
    fn place(&self, which: Which, x: i32, y: i32) {
        let at = PhysicalPosition::new(x, y);
        match which {
            Which::Gauge => {
                if let Some(gauge) = self.gauge_window() {
                    self.placed_gauge.set(Some((x, y)));
                    gauge.window().set_position(at);
                }
            }
            Which::Main => {
                if let Some(main) = self.main_window() {
                    self.placed_main.set(Some((x, y)));
                    main.window().set_position(at);
                }
            }
        }
    }

    fn remember_gauge(&self) {
        if let Some(gauge) = self.gauge_window().filter(|_| self.gauge_shown.get()) {
            let at = gauge.window().position();
            save_place(&self.data_dir, (at.x, at.y));
        }
    }
}

/// A window's outer rectangle.
fn rect(window: &slint::Window) -> Rect {
    let (at, size) = (window.position(), window.size());
    Rect {
        x: at.x,
        y: at.y,
        width: size.width as i32,
        height: size.height as i32,
    }
}

/// The screen the window is on; the main screen while the system does not say yet.
fn screen(window: &slint::Window) -> Option<Rect> {
    window
        .with_winit_window(|window| {
            window
                .current_monitor()
                .or_else(|| window.primary_monitor())
                .or_else(|| window.available_monitors().next())
        })
        .flatten()
        .map(|monitor| monitor_rect(&monitor))
}

fn monitor_rect(monitor: &winit::monitor::MonitorHandle) -> Rect {
    let (at, size) = (monitor.position(), monitor.size());
    Rect {
        x: at.x,
        y: at.y,
        width: size.width as i32,
        height: size.height as i32,
    }
}

/// Whether a saved place is still on a connected screen.
fn on_a_screen(window: &slint::Window, x: i32, y: i32) -> bool {
    window
        .with_winit_window(|window| {
            window
                .available_monitors()
                .any(|monitor| monitor_rect(&monitor).contains(x + 8, y + 8))
        })
        .unwrap_or(false)
}

#[derive(Default, Serialize, Deserialize)]
struct Places {
    gauge: Option<(i32, i32)>,
}

fn load_place(data_dir: &Path) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(data_dir.join(PLACES)).ok()?;
    serde_json::from_str::<Places>(&text).ok()?.gauge
}

fn save_place(data_dir: &Path, at: (i32, i32)) {
    if load_place(data_dir) == Some(at) {
        return;
    }
    let json = serde_json::to_string(&Places { gauge: Some(at) }).expect("places serialize");
    let _ = std::fs::write(data_dir.join(PLACES), json + "\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gauge_place_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_place(dir.path()), None);
        save_place(dir.path(), (1740, 48));
        assert_eq!(load_place(dir.path()), Some((1740, 48)));
        std::fs::write(dir.path().join(PLACES), "not json").unwrap();
        assert_eq!(load_place(dir.path()), None);
    }
}
