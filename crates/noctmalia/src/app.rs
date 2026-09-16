//! The window: one process, one bridge, and whichever surface is in front.
//!
//! This file is the part that is not mail, not people and not calendar. It owns the window
//! chrome, the palette, the bridge connection, the surface switcher, and the keyboard — and then
//! hands each message to whichever surface it belongs to, through [`Face`] and nothing else.
//! Everything with an opinion about what is on screen lives in [`crate::surfaces`].
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
use crate::config;
use crate::control;
use crate::palette;
use crate::shell::Shell;
use crate::surfaces::{Face, Pressed, Surface, calendar, mail, people};
use crate::thunderbird::{self, Report, Supervisor};
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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

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
    /// A context-menu template was chosen, with its configured argv and the text it runs against.
    RunTemplate(Vec<String>, String),
    /// A template finished (or failed to even start, or timed out) — its stdout, or an error.
    TemplateRan(Result<String, String>),
    /// A name the control socket accepted as a `run` request — `docs/scripting-socket-plan.md`.
    ControlRun(String),
    /// What the Thunderbird this process runs is doing — `crate::thunderbird`.
    Backend(Report),
    /// SIGTERM or SIGINT: close the window, which is what takes Thunderbird down with it.
    Quit,
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

/// Where the Thunderbird on the other end of the bridge comes from.
#[derive(Clone)]
pub enum Backend {
    /// Something else attaches to the socket: `tools/fake-bridge.py`, a test, a Thunderbird
    /// somebody runs by hand.
    External,
    /// This process's own, fetched, spawned and stopped by [`Supervisor`].
    Managed(Supervisor),
}

/// What the window shows until the surfaces can: `docs/one-program-plan.md` Stream 2.1.
#[derive(Debug, Clone, PartialEq)]
enum Startup {
    Starting,
    Fetching { received: u64, total: u64 },
    Ready,
    Failed { reason: String, log: std::path::PathBuf },
}

pub struct App {
    chrome: chrome::Chrome,
    shell: Shell,
    backend: Backend,
    startup: Startup,
    surface: Surface,
    mail: Lift<mail::Mail>,
    people: Lift<people::People>,
    calendar: Lift<calendar::Calendar>,
    overlay: Option<Overlay>,
    global_pending: keymap::Pending,
    /// `docs/config-plan.md`. Read once at startup — nothing in the app changes it, and a change
    /// on disk takes another launch to be seen, which is fine for a file this small and this rare
    /// to edit.
    config: config::Config,
    /// The other end of `docs/scripting-socket-plan.md`'s control socket: names it has queued to
    /// run, one per accepted `run` request. `Arc<AsyncMutex<..>>` because [`App::subscription`] is
    /// `&self` and rebuilds this stream's identity on every call, but there is only ever one real
    /// receiver — see [`control_feed`].
    control: Arc<AsyncMutex<mpsc::UnboundedReceiver<String>>>,
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
    pub fn new(bridge: Bridge, backend: Backend, dev: bool) -> App {
        let mut shell = Shell::new(bridge, WINDOW.width);
        let loaded = config::load();
        if let Some(error) = loaded.error {
            shell.fail(format!("config: {error}"), Instant::now());
        }
        let mail = Lift::new(mail::Mail::new(), Message::Mail);
        let people = Lift::new(people::People::new(), Message::People);
        let calendar = Lift::new(calendar::Calendar::new(), Message::Calendar);

        // Computed now, before anything has loaded from the bridge, which is exactly what keeps
        // this to state-independent commands: a fresh surface has no folders to build
        // `Message::OpenFolder(id)` from, so nothing that needs one is in the registry at all.
        let registry = socket_registry([&mail as &dyn Lifted, &people, &calendar]);
        let (run_tx, run_rx) = mpsc::unbounded_channel();
        let status = {
            let bridge = shell.bridge();
            let supervisor = match &backend {
                Backend::Managed(supervisor) => Some(supervisor.clone()),
                Backend::External => None,
            };
            Arc::new(move || {
                let mut status = bridge_status(&bridge);
                status["thunderbird"] = supervisor.as_ref().map_or(serde_json::Value::Null, |s| s.status().json());
                status
            }) as control::Status
        };
        let raw = dev.then(|| shell.bridge());
        if let Err(error) = control::spawn(registry, run_tx, status, raw) {
            eprintln!("noctmalia: control socket: {error}");
        }

        App {
            chrome: chrome::initial(),
            shell,
            backend,
            startup: Startup::Starting,
            surface: Surface::Mail,
            people,
            mail,
            calendar,
            overlay: None,
            global_pending: keymap::Pending::default(),
            config: loaded.config,
            control: Arc::new(AsyncMutex::new(run_rx)),
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
            Subscription::run_with(ControlFeed(Arc::clone(&self.control)), control_feed),
            Subscription::run(sigterm),
            Subscription::run(sigint),
        ];
        if let Backend::Managed(supervisor) = &self.backend {
            subscriptions.push(Subscription::run_with(Reports(supervisor.clone()), reports));
        }
        // iced re-reads the subscriptions after every message, so starting an animation in
        // `update` turns this on for exactly as long as it runs.
        let now = Instant::now();
        let animating = self.shell.animating(now) || self.faces().iter().any(|face| face.animating(now));
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
                self.startup = Startup::Ready;
                self.shell.hush(now);
                return self.resync();
            }
            Message::Bridge(Event::Lost) => {
                if self.shell.connected() {
                    eprintln!("noctmalia: Thunderbird went away");
                }
                self.shell.set_connected(false);
                if self.startup == Startup::Ready {
                    self.startup = Startup::Starting;
                }
            }
            Message::Backend(report) => {
                self.startup = match report {
                    Report::Fetching { received, total } => Startup::Fetching { received, total },
                    Report::Starting | Report::Restarting => Startup::Starting,
                    Report::Failed { reason, log } => Startup::Failed { reason, log },
                };
            }
            Message::Quit => return iced::exit(),
            Message::Bridge(Event::Lagged(_)) => return self.resync(),
            Message::Bridge(Event::Notify { name, data }) => {
                let (shell, faces) = self.split();
                return Task::batch(faces.map(|face| face.notify(&name, &data, shell)));
            }

