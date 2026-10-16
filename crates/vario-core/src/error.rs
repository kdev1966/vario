use std::io;
use std::path::{Path, PathBuf};

use crate::lang::Lang;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A configuration file cannot be changed safely; it was left as it was.
    #[error("{} {reason}; it was left unchanged.", path.display())]
    Config { path: PathBuf, reason: Reason },

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Why a file cannot be changed safely.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Reason {
    /// The value at this key path is not a JSON object.
    #[error("'{0}' is not an object")]
    NotAnObject(String),
    #[error("hooks for {0} are not a list")]
    HooksNotAList(String),
    /// A file at Vario's path that does not carry Vario's marker.
    #[error("is not managed by Vario (its marker is missing)")]
    Foreign,
    #[error("is not UTF-8 text")]
    NotUtf8,
    #[error("does not contain a JSON object")]
    NotJsonObject,
    #[error("is not plain JSON (comments?)")]
    NotPlainJson,
    #[error("cannot be recorded for restore ({0})")]
    Unrecordable(String),
    #[error("is not a valid backup manifest ({0})")]
    BadManifest(String),
}

impl Reason {
    pub fn text(&self, lang: Lang) -> String {
        match lang {
            Lang::En => self.to_string(),
            Lang::Fr => match self {
                Reason::NotAnObject(key) => format!("« {key} » n'est pas un objet"),
                Reason::HooksNotAList(event) => {
                    format!("les hooks de {event} ne sont pas une liste")
                }
                Reason::Foreign => "n'est pas géré par Vario (son marqueur manque)".into(),
                Reason::NotUtf8 => "n'est pas du texte UTF-8".into(),
                Reason::NotJsonObject => "ne contient pas d'objet JSON".into(),
                Reason::NotPlainJson => "n'est pas du JSON simple (commentaires ?)".into(),
                Reason::Unrecordable(detail) => {
                    format!("ne peut pas être enregistré pour la restauration ({detail})")
                }
                Reason::BadManifest(detail) => {
                    format!("n'est pas un manifeste de sauvegarde valide ({detail})")
                }
            },
        }
    }
}

impl Error {
    pub(crate) fn config(path: &Path, reason: Reason) -> Self {
        Error::Config {
            path: path.to_path_buf(),
            reason,
        }
    }

    /// The message in `lang`; the system's own error text is not translated.
    pub fn text(&self, lang: Lang) -> String {
        match (self, lang) {
            (_, Lang::En) => self.to_string(),
            (Error::Config { path, reason }, Lang::Fr) => format!(
                "{} : {} ; le fichier n'a pas été modifié.",
                path.display(),
                reason.text(lang)
            ),
            (Error::Io { path, source }, Lang::Fr) => {
                format!("{} : {source}", path.display())
            }
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
