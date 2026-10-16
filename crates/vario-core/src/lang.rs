//! The language of the text shown to people. Only that text is translated: what AI
//! agents read (sub-agents, instructions) stays in English.

use crate::clients::Env;

/// Overrides the detected language: `VARIO_LANG=fr`.
pub const ENV_LANG: &str = "VARIO_LANG";
/// Vario's own override, then the POSIX precedence for messages.
const VARIABLES: [&str; 5] = [ENV_LANG, "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"];

/// The text in the user's language: `tr!(lang, "Done: {count}.", "Terminé : {count}.")`.
/// Both literals may use the same inline arguments. Every front end words its own
/// messages with it.
#[macro_export]
macro_rules! tr {
    ($lang:expr, $en:literal, $fr:literal) => {
        match $lang {
            $crate::lang::Lang::En => format!($en),
            $crate::lang::Lang::Fr => format!($fr),
        }
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    /// `fr_FR.UTF-8` and `fr-TN` give French; `de:fr` gives the first supported one;
    /// `C` or an unsupported language gives `None`.
    pub fn parse(value: &str) -> Option<Lang> {
        value.split(':').find_map(|part| {
            let code = part.trim().split(['_', '.', '@', '-']).next()?;
            match code.to_lowercase().as_str() {
                "en" => Some(Lang::En),
                "fr" => Some(Lang::Fr),
                _ => None,
            }
        })
    }

    /// The language set in the environment, if any.
    pub fn from_env(env: &Env) -> Option<Lang> {
        VARIABLES
            .iter()
            .filter_map(|name| env.var(name))
            .find_map(|value| Lang::parse(&value.to_string_lossy()))
    }

    /// The environment's language, else the system's preferred one, else English.
    /// macOS terminals often leave `LANG` unset, hence the system lookup.
    pub fn detect(env: &Env) -> Lang {
        Lang::from_env(env)
            .or_else(|| sys_locale::get_locales().find_map(|locale| Lang::parse(&locale)))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn parse_locales() {
        assert_eq!(Lang::parse("fr_FR.UTF-8"), Some(Lang::Fr));
        assert_eq!(Lang::parse("fr-TN"), Some(Lang::Fr));
        assert_eq!(Lang::parse("de:fr"), Some(Lang::Fr));
        assert_eq!(Lang::parse("en_US"), Some(Lang::En));
        assert_eq!(Lang::parse("C"), None);
        assert_eq!(Lang::parse(""), None);
    }

    #[test]
    fn the_override_comes_first_and_unsupported_values_are_skipped() {
        let mut env = Env::default();
        assert_eq!(Lang::from_env(&env), None);
        env.vars.insert("LC_ALL".into(), OsString::from("C"));
        env.vars
            .insert("LANG".into(), OsString::from("fr_FR.UTF-8"));
        assert_eq!(Lang::from_env(&env), Some(Lang::Fr));
        env.vars.insert(ENV_LANG.into(), OsString::from("en"));
        assert_eq!(Lang::from_env(&env), Some(Lang::En));
    }
}