            Message::Key(key, modifiers, captured) => return self.press(key, modifiers, captured, now),
            Message::Traverse(forwards) => {
                return if forwards { operation::focus_next() } else { operation::focus_previous() };
            }
            Message::Show(surface) => return self.show(surface, now),
            Message::Dismiss => self.shell.hush(now),
            Message::Resized(width) => {
                self.shell.set_width(width);
                let (shell, faces) = self.split();
                for face in faces {
                    face.resized(shell, now);
                }
            }
            // A frame the animations asked for, or one NOCTMALIA_FPS=drive asked for. Either way
            // the work is in `view`: arriving here at all is what schedules the redraw.
            Message::Frame => {}
            Message::Palette(palette) => {
                theme::set_palette(palette);
                self.accent = palette.primary;
            }
            Message::PaletteQuery(text) => self.rescope_overlay(text),
            Message::PaletteDismiss => self.overlay = None,
            Message::RunTemplate(argv, selection) => {
                return Task::perform(run_template(argv, selection, TEMPLATE_TIMEOUT), Message::TemplateRan);
            }
            Message::TemplateRan(Ok(output)) => {
                self.shell.announce(if output.is_empty() { "(no output)".to_string() } else { truncated(output) }, now)
            }
            Message::TemplateRan(Err(error)) => self.shell.fail(truncated(error), now),
            Message::ControlRun(name) => {
                if let Some(message) = self.message_for_socket_name(&name) {
                    return Task::done(message);
                }
                // The socket only ever queues a name from its own registry, so this is not a user
                // mistake — either that registry drifted from `all_commands()`, or the surface
                // that used to expose it no longer does. Either way there is nothing to run, and
                // the toast says which name went looking for a home and found none.
                self.shell.fail(format!("control: {name:?} is no longer available"), now);
            }

