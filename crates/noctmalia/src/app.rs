//! The rolodex: address books on the left, contacts in the middle, one card on the right.

use crate::contacts::{self, AddressBook, Contact};
use crate::palette;
use crate::vcard::{self, Card, Entry};
use iced::advanced::widget::Id;
use iced::keyboard::{self, key::Named};
use iced::widget::operation;
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::tooltip;
use iced::widget::{button, column, container, pin, row, scrollable, space, stack, text, text_input};
use iced::{Alignment, Animation, Border, Color, Element, Length, Padding, Size, Subscription, Task, Theme, border};
use noctalia_iced::chrome;
use noctalia_iced::motion::{self, Replay};
use noctalia_iced::theme::{self, ButtonVariant, Palette};
use noctalia_iced::widgets;
use noctmalia_bridge::{Bridge, Event};
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// Tabler codepoints from the bundled icon font.
mod icon {
    pub const ADDRESS_BOOK: char = '\u{f021}';
    pub const MAIL: char = '\u{eae5}';
    pub const PHONE: char = '\u{eb09}';
    pub const WORLD: char = '\u{eb54}';
    pub const MAP_PIN: char = '\u{eae8}';
    pub const NOTE: char = '\u{eb6d}';
    pub const SEARCH: char = '\u{eb1c}';
    pub const TRASH: char = '\u{eb41}';
    pub const PENCIL: char = '\u{eb04}';
    pub const PLUS: char = '\u{eb0b}';
    pub const X: char = '\u{eb55}';
    pub const BUILDING: char = '\u{ea4f}';
    pub const LOCK: char = '\u{eae2}';
    pub const CLOUD: char = '\u{ea76}';
    pub const ALERT: char = '\u{ea05}';
    pub const PLUG: char = '\u{ea9e}';
}

/// The window the application asks for, and the size the layout assumes until the compositor has
/// said otherwise — one configure later it is measuring the real thing.
pub const WINDOW: Size = Size::new(1100.0, 720.0);
pub const WINDOW_MIN: Size = Size::new(760.0, 480.0);

/// Wide enough for the books Thunderbird ships with — "Personal Address Book" is 21 characters and
/// was being cut at 200. Names longer than that still clip; the rail is a rail.
const RAIL_WIDTH: f32 = 228.0;
/// The rail with its labels gone: the selection bar, one icon per book, and nothing else.
const RAIL_COLLAPSED: f32 = 60.0;
const LIST_WIDTH: f32 = 300.0;
/// The card reads as a column, not as a banner stretched across the window.
const DETAIL_WIDTH: f32 = 560.0;
/// Under this much room the card starts folding names onto two lines and breaking addresses
/// mid-word. It is the width at which the rail gives up its labels, not a width the card needs:
/// collapsing the rail on this window buys about 140px, which is enough.
const DETAIL_CRAMPED: f32 = 420.0;
/// Under this much room the editor's actions drop to a row of their own. Above it the avatar, the
/// title and both buttons sit on one line; below it the title is what gives way, and a title
/// squeezed to nothing is not a title.
const HERO_STACKS: f32 = 380.0;
const AVATAR: f32 = 36.0;
const AVATAR_LARGE: f32 = 64.0;
const CONTACT_ROW: f32 = 54.0;
const BOOK_ROW: f32 = 36.0;
/// The hairline between rows in a list, and so the difference between a row and a row pitch.
const ROW_GAP: f32 = 1.0;
const CONTACT_PITCH: f32 = CONTACT_ROW + ROW_GAP;
const BOOK_PITCH: f32 = BOOK_ROW + ROW_GAP;
/// The slot a rail row's icon or initial sits in, so the two line up with each other.
const MARK: f32 = 16.0;
/// The vCard type field — "work", "home", "cell". Wide enough for those and no wider.
const LABEL_FIELD: f32 = 88.0;

// ── Motion ──────────────────────────────────────────────────────────────────
// iced has no opacity or transform that survives clipping: `float` moves its content into an
// overlay, which would let a scrolling pane draw over its neighbours. So everything here animates
// through layout and colour — padding, width, height, and blends towards the surface — which the
// renderer clips and hit-tests exactly as it does a still frame.

/// How far below home the detail pane starts when what it shows changes.
const DETAIL_RISE: f32 = 12.0;
/// How far left of home a contact row starts in the staggered reveal.
const ROW_SLIDE: f32 = 10.0;
/// Rows past this one skip the stagger: they are below the fold, and the tail of a long list
/// arriving one row at a time is a wait, not a flourish.
const STAGGER_ROWS: usize = 14;
/// Each row starts this much of the reveal after the one above it, and takes this long to arrive.
/// `STAGGER_ROWS * STEP + WINDOW` stays under 1.0, or the last row is still moving at the end.
const STAGGER_STEP: f32 = 0.045;
const STAGGER_WINDOW: f32 = 0.35;

/// Names the contact list so the keyboard can scroll it.
const LIST_ID: &str = "contacts";
/// Names the editor's first field, so opening the form puts the cursor in it.
const FIRST_FIELD: &str = "first-name";

/// A field in the editor. Repeating fields carry their row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    First,
    Last,
    Nickname,
    Organisation,
    Role,
    Note,
    Email(usize),
    EmailLabel(usize),
    Phone(usize),
    PhoneLabel(usize),
    Url(usize),
}

/// The repeating sections rows can be added to and removed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    Email,
    Phone,
    Url,
}

#[derive(Debug, Clone)]
pub enum Message {
    Chrome(chrome::Action),
    Bridge(Event),
    Books(contacts::Result<Vec<AddressBook>>),
    /// Tagged with the generation that asked, so a slow search cannot overwrite a newer one.
    Contacts(u64, contacts::Result<Vec<Contact>>),
    SelectBook(Option<String>),
    Query(String),
    Select(String),
    New,
    Edit,
    Cancel,
    Field(Field, String),
    Add(Rows),
    Remove(Rows, usize),
    Save,
    Saved(contacts::Result<String>),
    AskDelete,
    Delete,
    Deleted(contacts::Result<()>),
    Dismiss,
    /// The window was resized. Only the width matters: it decides whether the rail keeps its labels.
    Resized(f32),
    /// Move focus through the editor's fields: true forwards, false back.
    Traverse(bool),
    /// Move the selection through the contact list: -1 up, 1 down.
    Step(i32),
    /// Back out of whatever is open: the editor, a delete confirmation, the notice.
    Escape,
    /// The contact list scrolled. Kept so the keyboard can tell whether the row it just selected is
    /// still on screen.
    Scrolled(Viewport),
    /// Only when `NOCTMALIA_FPS` is set: an animation frame, used to measure render cost.
    Frame,
    /// The Noctalia shell's palette changed.
    Palette(Palette),
}

/// Everything on screen that is currently in motion.
///
/// Two kinds, because they answer different questions. An [`Animation`] holds a *value* and moves
/// from wherever it currently is, which is what a selection bar needs when the user picks a third
/// row before it reached the second. A [`Replay`] holds no value and starts over on command, which
/// is what an arrival needs: the pane rises the same way every time its contents change.
struct Motion {
    /// Which row the contact list's selection bar is travelling to, counted from the top.
    contact: Animation<f32>,
    /// Whether that bar is on screen at all — it fades rather than blinking out.
    contact_shown: Animation<bool>,
    /// The same, for the address-book rail. "All contacts" is row 0, so the rail always has one.
    book: Animation<f32>,
    /// The detail pane arriving, replayed whenever what it shows changes.
    detail: Replay,
    /// The contact list arriving, staggered down the rows.
    reveal: Replay,
    /// The notice banner opening and closing.
    notice: Animation<bool>,
    /// The rail trading its labels for icons.
    collapse: Animation<bool>,
    /// The editor's avatar taking the accent, once what is being typed amounts to a name.
    identity: Animation<bool>,
}

impl Motion {
    fn new() -> Motion {
        Motion {
            contact: motion::spring_animation(0.0),
            contact_shown: motion::glide_animation(false),
            book: motion::spring_animation(0.0),
            detail: Replay::settled(motion::SETTLE, motion::NORMAL),
            reveal: Replay::settled(motion::GLIDE, motion::SLOW),
            notice: motion::settle_animation(false),
            collapse: motion::settle_animation(false),
            identity: motion::glide_animation(false),
        }
    }

    /// Whether anything still has somewhere to be. The window only subscribes to frames while this
    /// holds, so an idle rolodex costs nothing.
    fn animating(&self, now: Instant) -> bool {
        self.contact.is_animating(now)
            || self.contact_shown.is_animating(now)
            || self.book.is_animating(now)
            || self.detail.is_animating(now)
            || self.reveal.is_animating(now)
            || self.notice.is_animating(now)
            || self.collapse.is_animating(now)
            || self.identity.is_animating(now)
    }
}

struct Editor {
    /// `None` while creating.
    id: Option<String>,
    book: String,
    card: Card,
}

