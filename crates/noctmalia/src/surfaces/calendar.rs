//! Calendar: a mini month and the list of calendars on the left, month/week/day/agenda on the
//! right.
//!
//! Two things worth finding here rather than in a document:
//!
//! - **A calendar's colour is assigned, not read.** Thunderbird's own `color` is an arbitrary hex;
//!   the design language draws only the sixteen palette roles (`docs/design.md` "Blends in by
//!   construction"), so each calendar cycles through `primary`/`secondary`/`tertiary` by its
//!   position in the list instead. Thunderbird's colour is still round-tripped — `crate::calendar`
//!   reads it — just never drawn.
//! - **Recurrence is presets, not a rule builder.** `calendar.items.query` already expands a
//!   recurring event server-side (`docs/design.md` "Calendar" — no RRULE engine needed here), so
//!   the only thing the editor writes itself is one of five gcal-style presets
//!   ([`ical::Recur::PRESETS`]); an existing custom rule still displays as "Repeats" and survives a
//!   save that does not touch it.

use crate::calendar::{self, Cal, Item};
use crate::ical::{self, Event, Recur, When};
use crate::shell::Shell;
use crate::surfaces::{self, Pressed, Surface};
use crate::ui::{self, ROW_GAP, icon};
#[cfg(test)]
use chrono::Weekday;
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveTime};
use iced::advanced::widget::Id;
use iced::keyboard::{Key, Modifiers};
use iced::widget::scrollable::AbsoluteOffset;
use iced::widget::{
    Stack, button, checkbox, column, container, mouse_area, operation, pick_list, pin, row, scrollable, space, text,
    text_input,
};
use iced::{Alignment, Color, Element, Length, Padding, Task};
use noctalia_iced::keymap::{self, Keymap};
use noctalia_iced::theme::{self, ButtonVariant};
use noctalia_iced::widgets;
use std::time::Instant;

const RAIL_WIDTH: f32 = 200.0;
const MINI_DAY: f32 = 24.0;
const HOUR_HEIGHT: f32 = 48.0;
const GUTTER: f32 = 48.0;
const CHIPS_PER_CELL: usize = 3;

const TITLE_FIELD: &str = "calendar-title";
const TIMELINE_ID: &str = "calendar-timeline";
/// Where Week/Day opens scrolled to, in hours — the middle of a working morning rather than
/// midnight.
const MORNING: f32 = 7.0;

/// Scrolls the Week/Day timeline to [`MORNING`]. See [`Calendar::reload_and_maybe_scroll`].
fn scroll_to_morning() -> Task<Message> {
    operation::scroll_to(Id::new(TIMELINE_ID), AbsoluteOffset { x: 0.0, y: MORNING * HOUR_HEIGHT })
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    Cals(calendar::Result<Vec<Cal>>),
    Items(u64, calendar::Result<Vec<Item>>),
    Toggle(String),
    Visible(calendar::Result<()>),
    View(ViewKind),
    Today,
    JumpTo(NaiveDate),
    Step(i32),
    New(NaiveDate, Option<u32>),
    Open(String),
    Calendar(String),
    Field(Field, String),
    AllDay(bool),
    Recur(Recur),
    ToggleReminder(i64),
    Save,
    Saved(calendar::Result<Box<Item>>),
    AskDelete,
    Delete,
    Deleted(calendar::Result<()>),
    Cancel,
    Escape,
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Location,
    Description,
    StartDate,
    StartTime,
    EndDate,
    EndTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Month,
    Week,
    Day,
    Agenda,
}

impl ViewKind {
    const ALL: [ViewKind; 4] = [ViewKind::Month, ViewKind::Week, ViewKind::Day, ViewKind::Agenda];

    fn label(self) -> &'static str {
        match self {
            ViewKind::Month => "Month",
            ViewKind::Week => "Week",
            ViewKind::Day => "Day",
            ViewKind::Agenda => "Agenda",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    Today,
    Next,
    Prev,
    New,
    Month,
    Week,
    Day,
    Agenda,
    Refresh,
    Escape,
    Go(Surface),
}

thread_local! {
    static KEYS: Keymap<Binding> = surfaces::switches(
        Keymap::new()
            .counted()
            .bind("t", Binding::Today)
            .bind("j", Binding::Next)
            .bind("<Down>", Binding::Next)
            .bind("l", Binding::Next)
            .bind("<Right>", Binding::Next)
            .bind("k", Binding::Prev)
            .bind("<Up>", Binding::Prev)
            .bind("h", Binding::Prev)
            .bind("<Left>", Binding::Prev)
            .bind("n", Binding::New)
            .bind("m", Binding::Month)
            .bind("w", Binding::Week)
            .bind("d", Binding::Day)
            .bind("a", Binding::Agenda)
            .bind("<C-r>", Binding::Refresh)
            .bind("<Esc>", Binding::Escape),
        Binding::Go,
    );
}

// ── The editor ──────────────────────────────────────────────────────────────

/// The panel that creates or edits one event. Dates and times are kept as the text the user is
/// typing rather than parsed eagerly — `Message::Save` is where they are made sense of, so a
/// half-typed date does not fight the user for what it means yet.
struct Editor {
    /// `None` while creating.
    id: Option<String>,
    calendar_id: String,
    event: Event,
    all_day: bool,
    start_date: String,
    start_time: String,
    /// The last calendar day, inclusive — not `event.end`'s exclusive bound — because that is how
    /// a person names the last day of a trip.
    end_date: String,
    end_time: String,
}

impl Editor {
    fn blank(calendar_id: String, day: NaiveDate, hour: Option<u32>) -> Editor {
        let all_day = hour.is_none();
        let start_time = hour.map_or_else(|| "09:00".to_string(), |hour| format!("{hour:02}:00"));
        let end_time = hour.map_or_else(|| "10:00".to_string(), |hour| format!("{:02}:00", (hour + 1).min(23)));
        let start = if all_day {
            When::Date(day)
        } else {
            let at = NaiveTime::from_hms_opt(hour.unwrap_or(9), 0, 0).unwrap_or_default();
            When::Time(ical::local_from_naive(day.and_time(at)))
        };
        let end = if all_day {
            When::Date(day.succ_opt().unwrap_or(day))
        } else {
            When::Time(start.instant() + ChronoDuration::hours(1))
        };
        Editor {
            id: None,
            calendar_id,
            event: Event::blank(start, end),
            all_day,
            start_date: format_date(day),
            start_time,
            end_date: format_date(day),
            end_time,
        }
    }