            // Only the application holds the `Supervisor` that can put a window up at all; the
            // mail surface never sees this one.
            Message::Mail(mail::Message::OpenThunderbirdSettings) => match &self.backend {
                Backend::Managed(supervisor) => supervisor.open_account_wizard(),
                Backend::External => self.shell.fail(
                    "opening Thunderbird settings needs noctmalia's own Thunderbird, not an external one".to_string(),
                    now,
                ),
            },
            Message::Mail(message) => {
                let delivered = self.mail.deliver(message, &mut self.shell, now);
                return self.deliver(delivered);
            }
            Message::People(message) => {
                let delivered = self.people.deliver(message, &mut self.shell, now);
                return self.deliver(delivered);
            }
            Message::Calendar(message) => {
                let delivered = self.calendar.deliver(message, &mut self.shell, now);
                return self.deliver(delivered);
            }
        }
        Task::none()
    }

    /// What a surface's message came to: a task to run, or a menu to draw over it.
    fn deliver(&mut self, delivered: Delivered) -> Task<Message> {
        match delivered {
            Delivered::ContextMenu(text, context) => self.open_context_menu(text, context),
            Delivered::Task(task) => task,
        }
    }

    /// The surface in front.
    fn front(&self) -> &dyn Lifted {
        match self.surface {
            Surface::Mail => &self.mail,
            Surface::People => &self.people,
            Surface::Calendar => &self.calendar,
        }
    }

    fn front_mut(&mut self) -> &mut dyn Lifted {
        match self.surface {
            Surface::Mail => &mut self.mail,
            Surface::People => &mut self.people,
            Surface::Calendar => &mut self.calendar,
        }
    }

    /// Every surface, in switcher order.
    fn faces(&self) -> [&dyn Lifted; 3] {
        [&self.mail, &self.people, &self.calendar]
    }

    /// Every surface and the shell, borrowed apart so one can be handed to the others.
    fn split(&mut self) -> (&Shell, [&mut dyn Lifted; 3]) {
        (&self.shell, [&mut self.mail, &mut self.people, &mut self.calendar])
    }

    /// Everything again, from nothing. Every surface, because any of them may be one keystroke
    /// away.
    fn resync(&mut self) -> Task<Message> {
        let (shell, faces) = self.split();
        Task::batch(faces.map(|face| face.resync(shell)))
    }

    fn show(&mut self, surface: Surface, now: Instant) -> Task<Message> {
        if self.surface == surface {
            return Task::none();
        }
        self.surface = surface;
        self.shell.hush(now);
        // A surface arriving replays its own entrance, so switching looks like arriving somewhere
        // rather than like a redraw.
        self.front_mut().entered(now);
        Task::none()
    }

    /// Every keybound action worth finding by name, across all three surfaces — the command
    /// palette's full candidate list before a surface prefix narrows it.
    fn all_commands(&self) -> Vec<Command<Message>> {
        self.faces().into_iter().flat_map(|face| face.commands()).collect()
    }

    /// Every currently loaded row, across all three surfaces — quick-open's full candidate list.
    fn all_quick_items(&self) -> Vec<Command<Message>> {
        self.faces().into_iter().flat_map(|face| face.quick_items()).collect()
    }

    /// The message a control-socket `run` request for `name` actually sends, found the same way
    /// the palette finds anything: the live, current `all_commands()` — not the socket's own
    /// registry snapshot from startup, so a command that has since become unavailable is a clean
    /// miss rather than a stale message built from data that no longer exists.
    fn message_for_socket_name(&self, name: &str) -> Option<Message> {
        self.all_commands()
            .into_iter()
            .find(|command| command.exposed_to_socket && command.label == name)
            .map(|command| command.message)
    }

    /// Opens a picker seeded from `all`, scoped to the current surface — the un-prefixed state
    /// want.md asked for: what the surface in front of you can do, with nothing typed yet.
    fn open_overlay(&mut self, all: Vec<Command<Message>>) -> Task<Message> {
        let scoped: Vec<Command<Message>> =
            all.iter().filter(|command| command.surface == self.surface).cloned().collect();
        self.overlay = Some(Overlay { picker: Picker::new(scoped), all });
        operation::focus(picker::query_id())
    }

    /// A right-click on `text`, tagged `context` (`"mail-body"`, `"mail-compose"`,
    /// `"person-field"`, `"event-description"`). Populates a menu from `docs/config-plan.md`'s
    /// templates whose own `contexts` matches this one, or names none (which means everywhere) —
    /// `docs/context-commands-plan.md` Stream 2. No matching templates means nothing to show, not
    /// an empty menu floating over nothing.
    fn open_context_menu(&mut self, text: String, context: &'static str) -> Task<Message> {
        let items: Vec<Command<Message>> = self
            .config
            .templates
            .iter()
            .filter(|template| template.contexts.is_empty() || template.contexts.iter().any(|c| c == context))
            .map(|template| Command {
                surface: self.surface,
                label: template.name.clone(),
                hint: None,
                message: Message::RunTemplate(template.command.clone(), text.clone()),
                exposed_to_socket: false,
            })
            .collect();
        if items.is_empty() {
            return Task::none();
        }
        self.overlay = Some(Overlay { picker: Picker::new(items.clone()), all: items });
        operation::focus(picker::query_id())
    }

    /// A query changed. A leading `m `/`p `/`k ` re-seeds the whole candidate list to that
    /// surface instead of filtering the current one — `docs/command-palette-plan.md` §4.1 —
    /// implemented as a prefix strip before the fuzzy match ever runs, not a mode switch.
    fn rescope_overlay(&mut self, text: String) {
        let Some(overlay) = &mut self.overlay else { return };
        let scoped_to =
            Surface::ALL.into_iter().find(|surface| text.starts_with(surface.prefix()) && text[1..].starts_with(' '));
        let (surface, rest) = match scoped_to {
            Some(surface) => (surface, text[2..].to_string()),
            None => (self.surface, text),
        };
        let items: Vec<Command<Message>> =
            overlay.all.iter().filter(|command| command.surface == surface).cloned().collect();
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
                let chosen =
                    overlay.picker.selected(|command| command.label.as_str()).map(|command| command.message.clone());
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
        match self.front_mut().press(&key, modifiers) {
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
            let surface = self.front().view(&self.shell, now);
            column![container(surface).height(Length::Fill), self.shell.notice(now, Message::Dismiss)].into()
        } else {
            self.starting()
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

    /// The page before the surfaces: what is happening, said once, and nothing about sockets.
    fn starting(&self) -> Element<'_, Message> {
        let managed = matches!(self.backend, Backend::Managed(_));
        match &self.startup {
            Startup::Fetching { received, total } => ui::starting(
                ui::icon::DOWNLOAD,
                "Getting Thunderbird",
                format!(
                    "Downloading Thunderbird {}, {} of {} MB",
                    thunderbird::VERSION,
                    received / 1_000_000,
                    total / 1_000_000
                ),
                Some((*received as f32 / (*total).max(1) as f32).clamp(0.0, 1.0)),
            ),
            Startup::Failed { reason, log } => ui::starting(
                ui::icon::ALERT_TRIANGLE,
                "Thunderbird could not start",
                format!("{reason}\nThe log is at {}", log.display()),
                None,
            ),
            Startup::Starting | Startup::Ready if managed => ui::starting(
                ui::icon::PLUG,
                "Starting Thunderbird",
                "A moment; it runs in the background.".to_string(),
                None,
            ),
            Startup::Starting | Startup::Ready => ui::starting(
                ui::icon::PLUG,
                "Waiting for Thunderbird",
                format!("Nothing has attached to {} yet.", self.shell.socket()),
                None,
            ),
        }
    }

    /// The surfaces you are not in, and whatever is half-typed.
    fn switcher(&self) -> Element<'_, Message> {
        let mut bar = row![].spacing(theme::SPACE_XS).align_y(Alignment::Center);
        for surface in Surface::ALL.into_iter().filter(|surface| *surface != self.surface) {
            let capsule = chrome::capsule(surface.glyph(), Message::Show(surface), false);
            bar = bar.push(ui::tip(capsule, surface.hint()));
        }
        if let Some((label, color)) = self.mode().badge() {
            bar = bar.push(
                container(text(label).size(theme::FONT_MINI).font(theme::semibold()).color(color))
                    .padding(iced::Padding::from([1.0, theme::SPACE_XS]))
                    .style(theme::track),
            );
        }
        // A half-typed sequence appears where the thing it is about to change already is. It is the
        // whole of the modal feedback, and it is one line of text.
        let typed = self.front().typed();
        if !typed.is_empty() {
            bar = bar.push(
                container(text(typed).size(theme::FONT_MINI).font(theme::semibold()).color(theme::palette().primary))
                    .padding(iced::Padding::from([1.0, theme::SPACE_XS]))
                    .style(theme::track),
            );
        }
        bar.into()
    }

    /// Which of `docs/mode-visual-plan.md`'s states the window is in right now. Overlay wins over
    /// compose — a picker sitting over an open composer swallows every key exactly as it would
    /// over the index, so what it looks like takes precedence over what's underneath it.
    fn mode(&self) -> Mode {
        if self.overlay.is_some() {
            return Mode::Overlay;
        }
        if self.front().composing() { Mode::Compose } else { Mode::Browse }
    }
}

