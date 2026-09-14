//! The rolodex: address books on the left, contacts in the middle, one card on the right.

use crate::contacts::{self, AddressBook, Contact};
use crate::palette;
use crate::vcard::{self, Card, Entry};
use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme, border};
use noctalia_iced::chrome;
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

const RAIL_WIDTH: f32 = 200.0;
const LIST_WIDTH: f32 = 300.0;
/// The card reads as a column, not as a banner stretched across the window.
const DETAIL_WIDTH: f32 = 560.0;
const AVATAR: f32 = 36.0;
const AVATAR_LARGE: f32 = 64.0;
const CONTACT_ROW: f32 = 54.0;
const BOOK_ROW: f32 = 36.0;
const INDICATOR: f32 = 3.0;
/// The detail pane wants more air than the theme's largest spacing token.
const SPACE_LG: f32 = 20.0;

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
    /// Only when `NOCTMALIA_FPS` is set: an animation frame, used to measure render cost.
    Frame,
    /// The Noctalia shell's palette changed.
    Palette(Palette),
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
        let mut subscriptions =
            vec![chrome::events().map(Message::Chrome), Subscription::run_with(Feed(self.bridge.clone()), feed)];
        subscriptions.push(Subscription::run(palette_changes));
        if self.frames.as_ref().is_some_and(|frames| frames.drive) {
            subscriptions.push(iced::window::frames().map(|_| Message::Frame));
        }
        Subscription::batch(subscriptions)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Chrome(chrome::Action::Changed(chrome)) => self.chrome = chrome,
            Message::Chrome(action) => return chrome::perform(action).map(Message::Chrome),

            Message::Bridge(Event::Hello(_)) => {
                // Hello also fires when Thunderbird restarts under a live socket, so this is a
                // full resync, not just first contact.
                self.connected = true;
                self.notice = None;
                return self.load_books();
            }
            Message::Bridge(Event::Lost) => {
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
                        self.contacts = contacts;
                        if self.selected.as_ref().is_some_and(|id| !self.has(id)) {
                            self.selected = None;
                        }
                    }
                    Err(error) => self.fail(error),
                }
            }

            Message::SelectBook(book) => {
                self.book = book;
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
            }

            Message::New => {
                let Some(book) = self.writable_book() else {
                    self.fail("No writable address book".to_string());
                    return Task::none();
                };
                self.selected = None;
                self.confirming_delete = false;
                self.editor = Some(Editor { id: None, book, card: vcard::blank() });
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
                }
            }
            Message::Cancel => {
                self.editor = None;
                self.confirming_delete = false;
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
                self.notice = Some(Notice { text: "Saved".to_string(), bad: false });
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
                self.notice = Some(Notice { text: "Deleted".to_string(), bad: false });
                return self.reload();
            }
            Message::Deleted(Err(error)) => self.fail(error),

            Message::Dismiss => self.notice = None,
            // Only arrives under NOCTMALIA_FPS=drive; the counting happens in `view`.
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
        self.notice = Some(Notice { text: error, bad: true });
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
        let body: Element<Message> = if self.connected {
            column![
                row![self.rail(), hairline_y(), self.list(), hairline_y(), self.detail()].height(Length::Fill),
                self.notice(),
            ]
            .into()
        } else {
            waiting(self.bridge.path().display().to_string())
        };
        chrome::frame(self.chrome, "Contacts", body, Message::Chrome)
    }

    fn rail(&self) -> Element<'_, Message> {
        let mut books = column![book_row("All contacts", None, self.book.is_none(), None)].spacing(1);
        for book in &self.books {
            let selected = self.book.as_ref() == Some(&book.id);
            let badge = match (book.read_only, book.remote) {
                (true, _) => Some(icon::LOCK),
                (false, true) => Some(icon::CLOUD),
                (false, false) => None,
            };
            books = books.push(book_row(&book.name, Some(book.id.clone()), selected, badge));
        }

        container(
            column![caption("Address books"), scrollable(books).style(theme::scrollable_style)]
                .spacing(theme::SPACE_SM),
        )
        .width(RAIL_WIDTH)
        .height(Length::Fill)
        .padding(theme::SPACE_MD)
        .into()
    }

    fn list(&self) -> Element<'_, Message> {
        let count = self.contacts.len();
        let header = row![
            caption(if count == 1 { "1 contact".to_string() } else { format!("{count} contacts") }),
            space().width(Length::Fill),
            // Accent is for selection and for Save; a filled accent block here is the loudest
            // thing on screen and it is not the most important one.
            glyph_button(icon::PLUS, "New", ButtonVariant::Default, Message::New),
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
            let mut rows = column![].spacing(1);
            for contact in &self.contacts {
                rows = rows.push(contact_row(contact, self.selected.as_deref() == Some(&contact.id)));
            }
            scrollable(rows).style(theme::scrollable_style).height(Length::Fill).into()
        };

        container(column![header, search, body].spacing(theme::SPACE_SM))
            .width(LIST_WIDTH)
            .height(Length::Fill)
            .padding(theme::SPACE_MD)
            .into()
    }

    fn detail(&self) -> Element<'_, Message> {
        let content: Element<Message> = match (&self.editor, self.selected_contact()) {
            (Some(editor), _) => self.editor_view(editor),
            (None, Some(contact)) => self.card_view(contact),
            (None, None) => placeholder(),
        };
        container(container(content).max_width(DETAIL_WIDTH))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(SPACE_LG)
            .into()
    }

    fn card_view<'a>(&'a self, contact: &'a Contact) -> Element<'a, Message> {
        let card = &contact.card;
        let mut heading =
            column![text(card.display_name()).size(theme::FONT_HEADER).font(theme::semibold())].spacing(2);
        let subtitle: Vec<&str> =
            [card.role.as_str(), card.organisation.as_str()].into_iter().filter(|part| !part.is_empty()).collect();
        if !subtitle.is_empty() {
            heading = heading
                .push(text(subtitle.join(" · ")).size(theme::FONT_BODY).color(theme::palette().on_surface_variant));
        }
        if !card.nickname.is_empty() {
            heading = heading.push(caption(format!("“{}”", card.nickname)));
        }

        let header =
            row![avatar(card, AVATAR_LARGE, true), heading].spacing(theme::SPACE_MD).align_y(Alignment::Center);

        let actions = row![
            glyph_button(icon::PENCIL, "Edit", ButtonVariant::Default, Message::Edit),
            if self.confirming_delete {
                danger_button("Really delete?", Message::Delete)
            } else {
                glyph_button(icon::TRASH, "Delete", ButtonVariant::Tab, Message::AskDelete)
            },
        ]
        .spacing(theme::SPACE_SM);

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

    fn editor_view<'a>(&'a self, editor: &'a Editor) -> Element<'a, Message> {
        let card = &editor.card;
        let title = if editor.id.is_some() { "Edit contact" } else { "New contact" };
        let header = row![
            text(title).size(theme::FONT_TITLE).font(theme::semibold()),
            space().width(Length::Fill),
            widgets::action("Cancel", ButtonVariant::Tab, Some(Message::Cancel)),
            widgets::action("Save", ButtonVariant::Primary, Some(Message::Save)),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

        let who = group(vec![
            row![
                input("First name", &card.name.given, Field::First),
                input("Last name", &card.name.family, Field::Last)
            ]
            .spacing(theme::SPACE_SM)
            .into(),
            row![
                input("Organisation", &card.organisation, Field::Organisation),
                input("Title", &card.role, Field::Role)
            ]
            .spacing(theme::SPACE_SM)
            .into(),
            input("Nickname", &card.nickname, Field::Nickname),
        ]);

        let mut body = column![header, who].spacing(theme::SPACE_MD);
        body = body.push(section("Email", Rows::Email, &card.emails, Field::Email, Field::EmailLabel, true));
        body = body.push(section("Phone", Rows::Phone, &card.phones, Field::Phone, Field::PhoneLabel, true));
        body = body.push(section("Links", Rows::Url, &card.urls, Field::Url, Field::Url, false));
        body = body.push(column![caption("Note"), input("", &card.note, Field::Note)].spacing(theme::SPACE_XS));

        // Addresses are seven components each; the card shows them, but editing them needs a form of
        // its own. Until then, say they are kept rather than letting a save look like it lost them.
        if !card.addresses.is_empty() {
            let kept = format!(
                "{} postal address{} kept unchanged",
                card.addresses.len(),
                if card.addresses.len() == 1 { "" } else { "es" }
            );
            body = body.push(caption(kept));
        }

        scrollable(body.padding(Padding { right: theme::SPACE_SM, ..Padding::ZERO }))
            .style(theme::scrollable_style)
            .height(Length::Fill)
            .into()
    }

    fn notice(&self) -> Element<'_, Message> {
        let Some(notice) = &self.notice else {
            return space().height(0).into();
        };
        let color = if notice.bad { theme::palette().error } else { theme::palette().tertiary };
        let glyph = if notice.bad { icon::ALERT } else { theme::icon::CHECK };
        container(
            row![
                widgets::icon(glyph, theme::FONT_BODY).color(color),
                text(notice.text.clone()).size(theme::FONT_CAPTION),
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
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_MD]))
        .style(move |_| container::Style {
            background: Some(theme::palette().surface_variant.into()),
            border: Border { color: theme::alpha(color, 0.5), width: theme::BORDER, ..Border::default() },
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
    container(space().width(Length::Fill).height(theme::BORDER))
        .style(|_| container::Style {
            background: Some(theme::alpha(theme::palette().outline, 0.7).into()),
            ..container::Style::default()
        })
        .into()
}

/// List rows mark selection with a leading bar and a quiet fill. `ButtonVariant`'s own hover is a
/// full mint block and `Selected` a full accent block — at list length that is a wall of colour.
fn row_style(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = if selected {
            Some(theme::palette().surface_variant.into())
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(theme::alpha(theme::palette().surface_variant, 0.6).into())
        } else {
            None
        };
        button::Style {
            background,
            text_color: theme::palette().on_surface,
            border: border::rounded(theme::RADIUS_MD),
            ..button::Style::default()
        }
    }
}

/// The accent bar down the left of a selected row.
fn indicator<'a>(selected: bool) -> Element<'a, Message> {
    container(space().width(INDICATOR).height(Length::Fill))
        .padding(Padding::from([theme::SPACE_XS, 0.0]))
        .style(move |_| container::Style {
            background: selected.then(|| theme::palette().primary.into()),
            border: border::rounded(INDICATOR),
            ..container::Style::default()
        })
        .into()
}

fn book_row<'a>(label: &str, id: Option<String>, selected: bool, badge: Option<char>) -> Element<'a, Message> {
    let mut line =
        row![text(label.to_string()).size(theme::FONT_BODY)].spacing(theme::SPACE_XS).align_y(Alignment::Center);
    line = line.push(space().width(Length::Fill));
    if let Some(badge) = badge {
        line = line.push(widgets::icon(badge, theme::FONT_CAPTION).color(theme::palette().on_surface_variant));
    }
    button(row![indicator(selected), line].spacing(theme::SPACE_SM).align_y(Alignment::Center))
        .width(Length::Fill)
        .height(BOOK_ROW)
        .padding(Padding::from([0.0, theme::SPACE_SM]))
        .style(row_style(selected))
        .on_press(Message::SelectBook(id))
        .into()
}

fn contact_row<'a>(contact: &'a Contact, selected: bool) -> Element<'a, Message> {
    let card = &contact.card;
    let subtitle = card
        .emails
        .iter()
        .chain(card.phones.iter())
        .map(|entry| entry.value.clone())
        .find(|value| !value.is_empty())
        .unwrap_or_default();

    let mut lines = column![text(card.display_name()).size(theme::FONT_BODY).font(theme::semibold())].spacing(1);
    if !subtitle.is_empty() {
        lines = lines.push(caption(subtitle));
    }

    let content = row![indicator(selected), avatar(card, AVATAR, selected), lines]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

    button(content)
        .width(Length::Fill)
        .height(CONTACT_ROW)
        .padding(Padding::from([0.0, theme::SPACE_SM]))
        .style(row_style(selected))
        .on_press(Message::Select(contact.id.clone()))
        .into()
}

