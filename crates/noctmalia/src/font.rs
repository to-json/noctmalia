//! The desktop's UI font.
//!
//! iced renders text through cosmic-text, whose `FontSystem` hardcodes its generic families:
//! `sans-serif` is the literal name `Open Sans`. fontdb does no metric aliasing, so where that font
//! is not installed the fallback is whatever the database offers first — on a Linux box, often a
//! serif. An application that never names a family therefore renders in something no other window
//! on the desktop uses. Ask fontconfig what the system means by sans-serif instead.

use iced::Font;
use std::sync::OnceLock;

/// What fontconfig resolves a generic family to — the same answer every other application gets.
fn resolve(generic: &str) -> Option<String> {
    let output = std::process::Command::new("fc-match").args(["-f", "%{family[0]}", generic]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let family = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!family.is_empty()).then_some(family)
}

static FAMILY: OnceLock<Option<&'static str>> = OnceLock::new();

/// The family fontconfig resolves `sans-serif` to, or `None` where it cannot be asked.
pub fn family() -> Option<&'static str> {
    // Resolved once, and iced wants a name that outlives the application.
    *FAMILY.get_or_init(|| resolve("sans-serif").map(|family| &*Box::leak(family.into_boxed_str())))
}

/// Points iced's text stack at the families fontconfig resolves.
///
/// This is the fix at the source rather than at each call site: with the generics corrected,
/// `Font::DEFAULT` lands on the desktop's font, and so does everything derived from it — which is
/// what makes `Font { weight: Semibold, ..Font::DEFAULT }` stop being a trap. It reaches iced's own
/// widgets and any library that never names a family.
///
/// Call it before the first frame. It forces the font database to load, which is where iced would
/// spend that time anyway. No patched crates involved: `font_system()` is public API.
pub fn adopt_system_families() {
    let Ok(mut system) = iced::advanced::graphics::text::font_system().write() else {
        // Poisoned by a panic elsewhere in text handling; the defaults still draw something.
        return;
    };
    let database = system.raw().db_mut();
    if let Some(family) = resolve("sans-serif") {
        database.set_sans_serif_family(family);
    }
    if let Some(family) = resolve("monospace") {
        database.set_monospace_family(family);
    }
    if let Some(family) = resolve("serif") {
        database.set_serif_family(family);
    }
}

/// The interface font. Pass it to iced's `default_font` and to `theme::set_font`, so the content
/// and the window chrome draw in the same family.
pub fn ui() -> Font {
    family().map(Font::with_name).unwrap_or(Font::DEFAULT)
}
