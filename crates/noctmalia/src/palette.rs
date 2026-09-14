//! Following the palette the user's Noctalia shell is actually running.
//!
//! Noctalia resolves its colours from one of four sources — a built-in palette, one generated from
//! the wallpaper, a community palette, or a custom one — and only the last two exist as files
//! anyone else can read. Built-ins are compiled into the shell, and "pure black" re-anchors the
//! whole dark surface ramp rather than darkening a single role, so reading `settings.toml` and the
//! palette files it names cannot tell us what is actually on screen.
//!
//! What Noctalia does offer is templates: on every theme change it renders the palette it resolved
//! through whatever template files are configured. So noctmalia ships one, registers it, and reads
//! what comes out. That follows all four sources, and "pure black" along with them, because the
//! shell has already done the resolving — there is nothing left for us to reimplement or to get
//! out of step with when Noctalia changes.
//!
//! [`install`] writes both halves. Until it has run there is nothing to read, and the caller keeps
//! noctalia-iced's built-in palette.

use noctalia_iced::theme::{self, Palette};
use noctalia_iced::widgets::parse_hex;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

/// How often the rendered palette is re-read. It changes when a person changes their theme, so this
/// is about feeling immediate, not about throughput.
const POLL: Duration = Duration::from_secs(1);

/// The template Noctalia renders for us. Shipped in the binary so [`install`] needs nothing but the
/// binary itself — a checkout that moves afterwards does not break the registration.
const TEMPLATE: &str = include_str!("../assets/palette.tpl");

/// The name of the file we register ourselves in. Noctalia merges every `*.toml` in its config
/// directory, so this sits beside its own settings rather than editing them.
const REGISTRATION: &str = "noctmalia.toml";

/// The sixteen roles, as the template writes them. Every one is optional: a render that omits one
/// keeps noctalia-iced's default for it rather than failing to load at all.
#[derive(Debug, Default, Deserialize)]
struct Roles {
    primary: Option<String>,
    on_primary: Option<String>,
    secondary: Option<String>,
    on_secondary: Option<String>,
    tertiary: Option<String>,
    on_tertiary: Option<String>,
    error: Option<String>,
    on_error: Option<String>,
    surface: Option<String>,
    on_surface: Option<String>,
    surface_variant: Option<String>,
    on_surface_variant: Option<String>,
    outline: Option<String>,
    shadow: Option<String>,
    hover: Option<String>,
    on_hover: Option<String>,
}

impl Roles {
    fn into_palette(self) -> Palette {
        let base = theme::DEFAULT_PALETTE;
        let color = |value: Option<String>, fallback| value.as_deref().and_then(parse_hex).unwrap_or(fallback);
        Palette {
            primary: color(self.primary, base.primary),
            on_primary: color(self.on_primary, base.on_primary),
            secondary: color(self.secondary, base.secondary),
            on_secondary: color(self.on_secondary, base.on_secondary),
            tertiary: color(self.tertiary, base.tertiary),
            on_tertiary: color(self.on_tertiary, base.on_tertiary),
            error: color(self.error, base.error),
            on_error: color(self.on_error, base.on_error),
            surface: color(self.surface, base.surface),
            on_surface: color(self.on_surface, base.on_surface),
            surface_variant: color(self.surface_variant, base.surface_variant),
            on_surface_variant: color(self.on_surface_variant, base.on_surface_variant),
            outline: color(self.outline, base.outline),
            shadow: color(self.shadow, base.shadow),
            hover: color(self.hover, base.hover),
            on_hover: color(self.on_hover, base.on_hover),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Ours: the template we ask Noctalia to render, and the file it renders to. Generated state rather
/// than anything a person edits, so it belongs under the state directory.
fn state_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(path) => Some(PathBuf::from(path).join("noctmalia")),
        None => Some(home()?.join(".local/state/noctmalia")),
    }
}

/// Noctalia's, where the registration goes.
fn noctalia_config_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) => Some(PathBuf::from(path).join("noctalia")),
        None => Some(home()?.join(".config/noctalia")),
    }
}

/// Where Noctalia writes the palette it resolved.
pub fn rendered_path() -> Option<PathBuf> {
    Some(state_dir()?.join("palette.json"))
}

/// The palette Noctalia is running, or `None` if it cannot be read — [`install`] has not run, the
/// shell is not installed, a file we do not understand. The caller keeps noctalia-iced's default.
pub fn load() -> Option<Palette> {
    let rendered = std::fs::read_to_string(rendered_path()?).ok()?;
    let roles: Roles = serde_json::from_str(&rendered).ok()?;
    Some(roles.into_palette())
}

/// What [`install`] did, for the caller to report.
pub struct Installed {
    pub template: PathBuf,
    pub registration: PathBuf,
    pub rendered: PathBuf,
    /// Whether Noctalia rendered it there and then. When it did not — the shell is not running, or
    /// not installed — the next theme change still will.
    pub applied: bool,
}

