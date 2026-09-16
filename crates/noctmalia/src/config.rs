//! `$XDG_CONFIG_HOME/noctmalia/config.toml`, read once at startup — `docs/config-plan.md`.
//!
//! Missing file: built-in defaults, no complaint. Malformed file: the defaults, plus an error
//! naming the file — never a silent fallback that hides a typo. `config.example.toml` at the repo
//! root is the complete example this schema is measured against; keep it exhaustive as new
//! sections land.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One right-click command template — `docs/context-commands-plan.md`'s first real consumer of
/// this schema. `command` is an argv array, never a shell string: the selection becomes exactly
/// one element of it, so there is no shell quoting to get wrong. `contexts` names which surfaces'
/// selections it applies to (`mail-body`, `person-field`, `event-description`, …); empty means
/// everywhere.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Template {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub contexts: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default, rename = "template")]
    pub templates: Vec<Template>,
}

/// `$XDG_CONFIG_HOME/noctmalia/config.toml`, or `$HOME/.config/noctmalia/config.toml` where
/// `XDG_CONFIG_HOME` is unset — the same precedence `crate::palette` uses for Noctalia's own
/// config directory. Falls back to a temp directory only if neither is set, which just means no
/// config persists — loading never fails outright over a missing `$HOME`.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("noctmalia").join("config.toml")
}

/// What loading found: the config either way, and an error to show if the file existed but didn't
/// parse. Loading never fails outright — a config that won't parse is a toast, not a crash.
pub struct Loaded {
    pub config: Config,
    pub error: Option<String>,
}

pub fn load() -> Loaded {
    load_from(&path())
}

fn load_from(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(config) => Loaded { config, error: None },
            Err(error) => Loaded { config: Config::default(), error: Some(format!("{}: {error}", path.display())) },
        },
        Err(_) => Loaded { config: Config::default(), error: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctmalia-config-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join("config.toml")
    }

    #[test]
    fn a_missing_file_is_the_default_config_with_no_error() {
        let path = scratch("missing").with_file_name("does-not-exist.toml");
        let loaded = load_from(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.error.is_none());
    }

    #[test]
    fn a_well_formed_file_round_trips() {
        let path = scratch("good");
        std::fs::write(
            &path,
            "[[template]]\nname = \"define\"\ncommand = [\"dict\", \"{selection}\"]\ncontexts = [\"mail-body\"]\n",
        )
        .expect("write");
        let loaded = load_from(&path);
        assert!(loaded.error.is_none());
        assert_eq!(loaded.config.templates.len(), 1);
        assert_eq!(loaded.config.templates[0].name, "define");
        assert_eq!(loaded.config.templates[0].command, vec!["dict", "{selection}"]);
        assert_eq!(loaded.config.templates[0].contexts, vec!["mail-body"]);
    }

    #[test]
    fn contexts_defaults_to_empty_which_means_everywhere() {
        let path = scratch("no-contexts");
        std::fs::write(&path, "[[template]]\nname = \"open-in-browser\"\ncommand = [\"xdg-open\", \"{selection}\"]\n")
            .expect("write");
        let loaded = load_from(&path);
        assert!(loaded.config.templates[0].contexts.is_empty());
    }

    #[test]
    fn a_malformed_file_defaults_and_names_itself_in_the_error() {
        let path = scratch("bad");
        std::fs::write(&path, "this is not toml [[[").expect("write");
        let loaded = load_from(&path);
        assert_eq!(loaded.config, Config::default());
        let error = loaded.error.expect("a parse error");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }
}
