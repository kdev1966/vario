use std::ffi::OsString;
use std::fs;
use std::path::Path;

use vario_core::Error;
use vario_core::clients::{Env, by_name, subagents};
use vario_core::error::Reason;
use vario_core::files::{BLOCK_BEGIN, Session, restore};
#[cfg(unix)] // only the tests that put a `handoff` executable on PATH use it
use vario_core::handoff::{self, State};
use vario_core::install;
use vario_core::parts::Kind;

/// A temporary home with no client installed and an empty PATH.
fn fake_env(root: &Path) -> Env {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    Env {
        home,
        ..Env::default()
    }
}

fn session(root: &Path) -> Session {
    Session::with_backup_dir(false, root.join("backups"))
}

#[test]
fn claude_code_is_detected_by_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    assert!(install::detected(&env).is_empty());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let names: Vec<&str> = install::detected(&env).iter().map(|i| i.name).collect();
    assert_eq!(names, ["claude"]);
}

#[cfg(unix)]
#[test]
fn claude_code_is_detected_by_its_executable_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    fs::write(&claude, "#!/bin/sh\n").unwrap();
    env.vars.insert("PATH".into(), OsString::from(&bin));
    assert!(
        !by_name("claude").unwrap().detect(&env),
        "not executable yet"
    );
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(by_name("claude").unwrap().detect(&env));
}

#[test]
fn claude_code_install_then_remove_leaves_the_user_files_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".claude");
    fs::create_dir_all(&config).unwrap();
    let user_rules = "# My rules\n\n@FLAGS.md\n";
    fs::write(config.join("CLAUDE.md"), user_rules).unwrap();
    let claude = vec![by_name("claude").unwrap()];

    let outcomes = install::apply(&mut session(dir.path()), &claude, &env);
    assert!(outcomes[0].ok());
    assert_eq!(
        outcomes[0].changes.len(),
        5,
        "two agents, CLAUDE.md, then settings.json for the hooks and the status line"
    );

    let reader = fs::read_to_string(config.join("agents").join("vario-reader.md")).unwrap();
    assert!(reader.starts_with("---\nname: vario-reader\ndescription: \"Use proactively"));
    assert!(reader.contains("\ntools: Read, Grep, Glob\nmodel: haiku\n---\n"));
    assert!(reader.contains(subagents::MARKER_LINE));
    let worker = fs::read_to_string(config.join("agents").join("vario-worker.md")).unwrap();
    assert!(worker.contains("\ntools: Read, Grep, Glob, Edit, Write, Bash\nmodel: sonnet\n---\n"));
    let rules = fs::read_to_string(config.join("CLAUDE.md")).unwrap();
    assert!(rules.starts_with(user_rules) && rules.contains(BLOCK_BEGIN));

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config.join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        settings,
        serde_json::json!({
            "hooks": {
                "SubagentStop": [{
                    "matcher": "^(vario-reader|vario-worker)$",
                    "hooks": [{
                        "type": "command",
                        "command": "vario hook subagent-stop --via claude",
                        "timeout": 10,
                    }],
                }],
                "SessionStart": [{
                    "matcher": "startup|resume",
                    "hooks": [{
                        "type": "command",
                        "command": "vario hook session-start --via claude",
                        "timeout": 5,
                    }],
                }],
                "UserPromptSubmit": [{
                    "hooks": [{
                        "type": "command",
                        "command": "vario hook prompt-submit --via claude",
                        "timeout": 5,
                    }],
                }],
            },
            "statusLine": {"type": "command", "command": "vario statusline"},
        })
    );

    let status = install::status(&claude[0], &env);
    assert_eq!(
        status,
        [
            (Kind::Subagents, true),
            (Kind::Instructions, true),
            (Kind::Hooks, true),
            (Kind::Quota, true)
        ]
    );

    let again = install::apply(&mut session(dir.path()), &claude, &env);
    assert!(
        again[0].changes.is_empty(),
        "a second install changes nothing"
    );

    let removed = install::remove(&mut session(dir.path()), &claude, &env);
    assert!(removed[0].ok());
    assert_eq!(
        fs::read_to_string(config.join("CLAUDE.md")).unwrap(),
        user_rules
    );
    assert!(!config.join("agents").join("vario-reader.md").exists());
    assert!(
        !config.join("settings.json").exists(),
        "the settings file Vario created for its hook is gone"
    );
    assert_eq!(
        install::status(&claude[0], &env),
        [
            (Kind::Subagents, false),
            (Kind::Instructions, false),
            (Kind::Hooks, false),
            (Kind::Quota, false)
        ]
    );
}

