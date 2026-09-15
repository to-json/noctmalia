//! Mail: folders on the left, the index in the middle, one letter on the right.
//!
//! The interaction model is mutt's rather than vim's — three focus regions, one modal keymap per
//! region, and no insert mode outside a text widget, because iced reports a key a focused input
//! consumed as captured and the application never sees it.
//!
//! Three things about this surface are decisions rather than implementation, and are worth finding
//! here rather than in a document:
//!
//! - **Every letter is Markdown by the time it reaches the screen** ([`crate::mime`]). Plain,
//!   Markdown and HTML all take the same road, which is what makes "we load nothing from the
//!   internet" structural instead of a policy: there is no image fetch anywhere in the pipeline, so
//!   remote content *cannot* load. Not "is blocked". The security panel names what a sender wanted
//!   fetched, and `\` shows the source it was asked for in.
//! - **Nothing is stored outside Thunderbird.** Read, flagged and tags are Thunderbird's and go
//!   back to IMAP. Threads are Gloda's `conversationID`. Search is Gloda's index. Rules are
//!   `msgFilterRules.dat`. Even undo keeps no state worth the name: a numeric message id belongs to
//!   wherever the message currently is, so undo remembers `headerMessageId` strings and asks
//!   Thunderbird where they went.
//! - **The index is windowed** ([`noctalia_iced::list`]). iced lays out everything inside a
//!   `scrollable`, and a fifty-thousand-message folder is the one place a column of rows does not
//!   survive.

use crate::mail::{self, Account, Counts, Draft, Flags, Folder, Header, Identity, Letter, Reply, Screen};
use crate::mime;
use crate::shell::Shell;
use crate::surfaces::{self, Pressed, Surface};
use crate::ui::{self, ROW_GAP, icon};
use iced::advanced::widget::Id;
use iced::keyboard::{Key, Modifiers};
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::{
    column, container, markdown, operation, row, scrollable, space, stack, text, text_editor, text_input,
};
use iced::{Alignment, Color, Element, Length, Padding, Task};
use noctalia_iced::keymap::{self, Keymap};
use noctalia_iced::list;
use noctalia_iced::motion::{self, Replay};
use noctalia_iced::theme::{self, ButtonVariant};
use noctalia_iced::widgets;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Wide enough for "Personal Address Book"-length folder names under an account heading.
const RAIL_WIDTH: f32 = 216.0;
/// The rail with its labels gone: the selection bar, one glyph per folder, and nothing else.
const RAIL_COLLAPSED: f32 = 56.0;
const INDEX_WIDTH: f32 = 352.0;
/// Under this much room for the letter, the rail gives up its labels.
const PAGER_CRAMPED: f32 = 440.0;
/// The letter reads as a column, not as a line stretched across a wide screen.
const PAGER_WIDTH: f32 = 720.0;

const MESSAGE_ROW: f32 = 56.0;
const MESSAGE_PITCH: f32 = MESSAGE_ROW + ROW_GAP;
const FOLDER_ROW: f32 = 30.0;
const FOLDER_PITCH: f32 = FOLDER_ROW + ROW_GAP;
/// How far a folder's name is pushed in per level of nesting.
const NEST: f32 = 12.0;

const INDEX_ID: &str = "mail-index";
const RAIL_ID: &str = "mail-rail";
const PAGER_ID: &str = "mail-pager";
const SEARCH_ID: &str = "mail-search";
const COMPOSE_TO: &str = "compose-to";

/// How far below home the letter starts when what it shows changes.
const PAGER_RISE: f32 = 12.0;
/// How much of the pager's height a `<Space>` moves.
const PAGE_FRACTION: f32 = 0.9;
/// How far left of home a row starts in the staggered reveal.
const ROW_SLIDE: f32 = 10.0;
/// Rows past this one skip the stagger: they are below the fold, and the tail of a long list
/// arriving one row at a time is a wait, not a flourish.
const STAGGER_ROWS: usize = 14;
/// Each row starts this much of the reveal after the one above it, and takes this long to arrive.
const STAGGER_STEP: f32 = 0.045;
const STAGGER_WINDOW: f32 = 0.35;

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    Accounts(mail::Result<Vec<Account>>),
    Identities(mail::Result<Vec<Identity>>),
    Counts(mail::Result<(String, Counts)>),
    OpenFolder(String),
    /// A page of the index, tagged with the generation that asked for it so a folder the user has
    /// already left cannot fill the list.
    Page(u64, mail::Result<mail::Page>),
    Select(u64),
    /// The letter, tagged with the message it is for.
    Letter(u64, mail::Result<Letter>),
    Conversations(u64, mail::Result<Vec<mail::Conversation>>),
    /// Open or close the selected row's thread.
    Fold,
    /// Open or close one named thread, from a click on its chevron.
    FoldAt(i64),
    Step(i32),
    Edge(bool),
    Scroll(f32),
    Focus(Region),
    Query(String),
    /// Put the cursor in the search box.
    Search,
    /// Run the query against Gloda's index rather than against what is loaded.
    Deep,
    Searched(u64, mail::Result<Vec<Header>>),
    Mark,
    Flag,
    Unread,
    NextUnread,
    Archive,
    Discard,
    Undo,
    Undone(mail::Result<usize>),
    Acted(&'static str, mail::Result<()>),
    Show(Showing),
    Raw(u64, mail::Result<String>),
    /// Fetch an attachment and write it where the desktop puts downloads.
    Save(String),
    Saved(mail::Result<String>),
    OpenLink(String),
    /// Hand the whole message to whatever the desktop opens `.eml` with.
    External,
    Compose(Option<Reply>),
    Field(Field, String),
    Body(text_editor::Action),
    Identity(String),
    Send,
    Draft,
    Sent(mail::Result<String>),
    Screen,
    ScreenFolder(String),
    ScreenGo,
    Screened(mail::Result<String>),
    Rules(mail::Result<Vec<mail::Rule>>),
    Tidied(mail::Result<usize>),
    Cancel,
    Escape,
    Refresh,
    IndexScrolled(Viewport),
    RailScrolled(Viewport),
    PagerScrolled(Viewport),
}

/// Which of the three regions has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Rail,
    Index,
    Pager,
}

/// What the right-hand pane is showing about the selected message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    Letter,
    /// Every header, which is what "we highlight funny looking headers" needs somewhere to point
    /// at.
    Headers,
    /// What the envelope gives away, and what the sender wanted fetched.
    Security,
    /// The bytes exactly as they arrived.
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    To,
    Cc,
    Subject,
}

// ── Keys ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    Down,
    Up,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    Open,
    Back,
    Fold,
    Mark,
    Flag,
    Unread,
    NextUnread,
    Archive,
    Discard,
    Undo,
    Compose,
    Reply,
    ReplyAll,
    Forward,
    Search,
    Source,
    Headers,
    Security,
    Letter,
    OpenElsewhere,
    Screen,
    Refresh,
    Escape,
    Go(Surface),
    Folder(&'static str),
}

/// The bindings every region shares, so `gm`, `<Esc>` and `<C-r>` mean the same thing wherever the
/// keyboard happens to be.
fn common(keys: Keymap<Binding>) -> Keymap<Binding> {
    surfaces::switches(
        keys.bind("gi", Binding::Folder("inbox"))
            .bind("gs", Binding::Folder("sent"))
            .bind("gd", Binding::Folder("drafts"))
            .bind("ga", Binding::Folder("archives"))
            .bind("gt", Binding::Folder("trash"))
            .bind("<C-r>", Binding::Refresh)
            .bind("<Esc>", Binding::Escape),
        Binding::Go,
    )
}

thread_local! {
    /// The index: where triage happens, and so where nearly every verb lives.
    static INDEX_KEYS: Keymap<Binding> = common(
        Keymap::new()
            .counted()
            .bind("j", Binding::Down)
            .bind("k", Binding::Up)
            .bind("<Down>", Binding::Down)
            .bind("<Up>", Binding::Up)
            .bind("gg", Binding::Top)
            .bind("G", Binding::Bottom)
            .bind("<C-d>", Binding::HalfDown)
            .bind("<C-u>", Binding::HalfUp)
            .bind("<CR>", Binding::Open)
            .bind("l", Binding::Open)
            .bind("<Right>", Binding::Open)
            .bind("h", Binding::Back)
            .bind("<Left>", Binding::Back)
            .bind("<Space>", Binding::Fold)
            .bind("x", Binding::Mark)
            .bind("s", Binding::Flag)
            .bind("N", Binding::Unread)
            .bind("n", Binding::NextUnread)
            .bind("e", Binding::Archive)
            .bind("d", Binding::Discard)
            .bind("u", Binding::Undo)
            .bind("m", Binding::Compose)
            .bind("r", Binding::Reply)
            .bind("R", Binding::ReplyAll)
            .bind("f", Binding::Forward)
            .bind("/", Binding::Search)
            .bind("\\", Binding::Source)
            .bind("H", Binding::Headers)
            .bind("!", Binding::Security)
            .bind("S", Binding::Screen)
            .bind("O", Binding::OpenElsewhere),
    );

    /// The pager: reading, and the handful of things you do to something you have just read.
    static PAGER_KEYS: Keymap<Binding> = common(
        Keymap::new()
            .counted()
            .bind("j", Binding::Down)
            .bind("k", Binding::Up)
            .bind("<Down>", Binding::Down)
            .bind("<Up>", Binding::Up)
            .bind("<Space>", Binding::PageDown)
            .bind("b", Binding::PageUp)
            .bind("<C-d>", Binding::HalfDown)
            .bind("<C-u>", Binding::HalfUp)
            .bind("gg", Binding::Top)
            .bind("G", Binding::Bottom)
            .bind("q", Binding::Back)
            .bind("h", Binding::Back)
            .bind("<Left>", Binding::Back)
            .bind("r", Binding::Reply)
            .bind("R", Binding::ReplyAll)
            .bind("f", Binding::Forward)
            .bind("e", Binding::Archive)
            .bind("d", Binding::Discard)
            .bind("s", Binding::Flag)
            .bind("u", Binding::Undo)
            .bind("m", Binding::Compose)
            .bind("\\", Binding::Source)
            .bind("H", Binding::Headers)
            .bind("!", Binding::Security)
            .bind("v", Binding::Letter)
            .bind("S", Binding::Screen)
            .bind("O", Binding::OpenElsewhere),
    );

    /// The rail: pick a folder and leave.
    static RAIL_KEYS: Keymap<Binding> = common(
        Keymap::new()
            .counted()
            .bind("j", Binding::Down)
            .bind("k", Binding::Up)
            .bind("<Down>", Binding::Down)
            .bind("<Up>", Binding::Up)
            .bind("gg", Binding::Top)
            .bind("G", Binding::Bottom)
            .bind("<CR>", Binding::Open)
            .bind("l", Binding::Open)
            .bind("<Right>", Binding::Open),
    );
}

// ── State ───────────────────────────────────────────────────────────────────

/// One row of the index: a message, and where it sits in its thread.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// Which of `headers` this row is.
    at: usize,
    /// 0 for a message on its own or the head of a thread, 1 for a reply under it.
    depth: usize,
    /// The conversation, when Gloda knows one.
    thread: Option<i64>,
    /// How many messages the thread holds, on its head row. 1 everywhere else.
    held: usize,
}

/// One line of the folder rail.
///
/// Accounts are in here rather than drawn around the folders because the selection bar travels on
/// a fixed pitch: a heading that were not a row would put the bar a row out. It is a row that
/// cannot be selected, which the keyboard steps over.
#[derive(Debug, Clone)]
enum Perch {
    /// An account's name, above its folders. Only when there is more than one account — a single
    /// account's name is a heading over the whole rail, which is no information at all.
    Account(String),
    /// A folder, and how deep it is nested.
    Folder(usize, Folder),
}

impl Perch {
    fn folder(&self) -> Option<&Folder> {
        match self {
            Perch::Folder(_, folder) => Some(folder),
            Perch::Account(_) => None,
        }
    }
}

/// What an undo would put back.
///
/// Two strings and a folder is the whole of it, which is the "no database" rule paying for itself:
/// a numeric message id belongs to wherever the message currently is, so undoing a move cannot use
/// one. `headerMessageId` does not change, and [`mail::restore`] asks Thunderbird where it went.
struct Undo {
    message_ids: Vec<String>,
    folder: String,
}

