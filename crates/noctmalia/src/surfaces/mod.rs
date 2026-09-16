//! The surfaces this window can be showing, and what they have in common.
//!
//! Only one client may hold the bridge socket, so mail, people and calendar are faces of one
//! process rather than three programs. Each surface owns its own state, its own messages and its
//! own keymap; what they share is [`Face`], the trait the window reaches every one of them
//! through, [`crate::shell::Shell`], and the pieces in [`crate::ui`].

pub mod calendar;
mod html_view;
pub mod mail;
pub mod people;

use crate::commands::Entry;
use crate::shell::Shell;
use iced::keyboard::{Key, Modifiers};
use iced::{Element, Task};
use noctalia_iced::keymap::Keymap;
use serde_json::Value;
use std::time::Instant;

/// Which face of the window is in front.
///
/// Thunderbird backs mail, calendar and contacts, and that is the whole list — which is why the
/// switcher is glyphs in the titlebar rather than a column of screen down the left.
///
/// Called `People` rather than `Contacts` throughout — `docs/command-palette-plan.md` Stream 1 —
/// so `c` is free for Calendar's own natural letter instead of Contacts and Calendar fighting
/// over it: `g c` and the palette prefix `c` go to Calendar, and People gets `g p`/`p`.
/// `docs/mail-plan.md`, a historical build record, keeps saying "Contacts"; see its 2026-09-15
/// addendum rather than that document's own prose being rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Mail,
    People,
    Calendar,
}

impl Surface {
    pub const ALL: [Surface; 3] = [Surface::Mail, Surface::People, Surface::Calendar];

    /// What the titlebar says you are looking at.
    pub fn title(self) -> &'static str {
        match self {
            Surface::Mail => "Mail",
            Surface::People => "People",
            Surface::Calendar => "Calendar",
        }
    }

    /// The glyph the other surfaces are offered as.
    pub fn glyph(self) -> char {
        match self {
            Surface::Mail => crate::ui::icon::MAIL,
            Surface::People => crate::ui::icon::ADDRESS_BOOK,
            Surface::Calendar => crate::ui::icon::CALENDAR,
        }
    }

    /// What the switcher says under the pointer, keystroke included — a feature is promoted where
    /// somebody is already looking, and then not again.
    pub fn hint(self) -> &'static str {
        match self {
            Surface::Mail => "Mail  ·  g m",
            Surface::People => "People  ·  g p",
            Surface::Calendar => "Calendar  ·  g c",
        }
    }

    /// The palette prefix letter that scopes a query to this surface's commands —
    /// `docs/command-palette-plan.md` Stream 4.
    pub fn prefix(self) -> char {
        match self {
            Surface::Mail => 'm',
            Surface::People => 'p',
            Surface::Calendar => 'c',
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
    keys.bind("gm", go(Surface::Mail)).bind("gc", go(Surface::Calendar)).bind("gp", go(Surface::People))
}

/// What each surface is to the window: its own state, messages and keymap, reached through this
/// and nothing else. `App` holds every surface behind this trait, so a method one surface grows
/// and the others lack has nowhere to hide. The signatures had drifted apart once before this
/// existed.
pub trait Face {
    type Message: Clone + std::fmt::Debug + Send + 'static;

    /// Which surface this is: the title, the glyph, the palette prefix.
    const SURFACE: Surface;

    fn update(&mut self, message: Self::Message, shell: &mut Shell, now: Instant) -> Task<Self::Message>;

    fn view(&self, shell: &Shell, now: Instant) -> Element<'_, Self::Message>;

    /// One key press, against this surface's own table.
    fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Self::Message>;

    /// A forwarded Thunderbird event, by name and payload.
    fn notify(&mut self, name: &str, data: &Value, shell: &Shell) -> Task<Self::Message>;

    /// Everything again, from nothing. Thunderbird saying hello is also Thunderbird having
    /// restarted under a live socket, so this is a resync rather than a first load.
    fn resync(&mut self, shell: &Shell) -> Task<Self::Message>;

    /// The entrance, replayed: switching to a surface should look like arriving at it.
    fn entered(&mut self, now: Instant);

    /// The window changed width. Only what depends on it needs re-deciding, which for most
    /// surfaces is nothing.
    fn resized(&mut self, shell: &Shell, now: Instant) {
        let _ = (shell, now);
    }

    /// Whether anything is still on its way somewhere. The window subscribes to frames only while
    /// some surface says yes.
    fn animating(&self, now: Instant) -> bool;

    /// What is half-typed, for the titlebar.
    fn typed(&self) -> String;

    /// Whether the keyboard is inside a form: `docs/mode-visual-plan.md`'s "compose" mode.
    fn composing(&self) -> bool;

    /// The keybound actions worth finding by name, for the command palette.
    fn commands(&self) -> Vec<Entry<Self::Message>>;

    /// The currently loaded rows, as jump targets for quick-open.
    fn quick_items(&self) -> Vec<Entry<Self::Message>>;

    /// A right-click's text and where it came from, when `message` is one. The application turns
    /// it into a menu before the surface ever sees it.
    fn context_menu(message: &Self::Message) -> Option<(&str, &'static str)>;
}