struct Notice {
    text: String,
    bad: bool,
}

pub struct Rolodex {
    bridge: Bridge,
    chrome: chrome::Chrome,
    connected: bool,
    loading: bool,
    books: Vec<AddressBook>,
    /// The selected book, or `None` for all of them.
    book: Option<String>,
    query: String,
    contacts: Vec<Contact>,
    generation: u64,
    selected: Option<String>,
    editor: Option<Editor>,
    confirming_delete: bool,
    notice: Option<Notice>,
    /// Where the contact list is scrolled to, as of its last scroll. `None` until it reports one.
    list_view: Option<Viewport>,
    /// The window's width, which is what decides whether all three panes fit.
    width: f32,
    motion: Motion,
    frames: Option<Frames>,
    /// Only the accent has to be held: the other roles are read straight out of the theme, but iced
    /// keeps `primary` in its own palette, so the `Theme` has to be rebuilt when it changes.
    accent: Color,
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

impl Rolodex {
    pub fn new(bridge: Bridge) -> Rolodex {
        Rolodex {
            bridge,
            chrome: chrome::initial(),
            connected: false,
            loading: false,
            books: Vec::new(),
            book: None,
            query: String::new(),
            contacts: Vec::new(),
            generation: 0,
            selected: None,
            editor: None,
            confirming_delete: false,
            notice: None,
            list_view: None,
            width: WINDOW.width,
            motion: Motion::new(),
            frames: Frames::enabled(),
            accent: theme::palette().primary,
        }
    }

    pub fn theme(&self) -> Theme {
        theme::noctalia(self.accent)
    }

    pub fn title(&self) -> String {
        "Contacts".to_string()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            chrome::events().map(Message::Chrome),
            keys(),
            iced::window::resize_events().map(|(_, size)| Message::Resized(size.width)),
            Subscription::run_with(Feed(self.bridge.clone()), feed),
        ];
        subscriptions.push(Subscription::run(palette_changes));
        // iced re-reads the subscriptions after every message, so starting an animation in
        // `update` turns this on for exactly as long as it runs.
        let animating = self.motion.animating(Instant::now());
        if animating || self.frames.as_ref().is_some_and(|frames| frames.drive) {
            subscriptions.push(iced::window::frames().map(|_| Message::Frame));
        }
        Subscription::batch(subscriptions)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // One reading for the whole message: two animations started from the same event should be
        // in step, not a few microseconds apart.
        let now = Instant::now();
        let task = self.step(message, now);
        // Almost anything can have opened or closed the detail pane, or changed how much room there
        // is for it, and `step` returns from a dozen places. Deciding here catches all of them;
        // re-aiming an animation at the value it already holds is a no-op.
        self.motion.collapse.go_mut(self.rail_collapsed(), now);
        self.motion.identity.go_mut(self.editor_named(), now);
        task
    }

    fn step(&mut self, message: Message, now: Instant) -> Task<Message> {
        match message {
            Message::Chrome(chrome::Action::Changed(chrome)) => self.chrome = chrome,
            Message::Chrome(action) => return chrome::perform(action).map(Message::Chrome),

            Message::Bridge(Event::Hello(_)) => {
                // Hello also fires when Thunderbird restarts under a live socket, so this is a
                // full resync, not just first contact. Saying so on stderr gives whatever started
                // this a way to tell the difference between "still coming up" and "stuck waiting".
                if !self.connected {
                    eprintln!("noctmalia: Thunderbird attached");
                }
                self.connected = true;
                self.hush(now);
                return self.load_books();
            }
            Message::Bridge(Event::Lost) => {
                if self.connected {
                    eprintln!("noctmalia: Thunderbird went away");
                }
                self.connected = false;
                self.loading = false;
            }
            // Any address-book change invalidates the list. A rolodex is small; reloading it beats
            // patching rows from event payloads that only sometimes carry the whole contact.
            Message::Bridge(Event::Notify { name, .. }) => {
                if name.starts_with("contacts.") || name.starts_with("addressBooks.") {
                    return self.reload();
                }
            }
            Message::Bridge(Event::Lagged(_)) => return self.load_books(),

            Message::Books(Ok(books)) => {
                self.books = books;
                if self.book.as_ref().is_some_and(|id| !self.books.iter().any(|book| &book.id == id)) {
                    self.book = None;
                }
                // The rail's rows just changed underneath the bar; put it where it belongs without
                // travelling there.
                self.motion.book = motion::spring_animation(self.book_row());
                return self.reload();
            }
            Message::Books(Err(error)) => self.fail(error),

            Message::Contacts(generation, result) => {
                // A reply from a search the user has already typed past.
                if generation != self.generation {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(contacts) => {
                        // Row 4 of a different list is a different contact, so a list that
                        // actually changed re-seats the bar rather than sliding it, and replays
                        // the reveal. A reload that came back with the same people — after a save,
                        // say — leaves both alone.
                        let same = self.contacts.iter().map(|contact| &contact.id).eq(contacts.iter().map(|c| &c.id));
                        self.contacts = contacts;
                        if self.selected.as_ref().is_some_and(|id| !self.has(id)) {
                            self.selected = None;
                        }
                        if same {
                            self.aim_selection(now);
                        } else {
                            self.seat_selection(now);
                            // The reveal is for a list arriving, not for a list being filtered. A
                            // search reloads on every keystroke, and restarting the stagger each
                            // time would have the results flickering in while they are typed.
                            if self.query.trim().is_empty() {
                                self.motion.reveal.restart(now);
                            } else {
                                self.motion.reveal = Replay::settled(motion::GLIDE, motion::SLOW);
                            }
                        }
                    }
                    Err(error) => self.fail(error),
                }
            }

            Message::SelectBook(book) => {
                self.book = book;
                self.motion.book.go_mut(self.book_row(), now);
                return self.reload();
            }
            Message::Query(query) => {
                self.query = query;
                return self.reload();
            }
            Message::Select(id) => {
                self.selected = Some(id);
                self.editor = None;
                self.confirming_delete = false;
                self.aim_selection(now);
                self.motion.detail.restart(now);
            }

            Message::New => {
                let Some(book) = self.writable_book() else {
                    self.fail("No writable address book".to_string());
                    return Task::none();
                };
                self.selected = None;
                self.confirming_delete = false;
                self.editor = Some(Editor { id: None, book, card: vcard::blank() });
                self.aim_selection(now);
                self.motion.detail.restart(now);
                // A new form opens with the cursor in it. Nobody reaches for the mouse to start
                // typing a name.
                return operation::focus(Id::new(FIRST_FIELD));
            }
            Message::Edit => {
                if let Some(contact) = self.selected_contact() {
                    let book = contact.book.clone().or_else(|| self.writable_book());
                    let Some(book) = book else {
                        self.fail("No writable address book".to_string());
                        return Task::none();
                    };
                    let card = contact.card.clone();
                    self.editor = Some(Editor { id: Some(contact.id.clone()), book, card });
                    self.motion.detail.restart(now);
                    return operation::focus(Id::new(FIRST_FIELD));
                }
            }
            Message::Cancel => {
                self.editor = None;
                self.confirming_delete = false;
                self.motion.detail.restart(now);
            }

            Message::Field(field, value) => {
                if let Some(editor) = &mut self.editor {
                    edit(&mut editor.card, field, value);
                }
            }
            Message::Add(rows) => {
                if let Some(editor) = &mut self.editor {
                    list_mut(&mut editor.card, rows).push(Entry::default());
                }
            }
            Message::Remove(rows, index) => {
                if let Some(editor) = &mut self.editor {
                    let list = list_mut(&mut editor.card, rows);
                    if index < list.len() {
                        list.remove(index);
                    }
                }
            }

            Message::Save => {
                let Some(editor) = &self.editor else { return Task::none() };
                let vcard = editor.card.to_vcard();
                let bridge = self.bridge.clone();
                return match editor.id.clone() {
                    Some(id) => Task::perform(contacts::update(bridge, id, vcard), Message::Saved),
                    None => {
                        let book = editor.book.clone();
                        Task::perform(contacts::create(bridge, book, vcard), Message::Saved)
                    }
                };
            }
            Message::Saved(Ok(id)) => {
                self.editor = None;
                self.selected = Some(id);
                self.aim_selection(now);
                self.motion.detail.restart(now);
                self.announce("Saved".to_string(), false, now);
                return self.reload();
            }
            Message::Saved(Err(error)) => self.fail(error),

            Message::AskDelete => self.confirming_delete = true,
            Message::Delete => {
                self.confirming_delete = false;
                if let Some(id) = self.selected.clone() {
                    return Task::perform(contacts::delete(self.bridge.clone(), id), Message::Deleted);
                }
            }
            Message::Deleted(Ok(())) => {
                self.selected = None;
                self.editor = None;
                self.aim_selection(now);
                self.motion.detail.restart(now);
                self.announce("Deleted".to_string(), false, now);
                return self.reload();
            }
            Message::Deleted(Err(error)) => self.fail(error),

            Message::Dismiss => self.hush(now),
            Message::Resized(width) => self.width = width,
            Message::Traverse(forwards) => {
                return if forwards { operation::focus_next() } else { operation::focus_previous() };
            }
            Message::Scrolled(viewport) => self.list_view = Some(viewport),
            Message::Step(delta) => {
                // While the editor is open the arrows belong to whatever is being typed into, and
                // moving the selection would throw the edit away.
                if self.editor.is_some() || self.contacts.is_empty() {
                    return Task::none();
                }
                let last = self.contacts.len() as i32 - 1;
                let row = match self.selected_row() {
                    Some(row) => (row as i32 + delta).clamp(0, last),
                    // Nothing selected yet: come in from whichever end the key points away from.
                    None if delta > 0 => 0,
                    None => last,
                };
                if let Some(contact) = self.contacts.get(row as usize) {
                    self.selected = Some(contact.id.clone());
                    self.confirming_delete = false;
                    self.aim_selection(now);
                    self.motion.detail.restart(now);
                    return self.follow_selection();
                }
            }
            Message::Escape => {
                if self.editor.is_some() {
                    self.editor = None;
                    self.motion.detail.restart(now);
                } else if self.confirming_delete {
                    self.confirming_delete = false;
                } else {
                    self.hush(now);
                }
            }
            // A frame the animations asked for, or one NOCTMALIA_FPS=drive asked for. Either way
            // the work is in `view`: arriving here at all is what schedules the redraw.
            Message::Frame => {}
            Message::Palette(palette) => {
                theme::set_palette(palette);
                self.accent = palette.primary;
            }
        }
        Task::none()
    }