#[test]
fn an_existing_status_line_is_wrapped_and_put_back() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".claude");
    fs::create_dir_all(&config).unwrap();
    let original = r#"{
  "statusLine": {
    "type": "command",
    "command": "npx -y paceline \"$HOME\"",
    "padding": 0
  }
}
"#;
    fs::write(config.join("settings.json"), original).unwrap();
    let claude = vec![by_name("claude").unwrap()];

    install::apply(&mut session(dir.path()), &claude, &env);
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config.join("settings.json")).unwrap()).unwrap();
    let line = &settings["statusLine"];
    let command = line["command"].as_str().unwrap();
    assert!(command.starts_with("vario statusline --then "), "{command}");
    assert_eq!(
        vario_core::parts::wrapped_status_line(command).as_deref(),
        Some("npx -y paceline \"$HOME\"")
    );
    assert_eq!(line["padding"], 0, "the user's other fields are kept");

    let again = install::apply(&mut session(dir.path()), &claude, &env);
    assert!(again[0].changes.is_empty(), "wrapping is not repeated");

    install::remove(&mut session(dir.path()), &claude, &env);
    assert_eq!(
        fs::read_to_string(config.join("settings.json")).unwrap(),
        original,
        "the user's status line is back, byte for byte"
    );
}

#[test]
fn claude_config_dir_is_respected() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let custom = dir.path().join("claude-config");
    env.vars
        .insert("CLAUDE_CONFIG_DIR".into(), OsString::from(&custom));
    install::apply(
        &mut session(dir.path()),
        &[by_name("claude").unwrap()],
        &env,
    );
    assert!(custom.join("agents").join("vario-worker.md").is_file());
    assert!(!env.home.join(".claude").exists());
}

#[test]
fn a_users_own_agent_with_the_same_name_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let agents = env.home.join(".claude").join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("vario-reader.md"), "mine").unwrap();

    let outcomes = install::apply(
        &mut session(dir.path()),
        &[by_name("claude").unwrap()],
        &env,
    );
    assert!(matches!(
        outcomes[0].errors[..],
        [Error::Config {
            reason: Reason::Foreign,
            ..
        }]
    ));
    assert_eq!(
        fs::read_to_string(agents.join("vario-reader.md")).unwrap(),
        "mine"
    );
}

#[test]
fn an_install_can_be_restored() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".claude");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("CLAUDE.md"), "mine\n").unwrap();
    let mut session = session(dir.path());
    install::apply(&mut session, &[by_name("claude").unwrap()], &env);

    restore(session.backup_dir()).unwrap();
    assert_eq!(
        fs::read_to_string(config.join("CLAUDE.md")).unwrap(),
        "mine\n"
    );
    assert!(!config.join("agents").join("vario-reader.md").exists());
}

#[cfg(unix)]
#[test]
fn handoff_is_absent_installed_or_connected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let claude = by_name("claude").unwrap();
    assert_eq!(handoff::state(&claude, &env), State::Absent);

    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let command = bin.join("handoff");
    fs::write(&command, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    env.vars.insert("PATH".into(), OsString::from(&bin));
    assert_eq!(handoff::state(&claude, &env), State::NotConnected);

    fs::write(
        env.home.join(".claude.json"),
        r#"{"mcpServers": {"handoff": {"command": "handoff"}}}"#,
    )
    .unwrap();
    assert_eq!(handoff::state(&claude, &env), State::Connected);

    let custom = dir.path().join("claude-config");
    env.vars
        .insert("CLAUDE_CONFIG_DIR".into(), OsString::from(&custom));
    assert_eq!(
        handoff::state(&claude, &env),
        State::NotConnected,
        "with CLAUDE_CONFIG_DIR, Claude Code reads .claude.json there"
    );
}

#[test]
fn cursor_is_detected_by_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    assert!(!by_name("cursor").unwrap().detect(&env));
    fs::create_dir_all(env.home.join(".cursor")).unwrap();
    assert!(by_name("cursor").unwrap().detect(&env));
    let names: Vec<&str> = install::detected(&env).iter().map(|i| i.name).collect();
    assert_eq!(names, ["cursor"]);
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let names: Vec<&str> = install::detected(&env).iter().map(|i| i.name).collect();
    assert_eq!(names, ["claude", "cursor"]);
}

