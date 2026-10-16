//! Careful edits of other tools' configuration files.
//!
//! Rules: back up before the first change, write atomically while keeping the file's
//! permissions, keep every key we do not own, and refuse to rewrite a file we cannot
//! parse exactly (for example JSON with comments) rather than risk losing content.
//! Every session records what it touched, so that [`restore`] can undo it.

use std::collections::{HashMap, HashSet};
use std::fs::{self, Permissions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Reason, Result, io};
use crate::lang::Lang;
use crate::paths;

pub const BLOCK_BEGIN: &str =
    "<!-- vario:begin (managed by Vario; edits inside are overwritten) -->";
pub const BLOCK_END: &str = "<!-- vario:end -->";
/// A block is recognized by this prefix, so the wording after it may change.
const BLOCK_PREFIX: &str = "<!-- vario:begin";

const MANIFEST: &str = "manifest.json";
const MANIFEST_VERSION: u32 = 1;

/// One change to a file, as shown in previews and reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A file that belongs to Vario was written.
    Written,
    Deleted,
    /// An entry was set at this key path (`mcpServers.vario`).
    EntrySet(String),
    EntryRemoved(String),
    /// Vario's hooks were set for these events (`SessionStart, Stop`).
    HooksSet(String),
    HooksRemoved(String),
    InstructionsSet,
    InstructionsRemoved,
    /// The instructions file only held Vario's block, so it was deleted.
    InstructionsFileDeleted,
}

impl Action {
    pub fn text(&self, lang: Lang) -> String {
        match (self, lang) {
            (Action::Written, Lang::En) => "written".into(),
            (Action::Written, Lang::Fr) => "écriture".into(),
            (Action::Deleted, Lang::En) => "deleted".into(),
            (Action::Deleted, Lang::Fr) => "suppression".into(),
            (Action::EntrySet(key), Lang::En) => format!("set {key}"),
            (Action::EntrySet(key), Lang::Fr) => format!("ajout de {key}"),
            (Action::EntryRemoved(key), Lang::En) => format!("removed {key}"),
            (Action::EntryRemoved(key), Lang::Fr) => format!("retrait de {key}"),
            (Action::HooksSet(events), Lang::En) => format!("hooks {events}"),
            (Action::HooksSet(events), Lang::Fr) => format!("hooks {events}"),
            (Action::HooksRemoved(events), Lang::En) => format!("removed hooks {events}"),
            (Action::HooksRemoved(events), Lang::Fr) => format!("retrait des hooks {events}"),
            (Action::InstructionsSet, Lang::En) => "Vario instructions".into(),
            (Action::InstructionsSet, Lang::Fr) => "consignes de Vario".into(),
            (Action::InstructionsRemoved, Lang::En) => "removed Vario instructions".into(),
            (Action::InstructionsRemoved, Lang::Fr) => "retrait des consignes de Vario".into(),
            (Action::InstructionsFileDeleted, Lang::En) => {
                "deleted (it only held Vario instructions)".into()
            }
            (Action::InstructionsFileDeleted, Lang::Fr) => {
                "suppression (il ne contenait que les consignes de Vario)".into()
            }
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.action.text(Lang::En))
    }
}

/// One run of setup or removal: where backups go, and what changed.
#[derive(Debug)]
pub struct Session {
    dry_run: bool,
    backup_dir: PathBuf,
    changes: Vec<Change>,
    touched: HashSet<PathBuf>,
    manifest: Manifest,
    /// In a dry run, what each path would hold (`None`: deleted), so that two parts
    /// making the same change to a shared file report it once, as a real run would.
    simulated: HashMap<PathBuf, Option<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: Vec<Entry>,
}

/// The state of a file before the session first changed it.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    /// Copy of the original, relative to the backup directory; `None` when the
    /// session created the file, so restoring deletes it.
    backup: Option<PathBuf>,
}

impl Session {
    /// A session whose backups go to a new, timestamped folder in Vario's data directory.
    pub fn new(dry_run: bool) -> Self {
        Self::in_data_dir(dry_run, &paths::data_dir())
    }

    /// Same, with an explicit data directory.
    pub fn in_data_dir(dry_run: bool, data_dir: &Path) -> Self {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string();
        Self::with_backup_dir(dry_run, backups_dir(data_dir).join(stamp))
    }