    fn from_item(item: &Item) -> Editor {
        let event = item.event.clone();
        let all_day = event.start.is_all_day();
        let end_date = if all_day { event.last_day() } else { event.end.date() };
        let time_of = |when: &When| match when {
            When::Time(time) => time.format("%H:%M").to_string(),
            When::Date(_) => "09:00".to_string(),
        };
        Editor {
            id: Some(item.id.clone()),
            calendar_id: item.calendar_id.clone(),
            start_date: format_date(event.start.date()),
            start_time: time_of(&event.start),
            end_date: format_date(end_date),
            end_time: time_of(&event.end),
            all_day,
            event,
        }
    }
}

fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// The five reminder presets the editor offers, and the minutes-before-start each one means.
const REMINDER_PRESETS: [(&str, i64); 4] =
    [("10 minutes before", 10), ("30 minutes before", 30), ("1 hour before", 60), ("1 day before", 1440)];

// ── The surface ─────────────────────────────────────────────────────────────

pub struct Calendar {
    loading: bool,
    /// Sorted by name once loaded, so a calendar's colour (its position in this list) does not
    /// shift as calendars are shown or hidden.
    cals: Vec<Cal>,
    items: Vec<Item>,
    generation: u64,
    view: ViewKind,
    /// The day every view is computed around: the month it names, the week it falls in, or the
    /// day itself.
    anchor: NaiveDate,
    editor: Option<Editor>,
    confirming_delete: bool,
    pending: keymap::Pending,
}

impl Default for Calendar {
    fn default() -> Calendar {
        Calendar::new()
    }
}

impl Calendar {
    pub fn new() -> Calendar {
        Calendar {
            loading: false,
            cals: Vec::new(),
            items: Vec::new(),
            generation: 0,
            view: ViewKind::Month,
            anchor: Local::now().date_naive(),
            editor: None,
            confirming_delete: false,
            pending: keymap::Pending::default(),
        }
    }

    pub fn update(&mut self, message: Message, shell: &mut Shell, now: Instant) -> Task<Message> {
        match message {
            Message::Cals(Ok(mut cals)) => {
                cals.sort_by_key(|cal| cal.name.to_lowercase());
                self.cals = cals;
                return self.reload(shell);
            }
            Message::Cals(Err(error)) => shell.fail(error, now),

            Message::Items(generation, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(items) => self.items = items,
                    Err(error) => shell.fail(error, now),
                }
            }

            Message::Toggle(id) => {
                if let Some(cal) = self.cals.iter_mut().find(|cal| cal.id == id) {
                    cal.hidden = !cal.hidden;
                    let visible = !cal.hidden;
                    return Task::batch([
                        Task::perform(calendar::set_visible(shell.bridge(), id, visible), Message::Visible),
                        self.reload(shell),
                    ]);
                }
            }
            Message::Visible(Err(error)) => shell.fail(error, now),
            Message::Visible(Ok(())) => {}

            Message::View(view) => {
                self.view = view;
                return self.reload_and_maybe_scroll(shell);
            }
            Message::Today => {
                self.anchor = Local::now().date_naive();
                return self.reload_and_maybe_scroll(shell);
            }
            Message::JumpTo(day) => {
                self.anchor = day;
                return self.reload_and_maybe_scroll(shell);
            }
            Message::Step(delta) => {
                if self.editor.is_some() {
                    return Task::none();
                }
                self.anchor = step(self.anchor, self.view, delta);
                return self.reload_and_maybe_scroll(shell);
            }

            Message::New(day, hour) => {
                let Some(calendar_id) = self.cals.iter().find(|cal| !cal.read_only).map(|cal| cal.id.clone()) else {
                    shell.fail("No writable calendar", now);
                    return Task::none();
                };
                self.confirming_delete = false;
                self.editor = Some(Editor::blank(calendar_id, day, hour));
                return operation::focus(Id::new(TITLE_FIELD));
            }
            Message::Open(id) => {
                if let Some(item) = self.items.iter().find(|item| item.id == id) {
                    self.confirming_delete = false;
                    self.editor = Some(Editor::from_item(item));
                    return operation::focus(Id::new(TITLE_FIELD));
                }
            }
            Message::Cancel => {
                self.editor = None;
                self.confirming_delete = false;
            }

            Message::Calendar(id) => {
                if let Some(editor) = &mut self.editor {
                    editor.calendar_id = id;
                }
            }
            Message::Field(field, value) => {
                if let Some(editor) = &mut self.editor {
                    match field {
                        Field::Title => editor.event.summary = value,
                        Field::Location => editor.event.location = value,
                        Field::Description => editor.event.description = value,
                        Field::StartDate => editor.start_date = value,
                        Field::StartTime => editor.start_time = value,
                        Field::EndDate => editor.end_date = value,
                        Field::EndTime => editor.end_time = value,
                    }
                }
            }
            Message::AllDay(all_day) => {
                if let Some(editor) = &mut self.editor {
                    editor.all_day = all_day;
                }
            }
            Message::Recur(recur) => {
                if let Some(editor) = &mut self.editor {
                    editor.event.rrule = recur.encode();
                }
            }
            Message::ToggleReminder(minutes) => {
                if let Some(editor) = &mut self.editor {
                    let duration = ChronoDuration::minutes(minutes);
                    match editor.event.alarms.iter().position(|alarm| *alarm == duration) {
                        Some(index) => {
                            editor.event.alarms.remove(index);
                        }
                        None => editor.event.alarms.push(duration),
                    }
                }
            }

            Message::Save => return self.save(shell, now),
            Message::Saved(Ok(_)) => {
                self.editor = None;
                shell.announce("Saved", now);
                return self.reload(shell);
            }
            Message::Saved(Err(error)) => shell.fail(error, now),

            Message::AskDelete => self.confirming_delete = true,
            Message::Delete => {
                self.confirming_delete = false;
                if let Some(editor) = &self.editor
                    && let Some(id) = editor.id.clone()
                {
                    return Task::perform(
                        calendar::remove(shell.bridge(), editor.calendar_id.clone(), id),
                        Message::Deleted,
                    );
                }
            }
            Message::Deleted(Ok(())) => {
                self.editor = None;
                shell.announce("Deleted", now);
                return self.reload(shell);
            }
            Message::Deleted(Err(error)) => shell.fail(error, now),

            Message::Escape => {
                if self.editor.is_some() {
                    self.editor = None;
                    self.confirming_delete = false;
                } else if self.confirming_delete {
                    self.confirming_delete = false;
                } else {
                    shell.hush(now);
                }
            }
            Message::Refresh => return self.reload(shell),
        }
        Task::none()
    }

