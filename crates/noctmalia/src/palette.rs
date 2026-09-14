//! Following the palette the user's Noctalia shell is actually running.
//!
//! noctalia-shell keeps its settings in `$XDG_STATE_HOME/noctalia/settings.toml` and its palettes as
//! JSON with a `dark` and a `light` set of the sixteen roles. Reading them means a wallpaper or
//! theme change carries into this window like it does into the rest of the desktop, which is the
//! whole point of the design language — noctalia-iced's built-in palette is only a fallback.

use noctalia_iced::theme::{self, Palette};
use noctalia_iced::widgets::parse_hex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// How often the files are re-read. They change when a person changes their theme, so this is about
/// feeling immediate, not about throughput.
const POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
struct Settings {
    theme: Option<ThemeSettings>,
}

#[derive(Debug, Deserialize)]
struct ThemeSettings {
    /// `custom`, `community`, or a built-in (which ships inside noctalia-shell, not on disk).
    #[serde(default)]
    source: String,
    #[serde(default)]
    custom_palette: String,
    #[serde(default)]
    community_palette: String,
    /// `dark` or `light`.
    #[serde(default)]
    mode: String,
}

/// A palette file: the same sixteen roles under each mode.
#[derive(Debug, Deserialize)]
struct PaletteFile {
    dark: Option<Roles>,
    light: Option<Roles>,
}

/// Every role is optional: a palette that omits one keeps noctalia-iced's default for it rather
/// than failing to load at all.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Roles {
    m_primary: Option<String>,
    m_on_primary: Option<String>,
    m_secondary: Option<String>,
    m_on_secondary: Option<String>,
    m_tertiary: Option<String>,
    m_on_tertiary: Option<String>,
    m_error: Option<String>,
    m_on_error: Option<String>,
    m_surface: Option<String>,
    m_on_surface: Option<String>,
    m_surface_variant: Option<String>,
    m_on_surface_variant: Option<String>,
    m_outline: Option<String>,
    m_shadow: Option<String>,
    m_hover: Option<String>,
    m_on_hover: Option<String>,
}

impl Roles {
    fn into_palette(self) -> Palette {
        let base = theme::DEFAULT_PALETTE;
        let color = |value: Option<String>, fallback| value.as_deref().and_then(parse_hex).unwrap_or(fallback);
        Palette {
            primary: color(self.m_primary, base.primary),
            on_primary: color(self.m_on_primary, base.on_primary),
            secondary: color(self.m_secondary, base.secondary),
            on_secondary: color(self.m_on_secondary, base.on_secondary),
            tertiary: color(self.m_tertiary, base.tertiary),
            on_tertiary: color(self.m_on_tertiary, base.on_tertiary),
            error: color(self.m_error, base.error),
            on_error: color(self.m_on_error, base.on_error),
            surface: color(self.m_surface, base.surface),
            on_surface: color(self.m_on_surface, base.on_surface),
            surface_variant: color(self.m_surface_variant, base.surface_variant),
            on_surface_variant: color(self.m_on_surface_variant, base.on_surface_variant),
            outline: color(self.m_outline, base.outline),
            shadow: color(self.m_shadow, base.shadow),
            hover: color(self.m_hover, base.hover),
            on_hover: color(self.m_on_hover, base.on_hover),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn state_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(path) => Some(PathBuf::from(path).join("noctalia")),
        None => Some(home()?.join(".local/state/noctalia")),
    }
}

fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) => Some(PathBuf::from(path).join("noctalia")),
        None => Some(home()?.join(".config/noctalia")),
    }
}

/// The palette file the settings point at, if it is one that lives on disk.
fn palette_path(settings: &ThemeSettings, state: &Path, config: &Path) -> Option<PathBuf> {
    match settings.source.as_str() {
        "custom" if !settings.custom_palette.is_empty() => {
            Some(config.join("palettes").join(format!("{}.json", settings.custom_palette)))
        }
        // Community palette names are stored URL-encoded, because they come from a catalogue where
        // they may contain spaces.
        "community" if !settings.community_palette.is_empty() => {
            Some(state.join("community-palettes").join(format!("{}.json", encode(&settings.community_palette))))
        }
        // Built-in palettes ship inside noctalia-shell; there is nothing to read.
        _ => None,
    }
}

/// Percent-encoding, matching how noctalia names the files it caches.
fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The palette Noctalia is running, or `None` if it cannot be read — a built-in theme, a shell that
/// is not installed, a file we do not understand. The caller keeps noctalia-iced's default.
pub fn load() -> Option<Palette> {
    load_from(&state_dir()?, &config_dir()?)
}

/// [`load`] against explicit directories, so it can be tested without a Noctalia install.
fn load_from(state: &Path, config: &Path) -> Option<Palette> {
    let settings: Settings = toml::from_str(&std::fs::read_to_string(state.join("settings.toml")).ok()?).ok()?;
    let settings = settings.theme?;

    let path = palette_path(&settings, state, config)?;
    let file: PaletteFile = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let roles = if settings.mode == "light" { file.light } else { file.dark };
    Some(roles.unwrap_or_default().into_palette())
}