#[test]
fn cursor_install_then_remove() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".cursor");
    let agents = config.join("agents");
    let cursor = vec![by_name("cursor").unwrap()];

    let outcomes = install::apply(&mut session(dir.path()), &cursor, &env);
    assert!(outcomes[0].ok());
    assert_eq!(
        outcomes[0].changes.len(),
        4,
        "two agents, then hooks.json for each of its two hooks"
    );

    let mut files: Vec<_> = fs::read_dir(&config)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(files, ["agents", "hooks.json"]);
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(
        hooks,
        serde_json::json!({"version": 1, "hooks": {
            "sessionStart": [{"command": "vario hook session-start --via cursor", "timeout": 10}],
            "subagentStop": [{"command": "vario hook subagent-stop --via cursor", "timeout": 10}],
        }})
    );
    let mut names: Vec<_> = fs::read_dir(&agents)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["vario-reader.md", "vario-worker.md"]);

    let reader = fs::read_to_string(agents.join("vario-reader.md")).unwrap();
    assert!(reader.starts_with("---\nname: vario-reader\n"));
    assert!(reader.contains("\nmodel: \"composer-2.5[fast=false]\"\nreadonly: true\n---\n"));
    assert!(reader.contains(subagents::MARKER_LINE));
    let worker = fs::read_to_string(agents.join("vario-worker.md")).unwrap();
    assert!(worker.contains("readonly: false"));
    assert!(worker.contains(subagents::MARKER_LINE));

    assert_eq!(
        install::status(&cursor[0], &env),
        [
            (Kind::Subagents, true),
            (Kind::Instructions, true),
            (Kind::Hooks, true)
        ]
    );

    let again = install::apply(&mut session(dir.path()), &cursor, &env);
    assert!(
        again[0].changes.is_empty(),
        "a second install changes nothing"
    );

    let removed = install::remove(&mut session(dir.path()), &cursor, &env);
    assert!(removed[0].ok());
    assert!(!agents.join("vario-reader.md").exists());
    assert!(!agents.join("vario-worker.md").exists());
    assert!(!config.join("hooks.json").exists());
    assert_eq!(
        install::status(&cursor[0], &env),
        [
            (Kind::Subagents, false),
            (Kind::Instructions, false),
            (Kind::Hooks, false)
        ]
    );
}

#[test]
fn cursor_twins_take_over_claude_code_agents() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let both = vec![by_name("claude").unwrap(), by_name("cursor").unwrap()];
    let outcomes = install::apply(&mut session(dir.path()), &both, &env);
    assert!(outcomes.iter().all(|outcome| outcome.ok()));

    let claude = fs::read_to_string(env.home.join(".claude/agents/vario-reader.md")).unwrap();
    let cursor = fs::read_to_string(env.home.join(".cursor/agents/vario-reader.md")).unwrap();
    assert!(claude.starts_with("---\nname: vario-reader\n"));
    assert!(cursor.starts_with("---\nname: vario-reader\n"));
    assert!(claude.contains("\nmodel: haiku\n"));
    assert!(cursor.contains("\nmodel: \"composer"));
    assert!(!cursor.contains("haiku"));
}

#[test]
fn a_users_own_cursor_agent_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let agents = env.home.join(".cursor").join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("vario-worker.md"), "mine").unwrap();

    let outcomes = install::apply(
        &mut session(dir.path()),
        &[by_name("cursor").unwrap()],
        &env,
    );
    assert!(matches!(
        outcomes[0].errors[..],
        [Error::Config {
            reason: Reason::Foreign,
            ..
        }]
    ));
    assert_eq!(
        fs::read_to_string(agents.join("vario-worker.md")).unwrap(),
        "mine"
    );
}

#[cfg(unix)]
#[test]
fn handoff_connected_in_cursor() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let claude = by_name("claude").unwrap();
    let cursor = by_name("cursor").unwrap();

    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let command = bin.join("handoff");
    fs::write(&command, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    env.vars.insert("PATH".into(), OsString::from(&bin));
    assert_eq!(handoff::state(&cursor, &env), State::NotConnected);

    fs::create_dir_all(env.home.join(".cursor")).unwrap();
    fs::write(
        env.home.join(".cursor").join("mcp.json"),
        r#"{"mcpServers": {"handoff": {"command": "handoff"}}}"#,
    )
    .unwrap();
    assert_eq!(handoff::state(&cursor, &env), State::Connected);
    assert_eq!(handoff::state(&claude, &env), State::NotConnected);
}