struct Composing {
    draft: Draft,
    body: text_editor::Content,
    /// A reply carries what it is replying to, so the compose pane can say so.
    about: Option<String>,
}

struct Motion {
    message: iced::Animation<f32>,
    message_shown: iced::Animation<bool>,
    folder: iced::Animation<f32>,
    pager: Replay,
    reveal: Replay,
    collapse: iced::Animation<bool>,
}

impl Motion {
    fn new() -> Motion {
        Motion {
            message: motion::spring_animation(0.0),
            message_shown: motion::glide_animation(false),
            folder: motion::spring_animation(0.0),
            pager: Replay::settled(motion::SETTLE, motion::NORMAL),
            reveal: Replay::settled(motion::GLIDE, motion::SLOW),
            collapse: motion::settle_animation(false),
        }
    }

    fn animating(&self, now: Instant) -> bool {
        self.message.is_animating(now)
            || self.message_shown.is_animating(now)
            || self.folder.is_animating(now)
            || self.pager.is_animating(now)
            || self.reveal.is_animating(now)
            || self.collapse.is_animating(now)
    }
}

pub struct Mail {
    accounts: Vec<Account>,
    /// The rail: account headings and folders, in the order they are drawn.
    rail: Vec<Perch>,
    counts: HashMap<String, Counts>,
    identities: Vec<Identity>,
    folder: Option<String>,

    /// Bumped whenever the index starts over, so a page from a folder already left is dropped.
    generation: u64,
    listing: bool,
    headers: Vec<Header>,
    rows: Vec<Row>,
    /// `headerMessageId` to the conversation Gloda put it in.
    threads: HashMap<String, i64>,
    /// The threads the reader has opened. A thread is a row until it is asked to be more.
    opened: HashSet<i64>,

    selected: Option<u64>,
    marked: HashSet<u64>,
    letters: HashMap<u64, Letter>,
    /// The selected letter, parsed. Only ever one: parsing is cheap and holding a document per
    /// message is not.
    parsed: Option<(u64, markdown::Content)>,
    source: Option<(u64, String)>,
    showing: Showing,
    waiting: bool,

    query: String,
    deep: bool,
    undo: Option<Undo>,
    composing: Option<Composing>,
    /// A rule proposed from the message in front of you, waiting for somewhere to put the mail.
    screening: Option<Screen>,
    /// The rules this account already has, shown beside the proposal so a second one for the same
    /// sender is obvious before it is made.
    rules: Vec<mail::Rule>,

    region: Region,
    pending: keymap::Pending,
    index_view: Option<Viewport>,
    pager_view: Option<Viewport>,
    rail_view: Option<Viewport>,
    motion: Motion,
}

impl Default for Mail {
    fn default() -> Mail {
        Mail::new()
    }
}

impl Mail {
    pub fn new() -> Mail {
        Mail {
            accounts: Vec::new(),
            rail: Vec::new(),
            counts: HashMap::new(),
            identities: Vec::new(),
            folder: None,
            generation: 0,
            listing: false,
            headers: Vec::new(),
            rows: Vec::new(),
            threads: HashMap::new(),
            opened: HashSet::new(),
            selected: None,
            marked: HashSet::new(),
            letters: HashMap::new(),
            parsed: None,
            source: None,
            showing: Showing::Letter,
            waiting: false,
            query: String::new(),
            deep: false,
            undo: None,
            composing: None,
            screening: None,
            rules: Vec::new(),
            region: Region::Index,
            pending: keymap::Pending::default(),
            index_view: None,
            pager_view: None,
            rail_view: None,
            motion: Motion::new(),
        }
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.motion.animating(now)
    }

    pub fn typed(&self) -> String {
        self.pending.typed()
    }

    /// Which region has the keyboard. For tests, and for anything that wants to say so.
    pub fn region(&self) -> Region {
        self.region
    }

    pub fn entered(&mut self, now: Instant) {
        self.motion.reveal.restart(now);
        self.motion.pager.restart(now);
        self.pending.clear();
    }

    /// Everything again, from nothing.
    pub fn resync(&mut self, shell: &Shell) -> Task<Message> {
        Task::batch([
            Task::perform(mail::accounts(shell.bridge()), Message::Accounts),
            Task::perform(mail::identities(shell.bridge()), Message::Identities),
        ])
    }

    /// The keybound actions worth finding by name — `docs/command-palette-plan.md` §3.2. Pure
    /// cursor movement and pane focus (`Binding::Down`/`Up`/`Top`/`Bottom`/`HalfDown`/`HalfUp`/
    /// `PageDown`/`PageUp`/`Open`/`Back`/`Fold`) is left out: it exists to be repeated or held,
    /// not looked up by name. `Go to <folder>` only appears for a folder this account actually
    /// has.
    pub fn commands(&self) -> Vec<crate::commands::Entry<Message>> {
        use crate::commands::Entry;
        let mut entries = vec![
            Entry::new("Mark", Some("x"), Message::Mark),
            Entry::new("Flag", Some("s"), Message::Flag),
            Entry::new("Mark unread", Some("N"), Message::Unread),
            Entry::new("Next unread", Some("n"), Message::NextUnread),
            Entry::new("Archive", Some("e"), Message::Archive),
            Entry::new("Delete", Some("d"), Message::Discard),
            Entry::new("Undo", Some("u"), Message::Undo),
            Entry::new("Compose", Some("m"), Message::Compose(None)),
            Entry::new("Reply", Some("r"), Message::Compose(Some(Reply::Sender))),
            Entry::new("Reply all", Some("R"), Message::Compose(Some(Reply::All))),
            Entry::new("Forward", Some("f"), Message::Compose(Some(Reply::Forward))),
            Entry::new("Search", Some("/"), Message::Search),
            Entry::new("Show raw source", Some("\\"), Message::Show(Showing::Source)),
            Entry::new("Show headers", Some("H"), Message::Show(Showing::Headers)),
            Entry::new("Show security surface", Some("!"), Message::Show(Showing::Security)),
            Entry::new("Open elsewhere", Some("O"), Message::External),
            Entry::new("Propose a screening rule", Some("S"), Message::Screen),
            Entry::new("Refresh", Some("<C-r>"), Message::Refresh),
        ];
        for (label, purpose, hint) in [
            ("Go to Inbox", "inbox", "g i"),
            ("Go to Sent", "sent", "g s"),
            ("Go to Drafts", "drafts", "g d"),
            ("Go to Archives", "archives", "g a"),
            ("Go to Trash", "trash", "g t"),
        ] {
            if let Some(id) = self.folder_for(purpose) {
                entries.push(Entry::new(label, Some(hint), Message::OpenFolder(id)));
            }
        }
        entries
    }

    /// The currently loaded rows, as jump targets for quick-open —
    /// `docs/command-palette-plan.md` §4.2. Only what has already paged in; a folder's own `/`
    /// filter is still the way to reach a message quick-open hasn't loaded yet.
    pub fn quick_items(&self) -> Vec<crate::commands::Entry<Message>> {
        self.headers
            .iter()
            .map(|header| {
                let label = if header.author.is_empty() {
                    header.subject.clone()
                } else {
                    format!("{} — {}", header.subject, header.author)
                };
                crate::commands::Entry::new(label, None, Message::Select(header.id))
            })
            .collect()
    }

    /// A forwarded Thunderbird event.
    ///
    /// New mail and a folder's counts changing are worth acting on; a message being updated by us
    /// is not, because we already drew the change. Reloading the whole index on every `onUpdated`
    /// would make marking fifty messages read fifty full listings.
    pub fn notify(&mut self, name: &str, data: &Value, shell: &Shell) -> Task<Message> {
        match name {
            "messages.onNewMailReceived" => {
                let arrived = data.get("folder").and_then(|folder| folder.get("id")).and_then(Value::as_str);
                let mut tasks = vec![self.refresh_counts(shell)];
                if arrived.is_some() && arrived == self.folder.as_deref() {
                    tasks.push(self.reload(shell));
                }
                Task::batch(tasks)
            }
            "folders.onFolderInfoChanged" => self.refresh_counts(shell),
            "folders.onCreated" | "folders.onDeleted" | "folders.onRenamed" | "accounts.onCreated"
            | "accounts.onDeleted" => Task::perform(mail::accounts(shell.bridge()), Message::Accounts),
            // A move or a delete performed elsewhere genuinely changes what is in front of us.
            "messages.onMoved" | "messages.onDeleted" | "messages.onCopied" => {
                Task::batch([self.refresh_counts(shell), self.reload(shell)])
            }
            _ => Task::none(),
        }
    }

    /// Every folder's counts. One round trip per folder, so this is for the moments when any of
    /// them could have changed: mail arriving, a move, a delete, a resync.
    fn refresh_counts(&self, shell: &Shell) -> Task<Message> {
        Task::batch(
            self.folders()
                .map(|folder| Task::perform(mail::counts(shell.bridge(), folder.id.clone()), Message::Counts)),
        )
    }

    /// Every folder in the rail, headings skipped.
    fn folders(&self) -> impl Iterator<Item = &Folder> {
        self.rail.iter().filter_map(Perch::folder)
    }

    /// Only the open folder's counts, which is all that reading a message can change. Marking
    /// fifty messages read should not be fifty times six round trips.
    fn refresh_open_count(&self, shell: &Shell) -> Task<Message> {
        match self.folder.clone() {
            Some(folder) => Task::perform(mail::counts(shell.bridge(), folder), Message::Counts),
            None => Task::none(),
        }
    }

    /// One key press, against the table for whichever region has the keyboard.
    pub fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Message> {
        // While the composer is open the keyboard belongs to it, except for the way out.
        if self.composing.is_some() {
            return match key.as_ref() {
                Key::Named(iced::keyboard::key::Named::Escape) => Pressed::Act(Message::Cancel),
                Key::Character("\r") | Key::Named(iced::keyboard::key::Named::Enter) if modifiers.command() => {
                    Pressed::Act(Message::Send)
                }
                Key::Character("s") if modifiers.command() => Pressed::Act(Message::Draft),
                _ => Pressed::Ignored,
            };
        }
        let table = match self.region {
            Region::Rail => &RAIL_KEYS,
            Region::Index => &INDEX_KEYS,
            Region::Pager => &PAGER_KEYS,
        };
        let resolved = table.with(|keys| keys.press(&mut self.pending, key, modifiers));
        let (binding, count) = match resolved {
            keymap::Resolved::Ignored => return Pressed::Ignored,
            keymap::Resolved::Pending => return Pressed::Pending,
            keymap::Resolved::Action(binding, count) => (binding, count),
        };
        let count = count as i32;
        Pressed::Act(match binding {
            Binding::Go(surface) => return Pressed::Switch(surface),
            Binding::Down => Message::Step(count),
            Binding::Up => Message::Step(-count),
            Binding::Top => Message::Edge(false),
            Binding::Bottom => Message::Edge(true),
            Binding::HalfDown => self.by_page(0.5 * count as f32),
            Binding::HalfUp => self.by_page(-0.5 * count as f32),
            Binding::PageDown => self.by_page(PAGE_FRACTION * count as f32),
            Binding::PageUp => self.by_page(-PAGE_FRACTION * count as f32),
            Binding::Open => match self.region {
                Region::Rail => Message::Focus(Region::Index),
                _ => Message::Focus(Region::Pager),
            },
            Binding::Back => match self.region {
                Region::Pager => Message::Focus(Region::Index),
                _ => Message::Focus(Region::Rail),
            },
            Binding::Fold => Message::Fold,
            Binding::Mark => Message::Mark,
            Binding::Flag => Message::Flag,
            Binding::Unread => Message::Unread,
            Binding::NextUnread => Message::NextUnread,
            Binding::Archive => Message::Archive,
            Binding::Discard => Message::Discard,
            Binding::Undo => Message::Undo,
            Binding::Compose => Message::Compose(None),
            Binding::Reply => Message::Compose(Some(Reply::Sender)),
            Binding::ReplyAll => Message::Compose(Some(Reply::All)),
            Binding::Forward => Message::Compose(Some(Reply::Forward)),
            Binding::Search => Message::Search,
            Binding::Source => Message::Show(Showing::Source),
            Binding::Headers => Message::Show(Showing::Headers),
            Binding::Security => Message::Show(Showing::Security),
            Binding::Letter => Message::Show(Showing::Letter),
            Binding::OpenElsewhere => Message::External,
            Binding::Screen => Message::Screen,
            Binding::Refresh => Message::Refresh,
            Binding::Escape => Message::Escape,
            Binding::Folder(purpose) => match self.folder_for(purpose) {
                Some(id) => Message::OpenFolder(id),
                None => return Pressed::Ignored,
            },
        })
    }

