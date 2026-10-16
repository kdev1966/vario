use std::io;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A configuration file cannot be changed safely; it was left as it was.
    #[error("{} {reason}; it was left unchanged.", path.display())]
    Config { path: PathBuf, reason: String },

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn config(path: &Path, reason: impl Into<String>) -> Self {
        Error::Config {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

/// Attach the file a failed I/O call was about: `fs::read(p).map_err(io(p))`.
pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}
