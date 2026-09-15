//! The window: one process, one bridge, and whichever surface is in front.
//!
//! This file is the part that is not mail and not people. It owns the window chrome, the
//! palette, the bridge connection, the surface switcher, and the keyboard — and then hands each
//! message to whichever surface it belongs to. Everything with an opinion about what is on screen
//! lives in [`crate::surfaces`].
//!
//! # The switcher is in the titlebar
//!
//! Not a rail down the left. Thunderbird backs three things, and three things never earns a
//! permanent column of screen — the rail each surface *does* want is the one showing its own
//! folders or address books. The titlebar already said which surface you were in, so it became the
//! switcher: the title names where you are, and the surfaces you are not in sit beside it as the
//! same 28px capsule the window controls are. It stays on the left, because a surface switch does
//! not want to live one pixel from Close.
//!
//! That is also the answer to modality. `g m` and `g c` switch from the keyboard, which is the
//! mutt-ness and stays — but a mode you cannot see is a mode that bites you, and a half-typed `g`
//! shows up in the titlebar next to the thing it is about to change.

use crate::commands::Command;
use crate::palette;
use crate::shell::Shell;
use crate::surfaces::{Pressed, Surface, calendar, mail, people};
use crate::ui;
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::operation;
use iced::widget::{column, container, row, space, text};
use iced::{Alignment, Element, Length, Padding, Size, Subscription, Task, Theme};
use noctalia_iced::chrome;
use noctalia_iced::keymap::{self, Keymap};
use noctalia_iced::picker::{self, Picker};
use noctalia_iced::theme::{self, Palette};
use noctmalia_bridge::{Bridge, Event};
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// The window the application asks for, and the size the layout assumes until the compositor has
/// said otherwise — one configure later it is measuring the real thing.
pub const WINDOW: Size = Size::new(1180.0, 760.0);
pub const WINDOW_MIN: Size = Size::new(760.0, 480.0);

#[derive(Debug, Clone)]
pub enum Message {
    Chrome(chrome::Action),
    Bridge(Event),
    /// A key press, with whether a focused widget had already taken it. A text input owns its own
    /// keys; taking them back would stop the cursor moving through what is being typed.
    Key(Key, Modifiers, bool),
    /// Move focus through a form's fields: true forwards, false back.
    Traverse(bool),
    Show(Surface),
    Dismiss,
    /// The window was resized. Only the width matters: it decides how the panes give way.
    Resized(f32),
    /// Only when `NOCTMALIA_FPS` is set: an animation frame, used to measure render cost.
    Frame,
    /// The Noctalia shell's palette changed.
    Palette(Palette),
    /// The command palette or quick-open's query field changed.
    PaletteQuery(String),
    /// The backdrop was clicked, or Escape was pressed while a picker was open.
    PaletteDismiss,
    People(people::Message),
    Mail(mail::Message),
    Calendar(calendar::Message),
}

/// `ctrl+k` (the command palette, verbs) and `ctrl+p` (quick-open, nouns) —
/// `docs/command-palette-plan.md` Stream 4. Neither belongs to any one surface's own [`Keymap`],
/// since both work the same way regardless of which surface is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Global {
    Palette,
    QuickOpen,
}

thread_local! {
    static GLOBAL_KEYS: Keymap<Global> =
        Keymap::new().bind("<C-k>", Global::Palette).bind("<C-p>", Global::QuickOpen);
}

/// A command palette or quick-open in progress: the fuzzy picker itself, and the full,
/// unscoped candidate list a surface-prefixed query re-filters from — `docs/command-palette-plan.md`
/// Stream 4.1.
struct Overlay {
    picker: Picker<Command<Message>>,
    all: Vec<Command<Message>>,
}

pub struct App {
    chrome: chrome::Chrome,
    shell: Shell,
    surface: Surface,
    people: people::People,
    mail: mail::Mail,
    calendar: calendar::Calendar,
    overlay: Option<Overlay>,
    global_pending: keymap::Pending,
    frames: Option<Frames>,
    /// Only the accent has to be held: the other roles are read straight out of the theme, but iced
    /// keeps `primary` in its own palette, so the `Theme` has to be rebuilt when it changes.
    accent: iced::Color,
}

