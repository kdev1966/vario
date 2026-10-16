//! Cohabitation rule 5: installing then removing Vario leaves Handoff's settings
//! and the user's own settings untouched, byte for byte.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use vario_core::clients::{Env, integrations};
use vario_core::files::{BLOCK_BEGIN, Session};
#[cfg(unix)] // only the tests that put a `handoff` executable on PATH use it
use vario_core::handoff::{self, State};
use vario_core::install;
use vario_core::parts::is_vario_hook_command;

const HANDOFF_BEGIN: &str =
    "<!-- handoff:begin (managed by Handoff; edits inside are overwritten) -->";
const HANDOFF_END: &str = "<!-- handoff:end -->";
const HANDOFF_BODY: &str = "## Handoff: shared project memory\n\nThis machine runs Handoff, a memory shared by every AI coding assistant the developer uses.\n- At the start of a task, call the `memory_get` tool of the `handoff` MCP server.\n- Never put secrets in the memory.";

const PY: &str = "/usr/bin/python3";

fn handoff_block() -> String {
    format!("{HANDOFF_BEGIN}\n{HANDOFF_BODY}\n{HANDOFF_END}\n")
}

fn claude_json() -> String {
    format!(
        r#"{{
  "numStartups": 42,
  "theme": "dark",
  "mcpServers": {{
    "handoff": {{
      "type": "stdio",
      "command": "{PY}",
      "args": [
        "-m",
        "ai_handoff",
        "serve"
      ],
      "env": {{}}
    }}
  }}
}}
"#
    )
}

fn claude_settings() -> String {
    format!(
        r#"{{
  "model": "opus",
  "hooks": {{
    "SessionStart": [
      {{
        "matcher": "startup|resume|clear|compact",
        "hooks": [
          {{
            "type": "command",
            "command": "{PY} -m ai_handoff hook claude session-start",
            "timeout": 20
          }}
        ]
      }}
    ],
    "Stop": [
      {{
        "hooks": [
          {{
            "type": "command",
            "command": "{PY} -m ai_handoff hook claude stop",
            "timeout": 20
          }}
        ]
      }},
      {{
        "hooks": [
          {{
            "type": "command",
            "command": "echo my own stop hook"
          }}
        ]
      }}
    ]
  }}
}}
"#
    )
}

fn cursor_mcp() -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "handoff": {{
      "command": "{PY}",
      "args": [
        "-m",
        "ai_handoff",
        "serve"
      ],
      "type": "stdio"
    }}
  }}
}}
"#
    )
}

fn cursor_hooks() -> String {
    format!(
        r#"{{
  "version": 1,
  "hooks": {{
    "sessionStart": [
      {{
        "command": "{PY} -m ai_handoff hook cursor session-start",
        "timeout": 20
      }}
    ],
    "stop": [
      {{
        "command": "{PY} -m ai_handoff hook cursor stop",
        "timeout": 20
      }}
    ]
  }}
}}
"#
    )
}

fn opencode_json() -> String {
    format!(
        r#"{{
  "small_model": "anthropic/claude-haiku-4-5",
  "mcp": {{
    "handoff": {{
      "type": "local",
      "command": [
        "{PY}",
        "-m",
        "ai_handoff",
        "serve"
      ],
      "enabled": true
    }}
  }}
}}
"#
    )
}

struct Paths {
    claude_json: PathBuf,
    claude_md: PathBuf,
    claude_settings: PathBuf,
    cursor_mcp: PathBuf,
    cursor_hooks: PathBuf,
    opencode_json: PathBuf,
    opencode_md: PathBuf,
}

impl Paths {
    fn json_files(&self) -> [(&Path, String); 5] {
        [
            (&self.claude_json, claude_json()),
            (&self.claude_settings, claude_settings()),
            (&self.cursor_mcp, cursor_mcp()),
            (&self.cursor_hooks, cursor_hooks()),
            (&self.opencode_json, opencode_json()),
        ]
    }
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A temporary home holding a realistic Handoff installation, and a PATH with
/// an executable `handoff`.
fn handoff_home(root: &Path) -> (Env, Paths) {
    let home = root.join("home");
    let paths = Paths {
        claude_json: home.join(".claude.json"),
        claude_md: home.join(".claude/CLAUDE.md"),
        claude_settings: home.join(".claude/settings.json"),
        cursor_mcp: home.join(".cursor/mcp.json"),
        cursor_hooks: home.join(".cursor/hooks.json"),
        opencode_json: home.join(".config/opencode/opencode.json"),
        opencode_md: home.join(".config/opencode/AGENTS.md"),
    };
    for (path, content) in paths.json_files() {
        write(path, &content);
    }
    write(
        &paths.claude_md,
        &format!("# My rules\n\n@FLAGS.md\n\n{}", handoff_block()),
    );
    write(&paths.opencode_md, &handoff_block());

    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let command = bin.join("handoff");
    fs::write(&command, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = Env {
        home,
        ..Env::default()
    };
    env.vars.insert("PATH".into(), OsString::from(&bin));
    (env, paths)
}

fn session(root: &Path) -> Session {
    Session::with_backup_dir(false, root.join("backups"))
}

/// Every file under `dir` (not the folders), path to bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, into);
            } else {
                into.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, &mut files);
    files
}