/// Registers the template with Noctalia, so it starts rendering the palette where [`load`] reads.
///
/// Both files are ours: the template under our own state directory, and a `noctmalia.toml` beside
/// Noctalia's settings rather than inside them. Removing the two undoes this completely.
pub fn install() -> Result<Installed, String> {
    let state = state_dir().ok_or("no home directory to install into")?;
    let config = noctalia_config_dir().ok_or("no home directory to install into")?;

    let template = state.join("palette.tpl");
    let rendered = state.join("palette.json");
    let registration = config.join(REGISTRATION);

    let write = |path: &PathBuf, contents: &str| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))
    };

    write(&template, TEMPLATE)?;

    // Absolute paths, resolved now: Noctalia expands `$XDG_*` in template paths, but those are only
    // set for some sessions, and a path that silently fails to expand is worse than a long one.
    let entry = format!(
        "# Written by `noctmalia --install-palette-template`. Delete this file and\n\
         # {} to undo it.\n\
         #\n\
         # Noctalia renders this on every theme change, which is how noctmalia follows the palette\n\
         # you are running — including built-in palettes and \"pure black\", neither of which can be\n\
         # read back out of settings.toml.\n\
         \n\
         [theme.templates.user.noctmalia]\n\
         input_path = \"{}\"\n\
         output_path = \"{}\"\n",
        template.display(),
        template.display(),
        rendered.display(),
    );
    write(&registration, &entry)?;

    // Render it now rather than leaving a first run on the fallback palette until something else
    // changes the theme. The reload comes first: a running shell read its config at startup and
    // does not know this file exists yet, so asking it to apply templates would only re-render the
    // ones it already had. A shell that is not running has nothing to tell; that is not a failure.
    let tell = |command: &str| {
        let _ = std::process::Command::new("noctalia")
            .args(["msg", command])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    tell("config-reload");
    tell("templates-apply");

    // Whether it worked is whether the file appears, not whether the command exited zero: it
    // reports on applying every configured template, not on ours in particular. The shell
    // acknowledges the message and renders afterwards, so this waits rather than asking once — a
    // second of patience here is the difference between telling the truth and telling someone to
    // go and change their theme for no reason.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !rendered.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let applied = rendered.exists();

    Ok(Installed { template, registration, rendered, applied })
}

/// Palette changes, as they happen.
///
/// Polling beats an inotify watch here: the file is small, a second of latency is invisible, and
/// polling is immune to the write-to-temp-and-rename that anything writing files this way does. The
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
    fn maps_the_role_names_the_template_writes() {
        let rendered = r##"{
          "_comment": "ignored",
          "primary": "#A7C080",
          "surface": "#232A2E",
          "on_surface": "#859289"
        }"##;
        let parsed: Roles = serde_json::from_str(rendered).expect("parse");
        let palette = parsed.into_palette();
        assert_eq!(palette.primary, parse_hex("#A7C080").unwrap());
        assert_eq!(palette.surface, parse_hex("#232A2E").unwrap());
        // A role the render leaves out keeps the built-in value rather than going black.
        assert_eq!(palette.outline, theme::DEFAULT_PALETTE.outline);
    }

    /// The point of the whole exercise: a "pure black" render is followed exactly, with no ramp
    /// arithmetic of our own to drift from Noctalia's.
    #[test]
    fn a_pure_black_render_is_taken_at_its_word() {
        let rendered = r##"{"surface": "#000000", "surface_variant": "#333A3E"}"##;
        let palette: Roles = serde_json::from_str(rendered).expect("parse");
        let palette = palette.into_palette();
        assert_eq!(palette.surface, parse_hex("#000000").unwrap());
        assert_eq!(palette.surface_variant, parse_hex("#333A3E").unwrap());
    }

    #[test]
    fn the_shipped_template_names_every_role() {
        // The template is what fills `Roles`, so a role missing from it is a role that silently
        // keeps the built-in colour on every desktop.
        for role in [
            "primary",
            "on_primary",
            "secondary",
            "on_secondary",
            "tertiary",
            "on_tertiary",
            "error",
            "on_error",
            "surface",
            "on_surface",
            "surface_variant",
            "on_surface_variant",
            "outline",
            "shadow",
            "hover",
            "on_hover",
        ] {
            assert!(TEMPLATE.contains(&format!("\"{role}\":")), "template does not write {role}");
        }
    }

    /// Whatever the template writes has to parse as the roles we read back.
    #[test]
    fn the_shipped_template_renders_into_roles() {
        let mut rendered = String::new();
        let mut rest = TEMPLATE;
        while let Some(start) = rest.find("{{") {
            rendered.push_str(&rest[..start]);
            rendered.push_str("#123456");
            let end = rest[start..].find("}}").expect("closed placeholder") + start + 2;
            rest = &rest[end..];
        }
        rendered.push_str(rest);

        let roles: Roles = serde_json::from_str(&rendered).expect("the template renders valid JSON");
        assert_eq!(roles.into_palette().primary, parse_hex("#123456").unwrap());
    }

    #[test]
    fn nothing_rendered_yet_is_not_an_error() {
        let roles: Result<Roles, _> = serde_json::from_str("not json");
        assert!(roles.is_err());
    }
}