    /// Scrolling by pages means different things in the two regions: rows in the index, pixels in
    /// the pager.
    fn by_page(&self, fraction: f32) -> Message {
        match self.region {
            Region::Pager => {
                let height = self.pager_view.map_or(600.0, |view| view.bounds().height);
                Message::Scroll(height * fraction)
            }
            _ => {
                let height = self.index_view.map_or(600.0, |view| view.bounds().height);
                let rows = (list::page(MESSAGE_PITCH, height) as f32 * fraction).round() as i32;
                Message::Step(rows.clamp(-999, 999))
            }
        }
    }

    fn folder_for(&self, purpose: &str) -> Option<String> {
        self.folders().find(|folder| folder.purpose() == purpose).map(|folder| folder.id.clone())
    }
}

// ── Update ──────────────────────────────────────────────────────────────────

impl Mail {
    pub fn update(&mut self, message: Message, shell: &mut Shell, now: Instant) -> Task<Message> {
        let task = self.step(message, shell, now);
        // Almost anything can have opened or closed the letter, or changed how much room there is
        // for it, and `step` returns from a dozen places.
        self.motion.collapse.go_mut(self.rail_collapsed(shell.width()), now);
        task
    }

    fn step(&mut self, message: Message, shell: &mut Shell, now: Instant) -> Task<Message> {
        match message {
            Message::Accounts(Ok(accounts)) => {
                self.accounts = accounts;
                self.rail = flatten(&self.accounts);
                let open = self.folder.clone().filter(|id| self.folders().any(|folder| &folder.id == id));
                self.folder = open.or_else(|| self.folder_for("inbox"));
                self.motion.folder = motion::spring_animation(self.folder_row() as f32);
                return Task::batch([self.refresh_counts(shell), self.reload(shell)]);
            }
            Message::Accounts(Err(error)) => shell.fail(error, now),
            Message::Identities(Ok(identities)) => self.identities = identities,
            Message::Identities(Err(error)) => shell.fail(error, now),
            Message::Counts(Ok((folder, counts))) => {
                self.counts.insert(folder, counts);
            }
            Message::Counts(Err(_)) => {}

            Message::OpenFolder(id) => {
                if self.folder.as_deref() != Some(id.as_str()) {
                    self.folder = Some(id);
                    self.motion.folder.go_mut(self.folder_row() as f32, now);
                    self.query.clear();
                    self.deep = false;
                    return Task::batch([self.reload(shell), self.follow_rail()]);
                }
            }

            Message::Page(generation, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                match result {
                    Ok(page) => {
                        let first = self.headers.is_empty();
                        self.headers.extend(page.messages);
                        self.headers.sort_by_key(|header| std::cmp::Reverse(header.when()));
                        self.rebuild(now);
                        if first {
                            self.motion.reveal.restart(now);
                        }
                        match page.id.filter(|_| self.headers.len() < mail::LIST_CAP) {
                            Some(list_id) => {
                                let bridge = shell.bridge();
                                return Task::perform(mail::page(bridge, list_id), move |result| {
                                    Message::Page(generation, result)
                                });
                            }
                            None => {
                                self.listing = false;
                                return self.ask_for_threads(shell);
                            }
                        }
                    }
                    Err(error) => {
                        self.listing = false;
                        shell.fail(error, now);
                    }
                }
            }

            Message::Conversations(generation, Ok(conversations)) => {
                if generation != self.generation {
                    return Task::none();
                }
                for conversation in conversations {
                    for message_id in conversation.messages {
                        self.threads.insert(message_id, conversation.id);
                    }
                }
                self.rebuild(now);
            }
            // Gloda indexes asynchronously and its schema is Thunderbird's private business, so a
            // thread that cannot be worked out is a flat list rather than an error in the user's
            // face. Say it once, on stderr, where it belongs.
            Message::Conversations(_, Err(error)) => eprintln!("noctmalia: no threads from Gloda: {error}"),

            Message::Select(id) => return self.select(id, shell, now),
            Message::Letter(id, Ok(letter)) => {
                self.waiting = false;
                if self.selected == Some(id) {
                    self.parsed = Some((id, markdown::Content::parse(&letter.body.markdown)));
                    self.motion.pager.restart(now);
                }
                self.letters.insert(id, letter);
            }
            Message::Letter(_, Err(error)) => {
                self.waiting = false;
                shell.fail(error, now);
            }

            Message::Fold => {
                if let Some(thread) = self.selected_row().and_then(|row| self.rows[row].thread) {
                    return self.step(Message::FoldAt(thread), shell, now);
                }
            }
            Message::FoldAt(thread) => {
                if !self.opened.remove(&thread) {
                    self.opened.insert(thread);
                }
                self.rebuild(now);
                return self.follow_index();
            }

            Message::Step(delta) => return self.move_selection(delta, shell, now),
            Message::Edge(last) => {
                let target = if last { self.rows.len() as i32 } else { -(self.rows.len() as i32) };
                return self.move_selection(target, shell, now);
            }
            Message::Scroll(pixels) => {
                let offset = self.pager_view.map_or(0.0, |view| view.absolute_offset().y);
                return operation::scroll_to(
                    Id::new(PAGER_ID),
                    AbsoluteOffset { x: 0.0, y: (offset + pixels).max(0.0) },
                );
            }
            Message::Focus(region) => {
                self.region = region;
                self.pending.clear();
                // Walking right off the index opens what is under the cursor, which is what makes
                // `l` mean the same thing in both places.
                if region == Region::Pager && self.selected.is_none() {
                    return self.move_selection(1, shell, now);
                }
            }

            Message::Query(query) => {
                self.query = query;
                if self.deep {
                    // A deep search is a set of results, not a filter; typing starts a new one.
                    self.deep = false;
                    return self.reload(shell);
                }
                self.rebuild(now);
            }
            Message::Search => return operation::focus(Id::new(SEARCH_ID)),
            Message::Deep => {
                let text = self.query.trim().to_string();
                if text.is_empty() {
                    return Task::none();
                }
                self.generation += 1;
                let generation = self.generation;
                self.listing = true;
                return Task::perform(mail::search(shell.bridge(), text, 500), move |result| {
                    Message::Searched(generation, result)
                });
            }
            Message::Searched(generation, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                self.listing = false;
                match result {
                    Ok(headers) => {
                        self.deep = true;
                        self.headers = headers;
                        self.rebuild(now);
                        self.motion.reveal.restart(now);
                        let found = self.headers.len();
                        shell.announce(format!("{found} found across every folder"), now);
                    }
                    Err(error) => shell.fail(format!("{error} — searching the loaded list instead"), now),
                }
            }

            Message::Mark => {
                if let Some(id) = self.selected {
                    if !self.marked.remove(&id) {
                        self.marked.insert(id);
                    }
                    return self.move_selection(1, shell, now);
                }
            }
            Message::Flag => {
                let ids = self.acting_on();
                if ids.is_empty() {
                    return Task::none();
                }
                // Toggle against what the first one is, so a mixed selection all ends up the same.
                let flagged = !self.header(ids[0]).is_some_and(|header| header.flagged);
                for id in &ids {
                    if let Some(header) = self.header_mut(*id) {
                        header.flagged = flagged;
                    }
                }
                let flags = Flags { flagged: Some(flagged), ..Flags::default() };
                return Task::perform(mail::mark(shell.bridge(), ids, flags), |result| {
                    Message::Acted("Flagged", result)
                });
            }
            Message::Unread => {
                let ids = self.acting_on();
                if ids.is_empty() {
                    return Task::none();
                }
                let read = !self.header(ids[0]).is_some_and(|header| header.read);
                for id in &ids {
                    if let Some(header) = self.header_mut(*id) {
                        header.read = read;
                    }
                }
                let flags = Flags { read: Some(read), ..Flags::default() };
                return Task::perform(mail::mark(shell.bridge(), ids, flags), |result| {
                    Message::Acted("Marked", result)
                });
            }
            Message::NextUnread => {
                let from = self.selected_row().map_or(0, |row| row + 1);
                let found = (from..self.rows.len())
                    .chain(0..from.min(self.rows.len()))
                    .find(|row| !self.headers[self.rows[*row].at].read);
                if let Some(row) = found {
                    let id = self.headers[self.rows[row].at].id;
                    return self.select(id, shell, now);
                }
                shell.announce("Nothing unread here", now);
            }

            Message::Archive => return self.remove("Archived", shell, now, mail::archive),
            Message::Discard => return self.remove("Deleted", shell, now, mail::discard),
            Message::Undo => {
                let Some(undo) = self.undo.take() else {
                    shell.announce("Nothing to undo", now);
                    return Task::none();
                };
                let bridge = shell.bridge();
                return Task::perform(mail::restore(bridge, undo.message_ids, undo.folder), Message::Undone);
            }
            Message::Undone(Ok(count)) => {
                shell.announce(if count == 1 { "Put back".to_string() } else { format!("{count} put back") }, now);
                return self.reload(shell);
            }
            Message::Undone(Err(error)) => shell.fail(error, now),
            Message::Acted(what, Ok(())) => {
                if self.undo.is_some() {
                    shell.announce(format!("{what} — u to undo"), now);
                    // A message left the folder, so more than this folder's numbers moved.
                    return self.refresh_counts(shell);
                }
                return self.refresh_open_count(shell);
            }
            Message::Acted(what, Err(error)) => {
                shell.fail(format!("{what}: {error}"), now);
                // What is on screen no longer matches what Thunderbird holds.
                return self.reload(shell);
            }

            Message::Show(showing) => {
                self.showing = if self.showing == showing { Showing::Letter } else { showing };
                if self.showing == Showing::Source
                    && let Some(id) = self.selected
                    && self.source.as_ref().is_none_or(|(had, _)| *had != id)
                {
                    return Task::perform(mail::raw(shell.bridge(), id), move |result| Message::Raw(id, result));
                }
            }
            Message::Raw(id, Ok(source)) => self.source = Some((id, source)),
            Message::Raw(_, Err(error)) => shell.fail(error, now),

            Message::Save(part) => {
                let Some(id) = self.selected else { return Task::none() };
                let name = self
                    .letters
                    .get(&id)
                    .and_then(|letter| letter.attachments.iter().find(|a| a.part_name == part))
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| "attachment".to_string());
                let bridge = shell.bridge();
                return Task::perform(
                    async move {
                        let bytes = mail::attachment(bridge, id, part).await?;
                        write_down(&name, &bytes)
                    },
                    Message::Saved,
                );
            }
            Message::Saved(Ok(path)) => shell.announce(format!("Saved to {path}"), now),
            Message::Saved(Err(error)) => shell.fail(error, now),
            Message::OpenLink(url) => match open_externally(&url) {
                Ok(()) => shell.announce(format!("Opened {url}"), now),
                Err(error) => shell.fail(error, now),
            },
            Message::External => {
                let Some(id) = self.selected else { return Task::none() };
                let subject = self.header(id).map(|header| header.subject.clone()).unwrap_or_default();
                let bridge = shell.bridge();
                return Task::perform(
                    async move {
                        let source = mail::raw(bridge, id).await?;
                        let path = write_down(&format!("{}.eml", file_name(&subject)), source.as_bytes())?;
                        open_externally(&path)?;
                        Ok(path)
                    },
                    Message::Saved,
                );
            }

            Message::Compose(reply) => return self.compose(reply, shell, now),
            Message::Field(field, value) => {
                if let Some(composing) = &mut self.composing {
                    match field {
                        Field::To => composing.draft.to = split_addresses(&value),
                        Field::Cc => composing.draft.cc = split_addresses(&value),
                        Field::Subject => composing.draft.subject = value,
                    }
                }
            }
            Message::Body(action) => {
                if let Some(composing) = &mut self.composing {
                    composing.body.perform(action);
                }
            }
            Message::Identity(id) => {
                if let Some(composing) = &mut self.composing {
                    composing.draft.identity = id;
                }
            }
            Message::Send | Message::Draft => {
                let deliver = if matches!(message, Message::Send) { mail::Deliver::Now } else { mail::Deliver::Draft };
                let Some(composing) = &self.composing else { return Task::none() };
                if composing.draft.identity.is_empty() {
                    shell.fail("No identity to send from", now);
                    return Task::none();
                }
                if matches!(deliver, mail::Deliver::Now) && composing.draft.to.is_empty() {
                    shell.fail("Nobody to send it to", now);
                    return Task::none();
                }
                let mut draft = composing.draft.clone();
                draft.markdown = composing.body.text();
                return Task::perform(mail::send(shell.bridge(), draft, deliver), Message::Sent);
            }
            Message::Sent(Ok(_)) => {
                self.composing = None;
                self.motion.pager.restart(now);
                shell.announce("Sent", now);
            }
            Message::Sent(Err(error)) => shell.fail(error, now),

            Message::Screen => {
                let Some(proposal) = self.propose() else {
                    shell.announce("Nothing to make a rule from", now);
                    return Task::none();
                };
                self.screening = Some(proposal);
                self.motion.pager.restart(now);
                if let Some(account) = self.account_of(self.folder.as_deref().unwrap_or_default()) {
                    return Task::perform(mail::rules(shell.bridge(), account), Message::Rules);
                }
            }
            Message::ScreenFolder(id) => {
                let found = self.folders().find(|folder| folder.id == id).cloned();
                if let (Some(screen), Some(folder)) = (&mut self.screening, found) {
                    screen.folder_name = folder.name;
                    screen.folder_path = folder.path;
                    screen.folder = id;
                }
            }
            Message::ScreenGo => {
                let Some(screen) = self.screening.clone() else { return Task::none() };
                if screen.folder.is_empty() {
                    shell.fail("Pick somewhere for it to go", now);
                    return Task::none();
                }
                let Some(account) = self.account_of(self.folder.as_deref().unwrap_or_default()) else {
                    return Task::none();
                };
                return Task::perform(mail::screen(shell.bridge(), account, screen), Message::Screened);
            }
            Message::Screened(Ok(name)) => {
                let screen = self.screening.take();
                shell.announce(format!("Rule “{name}” added"), now);
                // A rule made *because of* the mail in front of you should deal with the mail in
                // front of you, not only with the next one.
                let (Some(folder), Some(screen)) = (self.folder.clone(), screen) else {
                    return Task::none();
                };
                return Task::perform(mail::tidy(shell.bridge(), folder, screen), Message::Tidied);
            }
            Message::Screened(Err(error)) => shell.fail(error, now),
            Message::Rules(Ok(rules)) => self.rules = rules,
            Message::Rules(Err(_)) => self.rules.clear(),
            Message::Tidied(Ok(moved)) => {
                if moved > 0 {
                    shell.announce(
                        if moved == 1 {
                            "1 already here moved too".to_string()
                        } else {
                            format!("{moved} already here moved too")
                        },
                        now,
                    );
                    return self.reload(shell);
                }
            }
            Message::Tidied(Err(error)) => shell.fail(error, now),

            Message::Cancel => {
                self.composing = None;
                self.screening = None;
                self.motion.pager.restart(now);
            }
            Message::Escape => {
                if self.composing.is_some() || self.screening.is_some() {
                    return self.step(Message::Cancel, shell, now);
                }
                if self.showing != Showing::Letter {
                    self.showing = Showing::Letter;
                } else if !self.marked.is_empty() {
                    self.marked.clear();
                } else if !self.query.is_empty() || self.deep {
                    self.query.clear();
                    let was_deep = std::mem::take(&mut self.deep);
                    if was_deep {
                        return self.reload(shell);
                    }
                    self.rebuild(now);
                } else {
                    shell.hush(now);
                }
            }
            Message::Refresh => {
                shell.announce("Checking for mail", now);
                return Task::batch([self.reload(shell), self.refresh_counts(shell)]);
            }

            Message::IndexScrolled(view) => self.index_view = Some(view),
            Message::RailScrolled(view) => self.rail_view = Some(view),
            Message::PagerScrolled(view) => self.pager_view = Some(view),
        }
        Task::none()
    }

    /// Starts the index over for whatever folder is open.
    fn reload(&mut self, shell: &Shell) -> Task<Message> {
        let Some(folder) = self.folder.clone() else {
            self.headers.clear();
            self.rows.clear();
            return Task::none();
        };
        self.generation += 1;
        let generation = self.generation;
        self.headers.clear();
        self.rows.clear();
        self.marked.clear();
        self.listing = true;
        let bridge = shell.bridge();
        Task::perform(mail::list(bridge, folder), move |result| Message::Page(generation, result))
    }

    /// Asks Gloda which conversation each loaded message is in.
    ///
    /// Once per listing rather than per message: it is one round trip either way, and the index is
    /// perfectly usable flat while the answer is on its way.
    fn ask_for_threads(&mut self, shell: &Shell) -> Task<Message> {
        let generation = self.generation;
        let wanted: Vec<String> = self
            .headers
            .iter()
            .filter(|header| !header.message_id.is_empty() && !self.threads.contains_key(&header.message_id))
            .map(|header| header.message_id.clone())
            .take(2_000)
            .collect();
        if wanted.is_empty() {
            return Task::none();
        }
        Task::perform(mail::conversations(shell.bridge(), wanted), move |result| {
            Message::Conversations(generation, result)
        })
    }

    /// Selects a message, fetches it if it is not already held, and marks it read.
    fn select(&mut self, id: u64, shell: &mut Shell, now: Instant) -> Task<Message> {
        self.selected = Some(id);
        self.showing = Showing::Letter;
        self.aim(now);
        self.motion.pager.restart(now);
        self.parsed = self.letters.get(&id).map(|letter| (id, markdown::Content::parse(&letter.body.markdown)));

        let mut tasks = vec![self.follow_index()];
        if !self.letters.contains_key(&id) {
            self.waiting = true;
            tasks.push(Task::perform(mail::letter(shell.bridge(), id), move |result| Message::Letter(id, result)));
        }
        // Reading it is what makes it read. Thunderbird owns the flag; the row changes now so the
        // list does not wait for a round trip to stop being bold.
        if self.header(id).is_some_and(|header| !header.read) {
            if let Some(header) = self.header_mut(id) {
                header.read = true;
            }
            let flags = Flags { read: Some(true), ..Flags::default() };
            // The count comes back with the reply to the flag change; asking for it here as well
            // would be two round trips per folder for every message read.
            tasks.push(Task::perform(mail::mark(shell.bridge(), vec![id], flags), |result| {
                Message::Acted("Marked", result)
            }));
        }
        // The next row is usually the next thing read, and a round trip through a Python shim is
        // long enough to notice. One row ahead, never more: prefetching a screenful would be a
        // hundred round trips for the two the reader actually opens.
        if let Some(next) = self.selected_row().and_then(|row| self.rows.get(row + 1)) {
            let ahead = self.headers[next.at].id;
            if !self.letters.contains_key(&ahead) {
                tasks.push(Task::perform(mail::letter(shell.bridge(), ahead), move |result| {
                    Message::Letter(ahead, result)
                }));
            }
        }
        Task::batch(tasks)
    }

    /// Moves the selection by rows, in whichever region has the keyboard.
    fn move_selection(&mut self, delta: i32, shell: &mut Shell, now: Instant) -> Task<Message> {
        if self.region == Region::Rail {
            let Some(row) = self.perch_at(self.folder_row(), delta) else { return Task::none() };
            let Some(folder) = self.rail[row].folder() else { return Task::none() };
            let id = folder.id.clone();
            self.motion.folder.go_mut(row as f32, now);
            return Task::batch([self.follow_rail(), Task::done(Message::OpenFolder(id))]);
        }
        if self.rows.is_empty() {
            return Task::none();
        }
        let last = self.rows.len() as i32 - 1;
        let row = match self.selected_row() {
            Some(row) => (row as i32 + delta).clamp(0, last),
            None if delta > 0 => 0,
            None => last,
        };
        let id = self.headers[self.rows[row as usize].at].id;
        self.select(id, shell, now)
    }

    /// Archives or deletes, and takes the rows off the screen before Thunderbird has answered.
    ///
    /// The undo is a list of `headerMessageId` and the folder they came from, which is all the
    /// state there is: [`mail::restore`] asks Thunderbird where they ended up rather than assuming
    /// a numeric id survived the move.
    fn remove<F>(
        &mut self,
        what: &'static str,
        shell: &mut Shell,
        now: Instant,
        act: impl FnOnce(noctmalia_bridge::Bridge, Vec<u64>) -> F,
    ) -> Task<Message>
    where
        F: Future<Output = mail::Result<()>> + Send + 'static,
    {
        let ids = self.acting_on();
        if ids.is_empty() {
            return Task::none();
        }
        let Some(folder) = self.folder.clone() else { return Task::none() };
        let message_ids: Vec<String> =
            ids.iter().filter_map(|id| self.header(*id)).map(|header| header.message_id.clone()).collect();

        // Where the selection lands once these rows are gone: the row after the last one removed.
        let after = self.selected_row().map(|row| row.min(self.rows.len().saturating_sub(1)));
        self.headers.retain(|header| !ids.contains(&header.id));
        self.marked.clear();
        self.rebuild(now);
        self.selected = after
            .and_then(|row| self.rows.get(row.min(self.rows.len().saturating_sub(1))))
            .map(|row| self.headers[row.at].id);
        self.aim(now);
        self.undo = Some(Undo { message_ids, folder });

        let mut tasks = vec![Task::perform(act(shell.bridge(), ids), move |result| Message::Acted(what, result))];
        if let Some(id) = self.selected {
            tasks.push(self.select(id, shell, now));
        } else {
            self.parsed = None;
        }
        Task::batch(tasks)
    }

    /// What a verb applies to: everything marked, or else whatever is selected.
    fn acting_on(&self) -> Vec<u64> {
        if !self.marked.is_empty() {
            // In index order, so an undo puts them back in a sensible one.
            return self.headers.iter().map(|header| header.id).filter(|id| self.marked.contains(id)).collect();
        }
        self.selected.into_iter().collect()
    }

    fn header(&self, id: u64) -> Option<&Header> {
        self.headers.iter().find(|header| header.id == id)
    }

    fn header_mut(&mut self, id: u64) -> Option<&mut Header> {
        self.headers.iter_mut().find(|header| header.id == id)
    }

    fn selected_row(&self) -> Option<usize> {
        let id = self.selected?;
        self.rows.iter().position(|row| self.headers[row.at].id == id)
    }

    fn folder_row(&self) -> usize {
        let Some(id) = &self.folder else { return 0 };
        self.rail.iter().position(|perch| perch.folder().is_some_and(|folder| &folder.id == id)).unwrap_or(0)
    }

    fn account_of(&self, folder: &str) -> Option<String> {
        self.folders().find(|one| one.id == folder).map(|one| one.account.clone())
    }

    /// The next selectable row in the rail from `from`, stepping over account headings.
    fn perch_at(&self, from: usize, delta: i32) -> Option<usize> {
        let selectable: Vec<usize> = (0..self.rail.len()).filter(|row| self.rail[*row].folder().is_some()).collect();
        if selectable.is_empty() {
            return None;
        }
        let here = selectable.iter().position(|row| *row >= from).unwrap_or(selectable.len() - 1);
        let moved = (here as i32 + delta).clamp(0, selectable.len() as i32 - 1);
        Some(selectable[moved as usize])
    }

    fn aim(&mut self, now: Instant) {
        match self.selected_row() {
            Some(row) => {
                self.motion.message.go_mut(row as f32, now);
                self.motion.message_shown.go_mut(true, now);
            }
            None => self.motion.message_shown.go_mut(false, now),
        }
    }

    fn follow_index(&self) -> Task<Message> {
        let (Some(row), Some(view)) = (self.selected_row(), self.index_view) else { return Task::none() };
        let target = list::reveal(row, MESSAGE_PITCH, MESSAGE_ROW, view.absolute_offset().y, view.bounds().height);
        match target {
            Some(y) => operation::scroll_to(Id::new(INDEX_ID), AbsoluteOffset { x: 0.0, y }),
            None => Task::none(),
        }
    }

    fn follow_rail(&self) -> Task<Message> {
        let Some(view) = self.rail_view else { return Task::none() };
        let target =
            list::reveal(self.folder_row(), FOLDER_PITCH, FOLDER_ROW, view.absolute_offset().y, view.bounds().height);
        match target {
            Some(y) => operation::scroll_to(Id::new(RAIL_ID), AbsoluteOffset { x: 0.0, y }),
            None => Task::none(),
        }
    }

    /// Whether the rail should trade its labels for glyphs, at this window's width.
    fn rail_collapsed(&self, width: f32) -> bool {
        rail_collapses(width, self.selected.is_some() || self.composing.is_some())
    }

    /// The name of the folder that is open, for the index heading.
    fn folder_name(&self) -> String {
        self.folder
            .as_deref()
            .and_then(|id| self.folders().find(|folder| folder.id == id))
            .map(|folder| folder.name.clone())
            .unwrap_or_else(|| "Mail".to_string())
    }

    /// Rebuilds the index rows from the headers, the filter and what Gloda said about threads.
    ///
    /// A thread is one row until it is opened, which is the whole of the threading UI: a
    /// conversation you are not reading should cost one line, and the count is what says there is
    /// more. The newest message is the one on the head row, because that is the one a folder
    /// sorted by date is telling you about.
    fn rebuild(&mut self, now: Instant) {
        let filter = self.query.trim().to_lowercase();
        let matching: Vec<usize> = self
            .headers
            .iter()
            .enumerate()
            .filter(|(_, header)| {
                filter.is_empty()
                    || self.deep
                    || header.subject.to_lowercase().contains(&filter)
                    || header.author.to_lowercase().contains(&filter)
            })
            .map(|(index, _)| index)
            .collect();

        let mut members: HashMap<i64, Vec<usize>> = HashMap::new();
        for index in &matching {
            if let Some(thread) = self.threads.get(&self.headers[*index].message_id) {
                members.entry(*thread).or_default().push(*index);
            }
        }

        let mut rows = Vec::with_capacity(matching.len());
        let mut done: HashSet<i64> = HashSet::new();
        for index in matching {
            let thread = self.threads.get(&self.headers[index].message_id).copied();
            let Some(thread) = thread.filter(|thread| members.get(thread).is_some_and(|all| all.len() > 1)) else {
                rows.push(Row { at: index, depth: 0, thread: None, held: 1 });
                continue;
            };
            if !done.insert(thread) {
                continue;
            }
            let all = &members[&thread];
            rows.push(Row { at: index, depth: 0, thread: Some(thread), held: all.len() });
            if self.opened.contains(&thread) {
                rows.extend(all.iter().skip(1).map(|at| Row { at: *at, depth: 1, thread: Some(thread), held: 1 }));
            }
        }
        self.rows = rows;

        // A selection that is no longer on screen is no selection.
        if self.selected.is_some_and(|id| !self.rows.iter().any(|row| self.headers[row.at].id == id)) {
            self.selected = None;
            self.parsed = None;
        }
        self.aim(now);
    }

    /// Opens the composer, filled in from whatever is in front of you.
    fn compose(&mut self, reply: Option<Reply>, shell: &mut Shell, now: Instant) -> Task<Message> {
        let identity = self.identity_for_here();
        if identity.is_empty() {
            shell.fail("No identity to send from — check the account's settings", now);
            return Task::none();
        }
        let mut draft = Draft { identity, ..Draft::default() };
        let mut about = None;
        let mut quoted = String::new();

        if let Some(reply) = reply {
            let Some(id) = self.selected else {
                shell.announce("Nothing selected to reply to", now);
                return Task::none();
            };
            let Some(header) = self.header(id).cloned() else { return Task::none() };
            let from = header.from();
            draft.about = Some((id, reply));
            about = Some(match reply {
                Reply::Forward => format!("Forwarding “{}”", header.stem()),
                _ => format!("Replying to {}", from.label()),
            });
            let stem = header.stem().to_string();
            draft.subject = match reply {
                Reply::Forward => format!("Fwd: {stem}"),
                _ => format!("Re: {stem}"),
            };
            if reply != Reply::Forward {
                draft.to = vec![header.author.clone()];
            }
            if reply == Reply::All {
                // Everyone who was on it, except us — replying to all should not mean replying to
                // yourself as well.
                let mine: Vec<String> = self.identities.iter().map(|one| one.email.to_lowercase()).collect();
                draft.cc = header
                    .recipients
                    .iter()
                    .filter(|to| {
                        let address = mime::headers::address(to).address.to_lowercase();
                        !mine.contains(&address) && address != from.address.to_lowercase()
                    })
                    .cloned()
                    .collect();
            }
            // The letter is already Markdown, so quoting it is a matter of putting `>` in front —
            // which is what quoting has meant since before either format existed.
            if let Some(letter) = self.letters.get(&id) {
                let attribution = format!("On {}, {} wrote:", header.full_date(), from.label());
                let body: String = letter.body.markdown.lines().map(|line| format!("> {line}\n")).collect::<String>();
                quoted = format!("\n\n{attribution}\n\n{body}");
            }
        }

        self.composing = Some(Composing { draft, body: text_editor::Content::with_text(&quoted), about });
        self.motion.pager.restart(now);
        operation::focus(Id::new(COMPOSE_TO))
    }

    /// Which identity a message from here goes out as: the one belonging to this folder's account,
    /// or the first there is.
    fn identity_for_here(&self) -> String {
        let account = self.folder.as_deref().and_then(|folder| self.account_of(folder));
        self.identities
            .iter()
            .find(|identity| Some(&identity.account) == account.as_ref())
            .or_else(|| self.identities.first())
            .map(|identity| identity.id.clone())
            .unwrap_or_default()
    }

    /// A rule proposed from the message in front of you.
    ///
    /// A mailing list is the case worth getting right: `List-Id` is stable where a `From` is not,
    /// so a rule written against it keeps working when the list changes its sending address. Where
    /// there is no list, the sender is the next best thing.
    fn propose(&self) -> Option<Screen> {
        let id = self.selected?;
        let header = self.header(id)?;
        let letter = self.letters.get(&id);
        let list = letter
            .and_then(|letter| letter.headers.first("list-id"))
            .map(|value| value.trim().trim_start_matches('<').trim_end_matches('>').to_string())
            .filter(|value| !value.is_empty());
        let from = header.from();
        Some(match list {
            Some(list) => Screen {
                name: format!("List: {list}"),
                header: "list-id".to_string(),
                value: list,
                folder: String::new(),
                folder_path: String::new(),
                folder_name: String::new(),
            },
            None => Screen {
                name: format!("From {}", from.label()),
                header: "from".to_string(),
                value: if from.address.is_empty() { header.author.clone() } else { from.address.clone() },
                folder: String::new(),
                folder_path: String::new(),
                folder_name: String::new(),
            },
        })
    }
}