    fn fail(&mut self, error: String) {
        self.loading = false;
        self.announce(error, true, Instant::now());
    }

    /// Shows the banner. The text stays put once set: `hush` only closes it, so the words are still
    /// there to draw while it slides shut.
    fn announce(&mut self, text: String, bad: bool, now: Instant) {
        self.notice = Some(Notice { text, bad });
        self.motion.notice.go_mut(true, now);
    }

    fn hush(&mut self, now: Instant) {
        self.motion.notice.go_mut(false, now);
    }

    /// Whether what is in the editor amounts to a name yet — which is when its avatar stops being
    /// an empty outline and takes the accent.
    fn editor_named(&self) -> bool {
        self.editor.as_ref().is_some_and(|editor| !editor.card.display_name().is_empty())
    }

    /// Whether the detail pane has anything of its own to show, as opposed to the placeholder.
    fn showing_detail(&self) -> bool {
        self.editor.is_some() || self.selected_contact().is_some()
    }

    /// Whether the rail should trade its labels for icons, at this window's width.
    fn rail_collapsed(&self) -> bool {
        rail_collapses(self.width, self.showing_detail())
    }

    /// What the detail pane has to work with, once the rail has taken its share.
    fn detail_width(&self) -> f32 {
        let rail = if self.rail_collapsed() { RAIL_COLLAPSED } else { RAIL_WIDTH };
        self.width - rail - LIST_WIDTH - 2.0 * theme::BORDER
    }

    /// Which row of the contact list is selected, if the selection is in the list at all.
    fn selected_row(&self) -> Option<f32> {
        let id = self.selected.as_ref()?;
        self.contacts.iter().position(|contact| &contact.id == id).map(|row| row as f32)
    }

    /// Which row of the rail is selected. "All contacts" is row 0, and is also where a book that
    /// has gone missing lands, since that is what the rail falls back to showing.
    fn book_row(&self) -> f32 {
        let Some(id) = &self.book else { return 0.0 };
        self.books.iter().position(|book| &book.id == id).map_or(0.0, |row| row as f32 + 1.0)
    }

    /// Sends the selection bar to the selected row, fading it out if there is no longer one.
    fn aim_selection(&mut self, now: Instant) {
        if let Some(row) = self.selected_row() {
            self.motion.contact.go_mut(row, now);
            self.motion.contact_shown.go_mut(true, now);
        } else {
            self.motion.contact_shown.go_mut(false, now);
        }
    }

    /// Scrolls the list the shortest distance that brings the selected row fully into view, and
    /// not at all if it is already there. Without a viewport to compare against — before the list
    /// has ever scrolled — there is nothing to decide with, and short lists never need it.
    fn follow_selection(&self) -> Task<Message> {
        let (Some(row), Some(view)) = (self.selected_row(), self.list_view) else {
            return Task::none();
        };
        let offset = view.absolute_offset().y;
        let height = view.bounds().height;
        let top = row * CONTACT_PITCH;
        let bottom = top + CONTACT_ROW;
        let target = if top < offset {
            top
        } else if bottom > offset + height {
            bottom - height
        } else {
            return Task::none();
        };
        operation::scroll_to(Id::new(LIST_ID), AbsoluteOffset { x: 0.0, y: target.max(0.0) })
    }

    /// Puts the bar on the selected row without travelling: for when the rows underneath it have
    /// changed and the distance between them no longer means anything.
    fn seat_selection(&mut self, now: Instant) {
        let row = self.selected_row();
        self.motion.contact = motion::spring_animation(row.unwrap_or(0.0));
        self.motion.contact_shown.go_mut(row.is_some(), now);
    }

    fn has(&self, id: &str) -> bool {
        self.contacts.iter().any(|contact| contact.id == id)
    }

    fn selected_contact(&self) -> Option<&Contact> {
        let id = self.selected.as_ref()?;
        self.contacts.iter().find(|contact| &contact.id == id)
    }

    /// Where a new contact goes: the selected book if it takes writes, else the first that does.
    fn writable_book(&self) -> Option<String> {
        let writable = |id: &String| self.books.iter().any(|book| &book.id == id && !book.read_only);
        self.book
            .clone()
            .filter(writable)
            .or_else(|| self.books.iter().find(|book| !book.read_only).map(|book| book.id.clone()))
    }

    fn load_books(&mut self) -> Task<Message> {
        Task::perform(contacts::books(self.bridge.clone()), Message::Books)
    }

    fn reload(&mut self) -> Task<Message> {
        self.generation += 1;
        let generation = self.generation;
        let bridge = self.bridge.clone();
        let book = self.book.clone();
        let books = self.books.clone();
        let query = self.query.trim().to_string();
        self.loading = true;
        let load = async move {
            if query.is_empty() {
                contacts::list(bridge, book, books).await
            } else {
                contacts::search(bridge, query, book).await
            }
        };
        Task::perform(load, move |result| Message::Contacts(generation, result))
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
        let body: Element<Message> = if self.connected {
            column![
                row![self.rail(now), hairline_y(), self.list(now), hairline_y(), self.detail(now)].height(Length::Fill),
                self.notice(now),
            ]
            .into()
        } else {
            waiting(self.bridge.path().display().to_string())
        };
        chrome::frame(self.chrome, "Contacts", body, Message::Chrome)
    }

    fn rail(&self, now: Instant) -> Element<'_, Message> {
        // Where the bar is this frame, in rows. The rows read their fill off it too, so the quiet
        // surface behind the selection travels with the bar instead of jumping ahead of it.
        let at = self.motion.book.interpolate_with(|row| row, now);
        // How much of the rail's writing is left. The width and the text fade together, so the
        // labels are gone by the time there is no room for them rather than being cut off mid-word.
        let showing = 1.0 - self.motion.collapse.interpolate(0.0, 1.0, now).clamp(0.0, 1.0);

        // "All contacts" is a heading, not a book, so it keeps an icon; the books get their
        // initial. Thunderbird reports none of its own books as read-only or remote, so a glyph per
        // book would be four identical glyphs once the names are gone, and the letter is the only
        // thing left that tells them apart.
        let mut books =
            column![rail_row("All contacts", None, nearness(at, 0), Some(icon::ADDRESS_BOOK), None, showing)]
                .spacing(ROW_GAP);
        for (index, book) in self.books.iter().enumerate() {
            let badge = match (book.read_only, book.remote) {
                (true, _) => Some(icon::LOCK),
                (false, true) => Some(icon::CLOUD),
                (false, false) => None,
            };
            let fill = nearness(at, index + 1);
            books = books.push(rail_row(&book.name, Some(book.id.clone()), fill, None, badge, showing));
        }

        let rows = (self.books.len() + 1) as f32;
        let bar = selection_bar(at * BOOK_PITCH, BOOK_ROW, 1.0, rows * BOOK_PITCH - ROW_GAP);
        let heading = text("Address books")
            .size(theme::FONT_CAPTION)
            .color(motion::mix(theme::palette().surface, theme::palette().on_surface_variant, showing))
            .width(Length::Fill)
            .wrapping(text::Wrapping::None);