/// Palette changes, as they happen.
///
/// Polling beats an inotify watch here: the files are small, a second of latency is invisible, and
/// polling is immune to the write-to-temp-and-rename that anything editing config files does. The
/// thread only sends when the palette actually differs, so an unchanged theme costs no redraws.
pub struct Changes(mpsc::UnboundedReceiver<Palette>);

impl Changes {
    pub async fn next(&mut self) -> Option<Palette> {
        self.0.recv().await
    }
}

/// Starts watching. Returns the palette in force now, and a stream of later ones.
pub fn watch() -> (Option<Palette>, Changes) {
    let current = load();
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut last = current;
    std::thread::Builder::new()
        .name("noctmalia-palette".into())
        .spawn(move || {
            loop {
                std::thread::sleep(POLL);
                let found = load();
                if found != last {
                    last = found;
                    // A palette that disappears leaves the last one in force rather than snapping
                    // back to the built-in one mid-session.
                    if let Some(palette) = found
                        && sender.send(palette).is_err()
                    {
                        return;
                    }
                }
            }
        })
        .ok();
    (current, Changes(receiver))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_role_names_a_noctalia_palette_file_uses() {
        let file = r##"{"dark":{"mPrimary":"#A7C080","mSurface":"#232A2E","mOnSurface":"#859289"}}"##;
        let parsed: PaletteFile = serde_json::from_str(file).expect("parse");
        let palette = parsed.dark.expect("dark").into_palette();
        assert_eq!(palette.primary, parse_hex("#A7C080").unwrap());
        assert_eq!(palette.surface, parse_hex("#232A2E").unwrap());
        // A role the file leaves out keeps the built-in value rather than going black.
        assert_eq!(palette.outline, theme::DEFAULT_PALETTE.outline);
    }

    #[test]
    fn community_palette_names_are_encoded_the_way_noctalia_caches_them() {
        assert_eq!(encode("Everforest Alt"), "Everforest%20Alt");
        assert_eq!(encode("Everforest"), "Everforest");
    }

    #[test]
    fn a_builtin_theme_has_no_file_to_read() {
        let builtin = ThemeSettings {
            source: "builtin".into(),
            custom_palette: String::new(),
            community_palette: String::new(),
            mode: "dark".into(),
        };
        // Built-ins live inside noctalia-shell, so there is nothing to follow and the caller keeps
        // noctalia-iced's own palette.
        assert!(palette_path(&builtin, Path::new("/state"), Path::new("/config")).is_none());
    }

    /// A Noctalia install, as far as this module is concerned.
    fn install(directory: &Path, settings: &str, palette_file: &str) -> (PathBuf, PathBuf) {
        let state = directory.join("state");
        let config = directory.join("config");
        std::fs::create_dir_all(config.join("palettes")).expect("config");
        std::fs::create_dir_all(state.join("community-palettes")).expect("state");
        std::fs::write(state.join("settings.toml"), settings).expect("settings");
        std::fs::write(config.join("palettes").join("Mine.json"), palette_file).expect("palette");
        std::fs::write(state.join("community-palettes").join("Some%20Palette.json"), palette_file).expect("palette");
        (state, config)
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("noctmalia-palette-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    const PALETTE: &str = r##"{
      "dark":  {"mPrimary": "#A7C080", "mSurface": "#232A2E"},
      "light": {"mPrimary": "#434F55", "mSurface": "#9DA9A0"}
    }"##;

    #[test]
    fn reads_a_custom_palette_in_the_mode_the_settings_name() {
        let directory = scratch("custom");
        let settings = "config_version = 14

[theme]
source = \"custom\"
custom_palette = \"Mine\"
mode = \"dark\"
";
        let (state, config) = install(&directory, settings, PALETTE);

        let dark = load_from(&state, &config).expect("a palette");
        assert_eq!(dark.primary, parse_hex("#A7C080").unwrap());
        assert_eq!(dark.surface, parse_hex("#232A2E").unwrap());

        let light = settings.replace("mode = \"dark\"", "mode = \"light\"");
        std::fs::write(state.join("settings.toml"), light).expect("settings");
        let light = load_from(&state, &config).expect("a palette");
        assert_eq!(light.surface, parse_hex("#9DA9A0").unwrap());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn reads_a_community_palette_under_its_encoded_name() {
        let directory = scratch("community");
        let settings = "[theme]
source = \"community\"
community_palette = \"Some Palette\"
mode = \"dark\"
";
        let (state, config) = install(&directory, settings, PALETTE);
        assert_eq!(load_from(&state, &config).expect("a palette").primary, parse_hex("#A7C080").unwrap());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn no_noctalia_install_is_not_an_error() {
        assert!(load_from(Path::new("/nonexistent/state"), Path::new("/nonexistent/config")).is_none());
    }

    #[test]
    fn settings_naming_a_palette_that_is_not_there_fall_back() {
        let directory = scratch("missing");
        let settings = "[theme]
source = \"custom\"
custom_palette = \"Gone\"
mode = \"dark\"
";
        let (state, config) = install(&directory, settings, PALETTE);
        assert!(load_from(&state, &config).is_none());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
