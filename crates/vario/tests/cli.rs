//! End-to-end tests: run the real `vario` binary against a throwaway home directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USER_RULES: &str = "# My rules\n";

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// A temporary machine: its own home, data directory and an empty PATH.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    /// A home where Claude Code is installed, with the user's own `CLAUDE.md`.
    fn new() -> Self {
        let sandbox = Self::without_client();
        fs::create_dir_all(sandbox.claude_dir()).unwrap();
        fs::write(sandbox.claude("CLAUDE.md"), USER_RULES).unwrap();
        sandbox
    }

    /// A home with no `.claude` folder.
    fn without_client() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        Sandbox { dir }
    }

    fn data_dir(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn claude_dir(&self) -> PathBuf {
        self.dir.path().join("home").join(".claude")
    }

    fn claude(&self, file: &str) -> PathBuf {
        self.claude_dir().join(file)
    }

    fn rules(&self) -> String {
        fs::read_to_string(self.claude("CLAUDE.md")).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command(args).stdin(Stdio::null()).output().unwrap();
        Self::collect(output)
    }

    /// Like `run`, but writes `input` to the standard input, then closes it.
    fn run_with_stdin(&self, args: &[&str], input: &str) -> Output {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
        Self::collect(child.wait_with_output().unwrap())
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vario"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.dir.path().join("home"))
            .env("USERPROFILE", self.dir.path().join("home"))
            .env("VARIO_HOME", self.data_dir())
            .env("VARIO_LANG", "en")
            .env("PATH", self.dir.path().join("bin"));
        // A Windows process started without `SystemRoot` can fail to initialise.
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
    }

    fn collect(output: std::process::Output) -> Output {
        Output {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Escapes a string the way it appears inside a JSON string literal (Windows paths hold backslashes).
fn json_escaped(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn install_without_yes_changes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("nothing was changed"), "{}", out.stdout);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.data_dir()));
}

#[test]
fn install_dry_run_shows_the_plan_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--dry-run"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Planned changes"), "{}", out.stdout);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents")));
    assert!(!exists(&sandbox.data_dir()));
}

#[test]
fn install_then_doctor_reports_claude_code() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(sandbox.claude("agents/vario-reader.md").is_file());
    assert!(sandbox.claude("agents/vario-worker.md").is_file());
    assert!(sandbox.data_dir().join("setup.json").is_file());

    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Claude Code"), "{}", out.stdout);
    assert!(out.stdout.contains('✔'), "{}", out.stdout);
}

