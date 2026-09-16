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
use crate::surfaces::{self, Face, Pressed, Surface};
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
use serde_json::Value;
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

/// What `calendar.items.onAlarm`'s toast says — the event's own title, named plainly rather than
/// left as "an event" when the payload parses (it always should; the fallback is for a shape this
/// has never actually been seen in).
fn alarm_message(data: &Value) -> String {
    let title =
        data.get("item").cloned().and_then(|node| calendar::item_from_node(node).ok()).map(|item| item.event.summary);
    match title {
        Some(title) => format!("Reminder — {title}"),
        None => "Reminder".to_string(),
    }
}

// ── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    /// Right-clicked the description field, tagged `"event-description"`. Caught by
    /// `App::update` before it reaches here — see `mail::Message::ContextMenu`'s doc comment.
    ContextMenu(String, &'static str),
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
    /// The series an occurrence belongs to, fetched so it can be edited as itself.
    Master(calendar::Result<Box<Item>>),
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
    /// Thunderbird's alarm service fired a reminder — `calendar.items.onAlarm` — with the
    /// message already reduced to what the toast says.
    Alarm(String),
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
    Save,
    Escape,
    Go(Surface),
}

thread_local! {
    static KEYS: Keymap<Binding> = surfaces::switches(
        Keymap::new()
            .counted()
            .bind("<C-CR>", Binding::Save)
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

/// The panel that creates or edits one event: what is being typed, and nothing else.
///
/// Every field is the text in its box. Dates and times are made sense of in [`compose`], so a
/// half-typed date does not fight the user for what it means yet. What the form does not show —
/// the UID, the organiser and guests, a custom recurrence rule — rides along in `original` and
/// goes back out untouched, which is the whole reason the [`Event`] is kept apart from the form
/// rather than edited in place.
struct Editor {
    /// `None` while creating.
    id: Option<String>,
    calendar_id: String,
    /// The event as Thunderbird holds it, for everything the form does not edit. `None` while
    /// creating.
    original: Option<Event>,
    title: String,
    location: String,
    description: String,
    /// The raw `RRULE`, kept opaque so a custom rule survives a save that does not touch it.
    rrule: Option<String>,
    alarms: Vec<ChronoDuration>,
    all_day: bool,
    start_date: String,
    start_time: String,
    /// The last calendar day, inclusive — not the exclusive bound the format uses — because that
    /// is how a person names the last day of a trip.
    end_date: String,
    end_time: String,
}

impl Editor {
    fn blank(calendar_id: String, day: NaiveDate, hour: Option<u32>) -> Editor {
        let all_day = hour.is_none();
        let start_time = hour.map_or_else(|| "09:00".to_string(), |hour| format!("{hour:02}:00"));
        let end_time = hour.map_or_else(|| "10:00".to_string(), |hour| format!("{:02}:00", (hour + 1).min(23)));
        Editor {
            id: None,
            calendar_id,
            original: None,
            title: String::new(),
            location: String::new(),
            description: String::new(),
            rrule: None,
            alarms: Vec::new(),
            all_day,
            start_date: format_date(day),
            start_time,
            end_date: format_date(day),
            end_time,
        }
    }

    fn from_item(item: &Item) -> Editor {
        let event = &item.event;
        let all_day = event.start.is_all_day();
        let end_date = if all_day { event.last_day() } else { event.end.date() };
        // For an all-day event the times are only what a switch to timed would start from.
        let time_of = |when: &When, fallback: &str| match when {
            When::Time(time) => time.format("%H:%M").to_string(),
            When::Date(_) => fallback.to_string(),
        };
        Editor {
            id: Some(item.id.clone()),
            calendar_id: item.calendar_id.clone(),
            title: event.summary.clone(),
            location: event.location.clone(),
            description: event.description.clone(),
            rrule: event.rrule.clone(),
            alarms: event.alarms.clone(),
            all_day,
            start_date: format_date(event.start.date()),
            start_time: time_of(&event.start, "09:00"),
            end_date: format_date(end_date),
            end_time: time_of(&event.end, "10:00"),
            original: Some(event.clone()),
        }
    }

    /// Whether a save changes every occurrence of a series, which is worth saying on the panel.
    fn series(&self) -> bool {
        self.rrule.is_some()
    }

    /// The event the form describes, or the one thing wrong with it, in the words the banner
    /// says.
    fn compose(&self) -> Result<Event, &'static str> {
        if self.title.trim().is_empty() {
            return Err("Give the event a title");
        }
        let start_date = NaiveDate::parse_from_str(self.start_date.trim(), "%Y-%m-%d")
            .map_err(|_| "Start date should look like 2026-09-14")?;
        let end_date = NaiveDate::parse_from_str(self.end_date.trim(), "%Y-%m-%d")
            .map_err(|_| "End date should look like 2026-09-14")?;
        let (start, end) = if self.all_day {
            (When::Date(start_date), When::Date(end_date.succ_opt().unwrap_or(end_date)))
        } else {
            let start_time = NaiveTime::parse_from_str(self.start_time.trim(), "%H:%M")
                .map_err(|_| "Start time should look like 09:00")?;
            let end_time = NaiveTime::parse_from_str(self.end_time.trim(), "%H:%M")
                .map_err(|_| "End time should look like 10:00")?;
            (
                When::Time(ical::local_from_naive(start_date.and_time(start_time))),
                When::Time(ical::local_from_naive(end_date.and_time(end_time))),
            )
        };
        if end.instant() <= start.instant() {
            return Err("End has to be after start");
        }
        let mut event = self.original.clone().unwrap_or_else(|| Event::blank(start.clone(), end.clone()));
        event.summary = self.title.clone();
        event.location = self.location.clone();
        event.description = self.description.clone();
        event.rrule = self.rrule.clone();
        event.alarms = self.alarms.clone();
        event.start = start;
        event.end = end;
        Ok(event)
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

    fn save(&mut self, shell: &mut Shell, now: Instant) -> Task<Message> {
        let Some(editor) = &self.editor else { return Task::none() };
        let event = match editor.compose() {
            Ok(event) => event,
            Err(what) => {
                shell.fail(what, now);
                return Task::none();
            }
        };
        let calendar_id = editor.calendar_id.clone();
        let bridge = shell.bridge();
        let saved = |result: calendar::Result<Item>| Message::Saved(result.map(Box::new));
        match editor.id.clone() {
            Some(id) => Task::perform(calendar::update(bridge, calendar_id, id, event), saved),
            None => Task::perform(calendar::create(bridge, calendar_id, event), saved),
        }
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
        let title = match (editor.id.is_some(), editor.series()) {
            (false, _) => "New event",
            (true, false) => "Edit event",
            (true, true) => "Edit series",
        };
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

        let recur = Recur::decode(editor.rrule.as_deref());
        let recur_picker = column![
            ui::caption("Repeats"),
            pick_list(&Recur::PRESETS[..], (recur != Recur::Custom).then_some(recur), Message::Recur)
                .style(theme::pick_list_style)
                .text_size(theme::FONT_BODY),
        ]
        .spacing(theme::SPACE_XS);

        let mut reminders = column![ui::caption("Reminders")].spacing(theme::SPACE_XS);
        for (label, minutes) in REMINDER_PRESETS {
            let on = editor.alarms.iter().any(|alarm| alarm.num_minutes() == minutes);
            reminders = reminders.push(
                checkbox(on)
                    .label(label)
                    .on_toggle(move |_| Message::ToggleReminder(minutes))
                    .style(theme::checkbox_style),
            );
        }

        let original = editor.original.as_ref();
        let organizer = original.and_then(|event| event.organizer.as_ref());
        let guests = original.map(|event| event.attendees.as_slice()).unwrap_or_default();
        let attendees: Element<Message> = if organizer.is_some() || !guests.is_empty() {
            let mut names = Vec::new();
            if let Some(organizer) = organizer {
                names.push(format!("{organizer} (organizer)"));
            }
            names.extend(guests.iter().cloned());
            column![ui::caption("Guests"), text(names.join(", ")).size(theme::FONT_CAPTION)]
                .spacing(theme::SPACE_XS)
                .into()
        } else {
            space().height(0).into()
        };

        let content = column![
            heading,
            ui::hairline_x(),
            field("Title", Some(TITLE_FIELD), &editor.title, Field::Title),
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
            field("Location", None, &editor.location, Field::Location),
            // No selection to read out of a right-click — `docs/context-commands-plan.md` §0 —
            // so a template runs against the whole description, the same fallback every other
            // plain `text_input` context uses.
            mouse_area(field("Description", None, &editor.description, Field::Description))
                .on_right_press(Message::ContextMenu(editor.description.clone(), "event-description")),
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

impl Face for Calendar {
    type Message = Message;
    const SURFACE: Surface = Surface::Calendar;

    fn update(&mut self, message: Message, shell: &mut Shell, now: Instant) -> Task<Message> {
        match message {
            Message::ContextMenu(..) => {}
            Message::Cals(Ok(mut cals)) => {
                cals.sort_by_key(|cal| cal.name.to_lowercase());
                self.cals = cals;
                return self.reload(shell);
            }
            Message::Cals(Err(error)) => shell.report(&error, now),

            Message::Items(generation, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(items) => self.items = items,
                    Err(error) => shell.report(&error, now),
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
            Message::Visible(Err(error)) => shell.report(&error, now),
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
                let Some(item) = self.items.iter().find(|item| item.id == id) else { return Task::none() };
                self.confirming_delete = false;
                if item.instance.is_some() {
                    // One occurrence of a series. What Thunderbird expanded is that day's copy,
                    // with that day's start, and there is no call that edits one occurrence; a
                    // save goes to the series, so what is edited had better be the series — its
                    // own start, not the occurrence's written over it.
                    let (calendar_id, id) = (item.calendar_id.clone(), item.id.clone());
                    return Task::perform(calendar::get(shell.bridge(), calendar_id, id), |result| {
                        Message::Master(result.map(Box::new))
                    });
                }
                self.editor = Some(Editor::from_item(item));
                return operation::focus(Id::new(TITLE_FIELD));
            }
            Message::Master(Ok(item)) => {
                self.editor = Some(Editor::from_item(&item));
                return operation::focus(Id::new(TITLE_FIELD));
            }
            Message::Master(Err(error)) => shell.report(&error, now),
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
                        Field::Title => editor.title = value,
                        Field::Location => editor.location = value,
                        Field::Description => editor.description = value,
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
                    editor.rrule = recur.encode();
                }
            }
            Message::ToggleReminder(minutes) => {
                if let Some(editor) = &mut self.editor {
                    let duration = ChronoDuration::minutes(minutes);
                    match editor.alarms.iter().position(|alarm| *alarm == duration) {
                        Some(index) => {
                            editor.alarms.remove(index);
                        }
                        None => editor.alarms.push(duration),
                    }
                }
            }

            Message::Save => return self.save(shell, now),
            Message::Saved(Ok(_)) => {
                self.editor = None;
                shell.announce("Saved", now);
                return self.reload(shell);
            }
            Message::Saved(Err(error)) => shell.report(&error, now),

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
            Message::Deleted(Err(error)) => shell.report(&error, now),

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
            Message::Alarm(text) => shell.announce(text, now),
        }
        Task::none()
    }

    fn view(&self, shell: &Shell, now: Instant) -> Element<'_, Message> {
        if let Some(editor) = &self.editor {
            let width = self.grid_width(shell.width()).min(860.0);
            return row![self.rail(), ui::hairline_y(), self.editor_view(editor, width)].height(Length::Fill).into();
        }
        row![self.rail(), ui::hairline_y(), self.body(shell, now)].height(Length::Fill).into()
    }

    fn press(&mut self, key: &Key, modifiers: Modifiers) -> Pressed<Message> {
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
                Binding::Save => Message::Save,
                Binding::Escape => Message::Escape,
                Binding::Go(_) => unreachable!("handled above"),
            }),
        }
    }

    fn notify(&mut self, name: &str, data: &Value, shell: &Shell) -> Task<Message> {
        if name == "calendar.items.onAlarm" {
            // A reminder firing doesn't change what's in a folder or a range — nothing here
            // needs a reload, only the toast.
            return Task::done(Message::Alarm(alarm_message(data)));
        }
        if name.starts_with("calendar.calendars.") {
            return self.resync(shell);
        }
        if name.starts_with("calendar.items.") {
            return self.reload(shell);
        }
        Task::none()
    }

    fn resync(&mut self, shell: &Shell) -> Task<Message> {
        Task::perform(calendar::calendars(shell.bridge()), Message::Cals)
    }

    fn entered(&mut self, _now: Instant) {
        self.pending.clear();
    }

    fn animating(&self, _now: Instant) -> bool {
        false
    }

    fn typed(&self) -> String {
        self.pending.typed()
    }

    /// Whether an event is being created or edited — `docs/mode-visual-plan.md`'s "compose" mode.
    fn composing(&self) -> bool {
        self.editor.is_some()
    }

    /// The keybound actions worth finding by name — `docs/command-palette-plan.md` §3.2. Pure
    /// stepping (`Binding::Next`/`Prev`) is left out: it exists to be repeated, not looked up.
    fn commands(&self) -> Vec<crate::commands::Entry<Message>> {
        use crate::commands::Entry;
        vec![
            Entry::new("Jump to today", Some("t"), Message::Today).exposed(),
            Entry::new("New event", Some("n"), Message::New(self.anchor, Some(9))),
            Entry::new("Month view", Some("m"), Message::View(ViewKind::Month)).exposed(),
            Entry::new("Week view", Some("w"), Message::View(ViewKind::Week)).exposed(),
            Entry::new("Day view", Some("d"), Message::View(ViewKind::Day)).exposed(),
            Entry::new("Agenda view", Some("a"), Message::View(ViewKind::Agenda)).exposed(),
            Entry::new("Save event", Some("<C-CR>"), Message::Save),
            Entry::new("Refresh", Some("<C-r>"), Message::Refresh).exposed(),
        ]
    }

    /// The currently loaded rows, as jump targets for quick-open —
    /// `docs/command-palette-plan.md` §4.2.
    fn quick_items(&self) -> Vec<crate::commands::Entry<Message>> {
        // A recurring event is one thing to jump to, however many occurrences are loaded.
        let mut seen = std::collections::HashSet::new();
        self.items
            .iter()
            .filter(|item| seen.insert(item.id.as_str()))
            .map(|item| crate::commands::Entry::new(item.event.summary.clone(), None, Message::Open(item.id.clone())))
            .collect()
    }

    fn context_menu(message: &Message) -> Option<(&str, &'static str)> {
        match message {
            Message::ContextMenu(text, context) => Some((text, context)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_binding_is_a_prefix_of_another() {
        KEYS.with(|keys| assert_eq!(keys.conflicts(), Vec::<(String, String)>::new()));
    }

    #[test]
    fn a_fired_alarm_names_its_event() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e1\r\nSUMMARY:Standup\r\n\
                    DTSTART:20260101T090000Z\r\nDTEND:20260101T093000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let data = serde_json::json!({
            "item": {"id": "e1", "calendarId": "work", "item": ical},
            "alarm": {"action": "display"},
        });
        assert_eq!(alarm_message(&data), "Reminder — Standup");
    }

    #[test]
    fn an_alarm_payload_that_does_not_parse_still_toasts_something() {
        let data = serde_json::json!({"item": {"id": "e1", "calendarId": "work", "item": "not ical"}});
        assert_eq!(alarm_message(&data), "Reminder");
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

    fn shell() -> Shell {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let which = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("noctmalia-calendar-surface-{}-{which}.sock", std::process::id()));
        let mut shell = Shell::new(noctmalia_bridge::Bridge::spawn(path).expect("a socket"), 1180.0);
        shell.set_connected(true);
        shell
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a real day")
    }

    /// A weekly standup: the series starts on the 7th, and this is the occurrence on the 14th.
    fn standup() -> (Item, Item) {
        let at = |date: NaiveDate, hour: u32| When::Time(ical::local_from_naive(date.and_hms_opt(hour, 0, 0).unwrap()));
        let mut series = Event::blank(at(day(2026, 9, 7), 9), at(day(2026, 9, 7), 10));
        series.summary = "Standup".to_string();
        series.rrule = Some("FREQ=WEEKLY".to_string());
        series.organizer = Some("boss@x".to_string());
        series.attendees = vec!["me@x".to_string()];
        let mut occurrence = series.clone();
        occurrence.start = at(day(2026, 9, 14), 9);
        occurrence.end = at(day(2026, 9, 14), 10);
        let master = Item { id: "e1".into(), calendar_id: "c".into(), instance: None, event: series };
        let this_week = Item {
            id: "e1".into(),
            calendar_id: "c".into(),
            instance: Some("20260914T090000Z".into()),
            event: occurrence,
        };
        (master, this_week)
    }

    /// Editing what Thunderbird expanded would write the 14th over the series' own 7th.
    #[test]
    fn opening_an_occurrence_edits_the_series_it_belongs_to() {
        let (master, occurrence) = standup();
        let mut calendar = Calendar::new();
        let mut shell = shell();
        let now = Instant::now();
        calendar.items = vec![occurrence];
        let _ = calendar.update(Message::Open("e1".into()), &mut shell, now);
        assert!(calendar.editor.is_none(), "the series is being fetched; the occurrence is not edited as itself");
        let _ = calendar.update(Message::Master(Ok(Box::new(master))), &mut shell, now);
        let editor = calendar.editor.as_ref().expect("the series opened");
        assert!(editor.series());
        assert_eq!(editor.start_date, "2026-09-07", "the series' own start, not the occurrence's");
        assert_eq!(editor.title, "Standup");
    }

    #[test]
    fn an_event_that_does_not_recur_opens_at_once() {
        let (mut master, _) = standup();
        master.event.rrule = None;
        let mut calendar = Calendar::new();
        let mut shell = shell();
        calendar.items = vec![master];
        let _ = calendar.update(Message::Open("e1".into()), &mut shell, Instant::now());
        let editor = calendar.editor.as_ref().expect("opened");
        assert!(!editor.series());
    }

    /// The form is text; what it does not show goes back out as it came in.
    #[test]
    fn composing_keeps_what_the_form_does_not_show_and_applies_what_it_does() {
        let (master, _) = standup();
        let mut editor = Editor::from_item(&master);
        editor.title = "Standup (moved)".to_string();
        editor.start_time = "09:30".to_string();
        editor.end_time = "10:30".to_string();
        let event = editor.compose().expect("a valid form");
        assert_eq!(event.summary, "Standup (moved)");
        assert_eq!(event.uid, master.event.uid);
        assert_eq!(event.organizer.as_deref(), Some("boss@x"));
        assert_eq!(event.attendees, ["me@x"]);
        assert_eq!(event.rrule.as_deref(), Some("FREQ=WEEKLY"), "a rule the form did not touch survives");
        assert_eq!(event.start.instant().format("%H:%M").to_string(), "09:30");
    }

    #[test]
    fn composing_names_the_one_thing_wrong_with_the_form() {
        let mut editor = Editor::blank("c".into(), day(2026, 9, 14), Some(9));
        assert_eq!(editor.compose(), Err("Give the event a title"));
        editor.title = "x".to_string();
        editor.end_time = "ten".to_string();
        assert_eq!(editor.compose(), Err("End time should look like 10:00"));
        editor.end_time = "08:00".to_string();
        assert_eq!(editor.compose(), Err("End has to be after start"));
        editor.all_day = true;
        let event = editor.compose().expect("an all-day event ignores the times");
        assert!(event.start.is_all_day());
        assert_eq!(event.last_day(), day(2026, 9, 14));
    }

    #[test]
    fn quick_open_lists_a_recurring_event_once() {
        let (master, occurrence) = standup();
        let mut calendar = Calendar::new();
        calendar.items = vec![master, occurrence];
        assert_eq!(calendar.quick_items().len(), 1);
    }
}