    fn save(&mut self, shell: &mut Shell, now: Instant) -> Task<Message> {
        let Some(editor) = &self.editor else { return Task::none() };
        if editor.event.summary.trim().is_empty() {
            shell.fail("Give the event a title", now);
            return Task::none();
        }
        let Ok(start_date) = NaiveDate::parse_from_str(editor.start_date.trim(), "%Y-%m-%d") else {
            shell.fail("Start date should look like 2026-09-14", now);
            return Task::none();
        };
        let Ok(end_date) = NaiveDate::parse_from_str(editor.end_date.trim(), "%Y-%m-%d") else {
            shell.fail("End date should look like 2026-09-14", now);
            return Task::none();
        };
        let (start, end) = if editor.all_day {
            (When::Date(start_date), When::Date(end_date.succ_opt().unwrap_or(end_date)))
        } else {
            let Ok(start_time) = NaiveTime::parse_from_str(editor.start_time.trim(), "%H:%M") else {
                shell.fail("Start time should look like 09:00", now);
                return Task::none();
            };
            let Ok(end_time) = NaiveTime::parse_from_str(editor.end_time.trim(), "%H:%M") else {
                shell.fail("End time should look like 10:00", now);
                return Task::none();
            };
            (
                When::Time(ical::local_from_naive(start_date.and_time(start_time))),
                When::Time(ical::local_from_naive(end_date.and_time(end_time))),
            )
        };
        if end.instant() <= start.instant() {
            shell.fail("End has to be after start", now);
            return Task::none();
        }

        let mut event = editor.event.clone();
        event.start = start;
        event.end = end;
        let calendar_id = editor.calendar_id.clone();
        let bridge = shell.bridge();
        let saved = |result: calendar::Result<Item>| Message::Saved(result.map(Box::new));
        match editor.id.clone() {
            Some(id) => Task::perform(calendar::update(bridge, calendar_id, id, event), saved),
            None => Task::perform(calendar::create(bridge, calendar_id, event), saved),
        }
    }

    pub fn typed(&self) -> String {
        self.pending.typed()
    }

    pub fn entered(&mut self, _now: Instant) {
        self.pending.clear();
    }

    pub fn animating(&self, _now: Instant) -> bool {
        false
    }

    pub fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Message> {
        match KEYS.with(|keys| keys.press(&mut self.pending, key, modifiers)) {
            keymap::Resolved::Ignored => Pressed::Ignored,
            keymap::Resolved::Pending => Pressed::Pending,
            keymap::Resolved::Action(Binding::Go(surface), _) => Pressed::Switch(surface),
            keymap::Resolved::Action(binding, count) => Pressed::Act(match binding {
                Binding::Today => Message::Today,
                Binding::Next => Message::Step(count as i32),
                Binding::Prev => Message::Step(-(count as i32)),
                Binding::New => Message::New(self.anchor, Some(9)),
                Binding::Month => Message::View(ViewKind::Month),
                Binding::Week => Message::View(ViewKind::Week),
                Binding::Day => Message::View(ViewKind::Day),
                Binding::Agenda => Message::View(ViewKind::Agenda),
                Binding::Refresh => Message::Refresh,
                Binding::Escape => Message::Escape,
                Binding::Go(_) => unreachable!("handled above"),
            }),
        }
    }