/// Every account's folders, flattened into the order a rail reads in.
///
/// One account gets no heading — its name over the whole rail says nothing anybody needed telling.
/// Two or more and the rail is otherwise a run-on list of Inboxes and Trashes belonging to nobody
/// in particular, which is what it looked like the first time this ran against a profile with a
/// second account and Local Folders in it.
fn flatten(accounts: &[Account]) -> Vec<Perch> {
    let named = accounts.len() > 1;
    let mut rail = Vec::new();
    for account in accounts {
        let mut mine: Vec<(usize, Folder)> = Vec::new();
        let mut roots = account.folders.clone();
        roots.sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        for folder in &roots {
            // Some builds report the account root as a folder with no name; its children are the
            // folders somebody would recognise. Others list the top-level folders directly.
            if folder.name.is_empty() {
                let mut children = folder.children.clone();
                children.sort_by(|a, b| {
                    a.rank().cmp(&b.rank()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                for child in &children {
                    child.flatten(0, &mut mine);
                }
            } else {
                folder.flatten(0, &mut mine);
            }
        }
        if mine.is_empty() {
            continue;
        }
        if named {
            rail.push(Perch::Account(if account.name.is_empty() { account.id.clone() } else { account.name.clone() }));
        }
        rail.extend(mine.into_iter().map(|(depth, folder)| Perch::Folder(depth, folder)));
    }
    rail
}

/// An account's name, over its folders. Not selectable: the keyboard steps over it and the pointer
/// finds nothing to press.
fn account_row<'a>(name: &str, showing: f32) -> Element<'a, Message> {
    let colour = motion::mix(theme::palette().surface, theme::palette().on_surface_variant, showing);
    container(
        text(name.to_string())
            .size(theme::FONT_MINI)
            .font(theme::semibold())
            .color(colour)
            .wrapping(text::Wrapping::None),
    )
    .height(FOLDER_ROW)
    .center_y(FOLDER_ROW)
    .padding(Padding { left: theme::SPACE_SM + ui::MARK, right: theme::SPACE_SM, ..Padding::ZERO })
    .clip(true)
    .into()
}

/// Whether the rail should trade its labels for glyphs: only when there is a letter to show *and*
/// the three panes do not comfortably fit.
fn rail_collapses(width: f32, reading: bool) -> bool {
    reading && width - RAIL_WIDTH - INDEX_WIDTH - 2.0 * theme::BORDER < PAGER_CRAMPED
}

/// A comma-separated address field, as typed.
fn split_addresses(value: &str) -> Vec<String> {
    mime::headers::addresses(value)
        .into_iter()
        .map(
            |address| {
                if address.name.is_empty() {
                    address.address
                } else {
                    format!("{} <{}>", address.name, address.address)
                }
            },
        )
        .filter(|one| !one.is_empty())
        .collect()
}

/// A filename that will not surprise anybody.
fn file_name(subject: &str) -> String {
    let cleaned: String = subject
        .chars()
        .map(|character| if character.is_alphanumeric() || character == '-' { character } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() { "message".to_string() } else { trimmed.chars().take(60).collect() }
}

/// Where the desktop puts downloads.
fn downloads() -> std::path::PathBuf {
    if let Some(configured) = std::env::var_os("XDG_DOWNLOAD_DIR") {
        return std::path::PathBuf::from(configured);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

/// Writes a file without ever replacing one, and says where it went.
fn write_down(name: &str, bytes: &[u8]) -> mail::Result<String> {
    let directory = downloads();
    std::fs::create_dir_all(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.to_string(), String::new()),
    };
    // Mail is full of `image001.png`, and the second one is not the first one.
    for attempt in 0..1_000 {
        let candidate = match attempt {
            0 => directory.join(format!("{stem}{extension}")),
            n => directory.join(format!("{stem}-{n}{extension}")),
        };
        if candidate.exists() {
            continue;
        }
        std::fs::write(&candidate, bytes).map_err(|error| format!("{}: {error}", candidate.display()))?;
        return Ok(candidate.display().to_string());
    }
    Err(format!("{} already has a thousand of these", directory.display()))
}

/// Hands something to the desktop.
///
/// This is the one place in the program that can reach the internet, and it takes a deliberate
/// press to get here: nothing in the rendering path fetches anything, so a link is inert until
/// somebody clicks it and a remote image is never anything but a URL in a list.
fn open_externally(target: &str) -> mail::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open {target}: {error}"))
}

// ── View ────────────────────────────────────────────────────────────────────

/// How the letter is drawn. Every colour is a palette role, which is the point: a newsletter cannot
/// paint its own white card over a dark desktop, because it never gets to choose a colour at all.
fn reading() -> markdown::Settings {
    let palette = theme::palette();
    markdown::Settings::with_text_size(
        theme::FONT_BODY,
        markdown::Style {
            font: theme::font(),
            inline_code_padding: Padding::from([0.0, theme::SPACE_XS]),
            inline_code_highlight: markdown::Highlight {
                background: palette.surface_variant.into(),
                border: iced::border::rounded(theme::RADIUS_SM),
            },
            inline_code_color: palette.tertiary,
            inline_code_font: iced::Font::MONOSPACE,
            code_block_font: iced::Font::MONOSPACE,
            link_color: palette.primary,
        },
    )
}

/// The letter's renderer.
///
/// The one override that matters is [`Viewer::image`](markdown::Viewer::image): a Markdown letter
/// may well contain `![alt](url)`, and this is where that stops being a request to fetch something
/// and becomes a line of text saying what it would have been.
struct Reader;

impl<'a> markdown::Viewer<'a, Message> for Reader {
    fn on_link_click(url: markdown::Uri) -> Message {
        Message::OpenLink(url)
    }

    fn image(
        &self,
        settings: markdown::Settings,
        _url: &'a markdown::Uri,
        _title: &'a str,
        alt: &markdown::Text,
    ) -> Element<'a, Message> {
        container(
            row![
                widgets::icon(icon::PHOTO, theme::FONT_CAPTION).color(theme::palette().on_surface_variant),
                iced::widget::rich_text(alt.spans(settings.style)),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_SM]))
        .style(theme::track)
        .into()
    }
}

impl Mail {
    pub fn view(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        row![self.rail_pane(now), ui::hairline_y(), self.index_pane(now), ui::hairline_y(), self.right_pane(shell, now)]
            .height(Length::Fill)
            .into()
    }

    // ── The rail ────────────────────────────────────────────────────────────

    fn rail_pane(&self, now: Instant) -> Element<'_, Message> {
        let at = self.motion.folder.interpolate_with(|row| row, now);
        let showing = 1.0 - self.motion.collapse.interpolate(0.0, 1.0, now).clamp(0.0, 1.0);
        let focused = self.region == Region::Rail;

        let mut rows = column![].spacing(ROW_GAP);
        for (index, perch) in self.rail.iter().enumerate() {
            rows = rows.push(match perch {
                Perch::Account(name) => account_row(name, showing),
                Perch::Folder(depth, folder) => {
                    let counts = self.counts.get(&folder.id).copied().unwrap_or_default();
                    folder_row(folder, *depth, counts, ui::nearness(at, index), showing)
                }
            });
        }
        let total = list::total(self.rail.len(), FOLDER_PITCH, ROW_GAP);
        let bar = ui::selection_bar(at * FOLDER_PITCH, FOLDER_ROW, if self.rail.is_empty() { 0.0 } else { 1.0 }, total);

        let heading = text(if focused { "Folders ·" } else { "Folders" })
            .size(theme::FONT_CAPTION)
            .color(motion::mix(theme::palette().surface, theme::palette().on_surface_variant, showing))
            .width(Length::Fill)
            .wrapping(text::Wrapping::None);

        container(
            column![
                heading,
                scrollable(stack![rows, bar])
                    .id(Id::new(RAIL_ID))
                    .on_scroll(Message::RailScrolled)
                    .style(theme::scrollable_style)
            ]
            .spacing(theme::SPACE_SM),
        )
        .width(motion::lerp(RAIL_COLLAPSED, RAIL_WIDTH, showing))
        .height(Length::Fill)
        .padding(motion::lerp(theme::SPACE_SM, theme::SPACE_MD, showing))
        .clip(true)
        .into()
    }

    // ── The index ───────────────────────────────────────────────────────────

    fn index_pane(&self, now: Instant) -> Element<'_, Message> {
        let name = self.folder_name();
        // Messages rather than rows: a thread is one row and five messages, and the row count
        // would say eight where there are twelve.
        let shown: usize = self.rows.iter().map(|row| if row.depth == 0 { row.held } else { 0 }).sum();
        let counted = if self.listing {
            format!("{}…", self.headers.len())
        } else if !self.marked.is_empty() {
            format!("{} marked", self.marked.len())
        } else if self.deep {
            format!("{shown} found")
        } else if shown == 1 {
            "1 message".to_string()
        } else {
            format!("{shown} messages")
        };

        let header = row![
            column![
                text(name).size(theme::FONT_BODY).font(theme::semibold()).wrapping(text::Wrapping::None),
                ui::caption(counted),
            ]
            .spacing(1)
            .width(Length::Fill),
            ui::glyph_button(
                icon::PENCIL,
                "Write",
                theme::CONTROL_HEIGHT_SM,
                theme::button_style(ButtonVariant::Primary),
                Message::Compose(None),
            ),
        ]
        .align_y(Alignment::Center)
        .spacing(theme::SPACE_SM);

        let search = text_input(
            if self.deep { "Searched everywhere" } else { "Filter — Enter to search all mail" },
            &self.query,
        )
        .id(Id::new(SEARCH_ID))
        .on_input(Message::Query)
        .on_submit(Message::Deep)
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

        let body: Element<Message> = if self.rows.is_empty() {
            let message = if self.listing {
                "Loading…"
            } else if !self.query.trim().is_empty() {
                "Nothing matched"
            } else if self.folder.is_none() {
                "No folder"
            } else {
                "Nothing here"
            };
            container(ui::caption(message)).center_x(Length::Fill).padding(theme::SPACE_MD).into()
        } else {
            let offset = self.index_view.map_or(0.0, |view| view.absolute_offset().y);
            let height = self.index_view.map_or(0.0, |view| view.bounds().height);
            let window = list::window(self.rows.len(), MESSAGE_PITCH, offset, height);
            let at = self.motion.message.interpolate_with(|row| row, now);
            let shown = self.motion.message_shown.interpolate(0.0, 1.0, now);
            let reveal = self.motion.reveal.linear(now);

            let built = window.range().map(|index| {
                // The stagger is for a list arriving, and only for the rows somebody watches
                // arrive: past the first screenful it is a wait rather than a flourish.
                let arrived = if index >= STAGGER_ROWS {
                    1.0
                } else {
                    motion::stagger(motion::SETTLE, reveal, index, STAGGER_STEP, STAGGER_WINDOW)
                };
                self.message_row(index, ui::nearness(at, index) * shown, arrived)
            });
            let rows = list::windowed(window, MESSAGE_PITCH, ROW_GAP, built);
            let total = list::total(self.rows.len(), MESSAGE_PITCH, ROW_GAP);
            let bar = ui::selection_bar(at * MESSAGE_PITCH, MESSAGE_ROW, shown, total);
            scrollable(stack![rows, bar])
                .id(Id::new(INDEX_ID))
                .on_scroll(Message::IndexScrolled)
                .style(theme::scrollable_style)
                .height(Length::Fill)
                .into()
        };

        container(column![header, search, body].spacing(theme::SPACE_SM))
            .width(INDEX_WIDTH)
            .height(Length::Fill)
            .padding(theme::SPACE_MD)
            .into()
    }

    /// One row of the index.
    ///
    /// Two lines, fixed height, neither of which may wrap: the selection bar travels on a fixed
    /// pitch, so a subject that took a second line would put the bar a row out — and the windowed
    /// list would be measuring the wrong thing besides.
    fn message_row(&self, index: usize, fill: f32, arrived: f32) -> Element<'_, Message> {
        let row_at = self.rows[index];
        let header = &self.headers[row_at.at];
        let palette = theme::palette();
        let surface = palette.surface;
        let unread = !header.read;

        // Read mail steps back a role; unread mail is the only thing in the list at full weight.
        let strong =
            motion::mix(surface, if unread { palette.on_surface } else { palette.on_surface_variant }, arrived);
        let quiet = motion::mix(surface, palette.on_surface_variant, arrived);

        let mark: Element<Message> = if self.marked.contains(&header.id) {
            widgets::icon(icon::CHECK, theme::FONT_CAPTION).color(motion::mix(surface, palette.primary, arrived)).into()
        } else if unread {
            // A dot, not a tick: the mark says "there is something here", and a tick says the
            // opposite of what an unread message is.
            widgets::icon(icon::POINT, theme::FONT_CAPTION).color(motion::mix(surface, palette.primary, arrived)).into()
        } else {
            space().width(theme::FONT_CAPTION).into()
        };

        let from = header.from();
        let mut top = row![
            text(from.label().to_string())
                .size(theme::FONT_BODY)
                .font(if unread { theme::semibold() } else { theme::font() })
                .color(strong)
                .width(Length::Fill)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(theme::SPACE_XS)
        .align_y(Alignment::Center);
        if self.letters.get(&header.id).is_some_and(|letter| !letter.attachments.is_empty()) {
            top = top.push(widgets::icon(icon::PAPERCLIP, theme::FONT_MINI).color(quiet));
        }
        if header.flagged {
            top = top.push(widgets::icon(icon::FLAG_FILLED, theme::FONT_MINI).color(motion::mix(
                surface,
                palette.secondary,
                arrived,
            )));
        }
        top = top.push(text(header.stamp()).size(theme::FONT_MINI).color(quiet).wrapping(text::Wrapping::None));

        let subject = if header.subject.trim().is_empty() { "(no subject)" } else { header.subject.trim() };
        let mut bottom = row![].spacing(theme::SPACE_XS).align_y(Alignment::Center);
        // A thread is one row and a number until it is opened. The number is what says there is
        // more; the chevron is what opens it.
        if let (Some(thread), true) = (row_at.thread, row_at.held > 1) {
            let open = self.opened.contains(&thread);
            bottom = bottom.push(
                iced::widget::button(
                    row![
                        widgets::icon(if open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT }, theme::FONT_MINI),
                        text(format!("{}", row_at.held)).size(theme::FONT_MINI),
                    ]
                    .spacing(1)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([0.0, theme::SPACE_XS]))
                .style(ui::ghost_button)
                .on_press(Message::FoldAt(thread)),
            );
        }
        bottom = bottom.push(
            text(subject.to_string())
                .size(theme::FONT_CAPTION)
                .color(if unread { strong } else { quiet })
                .width(Length::Fill)
                .wrapping(text::Wrapping::None),
        );

        let content = row![
            ui::bar_gutter(),
            container(mark).width(ui::MARK).center_x(ui::MARK),
            container(column![top, bottom].spacing(2)).width(Length::Fill).clip(true),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

        iced::widget::button(content)
            .width(Length::Fill)
            .height(MESSAGE_ROW)
            .padding(Padding {
                top: 0.0,
                right: theme::SPACE_SM,
                bottom: 0.0,
                // A reply sits under the message it answers; the reveal slides the row in from the
                // left on the same padding.
                left: theme::SPACE_SM + row_at.depth as f32 * NEST + motion::lerp(ROW_SLIDE, 0.0, arrived),
            })
            .style(ui::row_style(fill))
            .on_press(Message::Select(header.id))
            .into()
    }
}