/// Counts redraws, so "it feels slow" can be given a number. `view` runs once per redraw, so this
/// measures the real on-demand path: move the mouse over the list and watch the rate.
///
/// - `NOCTMALIA_FPS=1` reports the rate and how long `view` itself takes.
/// - `NOCTMALIA_FPS=drive` also subscribes to animation frames, redrawing continuously whether or
///   not anything happened. That is the ceiling the machine can do; the gap between the two is
///   redraw scheduling rather than render cost.
struct Frames {
    since: Cell<Instant>,
    count: Cell<u32>,
    building: Cell<Duration>,
    drive: bool,
}

impl Frames {
    fn enabled() -> Option<Frames> {
        let setting = std::env::var("NOCTMALIA_FPS").ok()?;
        Some(Frames {
            since: Cell::new(Instant::now()),
            count: Cell::new(0),
            building: Cell::new(Duration::ZERO),
            drive: setting == "drive",
        })
    }

    /// Called from `view`, which iced runs once per redraw.
    fn drew(&self, building: Duration) {
        self.count.set(self.count.get() + 1);
        self.building.set(self.building.get() + building);
        let elapsed = self.since.get().elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            let count = self.count.get();
            let rate = count as f32 / elapsed.as_secs_f32();
            let view = self.building.get().as_secs_f32() * 1000.0 / count.max(1) as f32;
            eprintln!("noctmalia: {rate:.1} redraws/s, view() {view:.2} ms");
            self.count.set(0);
            self.building.set(Duration::ZERO);
            self.since.set(Instant::now());
        }
    }
}

impl App {
    pub fn new(bridge: Bridge) -> App {
        App {
            chrome: chrome::initial(),
            shell: Shell::new(bridge, WINDOW.width),
            surface: Surface::Mail,
            people: people::People::new(),
            mail: mail::Mail::new(),
            calendar: calendar::Calendar::new(),
            overlay: None,
            global_pending: keymap::Pending::default(),
            frames: Frames::enabled(),
            accent: theme::palette().primary,
        }
    }

    pub fn theme(&self) -> Theme {
        theme::noctalia(self.accent)
    }