    pub fn resync(&mut self, shell: &Shell) -> Task<Message> {
        Task::perform(calendar::calendars(shell.bridge()), Message::Cals)
    }

    pub fn notify(&mut self, name: &str, shell: &Shell) -> Task<Message> {
        if name.starts_with("calendar.calendars.") {
            return self.resync(shell);
        }
        if name.starts_with("calendar.items.") {
            return self.reload(shell);
        }
        Task::none()
    }

    fn reload(&mut self, shell: &Shell) -> Task<Message> {
        self.generation += 1;
        let generation = self.generation;
        let ids: Vec<String> = self.cals.iter().filter(|cal| !cal.hidden).map(|cal| cal.id.clone()).collect();
        let (start, end) = self.range();
        self.loading = true;
        let load = calendar::items(shell.bridge(), ids, ical::local_midnight(start), ical::local_midnight(end));
        Task::perform(load, move |result| Message::Items(generation, result))
    }

    /// A reload, and — for Week or Day — a scroll to a reasonable working hour rather than
    /// whatever the timeline was last left at. Without this, switching to Day always shows an
    /// empty small-hours grid, since a fresh view opens scrolled to midnight.
    fn reload_and_maybe_scroll(&mut self, shell: &Shell) -> Task<Message> {
        let reload = self.reload(shell);
        match self.view {
            ViewKind::Week | ViewKind::Day => Task::batch([reload, scroll_to_morning()]),
            ViewKind::Month | ViewKind::Agenda => reload,
        }
    }

    /// The `[start, end)` range the current view needs events for.
    fn range(&self) -> (NaiveDate, NaiveDate) {
        match self.view {
            ViewKind::Month => {
                let start = month_grid_start(self.anchor);
                (start, start + ChronoDuration::days(42))
            }
            ViewKind::Week => {
                let start = week_start(self.anchor);
                (start, start + ChronoDuration::days(7))
            }
            ViewKind::Day => (self.anchor, self.anchor + ChronoDuration::days(1)),
            ViewKind::Agenda => (self.anchor, self.anchor + ChronoDuration::days(30)),
        }
    }

    fn title(&self) -> String {
        match self.view {
            ViewKind::Month => self.anchor.format("%B %Y").to_string(),
            ViewKind::Week => {
                let start = week_start(self.anchor);
                let end = start + ChronoDuration::days(6);
                if start.month() == end.month() {
                    format!("{} – {}, {}", start.format("%b %-d"), end.format("%-d"), end.year())
                } else {
                    format!("{} – {}", start.format("%b %-d"), end.format("%b %-d, %Y"))
                }
            }
            ViewKind::Day => self.anchor.format("%A, %B %-d").to_string(),
            ViewKind::Agenda => "Upcoming".to_string(),
        }
    }

    fn cal_index(&self, id: &str) -> usize {
        self.cals.iter().position(|cal| cal.id == id).unwrap_or(0)
    }

    // ── View ────────────────────────────────────────────────────────────────

    pub fn view(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        if let Some(editor) = &self.editor {
            let width = self.grid_width(shell.width()).min(860.0);
            return row![self.rail(), ui::hairline_y(), self.editor_view(editor, width)].height(Length::Fill).into();
        }
        row![self.rail(), ui::hairline_y(), self.body(shell, now)].height(Length::Fill).into()
    }

    /// What the grid has to draw in, once the rail and the body's own padding have taken their
    /// share. The week/day timeline needs this as a real number rather than a `Length::Fill` —
    /// `pin` places its content at an absolute pixel offset, so a day column's `x` has to be a
    /// pixel too.
    fn grid_width(&self, window_width: f32) -> f32 {
        (window_width - RAIL_WIDTH - theme::BORDER - 2.0 * theme::SPACE_LG).max(200.0)
    }

    fn rail(&self) -> Element<'_, Message> {
        let mut calendars = column![].spacing(theme::SPACE_XS);
        for (index, cal) in self.cals.iter().enumerate() {
            let (swatch, _) = cal_color(index);
            let row = row![
                container(space().width(10).height(10)).style(move |_| container::Style {
                    background: Some(swatch.into()),
                    border: iced::border::rounded(3),
                    ..container::Style::default()
                }),
                text(cal.name.clone()).size(theme::FONT_CAPTION).wrapping(text::Wrapping::None),
                space().width(Length::Fill),
                checkbox(!cal.hidden)
                    .on_toggle({
                        let id = cal.id.clone();
                        move |_| Message::Toggle(id.clone())
                    })
                    .style(theme::checkbox_style)
                    .size(16),
            ]
            .spacing(theme::SPACE_XS)
            .align_y(Alignment::Center);
            calendars = calendars.push(row);
        }