/// One folder in the rail.
fn folder_row<'a>(folder: &Folder, depth: usize, counts: Counts, fill: f32, showing: f32) -> Element<'a, Message> {
    let palette = theme::palette();
    let surface = palette.surface;
    let bold = counts.unread > 0;
    let name = text(folder.name.clone())
        .size(theme::FONT_CAPTION)
        .font(if bold { theme::semibold() } else { theme::font() })
        .color(motion::mix(surface, palette.on_surface, showing))
        .width(Length::Fill)
        .wrapping(text::Wrapping::None);

    let mut line = row![
        ui::bar_gutter(),
        container(widgets::icon(folder.glyph(), theme::FONT_CAPTION).color(palette.on_surface_variant))
            .width(ui::MARK)
            .center_x(ui::MARK)
            .center_y(Length::Fill),
        name,
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Alignment::Center);
    if counts.unread > 0 {
        line = line.push(
            text(format!("{}", counts.unread)).size(theme::FONT_MINI).font(theme::semibold()).color(motion::mix(
                surface,
                palette.primary,
                showing.max(0.35),
            )),
        );
    }

    let control = iced::widget::button(container(line).clip(true))
        .width(Length::Fill)
        .height(FOLDER_ROW)
        .padding(Padding {
            left: theme::SPACE_SM + depth as f32 * NEST * showing,
            right: theme::SPACE_SM,
            ..Padding::ZERO
        })
        .style(ui::row_style(fill))
        .on_press(Message::OpenFolder(folder.id.clone()));

    if showing > 0.5 {
        return control.into();
    }
    // With the labels gone a glyph is all there is, and several folders share one.
    let hint = container(text(folder.name.clone()).size(theme::FONT_CAPTION))
        .padding(Padding::from([theme::SPACE_XS, theme::SPACE_SM]))
        .style(theme::track);
    iced::widget::tooltip(control, hint, iced::widget::tooltip::Position::Right).gap(theme::SPACE_XS).into()
}

