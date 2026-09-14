//! The surfaces this window can be showing, and what they have in common.
//!
//! Only one client may hold the bridge socket, so mail and contacts are two faces of one process
//! rather than two programs. Each surface owns its own state, its own messages and its own keymap;
//! what they share is [`crate::shell::Shell`], the pieces in [`crate::ui`], and this enum.

pub mod contacts;
pub mod mail;

use noctalia_iced::keymap::Keymap;

/// Which face of the window is in front.
///
/// Thunderbird backs mail, calendar and contacts, and that is the whole list — which is why the
/// switcher is two glyphs in the titlebar rather than a column of screen down the left. Calendar
/// joins this enum when there is a surface behind it; the switcher is written to take however many
/// there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Mail,
    Contacts,
}

impl Surface {
    pub const ALL: [Surface; 2] = [Surface::Mail, Surface::Contacts];

    /// What the titlebar says you are looking at.
    pub fn title(self) -> &'static str {
        match self {
            Surface::Mail => "Mail",
            Surface::Contacts => "Contacts",
        }
    }

    /// The glyph the other surfaces are offered as.
    pub fn glyph(self) -> char {
        match self {
            Surface::Mail => crate::ui::icon::MAIL,
            Surface::Contacts => crate::ui::icon::ADDRESS_BOOK,
        }
    }

    /// What the switcher says under the pointer, keystroke included — a feature is promoted where
    /// somebody is already looking, and then not again.
    pub fn hint(self) -> &'static str {
        match self {
            Surface::Mail => "Mail  ·  g m",
            Surface::Contacts => "Contacts  ·  g c",
        }
    }
}

/// What a surface made of a key press.
///
/// Generic over the surface's own message so that a surface never has to know the application's.
#[derive(Debug, Clone)]
pub enum Pressed<M> {
    /// No binding starts this way; the key is nobody's.
    Ignored,
    /// Part-way through a sequence. Swallow it and draw what has been typed.
    Pending,
    Act(M),
    /// Every surface can send you to every other one, which is why this is here rather than in a
    /// keymap of the application's: one pending sequence, one table, no ambiguity about whose `g`
    /// a `g` is.
    Switch(Surface),
}

/// The bindings every surface carries, whatever else it binds. Added to each surface's own table so
/// there is a single table per surface and therefore a single half-typed sequence in the window.
pub fn switches<A: Clone>(keys: Keymap<A>, go: impl Fn(Surface) -> A) -> Keymap<A> {
    keys.bind("gm", go(Surface::Mail)).bind("gc", go(Surface::Contacts))
}