        container(
            column![
                widgets::action("New event", ButtonVariant::Primary, Some(Message::New(self.anchor, Some(9)))),
                self.mini_month(),
                ui::hairline_x(),
                ui::caption("Calendars"),
                scrollable(calendars).style(theme::scrollable_style).height(Length::Fill),
            ]
            .spacing(theme::SPACE_MD),
        )
        .width(RAIL_WIDTH)
        .height(Length::Fill)
        .padding(theme::SPACE_MD)
        .into()
    }

    /// A small month grid for navigation. gcal's mini-month, without a calendar widget the
    /// toolkit does not have: plain buttons in a 7-wide grid.
    fn mini_month(&self) -> Element<'_, Message> {
        let start = month_grid_start(self.anchor);
        let today = Local::now().date_naive();
        let mut grid = column![].spacing(2);
        for week in 0..6 {
            let mut week_row = row![].spacing(2);
            for day_offset in 0..7 {
                let day = start + ChronoDuration::days(week * 7 + day_offset);
                let in_month = day.month() == self.anchor.month();
                let label = text(day.day().to_string()).size(theme::FONT_MINI);
                let label = if day == today {
                    label.color(theme::palette().primary).font(theme::semibold())
                } else if in_month {
                    label.color(theme::palette().on_surface)
                } else {
                    label.color(theme::palette().on_surface_variant)
                };
                let cell = button(container(label).center_x(MINI_DAY).center_y(MINI_DAY))
                    .width(MINI_DAY)
                    .height(MINI_DAY)
                    .padding(0)
                    .style(theme::bare_button)
                    .on_press(Message::JumpTo(day));
                week_row = week_row.push(cell);
            }
            grid = grid.push(week_row);
        }
        grid.into()
    }

    fn body(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        let labels: Vec<&str> = ViewKind::ALL.iter().map(|kind| kind.label()).collect();
        let switcher = widgets::segmented(&labels, self.view.index(), |index| Message::View(ViewKind::ALL[index]));
        let header = row![
            widgets::action("Today", ButtonVariant::Tab, Some(Message::Today)),
            widgets::action("‹", ButtonVariant::Tab, Some(Message::Step(-1))),
            widgets::action("›", ButtonVariant::Tab, Some(Message::Step(1))),
            text(self.title()).size(theme::FONT_TITLE).font(theme::semibold()),
            space().width(Length::Fill),
            switcher,
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);

        let width = self.grid_width(shell.width());
        let grid: Element<Message> = match self.view {
            ViewKind::Month => self.month_view(),
            ViewKind::Week => self.timeline(&week_days(week_start(self.anchor)), width),
            ViewKind::Day => self.timeline(&[self.anchor], width),
            ViewKind::Agenda => self.agenda_view(width, now),
        };

        container(column![header, ui::hairline_x(), grid].spacing(theme::SPACE_MD).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(theme::SPACE_LG)
            .into()
    }

    // ── Month ──────────────────────────────────────────────────────────────

    fn month_view(&self) -> Element<'_, Message> {
        let start = month_grid_start(self.anchor);
        let today = Local::now().date_naive();

        let mut header = row![].spacing(theme::SPACE_XS);
        for day_offset in 0..7 {
            let day = start + ChronoDuration::days(day_offset);
            header = header
                .push(container(ui::caption(day.format("%a").to_string())).width(Length::Fill).center_x(Length::Fill));
        }

        let mut weeks = column![].spacing(theme::SPACE_XS).height(Length::Fill);
        for week in 0..6 {
            let mut week_row = row![].spacing(theme::SPACE_XS).height(Length::FillPortion(1));
            for day_offset in 0..7 {
                let day = start + ChronoDuration::days(week * 7 + day_offset);
                week_row = week_row.push(self.day_cell(day, day.month() == self.anchor.month(), day == today));
            }
            weeks = weeks.push(week_row);
        }
        column![header, weeks].spacing(theme::SPACE_XS).height(Length::Fill).into()
    }

    fn day_cell(&self, day: NaiveDate, in_month: bool, is_today: bool) -> Element<'_, Message> {
        let events: Vec<&Item> = self.items.iter().filter(|item| item.event.spans(day)).collect();
        let label = text(day.day().to_string()).size(theme::FONT_CAPTION);
        let label = if is_today {
            container(label.color(theme::palette().on_primary).font(theme::semibold()))
                .padding(Padding::from([0.0, theme::SPACE_XS]))
                .style(|_| container::Style {
                    background: Some(theme::palette().primary.into()),
                    border: iced::border::rounded(theme::RADIUS_SM),
                    ..container::Style::default()
                })
        } else {
            container(label.color(if in_month {
                theme::palette().on_surface
            } else {
                theme::palette().on_surface_variant
            }))
        };

        let mut chips = column![label].spacing(2);
        for item in events.iter().take(CHIPS_PER_CELL) {
            chips = chips.push(event_chip(item, self.cal_index(&item.calendar_id)));
        }
        if events.len() > CHIPS_PER_CELL {
            chips = chips.push(ui::caption(format!("+{} more", events.len() - CHIPS_PER_CELL)));
        }

        let content =
            container(chips.padding(theme::SPACE_XS)).width(Length::Fill).height(Length::Fill).style(theme::track);
        mouse_area(content).on_press(Message::New(day, Some(9))).into()
    }

    // ── Week / Day ─────────────────────────────────────────────────────────

    /// A gcal-style hour grid: `days.len()` columns (one for Day view, seven for Week), each hour
    /// `HOUR_HEIGHT` tall. `width` is the grid's real pixel width — known statically from the
    /// window's own width ([`Shell::width`]) rather than measured, because `pin` places a chip at
    /// an absolute pixel offset and needs one to place it with.
    fn timeline<'a>(&'a self, days: &[NaiveDate], width: f32) -> Element<'a, Message> {
        let all_day: Vec<&Item> = self
            .items
            .iter()
            .filter(|item| item.event.start.is_all_day() || item.event.last_day() != item.event.start.date())
            .collect();
        let mut all_day_row = row![space().width(GUTTER)].spacing(theme::SPACE_XS);
        for &day in days {
            let mut cell = column![].spacing(2);
            for item in all_day.iter().filter(|item| item.event.spans(day)) {
                cell = cell.push(event_chip(item, self.cal_index(&item.calendar_id)));
            }
            all_day_row = all_day_row.push(container(cell).width(Length::Fill));
        }

        let mut day_header = row![space().width(GUTTER)].spacing(theme::SPACE_XS);
        let today = Local::now().date_naive();
        for &day in days {
            let label = text(day.format("%a %-d").to_string()).size(theme::FONT_CAPTION);
            let label =
                if day == today { label.color(theme::palette().primary).font(theme::semibold()) } else { label };
            day_header = day_header.push(container(label).width(Length::Fill).center_x(Length::Fill));
        }

        const TOTAL_HEIGHT: f32 = 24.0 * HOUR_HEIGHT;
        let day_width = ((width - GUTTER - theme::SPACE_XS * days.len() as f32) / days.len() as f32).max(60.0);

        let hours = column((0..24).map(|hour| {
            let label = container(ui::caption(hour_label(hour))).width(GUTTER).height(HOUR_HEIGHT);
            let mut cells = row![label].spacing(theme::SPACE_XS);
            for &day in days {
                let cell = mouse_area(container(space().width(Length::Fill).height(HOUR_HEIGHT)).style(theme::track))
                    .on_press(Message::New(day, Some(hour as u32)));
                cells = cells.push(container(cell).width(Length::Fill));
            }
            cells.into()
        }));

        let timed: Vec<&Item> =
            self.items.iter().filter(|item| !all_day.iter().any(|other| other.id == item.id)).collect();
        let mut layers: Vec<Element<Message>> = vec![hours.into()];
        for (day_index, &day) in days.iter().enumerate() {
            let mut on_day: Vec<&Item> = timed.iter().filter(|item| item.event.spans(day)).copied().collect();
            on_day.sort_by_key(|item| item.event.start.instant());
            let (lane_of, lane_count) = lanes(&on_day);
            let lane_width = day_width / lane_count as f32;
            let day_start = ical::local_midnight(day);
            for (row_index, item) in on_day.iter().enumerate() {
                let started_before = item.event.start.instant() < day_start;
                let ends_after = item.event.end.instant() > day_start + ChronoDuration::days(1);
                let start_minutes =
                    if started_before { 0.0 } else { (item.event.start.instant() - day_start).num_minutes() as f32 };
                let end_minutes =
                    if ends_after { 24.0 * 60.0 } else { (item.event.end.instant() - day_start).num_minutes() as f32 };
                let y = start_minutes / 60.0 * HOUR_HEIGHT;
                let height = ((end_minutes - start_minutes) / 60.0 * HOUR_HEIGHT).max(18.0);
                let x = GUTTER
                    + theme::SPACE_XS
                    + day_index as f32 * (day_width + theme::SPACE_XS)
                    + lane_of[row_index] as f32 * lane_width;
                let chip = container(event_chip(item, self.cal_index(&item.calendar_id)))
                    .width((lane_width - 2.0).max(1.0))
                    .height(height);
                layers.push(pin(chip).x(x).y(y).width(Length::Fill).height(TOTAL_HEIGHT).into());
            }
        }

        let grid = scrollable(Stack::with_children(layers).width(Length::Fill).height(TOTAL_HEIGHT))
            .id(Id::new(TIMELINE_ID))
            .style(theme::scrollable_style)
            .height(Length::Fill);
        column![day_header, all_day_row, ui::hairline_x(), grid].spacing(theme::SPACE_XS).height(Length::Fill).into()
    }

    // ── Agenda ─────────────────────────────────────────────────────────────

    /// `width` is the same real pixel width `timeline` uses, capped — an agenda row is a line to
    /// read, and a line stretched across a wide window is not more readable for it. Capped with an
    /// explicit `Length::Fixed` rather than `Container::max_width`, since a `max_width`'d `Shrink`
    /// container does not reliably bound a `Length::Fill` row nested inside a `scrollable`.
    fn agenda_view(&self, width: f32, _now: Instant) -> Element<'_, Message> {
        if self.items.is_empty() {
            return ui::placeholder(icon::CALENDAR, "Nothing coming up", "The next 30 days are clear");
        }
        let mut days: Vec<NaiveDate> = self.items.iter().map(|item| item.event.start.date()).collect();
        days.sort();
        days.dedup();

        let mut list = column![].spacing(theme::SPACE_MD).width(Length::Fixed(width.min(720.0)));
        for day in days {
            let mut on_day: Vec<&Item> = self.items.iter().filter(|item| item.event.spans(day)).collect();
            on_day.sort_by_key(|item| item.event.start.instant());
            let mut rows = column![].spacing(ROW_GAP);
            for item in &on_day {
                rows = rows.push(agenda_row(item, self.cal_index(&item.calendar_id)));
            }
            let heading = text(day.format("%A, %B %-d").to_string()).size(theme::FONT_CAPTION).font(theme::semibold());
            list = list.push(column![heading, rows].spacing(theme::SPACE_XS));
        }
        scrollable(list).style(theme::scrollable_style).height(Length::Fill).into()
    }

    // ── Editor ─────────────────────────────────────────────────────────────

    fn editor_view<'a>(&'a self, editor: &'a Editor, width: f32) -> Element<'a, Message> {
        let title = if editor.id.is_some() { "Edit event" } else { "New event" };
        let mut heading = row![
            text(title).size(theme::FONT_TITLE).font(theme::semibold()).width(Length::Fill),
            widgets::action("Cancel", ButtonVariant::Tab, Some(Message::Cancel)),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center);
        if editor.id.is_some() {
            heading = heading.push(if self.confirming_delete {
                ui::danger_button("Really delete?", Message::Delete)
            } else {
                ui::danger_button("Delete", Message::AskDelete)
            });
        }
        heading = heading.push(widgets::action("Save", ButtonVariant::Primary, Some(Message::Save)));

        let choices: Vec<Choice> = self
            .cals
            .iter()
            .filter(|cal| !cal.read_only)
            .map(|cal| Choice { id: cal.id.clone(), label: cal.name.clone() })
            .collect();
        let chosen = choices.iter().find(|choice| choice.id == editor.calendar_id).cloned();

        let field = |label: &'a str, id: Option<&'static str>, value: &str, which: Field| {
            let mut input = text_input("", value)
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

        let dates = if editor.all_day {
            row![
                field("Start", None, &editor.start_date, Field::StartDate),
                field("End", None, &editor.end_date, Field::EndDate)
            ]
        } else {
            row![
                field("Start date", None, &editor.start_date, Field::StartDate),
                field("Start time", None, &editor.start_time, Field::StartTime),
                field("End date", None, &editor.end_date, Field::EndDate),
                field("End time", None, &editor.end_time, Field::EndTime),
            ]
        }
        .spacing(theme::SPACE_SM);

        let recur = editor.event.recur();
        let recur_picker = column![
            ui::caption("Repeats"),
            pick_list(&Recur::PRESETS[..], (recur != Recur::Custom).then_some(recur), Message::Recur)
                .style(theme::pick_list_style)
                .text_size(theme::FONT_BODY),
        ]
        .spacing(theme::SPACE_XS);

        let mut reminders = column![ui::caption("Reminders")].spacing(theme::SPACE_XS);
        for (label, minutes) in REMINDER_PRESETS {
            let on = editor.event.alarms.iter().any(|alarm| alarm.num_minutes() == minutes);
            reminders = reminders.push(
                checkbox(on)
                    .label(label)
                    .on_toggle(move |_| Message::ToggleReminder(minutes))
                    .style(theme::checkbox_style),
            );
        }

        let attendees: Element<Message> = if editor.event.organizer.is_some() || !editor.event.attendees.is_empty() {
            let mut names = Vec::new();
            if let Some(organizer) = &editor.event.organizer {
                names.push(format!("{organizer} (organizer)"));
            }
            names.extend(editor.event.attendees.iter().cloned());
            column![ui::caption("Guests"), text(names.join(", ")).size(theme::FONT_CAPTION)]
                .spacing(theme::SPACE_XS)
                .into()
        } else {
            space().height(0).into()
        };

        let content = column![
            heading,
            ui::hairline_x(),
            field("Title", Some(TITLE_FIELD), &editor.event.summary, Field::Title),
            row![
                column![
                    ui::caption("Calendar"),
                    pick_list(choices, chosen, |choice: Choice| Message::Calendar(choice.id))
                        .style(theme::pick_list_style)
                        .text_size(theme::FONT_BODY)
                        .width(Length::Fill),
                ]
                .spacing(theme::SPACE_XS)
                .width(Length::Fill),
                column![
                    checkbox(editor.all_day).label("All day").on_toggle(Message::AllDay).style(theme::checkbox_style)
                ]
                .width(Length::Shrink),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Alignment::Center),
            dates,
            field("Location", None, &editor.event.location, Field::Location),
            field("Description", None, &editor.event.description, Field::Description),
            recur_picker,
            reminders,
            attendees,
        ]
        .spacing(theme::SPACE_MD)
        .width(Length::Fixed(width));

        // An explicit `Length::Fixed` rather than `Container::max_width` — see `agenda_view`.
        container(scrollable(content).style(theme::scrollable_style))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(theme::SPACE_LG)
            .into()
    }
}

