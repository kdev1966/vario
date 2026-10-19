//! Opening Vario's interface from a hook: the floating gauge comes up with each
//! agent's session.
//!
//! The interface runs a single instance. A second run only asks the running
//! one to show its gauge, then leaves, so a hook can start it every time.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::choices::Choices;
use crate::clients::Env;
use crate::hooks;

/// The interfaces, the one tried first first: the Tauri one while it is tried out
/// beside the Slint one (`vario-gui`).
const PROGRAMS: [&str; 2] = ["vario-gauge-tauri", "vario-gui"];

/// The interface installed next to `vario` (the same folder, or `Vario.app`).
pub fn program(env: &Env) -> Option<PathBuf> {
    let dir = env.exe.parent()?;
    PROGRAMS
        .iter()
        .map(|name| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
}

/// Whether a window can be shown: not over SSH, and on Linux and BSD only with a
/// display server.
pub fn has_display(env: &Env) -> bool {
    if env.var("SSH_CONNECTION").is_some() || env.var("SSH_TTY").is_some() {
        return false;
    }
    cfg!(any(windows, target_os = "macos"))
        || env.var("DISPLAY").is_some()
        || env.var("WAYLAND_DISPLAY").is_some()
}

/// Open the gauge for a starting session, unless the user keeps it for when they ask,
/// no screen is there, or the interface is not installed. It runs detached: the hook
/// returns at once, and the gauge outlives the session.
pub fn open_gauge(env: &Env, data_dir: &Path) {
    if !Choices::gauge_with_sessions(data_dir) || !has_display(env) {
        return;
    }
    let Some(program) = program(env) else {
        return;
    };
    let mut command = Command::new(&program);
    command
        .arg("--gauge")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Err(err) = spawn_detached(&mut command) {
        hooks::log(data_dir, &format!("{}: {err}", program.display()));
    }
}

/// Start the process outside the client's process group (Unix) or console and job
/// (Windows), so that closing the agent does not close the gauge.
fn spawn_detached(command: &mut Command) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        command.spawn().map(drop)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // A job that forbids breaking away refuses the first form.
        command
            .creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .or_else(|_| command.creation_flags(detached).spawn())
            .map(drop)
    }
    #[cfg(not(any(unix, windows)))]
    {
        command.spawn().map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Env {
        Env {
            vars: vars
                .iter()
                .map(|(name, value)| (name.to_string(), value.into()))
                .collect(),
            ..Env::default()
        }
    }

    #[test]
    fn no_display_over_ssh() {
        assert!(!has_display(&env(&[
            ("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22"),
            ("DISPLAY", ":0"),
        ])));
        assert!(has_display(&env(&[("DISPLAY", ":0")])));
        assert_eq!(
            has_display(&env(&[])),
            cfg!(any(windows, target_os = "macos")),
            "Linux needs DISPLAY or WAYLAND_DISPLAY"
        );
    }

    #[test]
    fn the_interface_sits_next_to_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env(&[]);
        assert_eq!(program(&env), None, "no `vario` path");
        env.exe = dir.path().join("vario");
        assert_eq!(program(&env), None, "no interface installed");
        let gui = dir
            .path()
            .join(format!("vario-gui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&gui, "").unwrap();
        assert_eq!(program(&env), Some(gui));
    }

    #[test]
    fn the_tauri_interface_comes_first_when_installed() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env(&[]);
        env.exe = dir.path().join("vario");
        let suffix = std::env::consts::EXE_SUFFIX;
        std::fs::write(dir.path().join(format!("vario-gui{suffix}")), "").unwrap();
        let tauri = dir.path().join(format!("vario-gauge-tauri{suffix}"));
        std::fs::write(&tauri, "").unwrap();
        assert_eq!(program(&env), Some(tauri));
    }
}