    pub fn title(&self) -> String {
        self.surface.title().to_string()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            chrome::events().map(Message::Chrome),
            keys(),
            iced::window::resize_events().map(|(_, size)| Message::Resized(size.width)),
            Subscription::run_with(Feed(self.shell.bridge()), feed),
            Subscription::run(palette_changes),
        ];
        // iced re-reads the subscriptions after every message, so starting an animation in
        // `update` turns this on for exactly as long as it runs.
        let now = Instant::now();
        let animating = self.shell.animating(now)
            || self.people.animating(now)
            || self.mail.animating(now)
            || self.calendar.animating(now);
        if animating || self.frames.as_ref().is_some_and(|frames| frames.drive) {
            subscriptions.push(iced::window::frames().map(|_| Message::Frame));
        }
        Subscription::batch(subscriptions)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // One reading for the whole message: two animations started from the same event should be
        // in step, not a few microseconds apart.
        let now = Instant::now();
        match message {
            Message::Chrome(chrome::Action::Changed(chrome)) => self.chrome = chrome,
            Message::Chrome(action) => return chrome::perform(action).map(Message::Chrome),

            Message::Bridge(Event::Hello(_)) => {
                // Hello also fires when Thunderbird restarts under a live socket, so this is a
                // full resync, not just first contact. Saying so on stderr gives whatever started
                // this a way to tell the difference between "still coming up" and "stuck waiting".
                if !self.shell.connected() {
                    eprintln!("noctmalia: Thunderbird attached");
                }
                self.shell.set_connected(true);
                self.shell.hush(now);
                return self.resync();
            }
            Message::Bridge(Event::Lost) => {
                if self.shell.connected() {
                    eprintln!("noctmalia: Thunderbird went away");
                }
                self.shell.set_connected(false);
            }
            Message::Bridge(Event::Lagged(_)) => return self.resync(),
            Message::Bridge(Event::Notify { name, data }) => {
                return Task::batch([
                    self.people.notify(&name, &self.shell).map(Message::People),
                    self.mail.notify(&name, &data, &self.shell).map(Message::Mail),
                    self.calendar.notify(&name, &data, &self.shell).map(Message::Calendar),
                ]);
            }

            Message::Key(key, modifiers, captured) => return self.press(key, modifiers, captured, now),
            Message::Traverse(forwards) => {
                return if forwards { operation::focus_next() } else { operation::focus_previous() };
            }
            Message::Show(surface) => return self.show(surface, now),
            Message::Dismiss => self.shell.hush(now),
            Message::Resized(width) => self.shell.set_width(width),
            // A frame the animations asked for, or one NOCTMALIA_FPS=drive asked for. Either way
            // the work is in `view`: arriving here at all is what schedules the redraw.
            Message::Frame => {}
            Message::Palette(palette) => {
                theme::set_palette(palette);
                self.accent = palette.primary;
            }
            Message::PaletteQuery(text) => self.rescope_overlay(text),
            Message::PaletteDismiss => self.overlay = None,

            Message::People(message) => {
                return self.people.update(message, &mut self.shell, now).map(Message::People);
            }
            Message::Mail(message) => {
                return self.mail.update(message, &mut self.shell, now).map(Message::Mail);
            }
            Message::Calendar(message) => {
                return self.calendar.update(message, &mut self.shell, now).map(Message::Calendar);
            }
        }
        Task::none()
    }

    /// Everything again, from nothing. Both surfaces, because either may be one keystroke away.
    fn resync(&mut self) -> Task<Message> {
        Task::batch([
            self.mail.resync(&self.shell).map(Message::Mail),
            self.people.resync(&self.shell).map(Message::People),
            self.calendar.resync(&self.shell).map(Message::Calendar),
        ])
    }

    fn show(&mut self, surface: Surface, now: Instant) -> Task<Message> {
        if self.surface == surface {
            return Task::none();
        }
        self.surface = surface;
        self.shell.hush(now);
        // A surface arriving replays its own entrance, so switching looks like arriving somewhere
        // rather than like a redraw.
        match surface {
            Surface::Mail => self.mail.entered(now),
            Surface::People => self.people.entered(now),
            Surface::Calendar => self.calendar.entered(now),
        }
        Task::none()
    }

    /// Every keybound action worth finding by name, across all three surfaces — the command
    /// palette's full candidate list before a surface prefix narrows it.
    fn all_commands(&self) -> Vec<Command<Message>> {
        let mut all = Vec::new();
        all.extend(self.mail.commands().into_iter().map(|entry| Command::from_entry(Surface::Mail, entry.map(Message::Mail))));
        all.extend(
            self.people.commands().into_iter().map(|entry| Command::from_entry(Surface::People, entry.map(Message::People))),
        );
        all.extend(self.calendar.commands().into_iter().map(|entry| {
            Command::from_entry(Surface::Calendar, entry.map(Message::Calendar))
        }));
        all
    }

    /// Every currently loaded row, across all three surfaces — quick-open's full candidate list.
    fn all_quick_items(&self) -> Vec<Command<Message>> {
        let mut all = Vec::new();
        all.extend(
            self.mail.quick_items().into_iter().map(|entry| Command::from_entry(Surface::Mail, entry.map(Message::Mail))),
        );
        all.extend(
            self.people.quick_items().into_iter().map(|entry| Command::from_entry(Surface::People, entry.map(Message::People))),
        );
        all.extend(self.calendar.quick_items().into_iter().map(|entry| {
            Command::from_entry(Surface::Calendar, entry.map(Message::Calendar))
        }));
        all
    }

    /// Opens a picker seeded from `all`, scoped to the current surface — the un-prefixed state
    /// want.md asked for: what the surface in front of you can do, with nothing typed yet.
    fn open_overlay(&mut self, all: Vec<Command<Message>>) -> Task<Message> {
        let scoped: Vec<Command<Message>> = all.iter().filter(|command| command.surface == self.surface).cloned().collect();
        self.overlay = Some(Overlay { picker: Picker::new(scoped), all });
        operation::focus(picker::query_id())
    }

    /// A query changed. A leading `m `/`p `/`k ` re-seeds the whole candidate list to that
    /// surface instead of filtering the current one — `docs/command-palette-plan.md` §4.1 —
    /// implemented as a prefix strip before the fuzzy match ever runs, not a mode switch.
    fn rescope_overlay(&mut self, text: String) {
        let Some(overlay) = &mut self.overlay else { return };
        let scoped_to = Surface::ALL.into_iter().find(|surface| text.starts_with(surface.prefix()) && text[1..].starts_with(' '));
        let (surface, rest) = match scoped_to {
            Some(surface) => (surface, text[2..].to_string()),
            None => (self.surface, text),
        };
        let items: Vec<Command<Message>> = overlay.all.iter().filter(|command| command.surface == surface).cloned().collect();
        overlay.picker.set_items(items);
        overlay.picker.set_query(rest);
    }

    /// A picker's own keys: everything is swallowed while one is open, since a mode you're inside
    /// of doesn't leak keys to whatever it's covering.
    fn press_overlay(&mut self, key: &Key) -> Task<Message> {
        let Some(overlay) = &mut self.overlay else { return Task::none() };
        let visible = overlay.picker.matches(|command| command.label.as_str()).len();
        match key.as_ref() {
            Key::Named(Named::ArrowDown) => overlay.picker.move_selection(1, visible),
            Key::Named(Named::ArrowUp) => overlay.picker.move_selection(-1, visible),
            Key::Named(Named::Enter) => {
                let chosen = overlay.picker.selected(|command| command.label.as_str()).map(|command| command.message.clone());
                self.overlay = None;
                if let Some(message) = chosen {
                    return Task::done(message);
                }
            }
            Key::Named(Named::Escape) => self.overlay = None,
            _ => {}
        }
        Task::none()
    }

    /// One key press.
    ///
    /// Tab is the one key taken whether or not a widget wanted it: iced has no focus traversal of
    /// its own and a focused text input captures Tab, which is precisely the moment a form needs to
    /// move on to the next field. A picker's own keys (arrows, Enter, Escape) are the same story —
    /// checked before `captured` rather than after, so they work regardless of what the query field
    /// itself does with them. Everything else goes to the surface in front, and only when no widget
    /// wanted it first.
    fn press(&mut self, key: Key, modifiers: Modifiers, captured: bool, now: Instant) -> Task<Message> {
        if matches!(key.as_ref(), Key::Named(Named::Tab)) {
            return Task::done(Message::Traverse(!modifiers.shift()));
        }
        if self.overlay.is_some() {
            return self.press_overlay(&key);
        }
        match GLOBAL_KEYS.with(|keys| keys.press(&mut self.global_pending, &key, modifiers)) {
            keymap::Resolved::Action(Global::Palette, _) => return self.open_overlay(self.all_commands()),
            keymap::Resolved::Action(Global::QuickOpen, _) => return self.open_overlay(self.all_quick_items()),
            keymap::Resolved::Pending => return Task::none(),
            keymap::Resolved::Ignored => {}
        }
        if captured {
            return Task::none();
        }
        let pressed = match self.surface {
            Surface::Mail => wrap(self.mail.press(&key, modifiers), Message::Mail),
            Surface::People => wrap(self.people.press(&key, modifiers), Message::People),
            Surface::Calendar => wrap(self.calendar.press(&key, modifiers), Message::Calendar),
        };
        match pressed {
            Pressed::Act(message) => Task::done(message),
            Pressed::Switch(surface) => self.show(surface, now),
            Pressed::Ignored | Pressed::Pending => Task::none(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let started = self.frames.as_ref().map(|_| Instant::now());
        let element = self.build();
        if let (Some(frames), Some(started)) = (&self.frames, started) {
            frames.drew(started.elapsed());
        }
        element
    }

    fn build(&self) -> Element<'_, Message> {
        // One reading for the frame, so everything on screen is drawn at the same moment.
        let now = Instant::now();
        let body: Element<Message> = if self.shell.connected() {
            let surface = match self.surface {
                Surface::Mail => self.mail.view(&self.shell, now).map(Message::Mail),
                Surface::People => self.people.view(&self.shell, now).map(Message::People),
                Surface::Calendar => self.calendar.view(&self.shell, now).map(Message::Calendar),
            };
            column![container(surface).height(Length::Fill), self.shell.notice(now, Message::Dismiss)].into()
        } else {
            ui::waiting(self.shell.socket())
        };
        let body = match &self.overlay {
            Some(overlay) => picker::view(
                &overlay.picker,
                body,
                |command: &Command<Message>| command.label.as_str(),
                |command, selected| overlay_row(command, selected),
                Message::PaletteQuery,
                Message::PaletteDismiss,
            ),
            None => body,
        };
        chrome::frame_with(self.chrome, self.surface.title(), self.switcher(), body, Message::Chrome)
    }

    /// The surfaces you are not in, and whatever is half-typed.
    fn switcher(&self) -> Element<'_, Message> {
        let mut bar = row![].spacing(theme::SPACE_XS).align_y(Alignment::Center);
        for surface in Surface::ALL.into_iter().filter(|surface| *surface != self.surface) {
            let capsule = chrome::capsule(surface.glyph(), Message::Show(surface), false);
            bar = bar.push(ui::tip(capsule, surface.hint()));
        }
        // A half-typed sequence appears where the thing it is about to change already is. It is the
        // whole of the modal feedback, and it is one line of text.
        let typed = match self.surface {
            Surface::Mail => self.mail.typed(),
            Surface::People => self.people.typed(),
            Surface::Calendar => self.calendar.typed(),
        };
        if !typed.is_empty() {
            bar = bar.push(
                container(text(typed).size(theme::FONT_MINI).font(theme::semibold()).color(theme::palette().primary))
                    .padding(iced::Padding::from([1.0, theme::SPACE_XS]))
                    .style(theme::track),
            );
        }
        bar.into()
    }
}