// ── The right-hand pane ─────────────────────────────────────────────────────

/// An identity, as a picker shows it.
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    id: String,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl Mail {
    fn right_pane(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        let arrived = self.motion.pager.at(now);
        let content: Element<Message> = match (&self.composing, &self.screening) {
            (Some(composing), _) => self.composer(composing),
            (None, Some(screen)) => self.screener(screen),
            (None, None) => self.pager(shell, now),
        };
        // The pane rises into place on top padding. Doing it this way rather than with `float`
        // keeps it inside its own bounds: it never draws over the index while it moves.
        let rise = motion::lerp(PAGER_RISE, 0.0, arrived);
        container(container(content).max_width(PAGER_WIDTH))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: theme::SPACE_LG + rise,
                right: theme::SPACE_LG,
                bottom: theme::SPACE_MD,
                left: theme::SPACE_LG,
            })
            .into()
    }

    fn pager(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        let Some(id) = self.selected else {
            let hint = if self.headers.is_empty() { "nothing here yet" } else { "j and k move · Enter opens" };
            return ui::placeholder(icon::MAIL_OPENED, "Pick a message", hint);
        };
        let Some(header) = self.header(id) else {
            return ui::placeholder(icon::MAIL_OPENED, "Pick a message", "");
        };
        let letter = self.letters.get(&id);
        let _ = shell;

        let body: Element<Message> = match (self.showing, letter) {
            (_, None) => container(ui::caption(if self.waiting { "Opening…" } else { "…" }))
                .center_x(Length::Fill)
                .padding(theme::SPACE_LG)
                .into(),
            (Showing::Letter, Some(letter)) => self.letter_body(letter, now),
            (Showing::Headers, Some(letter)) => headers_view(letter),
            (Showing::Security, Some(letter)) => security_view(letter),
            (Showing::Source, Some(_)) => self.source_view(id),
        };

        let mut pane = column![self.envelope(header, letter), ui::hairline_x(), body].spacing(theme::SPACE_MD);
        if let Some(letter) = letter.filter(|letter| !letter.attachments.is_empty()) {
            pane = pane.push(attachments(&letter.attachments));
        }
        pane.height(Length::Fill).into()
    }

    /// Who it is from, what it is about, and what is odd about it.
    fn envelope<'a>(&'a self, header: &'a Header, letter: Option<&'a Letter>) -> Element<'a, Message> {
        let palette = theme::palette();
        let from = header.from();
        let subject = if header.subject.trim().is_empty() { "(no subject)" } else { header.subject.trim() };

        let to: String = header
            .recipients
            .iter()
            .map(|one| mime::headers::address(one).label().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut who = column![
            row![
                text(from.label().to_string()).size(theme::FONT_BODY).font(theme::semibold()),
                text(if from.name.is_empty() { String::new() } else { format!("  {}", from.address) })
                    .size(theme::FONT_MINI)
                    .color(palette.on_surface_variant),
                space().width(Length::Fill),
                text(header.full_date()).size(theme::FONT_MINI).color(palette.on_surface_variant),
            ]
            .spacing(theme::SPACE_XS)
            .align_y(Alignment::Center),
        ]
        .spacing(1)
        .width(Length::Fill);
        if !to.is_empty() {
            who = who.push(
                text(format!("to {to}"))
                    .size(theme::FONT_MINI)
                    .color(palette.on_surface_variant)
                    .wrapping(text::Wrapping::None),
            );
        }

        let mut badges = row![].spacing(theme::SPACE_XS).align_y(Alignment::Center);
        if let Some(letter) = letter {
            // Loudest first, and at most three: a badge row that needs reading is not a badge row.
            for finding in letter.findings.iter().take(3) {
                badges = badges.push(ui::tip(
                    ui::chip(finding.headline, level_colour(finding.level)),
                    // The sentence is in the panel; the pointer gets it too, since that is where
                    // somebody who noticed the badge is already looking.
                    "",
                ));
            }
            if letter.body.trackers > 0 {
                let word = if letter.body.trackers == 1 {
                    "1 tracker".to_string()
                } else {
                    format!("{} trackers", letter.body.trackers)
                };
                badges = badges.push(ui::chip(word, palette.secondary));
            }
            if !letter.body.images.is_empty() {
                badges = badges.push(ui::chip(
                    format!("{} remote images, none loaded", letter.body.images.len()),
                    palette.on_surface_variant,
                ));
            }
            badges = badges.push(ui::chip(letter.body.flavour.label(), palette.on_surface_variant));
        }

        column![
            text(subject.to_string())
                .size(theme::FONT_TITLE)
                .font(theme::semibold())
                .wrapping(text::Wrapping::WordOrGlyph),
            row![ui::avatar(&from.initials(), ui::AVATAR, 1.0, 1.0, 1.0), who]
                .spacing(theme::SPACE_MD)
                .align_y(Alignment::Center),
            badges,
            self.verbs(),
        ]
        .spacing(theme::SPACE_SM)
        .into()
    }

    /// The things you do to a message you have just read.
    fn verbs(&self) -> Element<'_, Message> {
        let showing = self.showing;
        let mut bar = row![
            ui::icon_button(icon::CORNER_UP_LEFT, "Reply  ·  r", false, Message::Compose(Some(Reply::Sender))),
            ui::icon_button(icon::CORNER_UP_RIGHT, "Reply to all  ·  R", false, Message::Compose(Some(Reply::All))),
            ui::icon_button(icon::ARROW_BACK_UP, "Forward  ·  f", false, Message::Compose(Some(Reply::Forward))),
            space().width(theme::SPACE_SM),
            ui::icon_button(icon::ARCHIVE, "Archive  ·  e", false, Message::Archive),
            ui::icon_button(icon::TRASH, "Delete  ·  d", false, Message::Discard),
            ui::icon_button(icon::FLAG, "Flag  ·  s", false, Message::Flag),
            ui::icon_button(icon::FILTER, "Make a rule from this  ·  S", false, Message::Screen),
            space().width(Length::Fill),
            ui::icon_button(
                icon::SHIELD,
                "What this gives away  ·  !",
                showing == Showing::Security,
                Message::Show(Showing::Security)
            ),
            ui::icon_button(icon::LIST, "Headers  ·  H", showing == Showing::Headers, Message::Show(Showing::Headers)),
            ui::icon_button(icon::CODE, "Source  ·  \\", showing == Showing::Source, Message::Show(Showing::Source)),
            ui::icon_button(icon::EXTERNAL_LINK, "Open elsewhere  ·  O", false, Message::External),
        ]
        .spacing(theme::SPACE_XS)
        .align_y(Alignment::Center);
        if self.undo.is_some() {
            bar = bar.push(ui::icon_button(icon::ARROW_BACK_UP, "Undo  ·  u", false, Message::Undo));
        }
        bar.into()
    }

    fn letter_body(&self, letter: &Letter, _now: Instant) -> Element<'_, Message> {
        let content: Element<Message> = match self.parsed.as_ref().filter(|(id, _)| *id == letter.id) {
            Some((_, parsed)) => markdown::view_with(parsed.items(), reading(), &Reader),
            None if letter.body.markdown.is_empty() => ui::caption("This message has no text in it."),
            // Between selecting and parsing there is one frame; show the source rather than a gap.
            None => text(letter.body.markdown.clone()).size(theme::FONT_BODY).into(),
        };
        scrollable(container(content).padding(Padding { right: theme::SPACE_MD, ..Padding::ZERO }))
            .id(Id::new(PAGER_ID))
            .on_scroll(Message::PagerScrolled)
            .style(theme::scrollable_style)
            .height(Length::Fill)
            .into()
    }

    fn source_view(&self, id: u64) -> Element<'_, Message> {
        let source = match self.source.as_ref().filter(|(had, _)| *had == id) {
            Some((_, source)) => source.as_str(),
            None => "…",
        };
        scrollable(
            text(source.to_string())
                .size(theme::FONT_MINI)
                .font(iced::Font::MONOSPACE)
                .wrapping(text::Wrapping::WordOrGlyph),
        )
        .id(Id::new(PAGER_ID))
        .on_scroll(Message::PagerScrolled)
        .style(theme::scrollable_style)
        .height(Length::Fill)
        .into()
    }

    // ── Composing ───────────────────────────────────────────────────────────

    fn composer<'a>(&'a self, composing: &'a Composing) -> Element<'a, Message> {
        let choices: Vec<Choice> = self
            .identities
            .iter()
            .map(|identity| Choice {
                id: identity.id.clone(),
                label: if identity.name.is_empty() {
                    identity.email.clone()
                } else {
                    format!("{} <{}>", identity.name, identity.email)
                },
            })
            .collect();
        let chosen = choices.iter().find(|choice| choice.id == composing.draft.identity).cloned();

        let title = composing.about.clone().unwrap_or_else(|| "New message".to_string());
        let heading = row![
            column![
                text(title).size(theme::FONT_TITLE).font(theme::semibold()).wrapping(text::Wrapping::None),
                ui::caption("Written in Markdown, sent as plain text — exactly the words you typed"),
            ]
            .spacing(1)
            .width(Length::Fill),
            widgets::action("Cancel", ButtonVariant::Tab, Some(Message::Cancel)),
            widgets::action("Save draft", ButtonVariant::Default, Some(Message::Draft)),
            widgets::action("Send", ButtonVariant::Primary, Some(Message::Send)),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

        let field = |label: &'a str, id: Option<&'static str>, value: String, which: Field| {
            let mut input = text_input("", &value)
                .on_input(move |value| Message::Field(which, value))
                .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD]))
                .size(theme::FONT_BODY)
                .style(theme::text_input_style);
            if let Some(id) = id {
                input = input.id(Id::new(id));
            }
            column![text(label).size(theme::FONT_MINI).color(theme::palette().on_surface_variant), input]
                .spacing(theme::SPACE_XS)
        };

        let from = row![
            column![
                text("From").size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
                iced::widget::pick_list(choices, chosen, |choice: Choice| Message::Identity(choice.id))
                    .placeholder("an identity")
                    .width(Length::Fill)
                    .style(theme::pick_list_style)
                    .text_size(theme::FONT_BODY)
                    .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD])),
            ]
            .spacing(theme::SPACE_XS)
            .width(Length::Fill),
        ];

        let body = text_editor(&composing.body)
            .placeholder("…")
            .size(theme::FONT_BODY)
            .on_action(Message::Body)
            .height(Length::Fill)
            .padding(theme::SPACE_MD)
            .style(|theme, status| {
                let mut style = theme::text_editor_style(theme, status);
                style.background = theme::palette().surface.into();
                style
            });

        column![
            heading,
            ui::hairline_x(),
            from,
            field("To", Some(COMPOSE_TO), composing.draft.to.join(", "), Field::To),
            field("Cc", None, composing.draft.cc.join(", "), Field::Cc),
            field("Subject", None, composing.draft.subject.clone(), Field::Subject),
            body,
            ui::caption("ctrl+Enter sends · ctrl+S saves a draft · Esc puts it away"),
        ]
        .spacing(theme::SPACE_MD)
        .height(Length::Fill)
        .into()
    }

    // ── Screening ───────────────────────────────────────────────────────────

    /// The rule this message suggests, one keystroke from where you noticed you wanted it.
    fn screener<'a>(&'a self, screen: &'a Screen) -> Element<'a, Message> {
        let choices: Vec<Choice> = self
            .folders()
            .filter(|folder| !matches!(folder.purpose(), "trash" | "outbox" | "drafts" | "templates"))
            .map(|folder| Choice { id: folder.id.clone(), label: folder.name.clone() })
            .collect();
        let chosen = choices.iter().find(|choice| choice.id == screen.folder).cloned();

        let sentence = format!("Everything where {} is {}", screen.header, screen.value);
        column![
            text("Screen mail like this").size(theme::FONT_TITLE).font(theme::semibold()),
            ui::caption("The rule is Thunderbird's own, in msgFilterRules.dat. Nothing is stored here."),
            ui::hairline_x(),
            container(
                column![
                    text(sentence).size(theme::FONT_BODY).wrapping(text::Wrapping::WordOrGlyph),
                    row![
                        text("goes to").size(theme::FONT_BODY).color(theme::palette().on_surface_variant),
                        iced::widget::pick_list(choices, chosen, |choice: Choice| Message::ScreenFolder(choice.id))
                            .placeholder("a folder")
                            .width(240)
                            .style(theme::pick_list_style)
                            .text_size(theme::FONT_BODY)
                            .padding(Padding::from([theme::SPACE_SM, theme::SPACE_MD])),
                    ]
                    .spacing(theme::SPACE_SM)
                    .align_y(Alignment::Center),
                ]
                .spacing(theme::SPACE_MD)
            )
            .width(Length::Fill)
            .padding(theme::CARD_PADDING)
            .style(theme::track),
            row![
                space().width(Length::Fill),
                widgets::action("Cancel", ButtonVariant::Tab, Some(Message::Cancel)),
                widgets::action(
                    "Make the rule",
                    ButtonVariant::Primary,
                    (!screen.folder.is_empty()).then_some(Message::ScreenGo)
                ),
            ]
            .spacing(theme::SPACE_SM),
            ui::caption(match screen.header.as_str() {
                "from" => "Mail already here from this sender moves too.",
                _ => "This one applies from the next message on.",
            }),
            self.existing_rules(),
        ]
        .spacing(theme::SPACE_MD)
        .into()
    }

    /// What this account already screens, so a second rule for the same sender is obvious before
    /// it is made rather than after.
    fn existing_rules(&self) -> Element<'_, Message> {
        if self.rules.is_empty() {
            return ui::caption("No rules on this account yet.");
        }
        let mut pane = column![ui::hairline_x(), ui::caption(format!("{} already here", self.rules.len()))]
            .spacing(theme::SPACE_SM);
        for rule in self.rules.iter().take(12) {
            let colour = if rule.enabled { theme::palette().on_surface } else { theme::palette().on_surface_variant };
            pane = pane.push(
                row![
                    widgets::icon(if rule.enabled { icon::CHECK } else { icon::X }, theme::FONT_MINI)
                        .color(theme::palette().on_surface_variant),
                    text(rule.name.clone()).size(theme::FONT_CAPTION).color(colour).wrapping(text::Wrapping::None),
                    ui::caption(rule.summary.clone()),
                ]
                .spacing(theme::SPACE_SM)
                .align_y(Alignment::Center),
            );
        }
        pane.into()
    }
}