#[test]
fn opencode_is_detected_by_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    assert!(!by_name("opencode").unwrap().detect(&env));
    fs::create_dir_all(env.home.join(".config").join("opencode")).unwrap();
    assert!(by_name("opencode").unwrap().detect(&env));

    let mut env = env;
    let xdg = dir.path().join("xdg");
    env.vars
        .insert("XDG_CONFIG_HOME".into(), OsString::from(&xdg));
    assert!(!by_name("opencode").unwrap().detect(&env));
    fs::create_dir_all(xdg.join("opencode")).unwrap();
    assert!(by_name("opencode").unwrap().detect(&env));
}

#[test]
fn opencode_install_then_remove_without_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".config").join("opencode");
    let opencode = vec![by_name("opencode").unwrap()];

    let outcomes = install::apply(&mut session(dir.path()), &opencode, &env);
    assert!(outcomes[0].ok());

    let reader = fs::read_to_string(config.join("agents").join("vario-reader.md")).unwrap();
    assert!(reader.starts_with("---\ndescription: \""));
    assert!(reader.contains("\nmode: subagent\n"));
    assert!(reader.contains("permission:\n  edit: deny\n  bash: deny\n"));
    assert!(!reader.contains("model:"));
    let worker = fs::read_to_string(config.join("agents").join("vario-worker.md")).unwrap();
    assert!(!worker.contains("permission:"));
    assert!(!worker.contains("model:"));
    let rules = fs::read_to_string(config.join("AGENTS.md")).unwrap();
    assert!(rules.contains(BLOCK_BEGIN));

    assert_eq!(
        install::status(&opencode[0], &env),
        [
            (Kind::Subagents, true),
            (Kind::Instructions, true),
            (Kind::Hooks, true)
        ]
    );

    let plugin = fs::read_to_string(config.join("plugins").join("vario.js")).unwrap();
    assert!(plugin.starts_with("// Generated by Vario"));
    assert!(plugin.contains("const VARIO = \"vario\";"));
    assert!(plugin.contains(r#"const AGENTS = ["vario-reader","vario-worker"];"#));
    assert!(plugin.contains("hook session-start --via opencode"));
    assert!(plugin.contains("hook subagent-stop --via opencode"));
    assert!(
        plugin.contains(r#".join("\n");"#),
        "the JavaScript escape stays an escape, not a line break inside a string"
    );

    let removed = install::remove(&mut session(dir.path()), &opencode, &env);
    assert!(removed[0].ok());
    assert!(!config.join("agents").join("vario-reader.md").exists());
    assert!(!config.join("agents").join("vario-worker.md").exists());
    assert!(!config.join("AGENTS.md").exists());
    assert!(!config.join("plugins").join("vario.js").exists());
}

#[test]
fn opencode_uses_small_model_for_the_reader() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".config").join("opencode");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("opencode.json"),
        r#"{"small_model": "anthropic/claude-haiku-5-5"}"#,
    )
    .unwrap();

    let outcomes = install::apply(
        &mut session(dir.path()),
        &[by_name("opencode").unwrap()],
        &env,
    );
    assert!(outcomes[0].ok());
    let reader = fs::read_to_string(config.join("agents").join("vario-reader.md")).unwrap();
    assert!(reader.contains("model: \"anthropic/claude-haiku-5-5\""));
    let worker = fs::read_to_string(config.join("agents").join("vario-worker.md")).unwrap();
    assert!(!worker.contains("model:"));
}

#[test]
fn opencode_keeps_reading_the_users_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let claude_md = env.home.join(".claude").join("CLAUDE.md");
    fs::create_dir_all(claude_md.parent().unwrap()).unwrap();
    let user_rules = "# My rules\n\nBe brief.\n";
    fs::write(&claude_md, user_rules).unwrap();
    let opencode = vec![by_name("opencode").unwrap()];

    let outcomes = install::apply(&mut session(dir.path()), &opencode, &env);
    assert!(outcomes[0].ok());
    let rules = fs::read_to_string(&claude_md).unwrap();
    assert!(rules.starts_with(user_rules) && rules.contains(BLOCK_BEGIN));
    assert!(
        !env.home
            .join(".config")
            .join("opencode")
            .join("AGENTS.md")
            .exists()
    );

    let removed = install::remove(&mut session(dir.path()), &opencode, &env);
    assert!(removed[0].ok());
    assert_eq!(fs::read_to_string(&claude_md).unwrap(), user_rules);
}

