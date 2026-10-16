//! Careful edits of other tools' configuration files.
//!
//! Rules: back up before the first change, write atomically while keeping the file's
//! permissions, keep every key we do not own, and refuse to rewrite a file we cannot
//! parse exactly (for example JSON with comments) rather than risk losing content.
//! Every session records what it touched, so that [`restore`] can undo it.

use std::collections::HashSet;
use std::fs::{self, Permissions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result, io};
use crate::paths;

pub const BLOCK_BEGIN: &str =
    "<!-- vario:begin (managed by Vario; edits inside are overwritten) -->";
pub const BLOCK_END: &str = "<!-- vario:end -->";
/// A block is recognized by this prefix, so the wording after it may change.
const BLOCK_PREFIX: &str = "<!-- vario:begin";

const MANIFEST: &str = "manifest.json";
const MANIFEST_VERSION: u32 = 1;

/// One run of setup or removal: where backups go, and what changed.
#[derive(Debug)]
pub struct Session {
    dry_run: bool,
    backup_dir: PathBuf,
    changes: Vec<String>,
    touched: HashSet<PathBuf>,
    manifest: Manifest,
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
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string();
        Self::with_backup_dir(dry_run, paths::data_dir().join("backups").join(stamp))
    }

    pub fn with_backup_dir(dry_run: bool, backup_dir: PathBuf) -> Self {
        Session {
            dry_run,
            backup_dir,
            changes: Vec::new(),
            touched: HashSet::new(),
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

    /// Descriptions of the changes made, or that a dry run would make.
    pub fn changes(&self) -> &[String] {
        &self.changes
    }

    /// Write `content` to `path`, unless the file already holds exactly that.
    pub fn write_text(
        &mut self,
        path: &Path,
        content: &str,
        description: impl Into<String>,
    ) -> Result<()> {
        match fs::read(path) {
            Ok(current) if current == content.as_bytes() => return Ok(()),
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(io(path)(err)),
        }
        self.changes.push(description.into());
        if self.dry_run {
            return Ok(());
        }
        self.record(path)?;
        atomic_write(path, content)
    }

    /// Delete `path` if it exists.
    pub fn delete(&mut self, path: &Path, description: impl Into<String>) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        self.changes.push(description.into());
        if self.dry_run {
            return Ok(());
        }
        self.record(path)?;
        fs::remove_file(path).map_err(io(path))
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
        let json = serde_json::to_string_pretty(&self.manifest).map_err(|err| {
            Error::config(path, format!("cannot be recorded for restore ({err})"))
        })?;
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

/// Put back every file a session changed, as it was before; delete the files it created.
/// Returns the restored paths. The current state is not backed up first.
pub fn restore(backup_dir: &Path) -> Result<Vec<PathBuf>> {
    let manifest_path = backup_dir.join(MANIFEST);
    let text = fs::read_to_string(&manifest_path).map_err(io(&manifest_path))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|err| {
        Error::config(
            &manifest_path,
            format!("is not a valid backup manifest ({err})"),
        )
    })?;
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
            return Err(Error::config(path, "is not UTF-8 text"));
        }
        Err(err) => return Err(io(path)(err)),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(data)) => Ok(data),
        Ok(_) => Err(Error::config(path, "does not contain a JSON object")),
        Err(_) => Err(Error::config(path, "is not plain JSON (comments?)")),
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
            Err(Error::config(path, "is not UTF-8 text"))
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