/// A calendar picker's entry.
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

/// One event, drawn as a small filled chip in whichever calendar's colour it belongs to.
fn event_chip<'a>(item: &'a Item, cal_index: usize) -> Element<'a, Message> {
    let (background, on) = cal_color(cal_index);
    let time = match &item.event.start {
        When::Time(time) => time.format("%-I:%M%p").to_string().to_lowercase(),
        When::Date(_) => String::new(),
    };
    let repeats = item.event.rrule.is_some();
    let mut label = item.event.summary.clone();
    if label.is_empty() {
        label = "(untitled)".to_string();
    }
    let content = row![
        text(if time.is_empty() { label.clone() } else { format!("{time} {label}") })
            .size(theme::FONT_MINI)
            .wrapping(text::Wrapping::None),
        space().width(Length::Fill),
        if repeats { widgets::icon(icon::REFRESH, theme::FONT_MINI).into() } else { Element::from(space().width(0)) },
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    button(content)
        .padding(Padding::from([1.0, theme::SPACE_XS]))
        .width(Length::Fill)
        .style(move |_, status| {
            let hovered =
                matches!(status, iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed);
            iced::widget::button::Style {
                background: Some(theme::alpha(background, if hovered { 1.0 } else { 0.85 }).into()),
                text_color: on,
                border: iced::border::rounded(theme::RADIUS_SM),
                ..iced::widget::button::Style::default()
            }
        })
        .on_press(Message::Open(item.id.clone()))
        .into()
}

