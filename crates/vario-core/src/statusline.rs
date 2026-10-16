//! `vario statusline`, Claude Code's status line command: it saves the quota Claude
//! Code passes to status lines, then shows either the status line it wraps, unchanged,
//! or a compact one of its own.
//!
//! Like the hooks, it never fails visibly: a status line runs after every answer.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::hooks;
use crate::lang::Lang;
use crate::quota::{self, Level, Quota};
use crate::tr;

/// What to print. `wrapped` is the user's own status line command, if Vario wraps one.
pub fn run(input: &str, wrapped: Option<&str>, data_dir: &Path, lang: Lang) -> String {
    let now = quota::now();
    if let Ok(value) = serde_json::from_str::<Value>(input)
        && let Some(quota) = Quota::from_status_line(&value, now)
        && let Err(err) = quota.save(data_dir)
    {
        hooks::log(data_dir, &format!("statusline: {err}"));
    }
    match wrapped {
        Some(command) => run_wrapped(command, input).unwrap_or_else(|err| {
            hooks::log(data_dir, &format!("statusline: {err}"));
            String::new()
        }),
        None => own_line(Quota::load(data_dir).as_ref(), now, lang),
    }
}

/// The wrapped command's output, given the same input on its standard input.
fn run_wrapped(command: &str, input: &str) -> std::io::Result<String> {
    let mut shell = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.args(["/C", command]);
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.args(["-c", command]);
        shell
    };
    let mut child = shell
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // A command that does not read its input may close it early: not an error.
        let _ = stdin.write_all(input.as_bytes());
    }
    let output = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// "Vario · 5 h 23 % · 7 j 41 %", and the thermostat's level when it is raised.
fn own_line(quota: Option<&Quota>, now: i64, lang: Lang) -> String {
    let Some(quota) = quota else {
        return "Vario".into();
    };
    let (five, seven) = quota.current(now);
    let mut parts = vec!["Vario".to_string()];
    if let Some(window) = five {
        let used = window.used_percentage.round();
        parts.push(tr!(lang, "5h {used}%", "5 h {used} %"));
    }
    if let Some(window) = seven {
        let used = window.used_percentage.round();
        parts.push(tr!(lang, "7d {used}%", "7 j {used} %"));
    }
    match quota.level(now) {
        Level::Normal => {}
        Level::Reinforced => parts.push(tr!(lang, "delegating more", "délégation renforcée")),
        Level::Alert => parts.push(tr!(lang, "alert", "alerte")),
        Level::Limit => parts.push(tr!(lang, "limit reached", "limite atteinte")),
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn input(five: f64, seven: f64) -> String {
        let now = quota::now();
        json!({"rate_limits": {
            "five_hour": {"used_percentage": five, "resets_at": now + 3600},
            "seven_day": {"used_percentage": seven, "resets_at": now + 86_400}
        }})
        .to_string()
    }

    #[test]
    fn saves_the_quota_and_shows_a_compact_line() {
        let dir = tempfile::tempdir().unwrap();
        let line = run(&input(23.4, 41.2), None, dir.path(), Lang::En);
        assert_eq!(line, "Vario · 5h 23% · 7d 41%");
        assert!(Quota::load(dir.path()).is_some());
        let line = run(&input(62.0, 41.0), None, dir.path(), Lang::Fr);
        assert_eq!(line, "Vario · 5 h 62 % · 7 j 41 % · délégation renforcée");
    }

    #[test]
    fn without_limits_the_last_quota_stays() {
        let dir = tempfile::tempdir().unwrap();
        run(&input(10.0, 20.0), None, dir.path(), Lang::En);
        let line = run(r#"{"model": {}}"#, None, dir.path(), Lang::En);
        assert_eq!(line, "Vario · 5h 10% · 7d 20%");
        assert_eq!(run("not json", None, dir.path(), Lang::En), line);
    }

    #[cfg(unix)]
    #[test]
    fn a_wrapped_status_line_is_shown_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let line = run(
            &input(30.0, 40.0),
            Some("printf 'mine: '; cat | head -c 14"),
            dir.path(),
            Lang::En,
        );
        assert_eq!(line, "mine: {\"rate_limits\"");
        assert!(
            Quota::load(dir.path()).is_some(),
            "the quota is still saved"
        );
    }
}
