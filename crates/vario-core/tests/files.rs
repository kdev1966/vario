use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use vario_core::Error;
use vario_core::files::{self, BLOCK_BEGIN, Session};

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("a JSON object").clone()
}

// Ported from Handoff's tests/test_integrations.py.

#[test]
fn markdown_block_insert_replace_remove() {
    let text = "# My rules\n\nKeep this.\n";
    let once = files::upsert_block(text, "first");
    let twice = files::upsert_block(&once, "second");
    assert_eq!(twice.matches(BLOCK_BEGIN).count(), 1);
    assert!(twice.contains("second") && !twice.contains("first"));
    assert!(twice.starts_with(text));
    assert_eq!(files::remove_block(&twice), text);
    assert_eq!(files::remove_block(&files::upsert_block("", "only")), "");
}

#[test]
fn read_json_refuses_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{\n  // comment\n  \"a\": 1\n}").unwrap();
    let err = files::read_json(&path).unwrap_err();
    assert!(matches!(err, Error::Config { .. }));
    assert!(err.to_string().contains("not plain JSON"));
}

#[test]
fn session_backs_up_once_and_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, "original").unwrap();

    let mut dry = Session::with_backup_dir(true, dir.path().join("backups-dry"));
    dry.write_text(&path, "changed", "test").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(dry.changes(), ["test"]);
    assert!(!dir.path().join("backups-dry").exists());

    let backups = dir.path().join("backups");
    let mut session = Session::with_backup_dir(false, backups.clone());
    session.write_text(&path, "one", "first").unwrap();
    session.write_text(&path, "two", "second").unwrap();
    let copies = find_named(&backups, "config.json");
    assert_eq!(copies.len(), 1);
    assert_eq!(fs::read_to_string(&copies[0]).unwrap(), "original");
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
}

#[cfg(unix)]
#[test]
fn atomic_write_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.json");
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    files::atomic_write(&path, "{\"a\": 1}").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// New in Vario.

#[test]
fn unchanged_content_is_not_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("same.md");
    fs::write(&path, "same").unwrap();
    let mut session = Session::with_backup_dir(false, dir.path().join("backups"));
    session.write_text(&path, "same", "nothing").unwrap();
    assert!(session.changes().is_empty());
    assert!(!dir.path().join("backups").exists());
}

#[test]
fn delete_backs_up_and_dry_run_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.md");
    fs::write(&path, "agent").unwrap();

    let mut dry = Session::with_backup_dir(true, dir.path().join("backups-dry"));
    dry.delete(&path, "remove agent").unwrap();
    assert!(path.exists());
    assert_eq!(dry.changes(), ["remove agent"]);

    let mut session = Session::with_backup_dir(false, dir.path().join("backups"));
    session.delete(&path, "remove agent").unwrap();
    session.delete(&path, "already gone").unwrap();
    assert!(!path.exists());
    assert_eq!(session.changes(), ["remove agent"]);
}

#[test]
fn restore_undoes_a_whole_session() {
    let dir = tempfile::tempdir().unwrap();
    let edited = dir.path().join("settings.json");
    let created = dir.path().join("agents").join("vario-lecteur.md");
    let deleted = dir.path().join("old.md");
    fs::write(&edited, "{\"theme\": \"dark\"}\n").unwrap();
    fs::write(&deleted, "old").unwrap();

    let backups = dir.path().join("backups");
    let mut session = Session::with_backup_dir(false, backups.clone());
    session.write_text(&edited, "{}\n", "edit").unwrap();
    session
        .write_text(&edited, "{\"a\": 1}\n", "edit again")
        .unwrap();
    session.write_text(&created, "agent", "create").unwrap();
    session.delete(&deleted, "delete").unwrap();

    let restored = files::restore(&backups).unwrap();
    assert_eq!(restored.len(), 3);
    assert_eq!(
        fs::read_to_string(&edited).unwrap(),
        "{\"theme\": \"dark\"}\n"
    );
    assert!(!created.exists());
    assert_eq!(fs::read_to_string(&deleted).unwrap(), "old");
}

#[cfg(unix)]
#[test]
fn writing_through_a_symlink_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles").join("settings.json");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "{}").unwrap();
    let link = dir.path().join("settings.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    files::atomic_write(&link, "{\"a\": 1}").unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&real).unwrap(), "{\"a\": 1}");
}

#[test]
fn read_json_cases() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        files::read_json(&dir.path().join("missing.json"))
            .unwrap()
            .is_empty()
    );

    let empty = dir.path().join("empty.json");
    fs::write(&empty, "  \n").unwrap();
    assert!(files::read_json(&empty).unwrap().is_empty());

    let bom = dir.path().join("bom.json");
    fs::write(&bom, "\u{feff}{\"a\": 1}").unwrap();
    assert_eq!(files::read_json(&bom).unwrap(), object(json!({"a": 1})));

    let list = dir.path().join("list.json");
    fs::write(&list, "[1, 2]").unwrap();
    let err = files::read_json(&list).unwrap_err();
    assert!(err.to_string().contains("does not contain a JSON object"));
}

#[test]
fn dump_json_keeps_key_order_and_unicode() {
    let data = object(json!({"zeta": 1, "alpha": "é"}));
    assert_eq!(
        files::dump_json(&data),
        "{\n  \"zeta\": 1,\n  \"alpha\": \"é\"\n}\n"
    );
}

#[test]
fn nested_mut_creates_or_reports_absence() {
    let mut data = object(json!({"hooks": {"Stop": []}, "theme": "dark"}));
    assert!(files::nested_mut(&mut data, &["mcpServers"], false).is_none());
    assert!(files::nested_mut(&mut data, &["theme", "x"], true).is_none());
    files::nested_mut(&mut data, &["mcpServers", "vario"], true)
        .unwrap()
        .insert("command".into(), json!("vario"));
    assert_eq!(data["mcpServers"]["vario"]["command"], "vario");
    assert_eq!(data["theme"], "dark");
}

#[test]
fn block_spacing_and_removal_in_the_middle() {
    assert_eq!(
        files::upsert_block("no newline", "x"),
        format!("no newline\n\n{BLOCK_BEGIN}\nx\n<!-- vario:end -->\n")
    );
    let text = format!("before\n\n{BLOCK_BEGIN}\nx\n<!-- vario:end -->\nafter\n");
    assert!(files::has_block(&text));
    assert_eq!(files::remove_block(&text), "before\n\nafter\n");
    assert!(!files::has_block("<!-- vario:begin without end"));
}

fn find_named(root: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == name) {
                found.push(path);
            }
        }
    }
    found
}
