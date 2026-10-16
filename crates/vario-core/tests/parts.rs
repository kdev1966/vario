use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use vario_core::Error;
use vario_core::files::{BLOCK_BEGIN, Session};
use vario_core::parts::{JsonEntry, JsonHooks, Kind, MarkdownBlock, OwnFile, Part};

fn session(dir: &Path) -> Session {
    Session::with_backup_dir(false, dir.join("backups"))
}

fn write_json(path: &Path, value: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value.to_string()).unwrap();
}

fn load(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn mcp_entry(path: &Path) -> JsonEntry {
    JsonEntry {
        path: path.to_path_buf(),
        keys: vec!["mcpServers".into()],
        name: "vario".into(),
        value: json!({"command": "vario"}),
        kind: Kind::Mcp,
    }
}

fn stop_hooks(path: &Path, ours: Value) -> JsonHooks {
    JsonHooks {
        path: path.to_path_buf(),
        keys: vec!["hooks".into()],
        entries: vec![("Stop".into(), ours)],
        defaults: Map::new(),
        kind: Kind::Hooks,
    }
}

// Ported from Handoff's tests/test_integrations.py.

#[test]
fn json_entry_keeps_other_servers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    write_json(
        &path,
        json!({"mcpServers": {"other": {"command": "x"}}, "theme": "dark"}),
    );
    let part = mcp_entry(&path);
    part.apply(&mut session(dir.path())).unwrap();
    assert!(part.present());
    assert_eq!(
        load(&path),
        json!({"mcpServers": {"other": {"command": "x"}, "vario": {"command": "vario"}}, "theme": "dark"})
    );
    part.remove(&mut session(dir.path())).unwrap();
    assert_eq!(
        load(&path),
        json!({"mcpServers": {"other": {"command": "x"}}, "theme": "dark"})
    );
    assert!(!part.present());
}

#[test]
fn json_entry_leaves_commented_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{ /* mine */ }").unwrap();
    let err = mcp_entry(&path)
        .apply(&mut session(dir.path()))
        .unwrap_err();
    assert!(matches!(err, Error::Config { .. }));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ /* mine */ }");
}

#[test]
fn json_hooks_are_idempotent_and_removable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    let theirs = json!({"hooks": [{"type": "command", "command": "their-script"}]});
    write_json(&path, json!({"hooks": {"Stop": [theirs]}}));
    let ours = json!({"hooks": [{"type": "command", "command": "/usr/local/bin/vario hook --via claude stop"}]});
    let part = stop_hooks(&path, ours.clone());
    part.apply(&mut session(dir.path())).unwrap();
    part.apply(&mut session(dir.path())).unwrap();
    assert_eq!(load(&path)["hooks"]["Stop"], json!([theirs, ours]));
    assert!(part.present());
    part.remove(&mut session(dir.path())).unwrap();
    assert_eq!(load(&path)["hooks"]["Stop"], json!([theirs]));
    assert!(!part.present());
}

#[test]
fn markdown_part_deletes_file_it_filled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    let part = MarkdownBlock {
        path: path.clone(),
        body: "rules".into(),
    };
    part.apply(&mut session(dir.path())).unwrap();
    part.apply(&mut session(dir.path())).unwrap();
    assert_eq!(
        fs::read_to_string(&path)
            .unwrap()
            .matches(BLOCK_BEGIN)
            .count(),
        1
    );
    assert!(part.present());
    part.remove(&mut session(dir.path())).unwrap();
    assert!(!path.exists());
}

// New in Vario.

#[test]
fn json_entry_removal_keeps_key_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    fs::write(&path, r#"{"mcpServers": {"a": 1, "vario": {}, "z": 2}}"#).unwrap();
    mcp_entry(&path).remove(&mut session(dir.path())).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\n  \"mcpServers\": {\n    \"a\": 1,\n    \"z\": 2\n  }\n}\n"
    );
}

#[test]
fn json_hooks_set_defaults_and_drop_emptied_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".cursor").join("hooks.json");
    let mut part = stop_hooks(&path, json!({"command": "vario hook --via cursor stop"}));
    part.defaults.insert("version".into(), json!(1));
    part.apply(&mut session(dir.path())).unwrap();
    assert_eq!(
        load(&path),
        json!({"version": 1, "hooks": {"Stop": [{"command": "vario hook --via cursor stop"}]}})
    );
    part.remove(&mut session(dir.path())).unwrap();
    assert!(
        !path.exists(),
        "a file holding only Vario's hooks and defaults is deleted"
    );

    write_json(
        &path,
        json!({"version": 1, "hooks": {"stop": [{"command": "mine"}]}}),
    );
    part.apply(&mut session(dir.path())).unwrap();
    part.remove(&mut session(dir.path())).unwrap();
    assert_eq!(
        load(&path),
        json!({"version": 1, "hooks": {"stop": [{"command": "mine"}]}})
    );
}