/// One row of the command palette or quick-open: the label, the surface it belongs to (as a
/// glyph, since the prefix letters are typed rather than shown), the keybinding if it has one, and
/// a highlight when it is the current selection.
fn overlay_row<'a>(command: &Command<Message>, selected: bool) -> Element<'a, Message> {
    let hint: Element<Message> = match command.hint {
        Some(hint) => text(hint).size(theme::FONT_MINI).color(theme::palette().on_surface_variant).into(),
        None => space().into(),
    };
    let glyph = noctalia_iced::widgets::icon(command.surface.glyph(), theme::FONT_CAPTION).color(theme::palette().on_surface_variant);
    container(
        row![glyph, text(command.label.clone()).size(theme::FONT_BODY), space().width(Length::Fill), hint]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD]))
    .style(move |_: &Theme| {
        if selected {
            container::Style {
                background: Some(theme::palette().hover.into()),
                text_color: Some(theme::palette().on_hover),
                ..container::Style::default()
            }
        } else {
            container::Style::default()
        }
    })
    .into()
}

/// Lifts a surface's own message into the application's.
fn wrap<M, N>(pressed: Pressed<M>, into: impl Fn(M) -> N) -> Pressed<N> {
    match pressed {
        Pressed::Ignored => Pressed::Ignored,
        Pressed::Pending => Pressed::Pending,
        Pressed::Act(message) => Pressed::Act(into(message)),
        Pressed::Switch(surface) => Pressed::Switch(surface),
    }
}