/// Initials in a circle. Only the selected contact gets the accent; a column of accent-filled
/// circles is the single loudest thing a contact list can do.
fn avatar<'a>(card: &Card, size: f32, accent: bool) -> Element<'a, Message> {
    let (background, foreground) = if accent {
        (theme::palette().primary, theme::palette().on_primary)
    } else {
        (theme::palette().surface_variant, theme::palette().on_surface_variant)
    };
    container(text(card.initials()).size(size * 0.34).font(theme::semibold()).color(foreground))
        .width(size)
        .height(size)
        .center_x(size)
        .center_y(size)
        .style(move |_| container::Style {
            background: Some(background.into()),
            border: Border {
                color: if accent { Color::TRANSPARENT } else { theme::palette().outline },
                width: theme::BORDER,
                ..border::rounded(size / 2.0)
            },
            ..container::Style::default()
        })
        .into()
}

/// Rows on one rounded surface, separated by hairlines, the way a contact card reads on paper.
fn group<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut body = column![];
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            body = body.push(hairline_x());
        }
        body = body.push(container(row).width(Length::Fill).padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD])));
    }
    container(body).width(Length::Fill).style(theme::track).into()
}

/// One line of the card: an icon, the label the vCard gave it, then the value.
fn field<'a>(glyph: char, label: &str, value: &str) -> Element<'a, Message> {
    let mut lines = column![].spacing(1);
    if !label.is_empty() {
        lines = lines.push(caption(label.to_string()));
    }
    lines = lines.push(text(value.to_string()).size(theme::FONT_BODY));
    row![widgets::icon(glyph, theme::FONT_BODY).color(theme::palette().on_surface_variant), lines]
        .spacing(theme::SPACE_MD)
        .align_y(Alignment::Center)
        .into()
}