    pub fn with_backup_dir(dry_run: bool, backup_dir: PathBuf) -> Self {
        Session {
            dry_run,
            backup_dir,
            changes: Vec::new(),
            touched: HashSet::new(),
            simulated: HashMap::new(),
            manifest: Manifest {
                version: MANIFEST_VERSION,
                entries: Vec::new(),
            },
        }
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    /// The changes made, or that a dry run would make.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Write `content` to `path`, unless the file already holds exactly that.
    pub fn write_text(&mut self, path: &Path, content: &str, action: Action) -> Result<()> {
        match fs::read(path) {
            Ok(current) if current == content.as_bytes() => return Ok(()),
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(io(path)(err)),
        }
        if self.dry_run {
            if self.simulated.get(path) == Some(&Some(content.to_string())) {
                return Ok(());
            }
            self.simulated
                .insert(path.to_path_buf(), Some(content.to_string()));
            self.note(path, action);
            return Ok(());
        }
        self.note(path, action);
        self.record(path)?;
        atomic_write(path, content)
    }

    /// Delete `path` if it exists.
    pub fn delete(&mut self, path: &Path, action: Action) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        if self.dry_run {
            if self.simulated.insert(path.to_path_buf(), None) == Some(None) {
                return Ok(());
            }
            self.note(path, action);
            return Ok(());
        }
        self.note(path, action);
        self.record(path)?;
        fs::remove_file(path).map_err(io(path))
    }

    fn note(&mut self, path: &Path, action: Action) {
        self.changes.push(Change {
            path: path.to_path_buf(),
            action,
        });
    }

    /// Save the state of `path` before its first change in this session.
    fn record(&mut self, path: &Path) -> Result<()> {
        if self.touched.contains(path) {
            return Ok(());
        }
        let absolute = std::path::absolute(path).map_err(io(path))?;
        let backup = if path.exists() {
            let relative = backup_relative(path)?;
            let target = self.backup_dir.join(&relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(io(parent))?;
            }
            fs::copy(path, &target).map_err(io(path))?;
            Some(relative)
        } else {
            None
        };
        self.touched.insert(path.to_path_buf());
        self.manifest.entries.push(Entry {
            path: absolute,
            backup,
        });
        let manifest_path = self.backup_dir.join(MANIFEST);
        let json = serde_json::to_string_pretty(&self.manifest)
            .map_err(|err| Error::config(path, Reason::Unrecordable(err.to_string())))?;
        fs::create_dir_all(&self.backup_dir).map_err(io(&self.backup_dir))?;
        atomic_write(&manifest_path, &(json + "\n"))
    }
}

/// Where the copy of `path` goes inside a backup folder: its absolute path, made relative.
fn backup_relative(path: &Path) -> Result<PathBuf> {
    let resolved = fs::canonicalize(path).map_err(io(path))?;
    let text = resolved.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    Ok(PathBuf::from(
        text.replace(':', "").trim_start_matches(['/', '\\']),
    ))
}

pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// The backup folders that can be restored, oldest first (their names are timestamps).
pub fn backups(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(backups_dir(data_dir)) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join(MANIFEST).is_file())
        .collect();
    folders.sort();
    folders
}

fn read_manifest(backup_dir: &Path) -> Result<Manifest> {
    let manifest_path = backup_dir.join(MANIFEST);
    let text = fs::read_to_string(&manifest_path).map_err(io(&manifest_path))?;
    serde_json::from_str(&text)
        .map_err(|err| Error::config(&manifest_path, Reason::BadManifest(err.to_string())))
}

/// What [`restore`] would do: each path, and whether it is put back (`true`) or
/// deleted because the session created it (`false`).
pub fn backed_up(backup_dir: &Path) -> Result<Vec<(PathBuf, bool)>> {
    Ok(read_manifest(backup_dir)?
        .entries
        .into_iter()
        .map(|entry| (entry.path, entry.backup.is_some()))
        .collect())
}