#[test]
fn opencode_writes_agents_md_when_it_exists_or_the_fallback_is_off() {
    let opencode = vec![by_name("opencode").unwrap()];
    let user_rules = "# My rules\n\nBe brief.\n";

    // (a) an existing AGENTS.md is the file OpenCode reads.
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let config = env.home.join(".config").join("opencode");
    let claude_md = env.home.join(".claude").join("CLAUDE.md");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(claude_md.parent().unwrap()).unwrap();
    fs::write(config.join("AGENTS.md"), "My agents text\n").unwrap();
    fs::write(&claude_md, user_rules).unwrap();
    let outcomes = install::apply(&mut session(dir.path()), &opencode, &env);
    assert!(outcomes[0].ok());
    let agents = fs::read_to_string(config.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("My agents text\n") && agents.contains(BLOCK_BEGIN));
    assert_eq!(fs::read_to_string(&claude_md).unwrap(), user_rules);

    // (b) with the Claude Code fallback disabled, AGENTS.md is created.
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let config = env.home.join(".config").join("opencode");
    let claude_md = env.home.join(".claude").join("CLAUDE.md");
    fs::create_dir_all(claude_md.parent().unwrap()).unwrap();
    fs::write(&claude_md, user_rules).unwrap();
    env.vars
        .insert("OPENCODE_DISABLE_CLAUDE_CODE".into(), OsString::from("1"));
    let outcomes = install::apply(&mut session(dir.path()), &opencode, &env);
    assert!(outcomes[0].ok());
    let agents = fs::read_to_string(config.join("AGENTS.md")).unwrap();
    assert!(agents.contains(BLOCK_BEGIN));
    assert_eq!(fs::read_to_string(&claude_md).unwrap(), user_rules);
}

#[cfg(unix)]
#[test]
fn handoff_connected_in_opencode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let opencode = by_name("opencode").unwrap();

    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let command = bin.join("handoff");
    fs::write(&command, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    env.vars.insert("PATH".into(), OsString::from(&bin));
    assert_eq!(handoff::state(&opencode, &env), State::NotConnected);

    let config = env.home.join(".config").join("opencode");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("opencode.json"),
        r#"{"mcp": {"handoff": {"type": "local"}}}"#,
    )
    .unwrap();
    assert_eq!(handoff::state(&opencode, &env), State::Connected);
}

#[test]
fn the_program_the_installed_hooks_run_is_found_while_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    assert_eq!(install::hook_program(&env), None, "nothing installed yet");

    let program = dir.path().join("bin").join("vario");
    fs::create_dir_all(program.parent().unwrap()).unwrap();
    fs::write(&program, "").unwrap();
    env.exe = program.clone();
    install::apply(
        &mut session(dir.path()),
        &[by_name("claude").unwrap()],
        &env,
    );
    assert_eq!(install::hook_program(&env), Some(program.clone()));

    fs::remove_file(&program).unwrap();
    assert_eq!(
        install::hook_program(&env),
        None,
        "a program that no longer exists is not reused"
    );
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn codex_install_then_remove() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let codex = vec![by_name("codex").unwrap()];
    assert!(!codex[0].detect(&env));
    let config = env.home.join(".codex");
    fs::create_dir_all(&config).unwrap();
    assert!(codex[0].detect(&env));

    let outcomes = install::apply(&mut session(dir.path()), &codex, &env);
    assert!(outcomes[0].ok(), "{:?}", outcomes[0].errors);

    let agents = config.join("agents");
    assert_eq!(
        names_in(&agents),
        ["vario-reader.toml", "vario-worker.toml"]
    );
    let reader = fs::read_to_string(agents.join("vario-reader.toml")).unwrap();
    assert!(reader.starts_with("# Generated by Vario"));
    assert!(reader.contains("model = \"gpt-6-luna\""));
    assert!(reader.ends_with("[features]\nshell_tool = false\n"));
    let worker = fs::read_to_string(agents.join("vario-worker.toml")).unwrap();
    assert!(!worker.contains("model ="));

    let rules = fs::read_to_string(config.join("AGENTS.md")).unwrap();
    assert!(rules.contains(BLOCK_BEGIN));
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(
        hooks["hooks"]["SubagentStop"][0]["hooks"][0]["command"],
        "vario hook subagent-stop --via codex"
    );
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "vario hook session-start --via codex"
    );

    assert_eq!(
        install::status(&codex[0], &env),
        [
            (Kind::Subagents, true),
            (Kind::Instructions, true),
            (Kind::Hooks, true)
        ]
    );
    let again = install::apply(&mut session(dir.path()), &codex, &env);
    assert!(
        again[0].changes.is_empty(),
        "a second install changes nothing"
    );

    let removed = install::remove(&mut session(dir.path()), &codex, &env);
    assert!(removed[0].ok(), "{:?}", removed[0].errors);
    assert!(!agents.join("vario-reader.toml").exists());
    assert!(!agents.join("vario-worker.toml").exists());
    assert!(!config.join("AGENTS.md").exists());
    assert!(!config.join("hooks.json").exists());
}

