//! One interface at a time. The first `vario-gui` holds a lock in the data folder; a
//! later one (a hook opening the gauge, Vario.app opened again) leaves a request
//! there, which the running one reads, then exits.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;

const LOCK: &str = "gui.lock";
const REQUESTS: &str = "gui.requests";

/// What a started interface wants to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Gauge,
    Main,
}

impl Want {
    fn name(self) -> &'static str {
        match self {
            Want::Gauge => "gauge",
            Want::Main => "main",
        }
    }
}

/// Holds the lock for as long as the interface runs.
pub struct Instance {
    _lock: Option<File>,
}

/// Become the running interface, or `None` when another one already is. When the
/// lock cannot be used at all (a file system without locks), this one runs anyway.
pub fn claim(data_dir: &Path) -> Option<Instance> {
    let _ = fs::create_dir_all(data_dir);
    let Ok(file) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join(LOCK))
    else {
        return Some(Instance { _lock: None });
    };
    match file.try_lock() {
        Ok(()) => {
            // Requests left by an earlier run are stale.
            let _ = fs::remove_file(data_dir.join(REQUESTS));
            Some(Instance { _lock: Some(file) })
        }
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(_)) => Some(Instance { _lock: None }),
    }
}

/// Ask the running interface to show something.
pub fn request(data_dir: &Path, want: Want) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join(REQUESTS))
    {
        let _ = writeln!(file, "{}", want.name());
    }
}

/// The requests left since the last call, in order, without repeats.
pub fn take_requests(data_dir: &Path) -> Vec<Want> {
    let path = data_dir.join(REQUESTS);
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let _ = fs::remove_file(&path);
    let mut wants = Vec::new();
    for want in text.lines().filter_map(|line| match line.trim() {
        "gauge" => Some(Want::Gauge),
        "main" => Some(Want::Main),
        _ => None,
    }) {
        if !wants.contains(&want) {
            wants.push(want);
        }
    }
    wants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_interface_leaves_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let first = claim(&data).expect("the first one runs");
        assert!(claim(&data).is_none(), "the second one does not");
        request(&data, Want::Gauge);
        request(&data, Want::Main);
        request(&data, Want::Gauge);
        assert_eq!(take_requests(&data), [Want::Gauge, Want::Main]);
        assert!(take_requests(&data).is_empty());
        drop(first);
        assert!(claim(&data).is_some(), "free again once the first one ends");
    }
}
