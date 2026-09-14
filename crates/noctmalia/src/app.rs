//! The window: one process, one bridge, and whichever surface is in front.
//!
//! This file is the part that is not mail and not contacts. It owns the window chrome, the
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

use crate::palette;
use crate::shell::Shell;
use crate::surfaces::{Pressed, Surface, contacts, mail};
use crate::ui;
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::operation;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Size, Subscription, Task, Theme};
use noctalia_iced::chrome;
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
    Contacts(contacts::Message),
    Mail(mail::Message),
}

pub struct App {
    chrome: chrome::Chrome,
    shell: Shell,
    surface: Surface,
    contacts: contacts::Contacts,
    mail: mail::Mail,
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
            contacts: contacts::Contacts::new(),
            mail: mail::Mail::new(),
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
        let animating = self.shell.animating(now) || self.contacts.animating(now) || self.mail.animating(now);
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
                    self.contacts.notify(&name, &self.shell).map(Message::Contacts),
                    self.mail.notify(&name, &data, &self.shell).map(Message::Mail),
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

            Message::Contacts(message) => {
                return self.contacts.update(message, &mut self.shell, now).map(Message::Contacts);
            }
            Message::Mail(message) => {
                return self.mail.update(message, &mut self.shell, now).map(Message::Mail);
            }
        }
        Task::none()
    }

    /// Everything again, from nothing. Both surfaces, because either may be one keystroke away.
    fn resync(&mut self) -> Task<Message> {
        Task::batch([
            self.mail.resync(&self.shell).map(Message::Mail),
            self.contacts.resync(&self.shell).map(Message::Contacts),
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
            Surface::Contacts => self.contacts.entered(now),
        }
        Task::none()
    }

    /// One key press.
    ///
    /// Tab is the one key taken whether or not a widget wanted it: iced has no focus traversal of
    /// its own and a focused text input captures Tab, which is precisely the moment a form needs to
    /// move on to the next field. Everything else goes to the surface in front, and only when no
    /// widget wanted it first.
    fn press(&mut self, key: Key, modifiers: Modifiers, captured: bool, now: Instant) -> Task<Message> {
        if matches!(key.as_ref(), Key::Named(Named::Tab)) {
            return Task::done(Message::Traverse(!modifiers.shift()));
        }
        if captured {
            return Task::none();
        }
        let pressed = match self.surface {
            Surface::Mail => wrap(self.mail.press(&key, modifiers), Message::Mail),
            Surface::Contacts => wrap(self.contacts.press(&key, modifiers), Message::Contacts),
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
                Surface::Contacts => self.contacts.view(&self.shell, now).map(Message::Contacts),
            };
            column![container(surface).height(Length::Fill), self.shell.notice(now, Message::Dismiss)].into()
        } else {
            ui::waiting(self.shell.socket())
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
            Surface::Contacts => self.contacts.typed(),
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