fn agenda_row<'a>(item: &'a Item, cal_index: usize) -> Element<'a, Message> {
    let (swatch, _) = cal_color(cal_index);
    let when = match &item.event.start {
        When::Time(time) => time.format("%-I:%M %p").to_string(),
        When::Date(_) => "All day".to_string(),
    };
    let mut label = item.event.summary.clone();
    if label.is_empty() {
        label = "(untitled)".to_string();
    }
    let content = row![
        container(space().width(4).height(Length::Fill))
            .style(move |_| container::Style { background: Some(swatch.into()), ..container::Style::default() }),
        column![text(label).size(theme::FONT_BODY), ui::caption(when)].spacing(1),
        space().width(Length::Fill),
        if item.event.rrule.is_some() {
            widgets::icon(icon::REFRESH, theme::FONT_CAPTION).color(theme::palette().on_surface_variant).into()
        } else {
            Element::from(space().width(0))
        },
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Alignment::Center);
    button(content)
        .padding(theme::SPACE_XS)
        .width(Length::Fill)
        .style(theme::bare_button)
        .on_press(Message::Open(item.id.clone()))
        .into()
}

/// Cycles a calendar's identity colour through the three "loud" roles the rest of the design
/// language already treats that way, rather than trusting Thunderbird's own arbitrary hex.
fn cal_color(index: usize) -> (Color, Color) {
    let palette = theme::palette();
    match index % 3 {
        0 => (palette.primary, palette.on_primary),
        1 => (palette.secondary, palette.on_secondary),
        _ => (palette.tertiary, palette.on_tertiary),
    }
}