        container(
            column![heading, scrollable(stack![books, bar]).style(theme::scrollable_style)].spacing(theme::SPACE_SM),
        )
        .width(motion::lerp(RAIL_COLLAPSED, RAIL_WIDTH, showing))
        .height(Length::Fill)
        .padding(motion::lerp(theme::SPACE_SM, theme::SPACE_MD, showing))
        .clip(true)
        .into()
    }

    fn list(&self, now: Instant) -> Element<'_, Message> {
        let count = self.contacts.len();
        let header = row![
            caption(if count == 1 { "1 contact".to_string() } else { format!("{count} contacts") }),
            space().width(Length::Fill),
            // Save's button, with a glyph in front: the same accent fill, the same control height
            // and the same padding, so the two read as the same kind of thing in two places.
            // `ButtonVariant::Default` — surface_variant on surface — is close enough to the
            // background in most Noctalia palettes to read as disabled instead.
            glyph_button(
                icon::PLUS,
                "New",
                theme::CONTROL_HEIGHT,
                theme::button_style(ButtonVariant::Primary),
                Message::New,
            ),
        ]
        .align_y(Alignment::Center);

        let search = text_input("Search", &self.query)
            .on_input(Message::Query)
            .icon(text_input::Icon {
                font: theme::ICON_FONT,
                code_point: icon::SEARCH,
                size: Some(theme::FONT_BODY.into()),
                spacing: theme::SPACE_SM,
                side: text_input::Side::Left,
            })
            .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD]))
            .size(theme::FONT_BODY)
            .style(theme::text_input_style);

        let body: Element<Message> = if self.contacts.is_empty() {
            let message = if self.loading {
                "Loading…"
            } else if self.query.trim().is_empty() {
                "Nothing here yet"
            } else {
                "Nothing matched"
            };
            container(caption(message)).center_x(Length::Fill).padding(theme::SPACE_MD).into()
        } else {
            let reveal = self.motion.reveal.linear(now);
            let at = self.motion.contact.interpolate_with(|row| row, now);
            // Nothing selected fades the bar out rather than blinking it away, and takes the rows'
            // fill with it.
            let shown = self.motion.contact_shown.interpolate(0.0, 1.0, now);

            let mut rows = column![].spacing(ROW_GAP);
            for (index, contact) in self.contacts.iter().enumerate() {
                let arrived = if index >= STAGGER_ROWS {
                    1.0
                } else {
                    motion::stagger(motion::SETTLE, reveal, index, STAGGER_STEP, STAGGER_WINDOW)
                };
                rows = rows.push(contact_row(contact, nearness(at, index) * shown, arrived));
            }

            let total = self.contacts.len() as f32 * CONTACT_PITCH - ROW_GAP;
            let bar = selection_bar(at * CONTACT_PITCH, CONTACT_ROW, shown, total);
            scrollable(stack![rows, bar])
                .id(Id::new(LIST_ID))
                .on_scroll(Message::Scrolled)
                .style(theme::scrollable_style)
                .height(Length::Fill)
                .into()
        };

        container(column![header, search, body].spacing(theme::SPACE_SM))
            .width(LIST_WIDTH)
            .height(Length::Fill)
            .padding(theme::SPACE_MD)
            .into()
    }

    fn detail(&self, now: Instant) -> Element<'_, Message> {
        let arrived = self.motion.detail.at(now);
        // The avatar is small enough to take the livelier spring, and it is the thing the eye goes
        // to first. 0.86 to 1.0, so the overshoot lands a whisker over full size.
        let pop = 0.86 + 0.14 * motion::SPRING.value(self.motion.detail.linear(now));

        let content: Element<Message> = match (&self.editor, self.selected_contact()) {
            (Some(editor), _) => self.editor_view(editor, now, pop),
            (None, Some(contact)) => self.card_view(contact, pop),
            (None, None) => placeholder(),
        };

        // The pane rises into place on top padding. Doing it this way rather than with `float`
        // keeps it inside its own bounds: it never draws over the list while it moves.
        let rise = motion::lerp(DETAIL_RISE, 0.0, arrived);
        container(container(content).max_width(DETAIL_WIDTH))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: theme::SPACE_LG + rise,
                right: theme::SPACE_LG,
                bottom: theme::SPACE_LG,
                left: theme::SPACE_LG,
            })
            .into()
    }

    fn card_view<'a>(&'a self, contact: &'a Contact, pop: f32) -> Element<'a, Message> {
        let card = &contact.card;
        // Everything in the heading wraps, and falls back to breaking a word when a single one is
        // wider than the pane — an address book is full of unbroken strings that no word wrap can
        // help with, and the pane is not always wide.
        let mut heading = column![
            text(card.display_name())
                .size(theme::FONT_HEADER)
                .font(theme::semibold())
                .wrapping(text::Wrapping::WordOrGlyph)
        ]
        .spacing(2)
        .width(Length::Fill);
        let subtitle: Vec<&str> =
            [card.role.as_str(), card.organisation.as_str()].into_iter().filter(|part| !part.is_empty()).collect();
        if !subtitle.is_empty() {
            heading = heading.push(
                text(subtitle.join(" · "))
                    .size(theme::FONT_BODY)
                    .color(theme::palette().on_surface_variant)
                    .wrapping(text::Wrapping::WordOrGlyph),
            );
        }
        if !card.nickname.is_empty() {
            heading = heading.push(
                text(format!("“{}”", card.nickname))
                    .size(theme::FONT_CAPTION)
                    .color(theme::palette().on_surface_variant)
                    .wrapping(text::Wrapping::WordOrGlyph),
            );
        }

        let header = row![avatar(&card.initials(), AVATAR_LARGE, 1.0, pop, 1.0), heading]
            .spacing(theme::SPACE_MD)
            .align_y(Alignment::Center);

        // Edit leads; Delete sits at the other end of the row rather than a thumb's width from it.
        let actions = row![
            glyph_button(
                icon::PENCIL,
                "Edit",
                theme::CONTROL_HEIGHT,
                theme::button_style(ButtonVariant::Default),
                Message::Edit,
            ),
            space().width(Length::Fill),
            if self.confirming_delete {
                danger_button("Really delete?", Message::Delete)
            } else {
                glyph_button(
                    icon::TRASH,
                    "Delete",
                    theme::CONTROL_HEIGHT,
                    theme::button_style(ButtonVariant::Tab),
                    Message::AskDelete,
                )
            },
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

        let mut fields: Vec<Element<Message>> = Vec::new();
        for entry in card.emails.iter().filter(|entry| !entry.value.is_empty()) {
            fields.push(field(icon::MAIL, &entry.kind, &entry.value));
        }
        for entry in card.phones.iter().filter(|entry| !entry.value.is_empty()) {
            fields.push(field(icon::PHONE, &entry.kind, &entry.value));
        }
        for entry in card.urls.iter().filter(|entry| !entry.value.is_empty()) {
            fields.push(field(icon::WORLD, &entry.kind, &entry.value));
        }
        if !card.organisation.is_empty() {
            fields.push(field(icon::BUILDING, "company", &card.organisation));
        }
        for address in card.addresses.iter().filter(|address| !address.is_empty()) {
            fields.push(field(icon::MAP_PIN, &address.kind, &address.lines().join("\n")));
        }
        if !card.note.is_empty() {
            fields.push(field(icon::NOTE, "note", &card.note));
        }

        let mut body = column![header, actions].spacing(theme::SPACE_MD);
        if fields.is_empty() {
            body = body.push(caption("No details yet"));
        } else {
            body = body.push(group(fields));
        }

        scrollable(body.padding(Padding { right: theme::SPACE_SM, ..Padding::ZERO }))
            .style(theme::scrollable_style)
            .height(Length::Fill)
            .into()
    }

    fn editor_view<'a>(&'a self, editor: &'a Editor, now: Instant, pop: f32) -> Element<'a, Message> {
        let card = &editor.card;
        let quiet = theme::palette().on_surface_variant;
        let title = if editor.id.is_some() { "Edit contact" } else { "New contact" };

        // The form has a subject, and it fills in as the name is typed: the avatar takes the
        // initials and the accent the moment there is a name to take them from. It is the same
        // avatar at the same size the card shows, so pressing Edit does not move it.
        let named = !card.display_name().is_empty();
        let initials = if named { card.initials() } else { String::new() };
        let accent = self.motion.identity.interpolate(0.0, 1.0, now);
        let standing = if named {
            text(card.display_name()).size(theme::FONT_CAPTION).color(quiet).wrapping(text::Wrapping::WordOrGlyph)
        } else {
            text("Not named yet").size(theme::FONT_CAPTION).color(quiet)
        };

        let actions = row![
            widgets::action("Cancel", ButtonVariant::Tab, Some(Message::Cancel)),
            // Nothing to save is not a thing to let someone do: a blank card becomes a contact with
            // no name, which is a row nobody finds again.
            widgets::action("Save", ButtonVariant::Primary, worth_saving(card).then_some(Message::Save)),
        ]
        .spacing(theme::SPACE_SM);
        let subject = row![
            avatar(&initials, AVATAR_LARGE, accent, pop, 1.0),
            column![
                text(title).size(theme::FONT_TITLE).font(theme::semibold()).wrapping(text::Wrapping::None),
                standing
            ]
            .spacing(2)
            .width(Length::Fill),
        ]
        .spacing(theme::SPACE_MD)
        .width(Length::Fill)
        .align_y(Alignment::Center);

        // The buttons are a fixed width and the avatar is a fixed width, so on a narrow pane it is
        // the title that gets squeezed out of existence. Below the breakpoint it keeps the line to
        // itself and the buttons take one of their own.
        let hero: Element<Message> = if self.detail_width() < HERO_STACKS {
            column![subject, row![space().width(Length::Fill), actions]].spacing(theme::SPACE_SM).into()
        } else {
            row![subject, actions].spacing(theme::SPACE_MD).align_y(Alignment::Center).into()
        };

        let who = column![
            row![
                named_input(FIRST_FIELD, "First name", &card.name.given, Field::First),
                labelled("Last name", "", &card.name.family, Field::Last),
            ]
            .spacing(theme::SPACE_SM),
            row![
                labelled("Organisation", "", &card.organisation, Field::Organisation),
                labelled("Role", "", &card.role, Field::Role),
            ]
            .spacing(theme::SPACE_SM),
            labelled("Nickname", "", &card.nickname, Field::Nickname),
        ]
        .spacing(theme::SPACE_MD);

        let mut body = column![hero, hairline_x(), who].spacing(theme::SPACE_LG);
        body = body.push(section("Email", "email", Rows::Email, &card.emails, Field::Email, Field::EmailLabel, true));
        body = body.push(section("Phone", "number", Rows::Phone, &card.phones, Field::Phone, Field::PhoneLabel, true));
        body = body.push(section("Links", "link", Rows::Url, &card.urls, Field::Url, Field::Url, false));
        body = body.push(labelled("Note", "Anything worth remembering", &card.note, Field::Note));

        // Addresses are seven components each; the card shows them, but editing them needs a form of
        // its own. Until then, say they are kept rather than letting a save look like it lost them.
        if !card.addresses.is_empty() {
            let kept = format!(
                "{} postal address{} kept unchanged",
                card.addresses.len(),
                if card.addresses.len() == 1 { "" } else { "es" }
            );
            body = body.push(
                row![widgets::icon(icon::MAP_PIN, theme::FONT_CAPTION).color(quiet), caption(kept)]
                    .spacing(theme::SPACE_SM)
                    .align_y(Alignment::Center),
            );
        }

        scrollable(body.padding(Padding { right: theme::SPACE_SM, ..Padding::ZERO }))
            .style(theme::scrollable_style)
            .height(Length::Fill)
            .into()
    }

    fn notice(&self, now: Instant) -> Element<'_, Message> {
        let open = self.motion.notice.interpolate(0.0, 1.0, now);
        // The text outlives the banner by exactly one animation: `hush` closes it and leaves the
        // words behind, so there is something to draw while it slides shut.
        let Some(notice) = self.notice.as_ref().filter(|_| open > 0.001) else {
            return space().height(0).into();
        };
        let color = if notice.bad { theme::palette().error } else { theme::palette().tertiary };
        let glyph = if notice.bad { icon::ALERT } else { theme::icon::CHECK };
        container(
            row![
                widgets::icon(glyph, theme::FONT_BODY).color(color),
                // The banner is one control-height row and clips to it, so the message stays on
                // one line rather than wrapping into a half-drawn second.
                text(notice.text.clone()).size(theme::FONT_CAPTION).wrapping(text::Wrapping::None),
                space().width(Length::Fill),
                button(widgets::icon(icon::X, theme::FONT_CAPTION).color(theme::palette().on_surface_variant))
                    .style(theme::bare_button)
                    .padding(theme::SPACE_XS)
                    .on_press(Message::Dismiss),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        // Opening on height, clipped, so the row slides out from under the edge. `open` overshoots
        // a little on the way in, which is the whole point of SETTLE.
        //
        // The banner clips while it slides, so its full height has to be a number rather than
        // whatever the content measures. Taking the design language's control height makes it a
        // token instead of a guess, and leaves room to spare for the dismiss button inside it.
        .height(theme::CONTROL_HEIGHT * open)
        .clip(true)
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_MD]))
        .style(move |_| container::Style {
            background: Some(theme::palette().surface_variant.into()),
            border: Border { color, width: theme::BORDER, ..Border::default() },
            ..container::Style::default()
        })
        .into()
    }
}