fn input<'a>(placeholder: &'a str, value: &str, field: Field) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(move |value| Message::Field(field, value))
        .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD]))
        .size(theme::FONT_BODY)
        .style(theme::text_input_style)
        .into()
}

/// A button with a glyph before its label.
fn glyph_button<'a>(glyph: char, label: &'a str, variant: ButtonVariant, message: Message) -> Element<'a, Message> {
    let content = row![widgets::icon(glyph, theme::FONT_BODY), text(label).size(theme::FONT_BODY)]
        .spacing(theme::SPACE_XS)
        .align_y(Alignment::Center);
    button(content)
        .height(theme::CONTROL_HEIGHT)
        .padding(Padding::from([0.0, theme::SPACE_MD]))
        .style(theme::button_style(variant))
        .on_press(message)
        .into()
}

fn danger_button<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(theme::FONT_BODY))
        .height(theme::CONTROL_HEIGHT)
        .padding(Padding::from([0.0, theme::SPACE_MD]))
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered | button::Status::Pressed => theme::palette().error,
                    _ => theme::alpha(theme::palette().error, 0.18),
                }
                .into(),
            ),
            text_color: match status {
                button::Status::Hovered | button::Status::Pressed => theme::palette().surface,
                _ => theme::palette().error,
            },
            border: border::rounded(theme::RADIUS_MD),
            ..button::Style::default()
        })
        .on_press(message)
        .into()
}