#[test]
fn json_hooks_refuse_an_event_that_is_not_a_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    write_json(&path, json!({"hooks": {"Stop": "oops"}}));
    let err = stop_hooks(&path, json!({"command": "vario hook stop"}))
        .apply(&mut session(dir.path()))
        .unwrap_err();
    assert!(err.to_string().contains("hooks for Stop are not a list"));
    assert_eq!(load(&path), json!({"hooks": {"Stop": "oops"}}));
}

#[test]
fn markdown_part_keeps_the_user_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.md");
    fs::write(&path, "# Mine\n\nKeep this.\n").unwrap();
    let part = MarkdownBlock {
        path: path.clone(),
        body: "delegate".into(),
    };
    part.apply(&mut session(dir.path())).unwrap();
    part.remove(&mut session(dir.path())).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "# Mine\n\nKeep this.\n");
}

#[test]
fn own_file_never_touches_a_file_without_its_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agents").join("vario-lecteur.md");
    let part = OwnFile {
        path: path.clone(),
        content: "---\nname: vario-lecteur\n---\n<!-- managed by Vario -->\nRead.\n".into(),
        kind: Kind::Subagents,
        marker: Some("<!-- managed by Vario -->".into()),
    };

    part.apply(&mut session(dir.path())).unwrap();
    assert!(part.present());
    part.remove(&mut session(dir.path())).unwrap();
    assert!(!path.exists());

    fs::write(&path, "my own agent").unwrap();
    assert!(!part.present());
    let err = part.apply(&mut session(dir.path())).unwrap_err();
    assert!(err.to_string().contains("not managed by Vario"));
    assert!(part.remove(&mut session(dir.path())).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "my own agent");
}

#[test]
fn dry_run_changes_nothing_but_reports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    let mut dry = Session::with_backup_dir(true, dir.path().join("backups"));
    mcp_entry(&path).apply(&mut dry).unwrap();
    assert!(!path.exists());
    assert_eq!(dry.changes().len(), 1);
}

#[test]
fn vario_hook_commands_are_recognized_by_their_program() {
    use vario_core::parts::is_vario_hook_command;
    assert!(is_vario_hook_command(
        "vario hook subagent-stop --via claude"
    ));
    assert!(is_vario_hook_command(
        "/usr/local/bin/vario hook session-start --via cursor"
    ));
    assert!(is_vario_hook_command(
        r#""/Users/ana b/bin/vario" hook session-start --via cursor"#
    ));
    assert!(is_vario_hook_command(
        r#""C:\Program Files\Vario\vario.exe" hook subagent-stop --via cursor"#
    ));
    assert!(!is_vario_hook_command(
        "python -m ai_handoff hook claude stop"
    ));
    assert!(!is_vario_hook_command("/opt/notvario hook stop"));
    assert!(!is_vario_hook_command("vario install"));
}

#[test]
fn json_hooks_are_present_only_with_the_exact_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.json");
    let old = json!({"command": "/old/place/vario hook stop --via cursor"});
    stop_hooks(&path, old)
        .apply(&mut session(dir.path()))
        .unwrap();
    let current = stop_hooks(
        &path,
        json!({"command": "/new/vario hook stop --via cursor"}),
    );
    assert!(!current.present(), "a moved binary is reported as missing");
    current.apply(&mut session(dir.path())).unwrap();
    assert!(current.present());
    assert_eq!(
        load(&path),
        json!({"hooks": {"Stop": [{"command": "/new/vario hook stop --via cursor"}]}}),
        "the old entry is replaced, not kept beside the new one"
    );
}

#[test]
fn the_program_of_a_hook_command() {
    use vario_core::parts::hook_program;
    assert_eq!(
        hook_program("/usr/local/bin/vario hook subagent-stop --via claude"),
        Some("/usr/local/bin/vario")
    );
    assert_eq!(
        hook_program(
            r#""/Applications/My Apps/Vario.app/Contents/MacOS/vario" hook session-start --via cursor"#
        ),
        Some("/Applications/My Apps/Vario.app/Contents/MacOS/vario")
    );
    assert_eq!(hook_program("python -m ai_handoff hook claude stop"), None);
}