// ── Pieces ──────────────────────────────────────────────────────────────────

/// Secondary text: section labels, counts, hints.
fn caption<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    text(label).size(theme::FONT_CAPTION).color(theme::palette().on_surface_variant).into()
}

fn hairline_y<'a>() -> Element<'a, Message> {
    container(space().width(theme::BORDER).height(Length::Fill)).style(theme::rule).into()
}

fn hairline_x<'a>() -> Element<'a, Message> {
    container(space().width(Length::Fill).height(theme::BORDER)).style(theme::rule).into()
}

/// Whether the rail should trade its labels for icons: only when there is something in the detail
/// pane *and* the three panes do not comfortably fit across the window. On a wide one all three fit,
/// and collapsing the rail would be taking something away for nothing.
fn rail_collapses(width: f32, showing_detail: bool) -> bool {
    showing_detail && width - RAIL_WIDTH - LIST_WIDTH - 2.0 * theme::BORDER < DETAIL_CRAMPED
}

/// How much of a row's fill the selection bar is currently giving it: 1 when the bar is on the
/// row, 0 once it is a whole row away. Two adjacent rows therefore cross-fade as the bar travels
/// between them, and a longer jump lights the rows it passes over for a frame or two.
fn nearness(at: f32, index: usize) -> f32 {
    (1.0 - (at - index as f32).abs()).clamp(0.0, 1.0)
}

/// List rows mark selection with a leading bar and a quiet fill. `ButtonVariant`'s own hover is a
/// full mint block and `Selected` a full accent block — at list length that is a wall of colour.
///
/// `fill` is how selected the row is, which is a fraction rather than a flag while the bar is on
/// its way.
///
/// Hover and selection share the one fill, because the palette has one quiet surface and not two:
/// `surface_variant` is it, and a weaker wash of it would be a colour Noctalia never named. What
/// says a row is *selected* rather than merely under the pointer is the accent — the bar down its
/// left and the accent in its avatar — which is the louder signal either way.
fn row_style(fill: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        // The only fraction left is time: the fill is that role arriving, not a shade of it.
        let strength = if hovered { 1.0 } else { fill };
        button::Style {
            background: (strength > 0.001).then(|| theme::alpha(theme::palette().surface_variant, strength).into()),
            text_color: theme::palette().on_surface,
            border: border::rounded(theme::RADIUS_MD),
            ..button::Style::default()
        }
    }
}

/// The accent bar down the left of the selected row, pinned over the rows so it can be anywhere
/// between them. It lives inside the scrollable with the rows, so it scrolls with them.
///
/// `y` is its top in pixels, `total` the height of the whole column of rows — the bar layer has to
/// be as tall as the rows layer or the stack would size itself to whichever is bigger and leave the
/// scrollable measuring the wrong content.
///
/// It can lie over the rows without stealing their clicks because `stack` only hides the cursor
/// from the layers beneath one that reports a `mouse_interaction`, and this is a `space` in a
/// container: neither implements one, so the rows go on receiving presses through it.
fn selection_bar<'a>(y: f32, row_height: f32, shown: f32, total: f32) -> Element<'a, Message> {
    let height = (row_height - theme::SPACE_XS * 2.0).max(1.0);
    let mark = container(space().width(theme::BORDER_EMPHASIZED).height(height)).style(move |_| container::Style {
        background: Some(theme::alpha(theme::palette().primary, shown.clamp(0.0, 1.0)).into()),
        border: border::rounded(theme::BORDER_EMPHASIZED),
        ..container::Style::default()
    });
    pin(mark).x(theme::SPACE_SM).y(y + theme::SPACE_XS).width(Length::Fill).height(total.max(0.0)).into()
}

/// The first character of a book's name, which is what identifies it once the rail has collapsed.
fn initial(label: &str) -> String {
    label.chars().find(|c| c.is_alphanumeric()).map_or_else(|| "•".to_string(), |c| c.to_uppercase().to_string())
}

/// The gap a row leaves for the bar to be drawn in.
fn bar_gutter<'a>() -> Element<'a, Message> {
    space().width(theme::BORDER_EMPHASIZED).into()
}