/// A repeating section: a labelled row per entry, plus an add button. `labelled` is false for links,
/// where the vCard type says nothing worth editing.
fn section<'a>(
    title: &'a str,
    rows: Rows,
    entries: &[Entry],
    value_field: fn(usize) -> Field,
    label_field: fn(usize) -> Field,
    labelled: bool,
) -> Element<'a, Message> {
    let header = row![
        caption(title),
        space().width(Length::Fill),
        button(widgets::icon(icon::PLUS, theme::FONT_BODY).color(theme::palette().primary))
            .style(theme::bare_button)
            .padding(theme::SPACE_XS)
            .on_press(Message::Add(rows)),
    ]
    .align_y(Alignment::Center);

    let mut body = column![header].spacing(theme::SPACE_XS);
    for (index, entry) in entries.iter().enumerate() {
        let mut line = row![].spacing(theme::SPACE_SM).align_y(Alignment::Center);
        if labelled {
            line = line.push(container(input("label", &entry.kind, label_field(index))).width(88));
        }
        line = line.push(input(title, &entry.value, value_field(index)));
        line = line.push(
            button(widgets::icon(icon::TRASH, theme::FONT_BODY).color(theme::palette().on_surface_variant))
                .style(theme::bare_button)
                .padding(theme::SPACE_XS)
                .on_press(Message::Remove(rows, index)),
        );
        body = body.push(line);
    }
    body.into()
}

fn placeholder<'a>() -> Element<'a, Message> {
    container(
        column![
            widgets::icon(icon::ADDRESS_BOOK, 32.0).color(theme::alpha(theme::palette().on_surface_variant, 0.4)),
            caption("Pick a contact"),
        ]
        .spacing(theme::SPACE_SM)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn waiting<'a>(path: String) -> Element<'a, Message> {
    container(
        column![
            widgets::icon(icon::PLUG, 32.0).color(theme::alpha(theme::palette().on_surface_variant, 0.4)),
            text("Waiting for Thunderbird").size(theme::FONT_TITLE).font(theme::semibold()),
            caption(format!("listening on {path}")),
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