/// Every key press, with whether a widget had already taken it.
///
/// The decision about what to do with a captured key is the application's, not this filter's —
/// Tab has to arrive either way — so the status travels with the key instead of being spent here.
fn keys() -> Subscription<Message> {
    iced::event::listen_with(|event, status, _| {
        let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };
        Some(Message::Key(key, modifiers, status != iced::event::Status::Ignored))
    })
}

/// Identifies the subscription by socket path; the bridge itself is not hashable.
struct Feed(Bridge);

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.path().hash(state);
    }
}

fn feed(feed: &Feed) -> impl iced::futures::Stream<Item = Message> + use<> {
    let events = feed.0.subscribe();
    iced::futures::stream::unfold(events, |mut events| async move {
        events.next().await.map(|event| (Message::Bridge(event), events))
    })
}

/// Palette changes from the Noctalia shell. One watcher per process: the subscription has no input
/// to key on, so iced keeps a single instance of it alive for the life of the application.
fn palette_changes() -> impl iced::futures::Stream<Item = Message> {
    let (_, changes) = palette::watch();
    iced::futures::stream::unfold(changes, |mut changes| async move {
        changes.next().await.map(|palette| (Message::Palette(palette), changes))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bridge bound to a socket nothing will ever connect to — the app only needs one to clone
    /// into a [`Shell`], and none of these tests touch Thunderbird.
    fn bridge() -> Bridge {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let which = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("noctmalia-app-test-{}-{which}.sock", std::process::id()));
        Bridge::spawn(path).expect("a socket in the temp directory")
    }

    fn press(app: &mut App, key: Key, modifiers: Modifiers) -> Task<Message> {
        app.press(key, modifiers, false, Instant::now())
    }

    #[test]
    fn ctrl_k_opens_the_palette_scoped_to_the_current_surface() {
        let mut app = App::new(bridge());
        assert_eq!(app.surface, Surface::Mail);
        let _ = press(&mut app, Key::Character("k".into()), Modifiers::CTRL);
        let overlay = app.overlay.as_ref().expect("ctrl+k opens the palette");
        for &index in &overlay.picker.matches(|command| command.label.as_str()) {
            assert_eq!(overlay.picker.item(index).surface, Surface::Mail, "unprefixed is the surface in front");
        }
    }

    #[test]
    fn ctrl_p_opens_quick_open_over_currently_loaded_rows_not_commands() {
        let mut app = App::new(bridge());
        let _ = press(&mut app, Key::Character("p".into()), Modifiers::CTRL);
        let overlay = app.overlay.as_ref().expect("ctrl+p opens quick-open");
        // Nothing has loaded yet in a freshly-built app, so there is nothing to jump to — the
        // point being proven is that quick-open asked mail for *rows*, not for its command list,
        // which is never empty.
        assert!(overlay.all.is_empty());
    }

    #[test]
    fn escape_closes_the_palette_without_running_anything() {
        let mut app = App::new(bridge());
        let all = app.all_commands();
        assert!(!all.is_empty(), "mail has commands to show");
        let _ = app.open_overlay(all);
        let _ = press(&mut app, Key::Named(Named::Escape), Modifiers::empty());
        assert!(app.overlay.is_none());
    }

    #[test]
    fn a_surface_prefix_rescopes_the_candidate_list_and_eats_itself_from_the_query() {
        let mut app = App::new(bridge());
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        app.rescope_overlay("p archive".to_string());
        let overlay = app.overlay.as_ref().unwrap();
        assert_eq!(overlay.picker.query(), "archive", "the prefix and its space are consumed");
        for &index in &overlay.picker.matches(|command| command.label.as_str()) {
            assert_eq!(overlay.picker.item(index).surface, Surface::People);
        }
    }

    #[test]
    fn a_bare_query_with_no_prefix_stays_scoped_to_the_current_surface() {
        let mut app = App::new(bridge());
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        app.rescope_overlay("archive".to_string());
        let overlay = app.overlay.as_ref().unwrap();
        assert_eq!(overlay.picker.query(), "archive");
        for &index in &overlay.picker.matches(|command| command.label.as_str()) {
            assert_eq!(overlay.picker.item(index).surface, Surface::Mail);
        }
    }

    #[test]
    fn arrow_keys_move_the_selection_and_enter_closes_the_palette() {
        let mut app = App::new(bridge());
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        let before = app.overlay.as_ref().unwrap().picker.selected(|command| command.label.as_str()).cloned();
        let _ = press(&mut app, Key::Named(Named::ArrowDown), Modifiers::empty());
        let after = app.overlay.as_ref().unwrap().picker.selected(|command| command.label.as_str()).cloned();
        assert_ne!(before.map(|c| c.label), after.map(|c| c.label), "moving down changes the selection");

        let _ = press(&mut app, Key::Named(Named::Enter), Modifiers::empty());
        assert!(app.overlay.is_none(), "choosing a command closes the palette");
    }

    /// Not a unit test of the pieces — those are above, and in `noctalia_iced::picker`'s own
    /// suite — this builds the real widget tree with the palette open and lays it out, the same
    /// reason `tests/mail.rs`'s `render` exists: a shadow, a stack of two opaque layers, and a
    /// scrollable built from a scored list are all real layout code with room to panic in.
    #[test]
    fn the_palette_lays_out_over_the_window_without_panicking() {
        let mut app = App::new(bridge());
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        let element = app.view().map(|_| ());
        let mut simulator = iced_test::simulator(element);
        let _ = simulator.snapshot(&Theme::Dark).expect("the palette lays out and draws");
    }

    /// Not an assertion — see `tests/shots.rs`'s own disclaimer. Writes a PNG so a person (or a
    /// screenshot-reading agent) can look at the palette rather than trust that "lays out without
    /// panicking" means it looks right.
    #[test]
    #[ignore = "writes a PNG rather than asserting"]
    fn shot_of_the_palette_open() {
        use iced::{Settings, Size};
        let mut app = App::new(bridge());
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        let element = app.view().map(|_| ());
        let settings = Settings {
            default_font: crate::font::ui(),
            fonts: vec![noctalia_iced::theme::ICON_FONT_BYTES.into()],
            ..Settings::default()
        };
        let mut simulator = iced_test::Simulator::with_size(settings, Size::new(1180.0, 720.0), element);
        let snapshot = simulator.snapshot(&Theme::Dark).expect("it draws");
        let directory =
            std::env::var("NOCTMALIA_SHOTS").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots").to_string());
        std::fs::create_dir_all(&directory).expect("somewhere to write to");
        let path = std::path::Path::new(&directory).join("palette-open.png");
        let _ = std::fs::remove_file(&path);
        assert!(snapshot.matches_image(&path).expect("write the png"));
        eprintln!("wrote {}", path.display());
    }
}