/// A rail row is one line, clipped, never two.
///
/// The bar travels on a fixed row pitch, so a wrapped label would put it on the wrong row — and a
/// wrapped label spilled into the row below it long before there was a bar to misplace. Address
/// book names are long ("Personal Address Book") and the rail is narrow; a sidebar truncates.
///
/// `glyph` marks the row when there is an icon worth using; otherwise the label's initial does it,
/// which is what survives the collapse. `badge` is the read-only or remote mark, and goes with the
/// label — at the collapsed width there is only room for one thing, and identity beats provenance.
///
/// `showing` is how much of the label is left as the rail collapses. Below half of it the name has
/// faded out, and a tooltip takes over the job of saying which book this is.
fn rail_row<'a>(
    label: &str,
    id: Option<String>,
    fill: f32,
    glyph: Option<char>,
    badge: Option<char>,
    showing: f32,
) -> Element<'a, Message> {
    let surface = theme::palette().surface;
    let quiet = theme::palette().on_surface_variant;
    let name = text(label.to_string())
        .size(theme::FONT_BODY)
        .color(motion::mix(surface, theme::palette().on_surface, showing))
        .width(Length::Fill)
        .wrapping(text::Wrapping::None);
    let mark: Element<Message> = match glyph {
        Some(glyph) => widgets::icon(glyph, theme::FONT_BODY).color(quiet).into(),
        None => text(initial(label)).size(theme::FONT_CAPTION).font(theme::semibold()).color(quiet).into(),
    };
    // A fixed slot, so a letter and an icon leave the names starting in the same place.
    let mark = container(mark).width(MARK).center_x(MARK).center_y(Length::Fill);
    let mut line = row![bar_gutter(), mark, name].spacing(theme::SPACE_SM).align_y(Alignment::Center);
    if let Some(badge) = badge {
        line = line.push(widgets::icon(badge, theme::FONT_CAPTION).color(motion::mix(surface, quiet, showing)));
    }

    // Text is clipped to the viewport it is handed, not to its own box, so the clip has to be a
    // container around it — otherwise a name too long for the rail draws straight over the list.
    let row_button = button(container(line).clip(true))
        .width(Length::Fill)
        .height(BOOK_ROW)
        .padding(Padding::from([0.0, theme::SPACE_SM]))
        .style(row_style(fill))
        .on_press(Message::SelectBook(id));

    if showing > 0.5 {
        return row_button.into();
    }
    let hint = container(text(label.to_string()).size(theme::FONT_CAPTION))
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_SM]))
        .style(theme::track);
    tooltip(row_button, hint, tooltip::Position::Right).gap(theme::SPACE_XS).into()
}

/// One row. `fill` is how selected it is, `arrived` how far into the staggered reveal it is —
/// fading up from the surface colour and sliding the last few pixels into place.
fn contact_row<'a>(contact: &'a Contact, fill: f32, arrived: f32) -> Element<'a, Message> {
    let card = &contact.card;
    let surface = theme::palette().surface;
    let subtitle = card
        .emails
        .iter()
        .chain(card.phones.iter())
        .map(|entry| entry.value.clone())
        .find(|value| !value.is_empty())
        .unwrap_or_default();

    // A row is a fixed height and the selection bar travels on that pitch, so neither line may
    // wrap: a two-line name would push the row past its own bounds and put the bar a row out.
    let name = motion::mix(surface, theme::palette().on_surface, arrived);
    let mut lines = column![
        text(card.display_name())
            .size(theme::FONT_BODY)
            .font(theme::semibold())
            .color(name)
            .wrapping(text::Wrapping::None)
    ]
    .spacing(1);
    if !subtitle.is_empty() {
        let quiet = motion::mix(surface, theme::palette().on_surface_variant, arrived);
        lines = lines.push(text(subtitle).size(theme::FONT_CAPTION).color(quiet).wrapping(text::Wrapping::None));
    }

    // Text clips to the viewport it is given rather than to its own box, so what keeps a long
    // address inside the list is this container, not the width of the column.
    let content = row![
        bar_gutter(),
        avatar(&card.initials(), AVATAR, fill, 1.0, arrived),
        container(lines).width(Length::Fill).clip(true)
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Alignment::Center);

    button(content)
        .width(Length::Fill)
        .height(CONTACT_ROW)
        // The reveal slides the row in from the left on its own padding.
        .padding(Padding {
            top: 0.0,
            right: theme::SPACE_SM,
            bottom: 0.0,
            left: theme::SPACE_SM + motion::lerp(ROW_SLIDE, 0.0, arrived),
        })
        .style(row_style(fill))
        .on_press(Message::Select(contact.id.clone()))
        .into()
}

/// Initials in a circle. Only the selected contact gets the accent; a column of accent-filled
/// circles is the single loudest thing a contact list can do.
///
/// `accent` is a fraction, so a row picking up the selection crossfades into the accent instead of
/// switching. `scale` grows the disc inside a box that keeps its size, so a contact arriving does
/// not shove the name beside it sideways. `fade` blends the whole thing towards the surface, which
/// is this toolkit's stand-in for an opacity it does not have.
fn avatar<'a>(initials: &str, size: f32, accent: f32, scale: f32, fade: f32) -> Element<'a, Message> {
    let palette = theme::palette();
    let accent = accent.clamp(0.0, 1.0);
    let blend = |quiet, loud| motion::mix(palette.surface, motion::mix(quiet, loud, accent), fade);
    let background = blend(palette.surface_variant, palette.primary);
    let foreground = blend(palette.on_surface_variant, palette.on_primary);
    // The outline is what the accent fill replaces. Rather than fading it to nothing, it becomes
    // the fill it sits on — same role, so it disappears without ever being a colour of its own.
    let outline = blend(palette.outline, palette.primary);

    let disc = size * scale;
    let circle = container(text(initials.to_string()).size(disc * 0.34).font(theme::semibold()).color(foreground))
        .width(disc)
        .height(disc)
        .center_x(disc)
        .center_y(disc)
        .style(move |_| container::Style {
            background: Some(background.into()),
            border: Border { color: outline, width: theme::BORDER, ..border::rounded(disc / 2.0) },
            ..container::Style::default()
        });
    container(circle).width(size).height(size).center_x(size).center_y(size).into()
}

/// Rows on one rounded surface, separated by hairlines, the way a contact card reads on paper.
fn group<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut body = column![];
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            body = body.push(hairline_x());
        }
        body = body.push(container(row).width(Length::Fill).padding(theme::CARD_PADDING));
    }
    container(body).width(Length::Fill).style(theme::track).into()
}

/// One line of the card: an icon, the label the vCard gave it, then the value.
///
/// The value is where the long unbroken strings live — `first.last@some.long.department.example`,
/// a URL with a query on it — so it wraps by word and breaks one if it has to. The column takes the
/// width that is left, which is what gives the wrap something to wrap against.
fn field<'a>(glyph: char, label: &str, value: &str) -> Element<'a, Message> {
    let mut lines = column![].spacing(1).width(Length::Fill);
    if !label.is_empty() {
        // Smaller than the value it names, so the two read as label and content rather than as two
        // lines of the same thing.
        lines = lines.push(text(label.to_string()).size(theme::FONT_MINI).color(theme::palette().on_surface_variant));
    }
    lines = lines.push(text(value.to_string()).size(theme::FONT_BODY).wrapping(text::Wrapping::WordOrGlyph));
    row![widgets::icon(glyph, theme::FONT_BODY).color(theme::palette().on_surface_variant), lines]
        .spacing(theme::SPACE_MD)
        .align_y(Alignment::Center)
        .into()
}

fn input<'a>(placeholder: &'a str, value: &str, field: Field) -> Element<'a, Message> {
    field_input(placeholder, value, field).into()
}

fn field_input<'a>(placeholder: &'a str, value: &str, field: Field) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(move |value| Message::Field(field, value))
        .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD]))
        .size(theme::FONT_BODY)
        .style(theme::text_input_style)
}

/// A labelled field that can be focused by name — the one the form opens on.
fn named_input<'a>(id: &'static str, label: &'a str, value: &str, field: Field) -> Element<'a, Message> {
    column![
        text(label).size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
        field_input("", value, field).id(Id::new(id)),
    ]
    .spacing(theme::SPACE_XS)
    .into()
}

/// A button with a glyph before its label.
fn glyph_button<'a>(
    glyph: char,
    label: &'a str,
    height: f32,
    style: impl Fn(&Theme, button::Status) -> button::Style + 'a,
    message: Message,
) -> Element<'a, Message> {
    // `button` does not centre its content: `layout::padded` puts the child at the padding offset
    // and stops. A shrink-height row in a fixed-height button therefore sits against the top, which
    // is what `widgets::action` avoids by filling the height and centring inside it. Same here.
    let content = row![widgets::icon(glyph, theme::FONT_BODY), text(label).size(theme::FONT_BODY)]
        .spacing(theme::SPACE_XS)
        .height(Length::Fill)
        .align_y(Alignment::Center);
    button(content).height(height).padding(Padding::from([0.0, theme::SPACE_MD])).style(style).on_press(message).into()
}