#[test]
fn codex_uses_codex_home_and_the_override_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let home = dir.path().join("ch");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("AGENTS.override.md"), "my own rules\n").unwrap();
    env.vars.insert("CODEX_HOME".into(), OsString::from(&home));
    let codex = vec![by_name("codex").unwrap()];
    assert!(codex[0].detect(&env), "CODEX_HOME is detected");

    let outcomes = install::apply(&mut session(dir.path()), &codex, &env);
    assert!(outcomes[0].ok(), "{:?}", outcomes[0].errors);

    let over = fs::read_to_string(home.join("AGENTS.override.md")).unwrap();
    assert!(over.contains("my own rules"));
    assert!(over.contains(BLOCK_BEGIN));
    assert!(!home.join("AGENTS.md").exists());
    assert!(home.join("agents").join("vario-reader.toml").is_file());
    assert!(home.join("agents").join("vario-worker.toml").is_file());
    assert!(!env.home.join(".codex").exists());
}

#[test]
fn gemini_install_then_remove() {
    let dir = tempfile::tempdir().unwrap();
    let env = fake_env(dir.path());
    let gemini = vec![by_name("gemini").unwrap()];
    assert!(!gemini[0].detect(&env));
    let config = env.home.join(".gemini");
    fs::create_dir_all(&config).unwrap();
    assert!(gemini[0].detect(&env));

    let outcomes = install::apply(&mut session(dir.path()), &gemini, &env);
    assert!(outcomes[0].ok(), "{:?}", outcomes[0].errors);

    let agents = config.join("agents");
    let reader = fs::read_to_string(agents.join("vario-reader.md")).unwrap();
    assert!(reader.contains("\nmodel: flash-lite\ntools:\n  - read_file\n  - grep_search\n---\n"));
    let worker = fs::read_to_string(agents.join("vario-worker.md")).unwrap();
    assert!(worker.contains("\nmodel: inherit\n---\n"));
    assert!(
        fs::read_to_string(config.join("GEMINI.md"))
            .unwrap()
            .contains(BLOCK_BEGIN)
    );
    assert_eq!(
        install::status(&gemini[0], &env),
        [(Kind::Subagents, true), (Kind::Instructions, true)]
    );

    let removed = install::remove(&mut session(dir.path()), &gemini, &env);
    assert!(removed[0].ok(), "{:?}", removed[0].errors);
    assert!(!agents.join("vario-reader.md").exists());
    assert!(!agents.join("vario-worker.md").exists());
    assert!(!config.join("GEMINI.md").exists());
    assert_eq!(
        install::status(&gemini[0], &env),
        [(Kind::Subagents, false), (Kind::Instructions, false)]
    );

    // GEMINI_CLI_HOME moves the whole .gemini folder.
    let other = dir.path().join("other");
    let mut env = fake_env(&other);
    let root = dir.path().join("g");
    fs::create_dir_all(root.join(".gemini")).unwrap();
    env.vars
        .insert("GEMINI_CLI_HOME".into(), OsString::from(&root));
    assert!(gemini[0].detect(&env));
    let outcomes = install::apply(&mut session(dir.path()), &gemini, &env);
    assert!(outcomes[0].ok(), "{:?}", outcomes[0].errors);
    assert!(root.join(".gemini/agents/vario-reader.md").is_file());
    assert!(root.join(".gemini/agents/vario-worker.md").is_file());
    assert!(root.join(".gemini/GEMINI.md").is_file());
    assert!(!env.home.join(".gemini").exists());
}