/// The window's mode, in the sense `docs/mode-visual-plan.md` means it: what a keypress does right
/// now. `Browse` shows no badge at all — it is the resting state, and a badge that is always on
/// screen stops meaning anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    Compose,
    Overlay,
}

impl Mode {
    /// The titlebar badge for this mode, or `None` for the resting state.
    fn badge(self) -> Option<(&'static str, iced::Color)> {
        match self {
            Mode::Browse => None,
            Mode::Compose => Some(("Compose", theme::palette().primary)),
            Mode::Overlay => Some(("Command", theme::palette().tertiary)),
        }
    }
}

/// What `control::spawn` is handed at startup: every command any surface has marked
/// `exposed_to_socket`, by label. Computed from fresh surfaces before anything has loaded from the
/// bridge — see the comment where this is called in [`App::new`] for why that is what keeps it to
/// state-independent commands without naming them twice.
fn socket_registry(faces: [&dyn Lifted; 3]) -> Vec<control::Exposed> {
    faces
        .into_iter()
        .flat_map(|face| face.commands())
        .filter(|command| command.exposed_to_socket)
        .map(|command| control::Exposed { name: command.label.clone(), label: command.label })
        .collect()
}

/// A surface with its messages lifted into the application's, so the one in front can be held as
/// `&dyn Lifted` and asked the same questions whichever it is. Everything here is [`Face`] with
/// the surface's own message type mapped away.
trait Lifted {
    fn view<'a>(&'a self, shell: &'a Shell, now: Instant) -> Element<'a, Message>;
    fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Message>;
    fn notify(&mut self, name: &str, data: &serde_json::Value, shell: &Shell) -> Task<Message>;
    fn resync(&mut self, shell: &Shell) -> Task<Message>;
    fn entered(&mut self, now: Instant);
    fn resized(&mut self, shell: &Shell, now: Instant);
    fn animating(&self, now: Instant) -> bool;
    fn typed(&self) -> String;
    fn composing(&self) -> bool;
    fn commands(&self) -> Vec<Command<Message>>;
    fn quick_items(&self) -> Vec<Command<Message>>;
}

/// A [`Face`] plus the one thing the application knows and the surface does not: which
/// [`Message`] variant carries its messages.
struct Lift<S: Face> {
    inner: S,
    lift: fn(S::Message) -> Message,
}

/// What delivering a surface's own message came to.
enum Delivered {
    /// It was a right-click: the application draws the menu, the surface never sees it.
    ContextMenu(String, &'static str),
    Task(Task<Message>),
}

impl<S: Face> Lift<S> {
    fn new(inner: S, lift: fn(S::Message) -> Message) -> Lift<S> {
        Lift { inner, lift }
    }

    fn deliver(&mut self, message: S::Message, shell: &mut Shell, now: Instant) -> Delivered {
        if let Some((text, context)) = S::context_menu(&message) {
            return Delivered::ContextMenu(text.to_string(), context);
        }
        Delivered::Task(self.inner.update(message, shell, now).map(self.lift))
    }
}

impl<S: Face> Lifted for Lift<S> {
    fn view<'a>(&'a self, shell: &'a Shell, now: Instant) -> Element<'a, Message> {
        self.inner.view(shell, now).map(self.lift)
    }

    fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Message> {
        wrap(self.inner.press(key, modifiers), self.lift)
    }

    fn notify(&mut self, name: &str, data: &serde_json::Value, shell: &Shell) -> Task<Message> {
        self.inner.notify(name, data, shell).map(self.lift)
    }

    fn resync(&mut self, shell: &Shell) -> Task<Message> {
        self.inner.resync(shell).map(self.lift)
    }

    fn entered(&mut self, now: Instant) {
        self.inner.entered(now);
    }

    fn resized(&mut self, shell: &Shell, now: Instant) {
        self.inner.resized(shell, now);
    }

    fn animating(&self, now: Instant) -> bool {
        self.inner.animating(now)
    }

    fn typed(&self) -> String {
        self.inner.typed()
    }