fn level_colour(level: mime::headers::Level) -> Color {
    let palette = theme::palette();
    match level {
        mime::headers::Level::Alarm => palette.error,
        mime::headers::Level::Caution => palette.secondary,
        mime::headers::Level::Note => palette.on_surface_variant,
    }
}

/// Every header, with the ones worth reading first.
fn headers_view(letter: &Letter) -> Element<'_, Message> {
    let palette = theme::palette();
    let mut rows = column![].spacing(theme::SPACE_XS);
    let interesting = |name: &str| mime::headers::INTERESTING.contains(&name);
    let mut lines: Vec<(&str, &str)> = letter.headers.iter().collect();
    lines.sort_by_key(|(name, _)| {
        mime::headers::INTERESTING.iter().position(|wanted| wanted == name).unwrap_or(usize::MAX)
    });
    for (name, value) in lines {
        rows = rows.push(
            row![
                container(
                    text(name.to_string()).size(theme::FONT_MINI).font(theme::semibold()).color(if interesting(name) {
                        palette.primary
                    } else {
                        palette.on_surface_variant
                    })
                )
                .width(170),
                text(value.to_string())
                    .size(theme::FONT_MINI)
                    .font(iced::Font::MONOSPACE)
                    .wrapping(text::Wrapping::WordOrGlyph)
                    .width(Length::Fill),
            ]
            .spacing(theme::SPACE_SM),
        );
    }
    scrollable(container(rows).padding(Padding { right: theme::SPACE_MD, ..Padding::ZERO }))
        .id(Id::new(PAGER_ID))
        .on_scroll(Message::PagerScrolled)
        .style(theme::scrollable_style)
        .height(Length::Fill)
        .into()
}