#[test]
fn remove_needs_yes_and_restores_the_user_content() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["install", "--yes"]).code, Some(0));

    let out = sandbox.run(&["remove"]);
    assert_eq!(out.code, Some(1), "{}{}", out.stdout, out.stderr);
    assert!(sandbox.claude("agents/vario-reader.md").is_file());
    assert!(sandbox.claude("agents/vario-worker.md").is_file());
    assert_ne!(sandbox.rules(), USER_RULES);
    assert!(sandbox.data_dir().join("setup.json").is_file());

    let out = sandbox.run(&["remove", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents/vario-reader.md")));
    assert!(!exists(&sandbox.claude("agents/vario-worker.md")));
    assert!(!exists(&sandbox.data_dir().join("setup.json")));
}

#[test]
fn restore_lists_then_puts_back_the_original_files() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["install", "--yes"]).code, Some(0));

    let out = sandbox.run(&["restore", "--list"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("(4 files)"), "{}", out.stdout);
    assert_eq!(out.stdout.lines().count(), 1, "{}", out.stdout);

    let out = sandbox.run(&["restore", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
    assert!(!exists(&sandbox.claude("agents/vario-reader.md")));
    assert!(!exists(&sandbox.claude("agents/vario-worker.md")));
    assert!(
        !exists(&sandbox.claude("settings.json")),
        "the hook file Vario created is gone"
    );
}

#[test]
fn an_unknown_client_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--client", "inconnu"]);
    assert_eq!(out.code, Some(2), "{}{}", out.stdout, out.stderr);
    assert!(out.stderr.contains("claude"), "{}", out.stderr);
    assert_eq!(sandbox.rules(), USER_RULES);
}

#[test]
fn a_home_without_a_client_installs_nothing() {
    let sandbox = Sandbox::without_client();
    let out = sandbox.run(&["install", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(
        out.stdout.contains("No supported AI client was found"),
        "{}",
        out.stdout
    );
    assert!(!exists(&sandbox.claude_dir()));
}

const CLAUDE_HOOK_ARGS: &str = "hook subagent-stop --via claude";

fn journal_lines(sandbox: &Sandbox) -> Vec<String> {
    fs::read_to_string(sandbox.data_dir().join("delegations.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn install_writes_hooks_that_run_this_binary() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["install", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);

    let exe = json_escaped(env!("CARGO_BIN_EXE_vario"));
    let settings = fs::read_to_string(sandbox.claude("settings.json")).unwrap();
    let plain = format!("\"command\": \"{exe} {CLAUDE_HOOK_ARGS}\"");
    let quoted = format!("\"command\": \"\\\"{exe}\\\" {CLAUDE_HOOK_ARGS}\"");
    assert!(
        settings.contains(&plain) || settings.contains(&quoted),
        "{settings}"
    );
    assert!(settings.contains("\"SubagentStop\""), "{settings}");

    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("Hooks"), "{}", out.stdout);
    let line = out
        .stdout
        .lines()
        .find(|line| line.contains("Claude Code"))
        .unwrap_or_else(|| panic!("{}", out.stdout));
    assert_eq!(line.matches('✔').count(), 3, "{line}");
}

#[test]
fn hook_journals_a_vario_sub_agent() {
    let sandbox = Sandbox::new();
    let args = ["hook", "subagent-stop", "--via", "claude"];
    let input = r#"{"agent_type":"vario-worker","cwd":"/w","last_assistant_message":"RESULT: done\nCHECKS: cargo test\nESCALATE: no - fine"}"#;
    let out = sandbox.run_with_stdin(&args, input);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(out.stdout, "");

    let lines = journal_lines(&sandbox);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
    assert!(line.contains(r#""agent":"vario-worker""#), "{line}");
    assert!(line.contains(r#""escalate":false"#), "{line}");
    assert!(line.contains(r#""result":"done""#), "{line}");

    let other = r#"{"agent_type":"Explore","cwd":"/w","last_assistant_message":"RESULT: x"}"#;
    let out = sandbox.run_with_stdin(&args, other);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(journal_lines(&sandbox).len(), 1);
}

#[test]
fn hook_never_fails() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["hook", "nope", "--via", "nope"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    let out = sandbox.run(&["hook"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);

    let out = sandbox.run_with_stdin(&["hook", "subagent-stop", "--via", "cursor"], "not json");
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert_eq!(out.stdout, "{}");
    let log = fs::read_to_string(sandbox.data_dir().join("hooks.log")).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
}

#[test]
fn cursor_session_start_injects_guidance() {
    let sandbox = Sandbox::new();
    let out = sandbox.run_with_stdin(&["hook", "session-start", "--via", "cursor"], "{}");
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.trim_start().starts_with('{'), "{}", out.stdout);
    assert!(out.stdout.contains("additional_context"), "{}", out.stdout);
    assert!(out.stdout.contains("vario-reader"), "{}", out.stdout);
    assert!(out.stdout.chars().count() <= 1000, "{}", out.stdout.len());
}

#[test]
fn doctor_fix_rewrites_a_hook_left_by_a_moved_binary() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["install", "--yes"]).code, Some(0));

    let exe = json_escaped(env!("CARGO_BIN_EXE_vario"));
    let exe = exe.as_str();
    let path = sandbox.claude("settings.json");
    let settings = fs::read_to_string(&path).unwrap();
    assert!(settings.contains(exe), "{settings}");
    let moved = settings.replace(exe, "/old/place/vario");
    assert!(moved.contains("/old/place/vario"), "{moved}");
    fs::write(&path, moved).unwrap();

    let out = sandbox.run(&["doctor"]);
    assert!(out.stdout.contains("incomplete"), "{}", out.stdout);

    let out = sandbox.run(&["doctor", "--fix", "--yes"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    let settings = fs::read_to_string(&path).unwrap();
    assert!(settings.contains(exe), "{settings}");
    assert!(!settings.contains("/old/place"), "{settings}");
    assert_eq!(settings.matches(CLAUDE_HOOK_ARGS).count(), 1, "{settings}");
}