fn danger_button<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(theme::FONT_BODY))
        .height(theme::CONTROL_HEIGHT)
        .padding(Padding::from([0.0, theme::SPACE_MD]))
        // At rest it is the same quiet surface every other button sits on, saying what it is in
        // `error` text; under the pointer it becomes the role, and the text its pair.
        .style(|_, status| {
            let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let palette = theme::palette();
            button::Style {
                background: Some(if hot { palette.error } else { palette.surface_variant }.into()),
                text_color: if hot { palette.on_error } else { palette.error },
                border: border::rounded(theme::RADIUS_MD),
                ..button::Style::default()
            }
        })
        .on_press(message)
        .into()
}

/// A repeating section: a labelled row per entry, plus an add button. `labelled` is false for links,
/// where the vCard type says nothing worth editing.
/// A repeating section: a labelled row per entry, and one way to add another. `labelled` is false
/// for links, where the vCard type says nothing worth editing.
///
/// `noun` is what one row holds, for the add button — "Add email" rather than a bare `+` parked at
/// the far right, which is both easier to hit and the only thing in an empty section to look at.
fn section<'a>(
    title: &'a str,
    noun: &'a str,
    rows: Rows,
    entries: &[Entry],
    value_field: fn(usize) -> Field,
    label_field: fn(usize) -> Field,
    labelled: bool,
) -> Element<'a, Message> {
    let mut body = column![section_header(title)].spacing(theme::SPACE_SM);
    for (index, entry) in entries.iter().enumerate() {
        let mut line = row![].spacing(theme::SPACE_SM).align_y(Alignment::Center);
        if labelled {
            line = line.push(container(input("work", &entry.kind, label_field(index))).width(LABEL_FIELD));
        }
        line = line.push(input(hint(rows), &entry.value, value_field(index)));
        line = line.push(remove_button(rows, index));
        body = body.push(line);
    }
    body = body.push(add_row(noun, rows));
    body.into()
}

/// A section title with a hairline running out to the edge: structure at the cost of one pixel.
fn section_header<'a>(title: &'a str) -> Element<'a, Message> {
    row![caption(title.to_string()), hairline_x()].spacing(theme::SPACE_SM).align_y(Alignment::Center).into()
}

/// What one of these rows should look like once it has something in it.
fn hint(rows: Rows) -> &'static str {
    match rows {
        Rows::Email => "name@example.com",
        Rows::Phone => "+1 555 0100",
        Rows::Url => "https://example.com",
    }
}

/// The empty slot at the end of a section, asking to be filled.
fn add_row<'a>(noun: &'a str, rows: Rows) -> Element<'a, Message> {
    let content =
        row![widgets::icon(icon::PLUS, theme::FONT_CAPTION), text(format!("Add {noun}")).size(theme::FONT_CAPTION)]
            .spacing(theme::SPACE_XS)
            .align_y(Alignment::Center);
    button(content)
        .width(Length::Fill)
        .height(theme::CONTROL_HEIGHT_SM)
        .padding(Padding::from([0.0, theme::SPACE_MD]))
        .style(ghost_button)
        .on_press(Message::Add(rows))
        .into()
}

/// An outline with nothing in it, which warms to the accent under the pointer.
///
/// It stops short of the full accent fill the real actions take — adding a row is not the same
/// order of thing as saving — but the edge and the text both move, so there is no doubt it is live.
fn ghost_button(_: &Theme, status: button::Status) -> button::Style {
    let palette = theme::palette();
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let edge = if hot { palette.primary } else { palette.outline };
    button::Style {
        background: hot.then(|| palette.surface_variant.into()),
        text_color: if hot { palette.primary } else { palette.on_surface_variant },
        border: border::rounded(theme::RADIUS_MD).color(edge).width(theme::BORDER),
        ..button::Style::default()
    }
}

/// Quiet until the pointer is on it, and then unmistakably the destructive one.
fn remove_button<'a>(rows: Rows, index: usize) -> Element<'a, Message> {
    button(widgets::icon(icon::TRASH, theme::FONT_BODY))
        .padding(theme::SPACE_XS)
        .style(|_, status| {
            let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let palette = theme::palette();
            button::Style {
                background: hot.then(|| palette.error.into()),
                text_color: if hot { palette.on_error } else { palette.on_surface_variant },
                border: border::rounded(theme::RADIUS_SM),
                ..button::Style::default()
            }
        })
        .on_press(Message::Remove(rows, index))
        .into()
}

/// An input under its own name.
///
/// The placeholder used to carry the label, which is the oldest bad habit in forms: it leaves a grid
/// of identical empty boxes, and the moment anything is typed the form forgets what it asked for.
fn labelled<'a>(label: &'a str, placeholder: &'a str, value: &str, field: Field) -> Element<'a, Message> {
    column![
        text(label).size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
        input(placeholder, value, field),
    ]
    .spacing(theme::SPACE_XS)
    .into()
}

/// Whether there is anything here worth writing to the address book. `display_name` already falls
/// back through the name to an email, so this is "has a name, an email, or a number".
fn worth_saving(card: &Card) -> bool {
    !card.display_name().is_empty() || card.phones.iter().any(|entry| !entry.value.is_empty())
}