/// What the envelope gives away, and what the sender wanted fetched.
fn security_view(letter: &Letter) -> Element<'_, Message> {
    let palette = theme::palette();
    let mut pane = column![].spacing(theme::SPACE_MD);

    if letter.findings.is_empty() {
        pane = pane.push(
            row![
                widgets::icon(icon::SHIELD_CHECK, theme::FONT_TITLE).color(palette.tertiary),
                text("Nothing about this message contradicts itself.").size(theme::FONT_BODY),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
        );
    }
    for finding in &letter.findings {
        let colour = level_colour(finding.level);
        pane = pane.push(
            container(
                row![
                    widgets::icon(
                        match finding.level {
                            mime::headers::Level::Alarm => icon::ALERT_TRIANGLE,
                            mime::headers::Level::Caution => icon::ALERT,
                            mime::headers::Level::Note => icon::INFO,
                        },
                        theme::FONT_BODY
                    )
                    .color(colour),
                    column![
                        text(finding.headline).size(theme::FONT_BODY).font(theme::semibold()).color(colour),
                        text(finding.detail.clone()).size(theme::FONT_CAPTION).wrapping(text::Wrapping::WordOrGlyph),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                ]
                .spacing(theme::SPACE_SM),
            )
            .width(Length::Fill)
            .padding(theme::CARD_PADDING)
            .style(theme::track),
        );
    }

    if let Some(where_to) = &letter.unsubscribe {
        pane = pane.push(
            row![
                ui::glyph_button(
                    icon::MAIL_OPENED,
                    "Unsubscribe",
                    theme::CONTROL_HEIGHT_SM,
                    theme::button_style(ButtonVariant::Default),
                    Message::OpenLink(where_to.clone()),
                ),
                ui::caption(where_to.clone()),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
        );
    }

    // The honest version of "block remote content": nothing here can load an image, so there is
    // nothing to unblock. What there is, is a list of what the sender wanted fetched — which is
    // information about them rather than a decision for you.
    pane = pane.push(ui::hairline_x());
    pane = pane.push(
        column![
            text(match letter.body.images.len() {
                0 => "The sender asked for nothing from the internet.".to_string(),
                1 => "The sender wanted one image loaded. It was not.".to_string(),
                many => format!("The sender wanted {many} images loaded. None were."),
            })
            .size(theme::FONT_CAPTION),
            ui::caption("Nothing in this program fetches an image, so there is nothing to allow."),
        ]
        .spacing(2),
    );
    for source in letter.body.images.iter().take(20) {
        pane = pane.push(
            row![
                widgets::icon(icon::PHOTO, theme::FONT_MINI).color(palette.on_surface_variant),
                text(source.clone())
                    .size(theme::FONT_MINI)
                    .font(iced::Font::MONOSPACE)
                    .color(palette.on_surface_variant)
                    .wrapping(text::Wrapping::WordOrGlyph)
                    .width(Length::Fill),
            ]
            .spacing(theme::SPACE_SM),
        );
    }

    scrollable(container(pane).padding(Padding { right: theme::SPACE_MD, ..Padding::ZERO }))
        .id(Id::new(PAGER_ID))
        .on_scroll(Message::PagerScrolled)
        .style(theme::scrollable_style)
        .height(Length::Fill)
        .into()
}

/// The strip of what came with the letter.
fn attachments(all: &[mime::Attachment]) -> Element<'_, Message> {
    let mut strip =
        row![widgets::icon(icon::PAPERCLIP, theme::FONT_CAPTION).color(theme::palette().on_surface_variant)]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center);
    for attachment in all {
        strip = strip.push(
            iced::widget::button(
                row![
                    text(attachment.name.clone()).size(theme::FONT_CAPTION).wrapping(text::Wrapping::None),
                    text(attachment.human_size()).size(theme::FONT_MINI).color(theme::palette().on_surface_variant),
                ]
                .spacing(theme::SPACE_XS)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([theme::SPACE_XS, theme::SPACE_SM]))
            .style(ui::ghost_button)
            .on_press(Message::Save(attachment.part_name.clone())),
        );
    }
    scrollable(strip).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new())).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rail only gives up its labels when the letter would otherwise be squeezed.
    #[test]
    fn the_rail_collapses_only_when_a_narrow_window_has_a_letter_to_show() {
        assert!(!rail_collapses(760.0, false), "nothing to make room for");
        assert!(rail_collapses(760.0, true), "the smallest window has to make room");
        assert!(!rail_collapses(1180.0, true), "the default window fits all three panes");
    }

    /// Collapsing has to buy the letter something, or it is just taking the labels away.
    #[test]
    fn collapsing_the_rail_buys_the_letter_most_of_what_it_was_missing() {
        let window = 760.0;
        let gutters = 2.0 * theme::BORDER;
        let cramped = window - RAIL_WIDTH - INDEX_WIDTH - gutters;
        let roomier = window - RAIL_COLLAPSED - INDEX_WIDTH - gutters;
        assert!(cramped < PAGER_CRAMPED, "this window is the cramped case");
        assert!(roomier > cramped + 150.0, "collapsing gained only {} px", roomier - cramped);
    }

    #[test]
    fn a_typed_address_field_becomes_a_list_of_addresses() {
        assert_eq!(split_addresses("a@b.example, Ada <c@d.example>"), ["a@b.example", "Ada <c@d.example>"]);
        assert_eq!(split_addresses("  "), Vec::<String>::new());
        // A comma inside a quoted name is part of the name.
        assert_eq!(split_addresses("\"Lovelace, Ada\" <a@b.example>"), ["Lovelace, Ada <a@b.example>"]);
    }

    #[test]
    fn a_subject_becomes_a_filename_nobody_has_to_think_about() {
        assert_eq!(file_name("Re: lunch?"), "Re__lunch");
        assert_eq!(file_name("../../etc/passwd"), "etc_passwd");
        assert_eq!(file_name(""), "message");
        assert_eq!(file_name("   "), "message");
        assert!(file_name(&"x".repeat(200)).len() <= 60);
    }

    /// A heading is a row so the selection bar keeps its pitch, and the keyboard has to walk past
    /// it as if it were not there.
    #[test]
    fn the_rail_steps_over_account_headings() {
        let folder = |name: &str| Folder {
            id: name.to_string(),
            name: name.to_string(),
            path: String::new(),
            account: String::new(),
            special_use: Vec::new(),
            kind: None,
            children: Vec::new(),
            favourite: false,
        };
        let mut mail = Mail::new();
        //  0 heading, 1 Inbox, 2 Trash, 3 heading, 4 Inbox, 5 Trash
        mail.rail = vec![
            Perch::Account("one".into()),
            Perch::Folder(0, folder("Inbox")),
            Perch::Folder(0, folder("Trash")),
            Perch::Account("two".into()),
            Perch::Folder(0, folder("Inbox2")),
            Perch::Folder(0, folder("Trash2")),
        ];
        // Down from the second account's first folder steps over the heading between them.
        assert_eq!(mail.perch_at(2, 1), Some(4));
        assert_eq!(mail.perch_at(4, -1), Some(2));
        // The ends hold rather than landing on a heading or running off.
        assert_eq!(mail.perch_at(1, -1), Some(1));
        assert_eq!(mail.perch_at(5, 1), Some(5));
        // A count travels through the headings too: three down from the top is the last folder.
        assert_eq!(mail.perch_at(1, 3), Some(5));
        assert_eq!(mail.perch_at(1, 99), Some(5));
    }

    /// Every table has to be unambiguous on its own, or a key means one thing until it means
    /// another.
    #[test]
    fn no_binding_is_a_prefix_of_another_in_any_region() {
        INDEX_KEYS.with(|keys| assert_eq!(keys.conflicts(), Vec::<(String, String)>::new()));
        PAGER_KEYS.with(|keys| assert_eq!(keys.conflicts(), Vec::<(String, String)>::new()));
        RAIL_KEYS.with(|keys| assert_eq!(keys.conflicts(), Vec::<(String, String)>::new()));
    }
}