/// Puts VS Code's user folder where the current system looks for it and returns it.
fn vscode_user_dir(env: &mut Env, root: &Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        env.home.join("Library/Application Support/Code/User")
    } else if cfg!(windows) {
        let appdata = root.join("appdata");
        env.vars.insert("APPDATA".into(), OsString::from(&appdata));
        appdata.join("Code").join("User")
    } else {
        let xdg = root.join("xdg");
        env.vars
            .insert("XDG_CONFIG_HOME".into(), OsString::from(&xdg));
        xdg.join("Code").join("User")
    }
}

#[test]
fn vscode_install_then_remove() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let user = vscode_user_dir(&mut env, dir.path());
    let vscode = vec![by_name("vscode").unwrap()];
    assert!(!vscode[0].detect(&env));
    fs::create_dir_all(&user).unwrap();
    assert!(vscode[0].detect(&env));

    let outcomes = install::apply(&mut session(dir.path()), &vscode, &env);
    assert!(outcomes[0].ok(), "{:?}", outcomes[0].errors);

    let copilot = env.home.join(".copilot");
    let agents = copilot.join("agents");
    let reader = fs::read_to_string(agents.join("vario-reader.agent.md")).unwrap();
    assert!(reader.contains("model: \"Claude Haiku 4.5\""));
    assert!(reader.contains("tools: ['read', 'search']"));
    let worker = fs::read_to_string(agents.join("vario-worker.agent.md")).unwrap();
    assert!(!worker.contains("model:"));
    assert!(!worker.contains("tools:"));
    assert!(
        fs::read_to_string(copilot.join("copilot-instructions.md"))
            .unwrap()
            .contains(BLOCK_BEGIN)
    );
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(copilot.join("hooks/vario.json")).unwrap())
            .unwrap();
    assert_eq!(
        hooks,
        serde_json::json!({"hooks": {
            "SessionStart": [
                {"type": "command", "command": "vario hook session-start --via vscode", "timeout": 5}
            ],
            "SubagentStop": [
                {"type": "command", "command": "vario hook subagent-stop --via vscode", "timeout": 10}
            ],
        }})
    );

    let removed = install::remove(&mut session(dir.path()), &vscode, &env);
    assert!(removed[0].ok(), "{:?}", removed[0].errors);
    assert!(!agents.join("vario-reader.agent.md").exists());
    assert!(!agents.join("vario-worker.agent.md").exists());
    assert!(!copilot.join("copilot-instructions.md").exists());
    assert!(!copilot.join("hooks/vario.json").exists());
}

#[cfg(unix)]
#[test]
fn handoff_in_the_new_clients() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut env = fake_env(dir.path());
    let codex = by_name("codex").unwrap();
    let gemini = by_name("gemini").unwrap();
    let vscode = by_name("vscode").unwrap();

    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let command = bin.join("handoff");
    fs::write(&command, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    env.vars.insert("PATH".into(), OsString::from(&bin));
    let user = vscode_user_dir(&mut env, dir.path());
    assert_eq!(handoff::state(&codex, &env), State::NotConnected);
    assert_eq!(handoff::state(&gemini, &env), State::NotConnected);
    assert_eq!(handoff::state(&vscode, &env), State::NotConnected);

    fs::create_dir_all(env.home.join(".codex")).unwrap();
    fs::write(
        env.home.join(".codex/config.toml"),
        "model = \"x\"\n\n[mcp_servers.handoff]\ncommand = \"handoff\"\n",
    )
    .unwrap();
    assert_eq!(handoff::state(&codex, &env), State::Connected);

    fs::create_dir_all(env.home.join(".gemini")).unwrap();
    fs::write(
        env.home.join(".gemini/settings.json"),
        r#"{"mcpServers": {"handoff": {"command": "handoff"}}}"#,
    )
    .unwrap();
    assert_eq!(handoff::state(&gemini, &env), State::Connected);

    fs::create_dir_all(&user).unwrap();
    fs::write(
        user.join("mcp.json"),
        r#"{"servers": {"handoff": {"command": "handoff"}}}"#,
    )
    .unwrap();
    assert_eq!(handoff::state(&vscode, &env), State::Connected);
}
