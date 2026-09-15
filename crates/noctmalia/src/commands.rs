//! The command registry: what the command palette and quick-open both draw from, and —
//! eventually — what `docs/scripting-socket-plan.md`'s control socket fronts.
//!
//! [`Entry`] is what a surface hands back: a label to fuzzy-match and show, an optional
//! keybinding to display beside it, and the message running it sends — generic over that
//! surface's own message type, the same way [`crate::surfaces::Pressed`] is. [`Command`] is what
//! the application turns an `Entry` into once it knows which surface it came from and has mapped
//! the message up to its own: a name the palette can filter by a surface prefix, and a home for
//! `docs/scripting-socket-plan.md`'s exposure flag once that plan lands.

use crate::surfaces::Surface;

/// One thing a surface knows how to do, before the application has tagged it with a surface or
/// mapped its message up to its own.
#[derive(Debug, Clone)]
pub struct Entry<M> {
    pub label: String,
    pub hint: Option<&'static str>,
    pub message: M,
    /// Whether `docs/scripting-socket-plan.md`'s control socket may run this by name. `false` on
    /// every `Entry::new` — a surface opts a specific entry in with [`Entry::exposed`], one at a
    /// time, rather than the registry's exhaustiveness deciding the socket's trust boundary.
    pub exposed_to_socket: bool,
}

impl<M> Entry<M> {
    pub fn new(label: impl Into<String>, hint: Option<&'static str>, message: M) -> Entry<M> {
        Entry { label: label.into(), hint, message, exposed_to_socket: false }
    }

    /// Marks this entry runnable over the control socket. Reach for this only for something
    /// read-only or navigational — `docs/scripting-socket-plan.md`'s own rule is that mutating
    /// actions (send, delete, discard) get opted in individually and deliberately, not by default.
    pub fn exposed(mut self) -> Entry<M> {
        self.exposed_to_socket = true;
        self
    }

    /// Carries the label, hint and exposure over to an [`Entry`] of the message the application
    /// actually sends — `app::Message::Mail`, say — the way [`crate::surfaces::Pressed`] does for
    /// a key press.
    pub fn map<M2>(self, f: impl FnOnce(M) -> M2) -> Entry<M2> {
        Entry { label: self.label, hint: self.hint, message: f(self.message), exposed_to_socket: self.exposed_to_socket }
    }
}

/// One palette entry, tagged with the surface it belongs to and carrying the application's own
/// message type — what a [`crate::surfaces::people::People`] or [`crate::surfaces::mail::Mail`]
/// hands back as an [`Entry`], once `app::App` has mapped it up.
#[derive(Debug, Clone)]
pub struct Command<M> {
    pub surface: Surface,
    pub label: String,
    pub hint: Option<&'static str>,
    pub message: M,
    /// Reserved for `docs/scripting-socket-plan.md`: whether a local process may invoke this over
    /// the control socket. Defaults to `false` on every entry built here — nothing is exposed
    /// until that plan deliberately opts a command in, so this registry's own exhaustiveness never
    /// becomes the socket's trust boundary by accident.
    pub exposed_to_socket: bool,
}

impl<M> Command<M> {
    pub fn from_entry(surface: Surface, entry: Entry<M>) -> Command<M> {
        Command { surface, label: entry.label, hint: entry.hint, message: entry.message, exposed_to_socket: entry.exposed_to_socket }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_an_entry_keeps_the_label_and_hint_and_only_changes_the_message() {
        let entry = Entry::new("Archive", Some("e"), 1);
        let mapped = entry.map(|n| n * 10);
        assert_eq!(mapped.label, "Archive");
        assert_eq!(mapped.hint, Some("e"));
        assert_eq!(mapped.message, 10);
    }

    #[test]
    fn a_command_built_from_an_entry_is_not_exposed_to_the_socket_by_default() {
        let entry = Entry::new("Send", None, ());
        let command = Command::from_entry(Surface::Mail, entry);
        assert!(!command.exposed_to_socket, "docs/scripting-socket-plan.md opts commands in one at a time");
        assert_eq!(command.surface, Surface::Mail);
    }

    #[test]
    fn exposed_carries_through_a_map() {
        let entry = Entry::new("Refresh", None, 1).exposed();
        let command = Command::from_entry(Surface::Mail, entry.map(|n| n * 10));
        assert!(command.exposed_to_socket);
    }
}