fn placeholder<'a>() -> Element<'a, Message> {
    container(
        column![
            // Decoration at the weight of a rule, so it takes the role rules are drawn in.
            widgets::icon(icon::ADDRESS_BOOK, 32.0).color(theme::palette().outline),
            text("Pick a contact").size(theme::FONT_BODY).color(theme::palette().on_surface_variant),
            // One role for secondary text; the size is what separates the hint from the line above.
            text("or ctrl+N to start a new one").size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
        ]
        .spacing(theme::SPACE_XS)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn waiting<'a>(path: String) -> Element<'a, Message> {
    container(
        column![
            widgets::icon(icon::PLUG, 32.0).color(theme::palette().outline),
            text("Waiting for Thunderbird").size(theme::FONT_TITLE).font(theme::semibold()),
            // A socket path is one unbroken word and can be longer than the window.
            text(format!("listening on {path}"))
                .size(theme::FONT_CAPTION)
                .color(theme::palette().on_surface_variant)
                .wrapping(text::Wrapping::WordOrGlyph),
        ]
        .spacing(theme::SPACE_SM)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

// ── Editing ─────────────────────────────────────────────────────────────────

fn list_mut(card: &mut Card, rows: Rows) -> &mut Vec<Entry> {
    match rows {
        Rows::Email => &mut card.emails,
        Rows::Phone => &mut card.phones,
        Rows::Url => &mut card.urls,
    }
}

fn edit(card: &mut Card, field: Field, value: String) {
    match field {
        // Editing either name component invalidates a stored FN; `to_vcard` regenerates it.
        Field::First => {
            card.name.given = value;
            card.formatted_name.clear();
        }
        Field::Last => {
            card.name.family = value;
            card.formatted_name.clear();
        }
        Field::Nickname => card.nickname = value,
        Field::Organisation => card.organisation = value,
        Field::Role => card.role = value,
        Field::Note => card.note = value,
        Field::Email(index) => set(&mut card.emails, index, |entry| entry.value = value),
        Field::EmailLabel(index) => set(&mut card.emails, index, |entry| entry.kind = value),
        Field::Phone(index) => set(&mut card.phones, index, |entry| entry.value = value),
        Field::PhoneLabel(index) => set(&mut card.phones, index, |entry| entry.kind = value),
        Field::Url(index) => set(&mut card.urls, index, |entry| entry.value = value),
    }
}

fn set(entries: &mut [Entry], index: usize, apply: impl FnOnce(&mut Entry)) {
    if let Some(entry) = entries.get_mut(index) {
        apply(entry);
    }
}

// ── Bridge events as a subscription ─────────────────────────────────────────

/// Identifies the subscription by socket path; the bridge itself is not hashable.
struct Feed(Bridge);

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.path().hash(state);
    }
}

/// The keyboard, for the things there is otherwise only a pointer for.
///
/// Only keys no widget wanted: a focused text input owns its own arrows, and taking them back would
/// stop the cursor moving through what is being typed. The chorded ones are safe either way — a
/// text input has no use for ctrl+n — but they go through the same filter for consistency.
fn keys() -> Subscription<Message> {
    iced::event::listen_with(|event, status, _| {
        let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };
        // Tab is the one key taken whether or not a widget wanted it. iced has no focus traversal
        // of its own, and a focused text input captures Tab — which is precisely the moment the
        // form needs to move on to the next field.
        if matches!(key.as_ref(), keyboard::Key::Named(Named::Tab)) {
            return Some(Message::Traverse(!modifiers.shift()));
        }
        if status != iced::event::Status::Ignored {
            return None;
        }
        match (key.as_ref(), modifiers.command()) {
            (keyboard::Key::Named(Named::ArrowDown), false) => Some(Message::Step(1)),
            (keyboard::Key::Named(Named::ArrowUp), false) => Some(Message::Step(-1)),
            (keyboard::Key::Named(Named::Escape), _) => Some(Message::Escape),
            (keyboard::Key::Character("n"), true) => Some(Message::New),
            (keyboard::Key::Character("e"), true) => Some(Message::Edit),
            (keyboard::Key::Character("s"), true) => Some(Message::Save),
            _ => None,
        }
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

fn feed(feed: &Feed) -> impl iced::futures::Stream<Item = Message> + use<> {
    let events = feed.0.subscribe();
    iced::futures::stream::unfold(events, |mut events| async move {
        events.next().await.map(|event| (Message::Bridge(event), events))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcard::Property;

    fn seeded() -> Card {
        let mut card = vcard::blank();
        card.name.given = "Alice".to_string();
        card.name.family = "Chen".to_string();
        card.formatted_name = "Alice Chen".to_string();
        card.emails = vec![Entry::default(), Entry::default()];
        card.phones = vec![Entry::default()];
        card
    }

    /// The rail only gives up its labels when the card would otherwise be squeezed. The widths
    /// here are the ones this actually runs at, so a change to any of the three constants that
    /// broke one of these cases would be caught rather than noticed.
    #[test]
    fn the_rail_collapses_only_when_a_narrow_window_has_a_card_to_show() {
        // Nothing in the detail pane: the rail keeps its labels however little room there is.
        assert!(!rail_collapses(760.0, false), "no card, nothing to make room for");
        assert!(!rail_collapses(1366.0, false));

        // Tiled beside another window on a 1366 screen, and at the smallest the window can be.
        assert!(rail_collapses(748.0, true), "half a laptop screen has to make room");
        assert!(rail_collapses(WINDOW_MIN.width, true), "the smallest window certainly does");

        // The window it opens at, and the whole of that screen: both have room to spare.
        assert!(!rail_collapses(WINDOW.width, true), "the default window fits all three panes");
        assert!(!rail_collapses(1366.0, true));
    }

    /// Collapsing has to actually buy the card something, or it is just taking the labels away.
    #[test]
    fn collapsing_the_rail_buys_the_card_most_of_what_it_was_missing() {
        let window = 748.0;
        let gutters = 2.0 * theme::BORDER;
        let cramped = window - RAIL_WIDTH - LIST_WIDTH - gutters;
        let roomier = window - RAIL_COLLAPSED - LIST_WIDTH - gutters;
        assert!(cramped < DETAIL_CRAMPED, "this window is the cramped case");
        assert!(roomier > cramped + 150.0, "collapsing gained only {} px", roomier - cramped);
        // Not all the way to comfortable on this window, but past the width the card folds names at.
        assert!(roomier > 340.0, "the card is still squeezed at {roomier} px");
    }

    /// The initial is all that identifies a book once the rail has collapsed, so it has to survive
    /// the names address books actually have.
    #[test]
    fn a_book_is_marked_by_the_first_letter_of_its_name() {
        assert_eq!(initial("Personal Address Book"), "P");
        assert_eq!(initial("work"), "W");
        assert_eq!(initial("  Collected Addresses"), "C");
        assert_eq!(initial("2nd account"), "2");
        assert_eq!(initial("École"), "É");
        // Nothing to take a letter from is still a row, and still needs a mark.
        assert_eq!(initial("— —"), "•");
        assert_eq!(initial(""), "•");
    }

    /// Save is offered only when there is a contact to save. A blank form written through would
    /// become a row with no name, which nobody ever finds again.
    #[test]
    fn an_empty_form_has_nothing_worth_saving() {
        assert!(!worth_saving(&vcard::blank()), "a blank card is not a contact");

        let mut named = vcard::blank();
        named.name.given = "Ada".to_string();
        assert!(worth_saving(&named));

        // `display_name` falls back to an email, so an address on its own counts.
        let mut emailed = vcard::blank();
        emailed.emails[0].value = "ada@example.com".to_string();
        assert!(worth_saving(&emailed));

        // A number does not reach `display_name`, and is still somebody worth writing down.
        let mut called = vcard::blank();
        called.phones[0].value = "+1 555 0100".to_string();
        assert!(worth_saving(&called));
    }

    #[test]
    fn repeating_fields_reach_the_row_their_index_names() {
        let mut card = seeded();
        edit(&mut card, Field::Email(1), "second@example.com".to_string());
        edit(&mut card, Field::EmailLabel(1), "home".to_string());
        edit(&mut card, Field::Phone(0), "+1 555 0100".to_string());

        assert_eq!(card.emails[0].value, "");
        assert_eq!(card.emails[1].value, "second@example.com");
        assert_eq!(card.emails[1].kind, "home");
        assert_eq!(card.phones[0].value, "+1 555 0100");
    }

    #[test]
    fn an_index_past_the_end_is_dropped_rather_than_panicking() {
        let mut card = seeded();
        // A stale message can arrive after its row was removed.
        edit(&mut card, Field::Email(9), "nowhere@example.com".to_string());
        assert_eq!(card.emails.len(), 2);
    }

    #[test]
    fn renaming_clears_the_stored_display_name_so_it_is_regenerated() {
        let mut card = seeded();
        edit(&mut card, Field::Last, "Chen-Okoro".to_string());
        assert_eq!(card.formatted_name, "", "a stale FN would outrank the new name");
        assert_eq!(card.display_name(), "Alice Chen-Okoro");
        assert!(card.to_vcard().contains("FN:Alice Chen-Okoro"));
    }

    #[test]
    fn empty_rows_are_left_out_of_the_saved_vcard() {
        let mut card = seeded();
        edit(&mut card, Field::Email(0), "alice@example.com".to_string());
        // The blank second email and the blank phone are placeholders, not data.
        let saved = card.to_vcard();
        assert_eq!(saved.matches("EMAIL").count(), 1, "{saved}");
        assert!(!saved.contains("TEL"), "{saved}");
    }

    #[test]
    fn adding_and_removing_rows_keeps_the_rest_in_order() {
        let mut card = seeded();
        edit(&mut card, Field::Email(0), "first@example.com".to_string());
        edit(&mut card, Field::Email(1), "second@example.com".to_string());

        list_mut(&mut card, Rows::Email).push(Entry::default());
        edit(&mut card, Field::Email(2), "third@example.com".to_string());
        list_mut(&mut card, Rows::Email).remove(1);

        let values: Vec<&str> = card.emails.iter().map(|entry| entry.value.as_str()).collect();
        assert_eq!(values, ["first@example.com", "third@example.com"]);
    }

    #[test]
    fn editing_never_drops_what_thunderbird_owns() {
        let mut card = seeded();
        card.rest.push(Property { name: "UID".into(), params: Vec::new(), raw: "urn:uuid:1".into(), group: None });
        edit(&mut card, Field::Note, "edited".to_string());
        assert!(card.to_vcard().contains("UID:urn:uuid:1"));
    }
}

#[cfg(test)]
mod swatches {
    use super::*;

    /// Not an assertion — a dump of what every button style resolves to against the palette the
    /// shell is actually running, so hover states can be looked at without a pointer to hover with.
    #[test]
    #[ignore]
    fn print_button_colours() {
        if let Some(live) = crate::palette::load() {
            theme::set_palette(live);
        }
        let theme = theme::noctalia(theme::palette().primary);
        let hex = |c: iced::Color| {
            let b = c.into_rgba8();
            format!("#{:02x}{:02x}{:02x}@{:.2}", b[0], b[1], b[2], c.a)
        };
        let show = |name: &str, style: &dyn Fn(&Theme, button::Status) -> button::Style| {
            for (label, status) in [
                ("rest", button::Status::Active),
                ("hover", button::Status::Hovered),
                ("press", button::Status::Pressed),
            ] {
                let s = style(&theme, status);
                let bg = match s.background {
                    Some(iced::Background::Color(c)) => hex(c),
                    _ => "none".to_string(),
                };
                println!("{name:14} {label:6} bg {bg:20} text {}", hex(s.text_color));
            }
        };
        println!("--- palette ---");
        let p = theme::palette();
        for (n, c) in [
            ("primary", p.primary),
            ("on_primary", p.on_primary),
            ("hover", p.hover),
            ("on_hover", p.on_hover),
            ("surface", p.surface),
            ("surface_variant", p.surface_variant),
            ("on_surface", p.on_surface),
            ("on_surface_variant", p.on_surface_variant),
            ("outline", p.outline),
            ("error", p.error),
            ("on_error", p.on_error),
            ("tertiary", p.tertiary),
            ("on_tertiary", p.on_tertiary),
            ("secondary", p.secondary),
            ("on_secondary", p.on_secondary),
            ("shadow", p.shadow),
        ] {
            println!("{n:20} {}", hex(c));
        }
        println!("--- buttons ---");
        show("Primary(New/Save)", &theme::button_style(ButtonVariant::Primary));
        show("Default(Edit)", &theme::button_style(ButtonVariant::Default));
        show("Tab(Cancel/Del)", &theme::button_style(ButtonVariant::Tab));
        show("ghost(Add)", &ghost_button);
        show("row(selected)", &row_style(1.0));
        show("row(unselected)", &row_style(0.0));
    }
}