/// Put back every file a session changed, as it was before; delete the files it created.
/// Returns the restored paths. The current state is not backed up first.
pub fn restore(backup_dir: &Path) -> Result<Vec<PathBuf>> {
    let manifest = read_manifest(backup_dir)?;
    let mut restored = Vec::new();
    for entry in manifest.entries.iter().rev() {
        match &entry.backup {
            Some(relative) => {
                let copy = backup_dir.join(relative);
                let bytes = fs::read(&copy).map_err(io(&copy))?;
                let permissions = fs::metadata(&copy).map_err(io(&copy))?.permissions();
                write_atomically(&entry.path, &bytes, Some(permissions))?;
            }
            None => match fs::remove_file(&entry.path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(io(&entry.path)(err)),
            },
        }
        restored.push(entry.path.clone());
    }
    Ok(restored)
}

/// Replace the file's content in one step, keeping its permissions. A symbolic link
/// is followed, so the file it points to is updated and the link is kept.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    write_atomically(path, content.as_bytes(), None)
}

fn write_atomically(path: &Path, bytes: &[u8], permissions: Option<Permissions>) -> Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).map_err(io(parent))?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".vario-tmp")
        .tempfile_in(parent)
        .map_err(io(&target))?;
    tmp.write_all(bytes).map_err(io(&target))?;
    tmp.as_file().sync_all().map_err(io(&target))?;
    let permissions =
        permissions.or_else(|| fs::metadata(&target).ok().map(|meta| meta.permissions()));
    if let Some(permissions) = permissions {
        fs::set_permissions(tmp.path(), permissions).map_err(io(&target))?;
    }
    tmp.persist(&target).map_err(|err| io(&target)(err.error))?;
    Ok(())
}

/// Read a JSON object; a missing or empty file is an empty object.
pub fn read_json(path: &Path) -> Result<Map<String, Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(err) if err.kind() == ErrorKind::InvalidData => {
            return Err(Error::config(path, Reason::NotUtf8));
        }
        Err(err) => return Err(io(path)(err)),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(data)) => Ok(data),
        Ok(_) => Err(Error::config(path, Reason::NotJsonObject)),
        Err(_) => Err(Error::config(path, Reason::NotPlainJson)),
    }
}

pub fn dump_json(data: &Map<String, Value>) -> String {
    let json = serde_json::to_string_pretty(data).expect("a JSON object always serializes");
    json + "\n"
}

/// Read a text file; a missing file is an empty text.
pub fn read_text(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(err) if err.kind() == ErrorKind::InvalidData => {
            Err(Error::config(path, Reason::NotUtf8))
        }
        Err(err) => Err(io(path)(err)),
    }
}

/// The object at `data[k1][k2]...`; `None` if absent or not an object.
pub fn nested<'a>(data: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    let mut current = data;
    for &key in keys {
        current = current.get(key)?.as_object()?;
    }
    Some(current)
}

/// The object at `data[k1][k2]...`, created when asked; `None` if absent or not an object.
pub fn nested_mut<'a>(
    data: &'a mut Map<String, Value>,
    keys: &[&str],
    create: bool,
) -> Option<&'a mut Map<String, Value>> {
    let mut current = data;
    for &key in keys {
        if !current.contains_key(key) {
            if !create {
                return None;
            }
            current.insert(key.to_owned(), Value::Object(Map::new()));
        }
        current = current.get_mut(key)?.as_object_mut()?;
    }
    Some(current)
}

/// Byte range of the first Vario block, including the newline that ends it.
fn find_block(text: &str) -> Option<(usize, usize)> {
    let start = text.find(BLOCK_PREFIX)?;
    let mut end = start + text[start..].find(BLOCK_END)? + BLOCK_END.len();
    if text[end..].starts_with('\n') {
        end += 1;
    }
    Some((start, end))
}

/// Insert or replace the Vario block in a Markdown text, keeping everything else.
pub fn upsert_block(text: &str, body: &str) -> String {
    let block = format!("{BLOCK_BEGIN}\n{}\n{BLOCK_END}\n", body.trim());
    if let Some((start, end)) = find_block(text) {
        return format!("{}{block}{}", &text[..start], &text[end..]);
    }
    let separator = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{text}{separator}{block}")
}

/// Remove every Vario block; a text left blank becomes empty.
pub fn remove_block(text: &str) -> String {
    let mut stripped = text.to_owned();
    while let Some((start, end)) = find_block(&stripped) {
        stripped.replace_range(start..end, "");
    }
    if stripped.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", stripped.trim_end_matches('\n'))
    }
}

pub fn has_block(text: &str) -> bool {
    find_block(text).is_some()
}