/// The JSON without Vario's hook entries, and without the event lists they leave empty.
fn without_vario(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    fn has_vario_command(value: &Value) -> bool {
        match value {
            Value::String(text) => is_vario_hook_command(text),
            Value::Array(items) => items.iter().any(has_vario_command),
            Value::Object(map) => map.values().any(has_vario_command),
            _ => false,
        }
    }
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|item| !has_vario_command(item))
                .map(without_vario)
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                // Vario's status line, installed where the user had none.
                .filter(|(key, value)| {
                    *key != "statusLine"
                        || !value["command"]
                            .as_str()
                            .is_some_and(|command| command.contains("vario statusline"))
                })
                .map(|(key, value)| (key.clone(), without_vario(value)))
                .filter(|(_, value)| value.as_array().is_none_or(|items| !items.is_empty()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// During the install, Vario may add its own hook entries beside Handoff's, and
/// nothing else; after the removal, every file is back byte for byte.
fn assert_handoff_intact(paths: &Paths, stage: &str) {
    for (path, expected) in paths.json_files() {
        let current = fs::read_to_string(path).unwrap();
        let parse = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        assert_eq!(
            without_vario(&parse(&current)),
            parse(&expected),
            "{} changed beyond Vario's own hooks {stage}",
            path.display()
        );
        if stage == "after the removal" {
            assert_eq!(
                current,
                expected,
                "{} not restored byte for byte",
                path.display()
            );
        }
    }
    for path in [&paths.claude_md, &paths.opencode_md] {
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.contains(&handoff_block()),
            "Handoff's block in {} is gone or altered {stage}",
            path.display()
        );
    }
}

#[test]
fn install_then_remove_leaves_handoff_and_user_files_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let (env, paths) = handoff_home(dir.path());
    let home = env.home.clone();
    let before = snapshot(&home);

    let outcomes = install::apply(&mut session(dir.path()), &integrations(), &env);
    for outcome in &outcomes {
        assert!(outcome.ok(), "install errors: {:?}", outcome.errors);
    }
    assert_handoff_intact(&paths, "during the install");
    for path in [&paths.claude_md, &paths.opencode_md] {
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.contains(BLOCK_BEGIN),
            "Vario's block is missing from {}",
            path.display()
        );
    }
    let claude_md = fs::read_to_string(&paths.claude_md).unwrap();
    assert!(
        claude_md.starts_with("# My rules\n\n@FLAGS.md\n"),
        "the user's rules stay in front"
    );
    assert!(snapshot(&home).len() > before.len(), "Vario added files");

    let outcomes = install::remove(&mut session(dir.path()), &integrations(), &env);
    for outcome in &outcomes {
        assert!(outcome.ok(), "remove errors: {:?}", outcome.errors);
    }
    assert_handoff_intact(&paths, "after the removal");
    assert_eq!(
        snapshot(&home),
        before,
        "the files differ from the original"
    );
}

#[cfg(unix)]
#[test]
fn handoff_stays_connected() {
    let dir = tempfile::tempdir().unwrap();
    let (env, _paths) = handoff_home(dir.path());
    // The clients this Handoff setup is connected to.
    let connected = |when: &str| {
        for integration in integrations()
            .into_iter()
            .filter(|integration| ["claude", "cursor", "opencode"].contains(&integration.name))
        {
            assert_eq!(
                handoff::state(&integration, &env),
                State::Connected,
                "{} {when}",
                integration.name
            );
        }
    };
    connected("before");
    install::apply(&mut session(dir.path()), &integrations(), &env);
    connected("during");
    install::remove(&mut session(dir.path()), &integrations(), &env);
    connected("after");
}