/// The Monday on or before `anchor`'s month starts — where a gcal-style month grid begins.
fn month_grid_start(anchor: NaiveDate) -> NaiveDate {
    let first = anchor.with_day(1).unwrap_or(anchor);
    week_start(first)
}

fn week_start(day: NaiveDate) -> NaiveDate {
    let back = day.weekday().num_days_from_monday();
    day - ChronoDuration::days(back as i64)
}

fn week_days(start: NaiveDate) -> [NaiveDate; 7] {
    std::array::from_fn(|index| start + ChronoDuration::days(index as i64))
}

/// Moves `anchor` by `delta` units of whatever the active view steps by: a day for Day and
/// Agenda, a week for Week, a month for Month.
fn step(anchor: NaiveDate, view: ViewKind, delta: i32) -> NaiveDate {
    match view {
        ViewKind::Day | ViewKind::Agenda => anchor + ChronoDuration::days(delta as i64),
        ViewKind::Week => anchor + ChronoDuration::days(delta as i64 * 7),
        ViewKind::Month => {
            let month = anchor.month0() as i32 + delta;
            let year = anchor.year() + month.div_euclid(12);
            let month = month.rem_euclid(12) as u32 + 1;
            NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(anchor)
        }
    }
}

fn hour_label(hour: usize) -> String {
    match hour {
        0 => String::new(),
        h if h < 12 => format!("{h} AM"),
        12 => "12 PM".to_string(),
        h => format!("{} PM", h - 12),
    }
}

/// Greedy lane assignment for overlapping events on one day — the same algorithm gcal's own week
/// view uses: sort by start, and give each event the first lane whose last event has already
/// ended. Returns each event's lane and the number of lanes in use, so callers can divide the
/// day's width evenly.
fn lanes(items: &[&Item]) -> (Vec<usize>, usize) {
    let mut lane_ends: Vec<chrono::DateTime<Local>> = Vec::new();
    let mut assignment = Vec::with_capacity(items.len());
    for item in items {
        let start = item.event.start.instant();
        match lane_ends.iter().position(|end| *end <= start) {
            Some(lane) => {
                lane_ends[lane] = item.event.end.instant();
                assignment.push(lane);
            }
            None => {
                lane_ends.push(item.event.end.instant());
                assignment.push(lane_ends.len() - 1);
            }
        }
    }
    let lanes = lane_ends.len().max(1);
    (assignment, lanes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_binding_is_a_prefix_of_another() {
        KEYS.with(|keys| assert_eq!(keys.conflicts(), Vec::<(String, String)>::new()));
    }

    #[test]
    fn the_month_grid_starts_on_the_monday_before_the_first() {
        // September 2026 starts on a Tuesday.
        let anchor = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        let start = month_grid_start(anchor);
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 8, 31).unwrap());
        assert_eq!(start.weekday(), Weekday::Mon);
    }

    #[test]
    fn stepping_a_month_view_moves_a_whole_month_and_wraps_the_year() {
        let december = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        assert_eq!(step(december, ViewKind::Month, 1), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(step(december, ViewKind::Month, -1), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    }

    #[test]
    fn overlapping_events_take_separate_lanes_and_a_later_one_reuses_a_freed_lane() {
        let base = ical::local_midnight(NaiveDate::from_ymd_opt(2026, 9, 14).unwrap());
        let at = |h: i64, minutes: i64| base + ChronoDuration::hours(h) + ChronoDuration::minutes(minutes);
        let make = |start, end| Item {
            id: "x".into(),
            calendar_id: "c".into(),
            instance: None,
            event: Event::blank(When::Time(start), When::Time(end)),
        };
        let a = make(at(9, 0), at(10, 0));
        let b = make(at(9, 30), at(10, 30));
        let c = make(at(10, 0), at(11, 0));
        let items = [&a, &b, &c];
        let (assignment, lanes) = lanes(&items);
        assert_eq!(lanes, 2, "a and b overlap and need two lanes");
        assert_ne!(assignment[0], assignment[1], "overlapping events do not share a lane");
        assert_eq!(assignment[2], assignment[0], "c starts after a ends and can reuse its lane");
    }
}
