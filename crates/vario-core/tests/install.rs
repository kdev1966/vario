use std::ffi::OsString;
use std::fs;
use std::path::Path;

use vario_core::Error;
use vario_core::clients::{Env, by_name, subagents};
use vario_core::error::Reason;
use vario_core::files::{BLOCK_BEGIN, Session, restore};
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
    assert_eq!(outcomes[0].changes.len(), 3);

    let reader = fs::read_to_string(config.join("agents").join("vario-reader.md")).unwrap();
    assert!(reader.starts_with("---\nname: vario-reader\ndescription: \"Use proactively"));
    assert!(reader.contains("\ntools: Read, Grep, Glob\nmodel: haiku\n---\n"));
    assert!(reader.contains(subagents::MARKER_LINE));
    let worker = fs::read_to_string(config.join("agents").join("vario-worker.md")).unwrap();
    assert!(worker.contains("\ntools: Read, Grep, Glob, Edit, Write, Bash\nmodel: sonnet\n---\n"));
    let rules = fs::read_to_string(config.join("CLAUDE.md")).unwrap();
    assert!(rules.starts_with(user_rules) && rules.contains(BLOCK_BEGIN));

    let status = install::status(&claude[0], &env);
    assert_eq!(
        status,
        [(Kind::Subagents, true), (Kind::Instructions, true)]
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
    assert_eq!(
        install::status(&claude[0], &env),
        [(Kind::Subagents, false), (Kind::Instructions, false)]
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