    fn composing(&self) -> bool {
        self.inner.composing()
    }

    fn commands(&self) -> Vec<Command<Message>> {
        self.inner.commands().into_iter().map(|entry| Command::from_entry(S::SURFACE, entry.map(self.lift))).collect()
    }

    fn quick_items(&self) -> Vec<Command<Message>> {
        self.inner
            .quick_items()
            .into_iter()
            .map(|entry| Command::from_entry(S::SURFACE, entry.map(self.lift)))
            .collect()
    }
}

/// What the control socket's `status` says: the bridge's own numbers, so "it is stuck" can be
/// asked from a shell and answered with a connection number, a count of calls in flight, and the
/// slowest thing that has happened.
fn bridge_status(bridge: &Bridge) -> serde_json::Value {
    let stats = bridge.stats();
    let sample = |sample: Option<noctmalia_bridge::Sample>| {
        sample.map(|sample| serde_json::json!({ "method": sample.method, "millis": sample.took.as_millis() as u64 }))
    };
    serde_json::json!({
        "socket": bridge.path().display().to_string(),
        "connection": stats.connection,
        "connections": stats.connections,
        "attached_seconds": stats.attached_for.map(|for_| for_.as_secs()),
        "in_flight": stats.in_flight,
        "calls": stats.calls,
        "failed": stats.failed,
        "timed_out": stats.timed_out,
        "slowest": sample(stats.slowest),
        "last": sample(stats.last),
    })
}

/// A hung script cannot be allowed to freeze the UI waiting on it — `docs/context-commands-plan.md`
/// §2.3.
const TEMPLATE_TIMEOUT: Duration = Duration::from_secs(10);
/// A toast is one line, not a document; long output is cut rather than growing the banner to fit.
const TEMPLATE_OUTPUT_CAP: usize = 400;

fn truncated(mut text: String) -> String {
    if text.len() > TEMPLATE_OUTPUT_CAP {
        text.truncate(TEMPLATE_OUTPUT_CAP);
        text.push('…');
    }
    text
}

/// Runs a context-command template: `argv`, with every `{selection}` replaced by `selection` —
/// substituted directly into the string and passed to `Command::arg()`, never through a shell, so
/// there is no quoting to get wrong and no injection to guard against. `docs/context-commands-plan.md`
/// §2.3. `timeout` is [`TEMPLATE_TIMEOUT`] in the app; a parameter here so a test can prove the
/// timeout path fires without a real suite-slowing wait.
async fn run_template(argv: Vec<String>, selection: String, timeout: Duration) -> Result<String, String> {
    let args: Vec<String> = argv.iter().map(|arg| arg.replace("{selection}", &selection)).collect();
    let Some((program, rest)) = args.split_first() else {
        return Err("empty command template".to_string());
    };
    let mut command = tokio::process::Command::new(program);
    command.args(rest).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|error| format!("{program}: {error}"))?;
    let Ok(waited) = tokio::time::timeout(timeout, child.wait_with_output()).await else {
        return Err(format!("{program}: timed out after {}s", timeout.as_secs()));
    };
    let output = waited.map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
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
    let glyph = noctalia_iced::widgets::icon(command.surface.glyph(), theme::FONT_CAPTION)
        .color(theme::palette().on_surface_variant);
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

/// Wraps the one control-socket receiver so `Subscription::run_with` can key a stable identity off
/// it. `App::subscription` constructs a fresh `ControlFeed` on every call — cloning the `Arc`, not
/// the receiver inside it — since there is exactly one receiver for the socket thread's one
/// sender, unlike [`Feed`]'s broadcast channel, which hands out a new one per subscriber.
struct ControlFeed(Arc<AsyncMutex<mpsc::UnboundedReceiver<String>>>);

impl Hash for ControlFeed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Only one of these ever exists per process, so any constant discriminates it from the
        // application's other subscriptions without needing to hash the receiver itself.
        "control".hash(state);
    }
}

fn control_feed(feed: &ControlFeed) -> impl iced::futures::Stream<Item = Message> + use<> {
    let receiver = Arc::clone(&feed.0);
    iced::futures::stream::unfold(receiver, |receiver| async move {
        let name = receiver.lock().await.recv().await?;
        Some((Message::ControlRun(name), receiver))
    })
}

/// What the supervisor reports, as messages. Keyed on a constant: there is one supervisor.
struct Reports(Supervisor);

impl Hash for Reports {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "thunderbird".hash(state);
    }
}

fn reports(feed: &Reports) -> impl iced::futures::Stream<Item = Message> + use<> {
    let receiver = feed.0.reports();
    iced::futures::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(report) => return Some((Message::Backend(report), receiver)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

/// SIGTERM, which is what a session ending or a `kill` sends: close the window, and the
/// Thunderbird goes with it, cleanly, in `main` after the runtime returns.
fn sigterm() -> impl iced::futures::Stream<Item = Message> {
    signal_stream(tokio::signal::unix::SignalKind::terminate())
}

fn sigint() -> impl iced::futures::Stream<Item = Message> {
    signal_stream(tokio::signal::unix::SignalKind::interrupt())
}

fn signal_stream(kind: tokio::signal::unix::SignalKind) -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::unfold(None, move |signal| async move {
        let mut signal = match signal {
            Some(signal) => signal,
            None => tokio::signal::unix::signal(kind).ok()?,
        };
        signal.recv().await?;
        Some((Message::Quit, Some(signal)))
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

    /// Every string in the laid-out window, titlebar included — `tests/mail.rs`'s `render`, aimed
    /// at the whole `App` rather than one surface, for asserting on the mode badge specifically.
    fn texts(app: &App) -> Vec<String> {
        use iced_selector::Candidate;
        use std::sync::{Arc, Mutex};

        let element = app.view().map(|_| ());
        let mut simulator = iced_test::simulator(element);
        let _ = simulator.snapshot(&Theme::Dark).expect("the window lays out and draws");

        let found: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let collecting = Arc::clone(&found);
        let _: Result<(), _> = simulator.find(move |candidate: Candidate<'_>| -> Option<()> {
            if let Candidate::Text { content, .. } = candidate {
                collecting.lock().expect("nothing else holds this").push(content.to_string());
            }
            None
        });
        let words = found.lock().expect("nothing else holds this");
        words.clone()
    }

    /// Locks in a correction: an earlier pass had this backward, keeping People on `c` (aliased
    /// from Contacts) and leaving Calendar on `k` — exactly what the People rename was supposed to
    /// fix. Calendar gets its own natural letter; People gets `p`.
    #[test]
    fn g_c_goes_to_calendar_and_g_p_goes_to_people() {
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = press(&mut app, Key::Character("g".into()), Modifiers::empty());
        let _ = press(&mut app, Key::Character("c".into()), Modifiers::empty());
        assert_eq!(app.surface, Surface::Calendar);

        let mut app = App::new(bridge(), Backend::External, false);
        let _ = press(&mut app, Key::Character("g".into()), Modifiers::empty());
        let _ = press(&mut app, Key::Character("p".into()), Modifiers::empty());
        assert_eq!(app.surface, Surface::People);
    }

    #[test]
    fn ctrl_k_opens_the_palette_scoped_to_the_current_surface() {
        let mut app = App::new(bridge(), Backend::External, false);
        assert_eq!(app.surface, Surface::Mail);
        let _ = press(&mut app, Key::Character("k".into()), Modifiers::CTRL);
        let overlay = app.overlay.as_ref().expect("ctrl+k opens the palette");
        for &index in &overlay.picker.matches(|command| command.label.as_str()) {
            assert_eq!(overlay.picker.item(index).surface, Surface::Mail, "unprefixed is the surface in front");
        }
    }

    #[test]
    fn ctrl_p_opens_quick_open_over_currently_loaded_rows_not_commands() {
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = press(&mut app, Key::Character("p".into()), Modifiers::CTRL);
        let overlay = app.overlay.as_ref().expect("ctrl+p opens quick-open");
        // Nothing has loaded yet in a freshly-built app, so there is nothing to jump to — the
        // point being proven is that quick-open asked mail for *rows*, not for its command list,
        // which is never empty.
        assert!(overlay.all.is_empty());
    }

    #[test]
    fn escape_closes_the_palette_without_running_anything() {
        let mut app = App::new(bridge(), Backend::External, false);
        let all = app.all_commands();
        assert!(!all.is_empty(), "mail has commands to show");
        let _ = app.open_overlay(all);
        let _ = press(&mut app, Key::Named(Named::Escape), Modifiers::empty());
        assert!(app.overlay.is_none());
    }

    #[test]
    fn a_surface_prefix_rescopes_the_candidate_list_and_eats_itself_from_the_query() {
        let mut app = App::new(bridge(), Backend::External, false);
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
    fn the_c_prefix_scopes_to_calendar_not_people() {
        let mut app = App::new(bridge(), Backend::External, false);
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        app.rescope_overlay("c today".to_string());
        let overlay = app.overlay.as_ref().unwrap();
        assert_eq!(overlay.picker.query(), "today");
        for &index in &overlay.picker.matches(|command| command.label.as_str()) {
            assert_eq!(overlay.picker.item(index).surface, Surface::Calendar);
        }
    }

    #[test]
    fn a_bare_query_with_no_prefix_stays_scoped_to_the_current_surface() {
        let mut app = App::new(bridge(), Backend::External, false);
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
        let mut app = App::new(bridge(), Backend::External, false);
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        let before = app.overlay.as_ref().unwrap().picker.selected(|command| command.label.as_str()).cloned();
        let _ = press(&mut app, Key::Named(Named::ArrowDown), Modifiers::empty());
        let after = app.overlay.as_ref().unwrap().picker.selected(|command| command.label.as_str()).cloned();
        assert_ne!(before.map(|c| c.label), after.map(|c| c.label), "moving down changes the selection");

        let _ = press(&mut app, Key::Named(Named::Enter), Modifiers::empty());
        assert!(app.overlay.is_none(), "choosing a command closes the palette");
    }

    // ── Mode badge: docs/mode-visual-plan.md §2.2 ────────────────────────────────────

    #[test]
    fn browse_shows_no_mode_badge() {
        let app = App::new(bridge(), Backend::External, false);
        assert_eq!(app.mode(), Mode::Browse);
        assert!(!texts(&app).iter().any(|text| text == "Compose" || text == "Command"));
    }

    #[test]
    fn opening_the_palette_shows_the_command_badge() {
        let mut app = App::new(bridge(), Backend::External, false);
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        assert_eq!(app.mode(), Mode::Overlay);
        assert!(texts(&app).iter().any(|text| text == "Command"));
    }

    fn identity() -> crate::mail::Identity {
        crate::mail::Identity {
            id: "id1".to_string(),
            email: "me@example.com".to_string(),
            name: "Me".to_string(),
            account: "a1".to_string(),
        }
    }

    #[test]
    fn composing_shows_the_compose_badge_and_closing_returns_to_browse() {
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = app.mail.inner.update(mail::Message::Identities(Ok(vec![identity()])), &mut app.shell, Instant::now());
        let _ = app.mail.inner.update(mail::Message::Compose(None), &mut app.shell, Instant::now());
        assert_eq!(app.mode(), Mode::Compose);
        assert!(texts(&app).iter().any(|text| text == "Compose"));

        let _ = app.mail.inner.update(mail::Message::Escape, &mut app.shell, Instant::now());
        assert_eq!(app.mode(), Mode::Browse);
        assert!(!texts(&app).iter().any(|text| text == "Compose"));
    }

    #[test]
    fn a_palette_open_over_a_draft_shows_the_overlay_badge_not_the_compose_one() {
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = app.mail.inner.update(mail::Message::Identities(Ok(vec![identity()])), &mut app.shell, Instant::now());
        let _ = app.mail.inner.update(mail::Message::Compose(None), &mut app.shell, Instant::now());
        assert!(app.mail.inner.composing(), "the draft actually needs to be open for this test to mean anything");
        let all = app.all_commands();
        let _ = app.open_overlay(all);
        assert_eq!(app.mode(), Mode::Overlay, "what's on top wins over what it's covering");
    }

    /// Not a unit test of the pieces — those are above, and in `noctalia_iced::picker`'s own
    /// suite — this builds the real widget tree with the palette open and lays it out, the same
    /// reason `tests/mail.rs`'s `render` exists: a shadow, a stack of two opaque layers, and a
    /// scrollable built from a scored list are all real layout code with room to panic in.
    #[test]
    fn the_palette_lays_out_over_the_window_without_panicking() {
        let mut app = App::new(bridge(), Backend::External, false);
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
        let mut app = App::new(bridge(), Backend::External, false);
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
        let directory = std::env::var("NOCTMALIA_SHOTS")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots").to_string());
        std::fs::create_dir_all(&directory).expect("somewhere to write to");
        let path = std::path::Path::new(&directory).join("palette-open.png");
        let _ = std::fs::remove_file(&path);
        assert!(snapshot.matches_image(&path).expect("write the png"));
        eprintln!("wrote {}", path.display());
    }

    /// Same disclaimer as `shot_of_the_palette_open`.
    #[test]
    #[ignore = "writes a PNG rather than asserting"]
    fn shot_of_a_context_menu_open() {
        use iced::{Settings, Size};
        let mut app = App::new(bridge(), Backend::External, false);
        app.config.templates = vec![
            config::Template {
                name: "Look up".to_string(),
                command: vec!["dict".to_string(), "{selection}".to_string()],
                contexts: vec![],
            },
            config::Template {
                name: "Open in browser".to_string(),
                command: vec!["xdg-open".to_string(), "{selection}".to_string()],
                contexts: vec![],
            },
        ];
        let _ = app.open_context_menu("the highlighted text".to_string(), "mail-body");
        let element = app.view().map(|_| ());
        let settings = Settings {
            default_font: crate::font::ui(),
            fonts: vec![noctalia_iced::theme::ICON_FONT_BYTES.into()],
            ..Settings::default()
        };
        let mut simulator = iced_test::Simulator::with_size(settings, Size::new(1180.0, 720.0), element);
        let snapshot = simulator.snapshot(&Theme::Dark).expect("it draws");
        let directory = std::env::var("NOCTMALIA_SHOTS")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots").to_string());
        std::fs::create_dir_all(&directory).expect("somewhere to write to");
        let path = std::path::Path::new(&directory).join("context-menu-open.png");
        let _ = std::fs::remove_file(&path);
        assert!(snapshot.matches_image(&path).expect("write the png"));
        eprintln!("wrote {}", path.display());
    }

    /// Same disclaimer as `shot_of_the_palette_open`.
    #[test]
    #[ignore = "writes a PNG rather than asserting"]
    fn shot_of_the_compose_badge() {
        use iced::{Settings, Size};
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = app.mail.inner.update(mail::Message::Identities(Ok(vec![identity()])), &mut app.shell, Instant::now());
        let _ = app.mail.inner.update(mail::Message::Compose(None), &mut app.shell, Instant::now());
        let element = app.view().map(|_| ());
        let settings = Settings {
            default_font: crate::font::ui(),
            fonts: vec![noctalia_iced::theme::ICON_FONT_BYTES.into()],
            ..Settings::default()
        };
        let mut simulator = iced_test::Simulator::with_size(settings, Size::new(1180.0, 720.0), element);
        let snapshot = simulator.snapshot(&Theme::Dark).expect("it draws");
        let directory = std::env::var("NOCTMALIA_SHOTS")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots").to_string());
        std::fs::create_dir_all(&directory).expect("somewhere to write to");
        let path = std::path::Path::new(&directory).join("compose-badge.png");
        let _ = std::fs::remove_file(&path);
        assert!(snapshot.matches_image(&path).expect("write the png"));
        eprintln!("wrote {}", path.display());
    }

    // ── Startup states: docs/one-program-plan.md Stream 2.1 ────────────────────────────

    #[test]
    fn an_external_backend_says_it_is_waiting_and_names_nothing_about_sockets_when_managed() {
        let app = App::new(bridge(), Backend::External, false);
        assert!(texts(&app).iter().any(|text| text == "Waiting for Thunderbird"));
    }

    #[test]
    fn a_fetch_in_progress_is_a_sentence_and_a_bar() {
        let mut app = App::new(bridge(), Backend::External, false);
        let _ = app.update(Message::Backend(Report::Fetching { received: 43_000_000, total: 86_000_000 }));
        assert!(texts(&app).iter().any(|text| text == "Getting Thunderbird"));
        assert!(texts(&app).iter().any(|text| text.contains("43 of 86 MB")));
    }

    #[test]
    fn a_failure_names_the_reason_and_the_log() {
        let mut app = App::new(bridge(), Backend::External, false);
        let log = std::path::PathBuf::from("/tmp/somewhere/thunderbird.log");
        let _ = app.update(Message::Backend(Report::Failed { reason: "systemd-run exited with 1".into(), log }));
        let words = texts(&app);
        assert!(words.iter().any(|text| text == "Thunderbird could not start"));
        assert!(
            words
                .iter()
                .any(|text| text.contains("systemd-run exited with 1")
                    && text.contains("/tmp/somewhere/thunderbird.log"))
        );
    }

    // ── Context commands: docs/context-commands-plan.md §3 ──────────────────────────────

    #[tokio::test]
    async fn a_template_substitutes_the_selection_into_the_argv() {
        let argv = vec!["/bin/echo".to_string(), "{selection}".to_string()];
        let output = run_template(argv, "hello world".to_string(), Duration::from_secs(5)).await;
        assert_eq!(output, Ok("hello world".to_string()));
    }

    #[tokio::test]
    async fn a_selection_full_of_shell_metacharacters_is_never_interpreted() {
        // If this ever went through `sh -c`, `$(...)` would run and the echoed text would differ.
        let selection = "$(echo pwned); rm -rf /nonexistent; `whoami` && true".to_string();
        let argv = vec!["/bin/echo".to_string(), "{selection}".to_string()];
        let output = run_template(argv, selection.clone(), Duration::from_secs(5)).await;
        assert_eq!(output, Ok(selection), "the selection arrives exactly as typed, not evaluated");
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_stderr_as_the_error() {
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "echo nope >&2; exit 1".to_string()];
        let output = run_template(argv, "unused".to_string(), Duration::from_secs(5)).await;
        assert_eq!(output, Err("nope".to_string()));
    }

    #[tokio::test]
    async fn a_missing_program_reports_an_error_rather_than_panicking() {
        let argv = vec!["/definitely/not/a/real/binary".to_string()];
        let output = run_template(argv, "x".to_string(), Duration::from_secs(5)).await;
        assert!(output.is_err());
    }

    #[tokio::test]
    async fn a_hanging_command_is_cut_off_by_the_timeout() {
        let argv = vec!["/bin/sleep".to_string(), "5".to_string()];
        let started = Instant::now();
        let output = run_template(argv, "x".to_string(), Duration::from_millis(50)).await;
        assert!(output.is_err(), "a timed-out template is an error, not a hang");
        assert!(started.elapsed() < Duration::from_secs(2), "the short timeout won, not sleep's five seconds");
    }

    #[test]
    fn output_past_the_cap_is_cut_with_a_marker() {
        let long = "x".repeat(TEMPLATE_OUTPUT_CAP + 50);
        let short = truncated(long);
        assert_eq!(short.chars().count(), TEMPLATE_OUTPUT_CAP + 1, "the cap, plus the marker");
        assert!(short.ends_with('…'));
    }

    #[test]
    fn output_under_the_cap_is_untouched() {
        assert_eq!(truncated("fine".to_string()), "fine");
    }

    #[test]
    fn a_context_menu_is_built_only_from_templates_that_match_or_name_no_context() {
        let mut app = App::new(bridge(), Backend::External, false);
        app.config.templates = vec![
            config::Template { name: "Everywhere".to_string(), command: vec!["true".to_string()], contexts: vec![] },
            config::Template {
                name: "Mail only".to_string(),
                command: vec!["true".to_string()],
                contexts: vec!["mail-body".to_string()],
            },
            config::Template {
                name: "People only".to_string(),
                command: vec!["true".to_string()],
                contexts: vec!["person-field".to_string()],
            },
        ];
        let _ = app.open_context_menu("some text".to_string(), "mail-body");
        let overlay = app.overlay.as_ref().expect("matching templates open a menu");
        let labels: Vec<&str> = overlay.all.iter().map(|command| command.label.as_str()).collect();
        assert!(labels.contains(&"Everywhere"));
        assert!(labels.contains(&"Mail only"));
        assert!(!labels.contains(&"People only"));
    }

    #[test]
    fn a_context_with_no_matching_templates_opens_no_menu() {
        let mut app = App::new(bridge(), Backend::External, false);
        app.config.templates = vec![config::Template {
            name: "People only".to_string(),
            command: vec!["true".to_string()],
            contexts: vec!["person-field".to_string()],
        }];
        let _ = app.open_context_menu("some text".to_string(), "mail-body");
        assert!(app.overlay.is_none());
    }
}
