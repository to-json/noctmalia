//! Minimal iCalendar (RFC 5545) `VEVENT` parsing and building — enough to drive the calendar
//! surface, no more.
//!
//! Thunderbird hands calendar items over as raw ICAL text (`format: "ical"`) and takes the same
//! back on create/update; `tools/smoke.sh`'s "calendar round trip" is what proves a real headless
//! Thunderbird accepts exactly this shape, `RRULE` included. This is not a general ICAL library:
//! it covers what one `VEVENT` here needs — `UID`, `SUMMARY`, `LOCATION`, `DESCRIPTION`, `DTSTART`/
//! `DTEND`/`DURATION`, `RRULE`, `VALARM`/`TRIGGER`, `ORGANIZER`/`ATTENDEE` — and keeps the rest out
//! of scope rather than pretending to model it.
//!
//! The low-level grammar — folded lines, `NAME;PARAM=value:value`, backslash-escaping — is the
//! same family `crate::vcard` already hand-rolls, because RFC 5545 and vCard's RFC 6350 share it.
//! This mirrors that file's approach (build the dozen things a letter, or here an event, is made
//! of, rather than pull in a crate) without sharing its code: a `VEVENT` nests components
//! (`VALARM`) where a vCard has no nesting at all, so the tokenizer here builds a small component
//! tree instead of a flat property list.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// A point in time an event's `DTSTART`/`DTEND` can be. RFC 5545 gives a `DATE` no timezone at
/// all — it names a day, not an instant — so keeping it separate from `Time` is what lets an
/// all-day event stay "September 14th" instead of becoming "September 14th, midnight, in
/// whichever zone the code happened to run in".
#[derive(Debug, Clone, PartialEq)]
pub enum When {
    /// An all-day bound. `DTEND` on an all-day event is exclusive in the spec (the day *after* the
    /// last day), which is why the surface has [`Event::last_day`] rather than reading `end`
    /// directly for display.
    Date(NaiveDate),
    /// An instant, already resolved to how this machine would show it: `Z` is UTC, `TZID` is
    /// resolved through `chrono-tz`, and a bare floating time is read as the viewer's own zone —
    /// which is what "floating" means in the spec, not a gap in this parser.
    Time(DateTime<Local>),
}

impl When {
    pub fn date(&self) -> NaiveDate {
        match self {
            When::Date(date) => *date,
            When::Time(time) => time.date_naive(),
        }
    }

    /// A comparable instant, for sorting and for grid placement. An all-day bound sorts at
    /// midnight, which is where a day belongs among the timed events around it.
    pub fn instant(&self) -> DateTime<Local> {
        match self {
            When::Date(date) => local_midnight(*date),
            When::Time(time) => *time,
        }
    }

    pub fn is_all_day(&self) -> bool {
        matches!(self, When::Date(_))
    }

    fn shift(&self, duration: Duration) -> When {
        match self {
            When::Time(time) => When::Time(*time + duration),
            // A day-granularity shift on a date-only bound; VALARM/DURATION on an all-day event is
            // rare, and this is the one sane reading of it.
            When::Date(date) => When::Date(*date + Duration::days(duration.num_days())),
        }
    }
}

/// Midnight, local, on `date` — the one moment on the clock a bare date is closest to standing
/// for. Ambiguous around a DST fall-back is vanishingly unlikely at midnight; falling back to
/// `Utc::now()`'s local reading rather than panicking is just refusing to crash over it.
pub fn local_midnight(date: NaiveDate) -> DateTime<Local> {
    let naive = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    local_from_naive(naive)
}

/// A naive wall-clock time as this machine's own zone would read it — RFC 5545's "floating time".
/// `pub` because the calendar surface needs it too, to turn what a person typed into a date/time
/// field into the same kind of instant a parsed event's `start`/`end` already is.
pub fn local_from_naive(naive: NaiveDateTime) -> DateTime<Local> {
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(time) | chrono::LocalResult::Ambiguous(time, _) => time,
        chrono::LocalResult::None => Utc::now().with_timezone(&Local),
    }
}

/// One occurrence of an event, parsed out of the `VEVENT` Thunderbird handed over.
///
/// `organizer`/`attendees` are read but not edited anywhere in the surface — no RSVP, no invite
/// UI — and are written back verbatim so saving an edit does not silently drop who is coming.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub start: When,
    pub end: When,
    /// The raw `RRULE` value, kept opaque so an existing custom rule survives a save that does not
    /// touch recurrence. [`Recur::decode`] turns it into one of the five presets the editor offers
    /// where it can.
    pub rrule: Option<String>,
    /// How long before `start` a reminder fires. Only "before start" triggers are understood —
    /// see [`parse_duration`] — which covers every reminder the editor itself can create.
    pub alarms: Vec<Duration>,
    pub organizer: Option<String>,
    pub attendees: Vec<String>,
}

impl Event {
    /// A blank event starting an hour from now, the way clicking an empty slot proposes one.
    pub fn blank(start: When, end: When) -> Event {
        Event {
            uid: new_uid(),
            summary: String::new(),
            location: String::new(),
            description: String::new(),
            start,
            end,
            rrule: None,
            alarms: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
        }
    }

    /// The last calendar day an all-day event covers — `end` is the spec's exclusive bound, one
    /// day past it.
    pub fn last_day(&self) -> NaiveDate {
        match &self.end {
            When::Date(date) => date.pred_opt().unwrap_or(*date),
            When::Time(time) => time.date_naive(),
        }
    }

    /// Whether `day` is one this event is showing on — inclusive on both ends, all-day's
    /// exclusive `DTEND` already accounted for by [`Event::last_day`].
    pub fn spans(&self, day: NaiveDate) -> bool {
        self.start.date() <= day && day <= self.last_day()
    }

    pub fn recur(&self) -> Recur {
        Recur::decode(self.rrule.as_deref())
    }

    /// Parses the `VEVENT` out of a full `VCALENDAR` document — what `calendar.items.query`/`get`
    /// hand back with `returnFormat: "ical"`.
    pub fn parse(text: &str) -> Option<Event> {
        let top = parse_components(text);
        let vevent = find(&top, "VEVENT")?;
        let get = |name: &str| vevent.properties.iter().find(|property| property.name == name);
        let text_of = |name: &str| get(name).map(Property::text).unwrap_or_default();

        let start = get("DTSTART").and_then(parse_when)?;
        let end = match get("DTEND").and_then(parse_when) {
            Some(end) => end,
            None => match get("DURATION").and_then(|property| parse_duration(&property.raw)) {
                Some(duration) => start.shift(duration),
                None => start.clone(),
            },
        };

        let alarms = vevent
            .children
            .iter()
            .filter(|child| child.name == "VALARM")
            .filter_map(|alarm| alarm.properties.iter().find(|property| property.name == "TRIGGER"))
            .filter_map(|trigger| parse_duration(&trigger.raw))
            // `TRIGGER` is negative for "before start", which is the only kind the reminder
            // presets build; a trigger after the start, or an absolute one, is left out rather
            // than stored as a nonsensical negative "time before".
            .filter(|duration| *duration < Duration::zero())
            .map(|duration| -duration)
            .collect();

        Some(Event {
            uid: text_of("UID"),
            summary: text_of("SUMMARY"),
            location: text_of("LOCATION"),
            description: text_of("DESCRIPTION"),
            start,
            end,
            rrule: get("RRULE").map(|property| property.raw.clone()),
            alarms,
            organizer: get("ORGANIZER").map(mailto_address),
            attendees: vevent.properties.iter().filter(|p| p.name == "ATTENDEE").map(mailto_address).collect(),
        })
    }

    /// A full `VCALENDAR` document, ready for `calendar.items.create`/`update` with
    /// `format: "ical"` — the exact shape `tools/smoke.sh` proves Thunderbird accepts.
    pub fn to_ical(&self) -> String {
        let mut lines = vec![
            "BEGIN:VCALENDAR".to_string(),
            "VERSION:2.0".to_string(),
            "PRODID:-//noctmalia//calendar//EN".to_string(),
            "BEGIN:VEVENT".to_string(),
            format!("UID:{}", escape(&self.uid)),
            format!("DTSTAMP:{}", Utc::now().format("%Y%m%dT%H%M%SZ")),
            when_line("DTSTART", &self.start),
            when_line("DTEND", &self.end),
            format!("SUMMARY:{}", escape(&self.summary)),
        ];
        if !self.location.is_empty() {
            lines.push(format!("LOCATION:{}", escape(&self.location)));
        }
        if !self.description.is_empty() {
            lines.push(format!("DESCRIPTION:{}", escape(&self.description)));
        }
        if let Some(rule) = &self.rrule {
            lines.push(format!("RRULE:{rule}"));
        }
        if let Some(organizer) = &self.organizer {
            lines.push(format!("ORGANIZER:mailto:{}", escape(organizer)));
        }
        for attendee in &self.attendees {
            lines.push(format!("ATTENDEE:mailto:{}", escape(attendee)));
        }
        for alarm in &self.alarms {
            lines.push("BEGIN:VALARM".to_string());
            lines.push("ACTION:DISPLAY".to_string());
            lines.push(format!("DESCRIPTION:{}", escape(&self.summary)));
            lines.push(format!("TRIGGER:-PT{}M", alarm.num_minutes().max(0)));
            lines.push("END:VALARM".to_string());
        }
        lines.push("END:VEVENT".to_string());
        lines.push("END:VCALENDAR".to_string());
        lines.into_iter().map(|line| fold(&line) + "\r\n").collect()
    }
}

/// The bare address out of an `ORGANIZER`/`ATTENDEE` value, dropping the `mailto:` scheme. `CN`
/// (a display name) is not kept: read-only display is all either property is for here, and
/// keeping the address rather than the name is what lets [`Event::to_ical`] write the property
/// back without inventing an address out of a name.
fn mailto_address(property: &Property) -> String {
    let value = property.text();
    value.strip_prefix("mailto:").unwrap_or(&value).to_string()
}

/// A gcal-style recurrence preset. `Custom` is what an existing rule this editor did not write
/// decodes to — shown as "Repeats" rather than offered back through the picker, since turning an
/// arbitrary `RRULE` into one of five presets would be guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recur {
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Custom,
}

impl Recur {
    /// What the picker offers. `Custom` is not among them — see the type's own doc.
    pub const PRESETS: [Recur; 5] = [Recur::None, Recur::Daily, Recur::Weekly, Recur::Monthly, Recur::Yearly];

    pub fn label(self) -> &'static str {
        match self {
            Recur::None => "Does not repeat",
            Recur::Daily => "Daily",
            Recur::Weekly => "Weekly",
            Recur::Monthly => "Monthly",
            Recur::Yearly => "Yearly",
            Recur::Custom => "Repeats",
        }
    }

    fn decode(rrule: Option<&str>) -> Recur {
        match rrule {
            None => Recur::None,
            Some("FREQ=DAILY") => Recur::Daily,
            Some("FREQ=WEEKLY") => Recur::Weekly,
            Some("FREQ=MONTHLY") => Recur::Monthly,
            Some("FREQ=YEARLY") => Recur::Yearly,
            Some(_) => Recur::Custom,
        }
    }

    /// The `RRULE` value a preset means, or `None` for a plain non-repeating event.
    pub fn encode(self) -> Option<String> {
        match self {
            Recur::None | Recur::Custom => None,
            Recur::Daily => Some("FREQ=DAILY".to_string()),
            Recur::Weekly => Some("FREQ=WEEKLY".to_string()),
            Recur::Monthly => Some("FREQ=MONTHLY".to_string()),
            Recur::Yearly => Some("FREQ=YEARLY".to_string()),
        }
    }
}

impl std::fmt::Display for Recur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A random-enough UID for an event this editor creates. Thunderbird only needs it to be unique;
/// nothing here needs it to be a UUID.
fn new_uid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let salt = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("noctmalia-{nanos:x}-{salt:x}")
}

// ── The grammar ─────────────────────────────────────────────────────────────

/// A property line: `NAME[;PARAM=VALUE]*:VALUE`. No `group.` prefix — nothing here writes one and
/// nothing a VEVENT needs reads one.
#[derive(Debug, Clone, PartialEq)]
struct Property {
    name: String,
    params: Vec<(String, String)>,
    /// The value exactly as it appeared, still escaped.
    raw: String,
}

impl Property {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        unescape(&self.raw)
    }
}

/// A `BEGIN:X` ... `END:X` block, with whatever nested blocks it holds — a `VEVENT`'s `VALARM`s.
#[derive(Debug, Clone, Default)]
struct Component {
    name: String,
    properties: Vec<Property>,
    children: Vec<Component>,
}

/// Groups a flat stream of content lines into the component tree `BEGIN`/`END` describe.
fn parse_components(text: &str) -> Vec<Component> {
    let mut stack: Vec<Component> = Vec::new();
    let mut top: Vec<Component> = Vec::new();
    for line in unfold(text) {
        if line.trim().is_empty() {
            continue;
        }
        let Some(colon) = value_colon(&line) else { continue };
        let (head, value) = line.split_at(colon);
        let raw = value[1..].to_string();

        let mut parts = split_unescaped(head, ';');
        let name = parts.remove(0).trim().to_uppercase();
        let params = parts
            .into_iter()
            .map(|part| match part.split_once('=') {
                Some((key, value)) => (key.trim().to_uppercase(), value.trim().trim_matches('"').to_string()),
                None => ("TYPE".to_string(), part.trim().to_string()),
            })
            .collect();

        match name.as_str() {
            "BEGIN" => stack.push(Component { name: raw.trim().to_uppercase(), ..Component::default() }),
            "END" => {
                if let Some(done) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(done),
                        None => top.push(done),
                    }
                }
            }
            _ => {
                if let Some(current) = stack.last_mut() {
                    current.properties.push(Property { name, params, raw });
                }
            }
        }
    }
    top
}

/// The first component named `name`, searched depth-first — a `VEVENT` is never nested, but this
/// does not need to assume that to find one.
fn find<'a>(components: &'a [Component], name: &str) -> Option<&'a Component> {
    for component in components {
        if component.name == name {
            return Some(component);
        }
        if let Some(found) = find(&component.children, name) {
            return Some(found);
        }
    }
    None
}

/// `DTSTART`/`DTEND` in whichever of the three forms RFC 5545 allows: a bare `DATE`, a `Z`-suffixed
/// UTC instant, or a `TZID`-qualified or floating local one.
fn parse_when(property: &Property) -> Option<When> {
    let raw = property.raw.trim();
    let is_date = property.param("VALUE").is_some_and(|value| value.eq_ignore_ascii_case("DATE"));
    if is_date || (raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_digit())) {
        return NaiveDate::parse_from_str(raw, "%Y%m%d").ok().map(When::Date);
    }
    if let Some(tzid) = property.param("TZID") {
        let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()?;
        let zone: chrono_tz::Tz = tzid.parse().ok()?;
        let at = match zone.from_local_datetime(&naive) {
            chrono::LocalResult::Single(at) | chrono::LocalResult::Ambiguous(at, _) => at,
            chrono::LocalResult::None => return None,
        };
        return Some(When::Time(at.with_timezone(&Local)));
    }
    if let Some(utc) = raw.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(When::Time(Utc.from_utc_datetime(&naive).with_timezone(&Local)));
    }
    // Floating: RFC 5545 says this is read in whatever zone the viewer is in, which is exactly
    // what treating it as this machine's local time already does.
    NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok().map(|naive| When::Time(local_from_naive(naive)))
}

/// Always written as `Z`-suffixed UTC for a timed bound — the one form `tools/smoke.sh` has
/// proven Thunderbird accepts, so writing never needs a `TZID` or a timezone database.
fn when_line(name: &str, when: &When) -> String {
    match when {
        When::Date(date) => format!("{name};VALUE=DATE:{}", date.format("%Y%m%d")),
        When::Time(time) => format!("{name}:{}", time.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")),
    }
}

/// An ISO 8601 duration — `TRIGGER`'s and `DURATION`'s shared grammar:
/// `[+-]P(nW|nD)?(T(nH)?(nM)?(nS)?)?`. Only whole-unit values, which is everything Thunderbird
/// itself writes and everything the reminder presets can build.
fn parse_duration(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    let (negative, rest) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };
    let rest = rest.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };

    let mut total = Duration::zero();
    let mut accumulate = |part: &str, hours: bool| -> Option<()> {
        let mut number = String::new();
        for character in part.chars() {
            if character.is_ascii_digit() {
                number.push(character);
                continue;
            }
            let value: i64 = number.parse().ok()?;
            number.clear();
            total += match (hours, character) {
                (false, 'W') => Duration::weeks(value),
                (false, 'D') => Duration::days(value),
                (true, 'H') => Duration::hours(value),
                (true, 'M') => Duration::minutes(value),
                (true, 'S') => Duration::seconds(value),
                _ => return None,
            };
        }
        Some(())
    };
    accumulate(date_part, false)?;
    if let Some(time_part) = time_part {
        accumulate(time_part, true)?;
    }
    Some(if negative { -total } else { total })
}

/// The colon that starts the value, skipping any inside a quoted parameter.
fn value_colon(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (index, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(index),
            _ => {}
        }
    }
    None
}

/// Joins continuation lines: a line starting with a space or tab continues the previous one.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match line.strip_prefix([' ', '\t']) {
            Some(continuation) => match lines.last_mut() {
                Some(last) => last.push_str(continuation),
                None => lines.push(continuation.to_string()),
            },
            None => lines.push(line.to_string()),
        }
    }
    lines
}

/// Folds at 75 octets, on a character boundary, with a leading space on each continuation.
fn fold(line: &str) -> String {
    const LIMIT: usize = 75;
    if line.len() <= LIMIT {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + line.len() / LIMIT * 3);
    let mut start = 0;
    let mut budget = LIMIT;
    for (index, character) in line.char_indices() {
        if index - start + character.len_utf8() > budget {
            out.push_str(&line[start..index]);
            out.push_str("\r\n ");
            start = index;
            budget = LIMIT - 1;
        }
    }
    out.push_str(&line[start..]);
    out
}

/// Splits on `separator`, ignoring ones preceded by a backslash.
fn split_unescaped(value: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            current.push(character);
            escaped = true;
        } else if character == separator {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    parts.push(current);
    parts
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(escaped) => out.push(escaped),
            None => out.push('\\'),
        }
    }
    out
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    const TIMED: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:standup-1\r\n\
DTSTAMP:20260910T090000Z\r\n\
DTSTART:20260914T170000Z\r\n\
DTEND:20260914T173000Z\r\n\
SUMMARY:Standup\r\n\
LOCATION:Kitchen\r\n\
DESCRIPTION:Bring status\\, not slides\r\n\
RRULE:FREQ=DAILY\r\n\
ORGANIZER;CN=Alice:mailto:alice@example.com\r\n\
ATTENDEE:mailto:bob@example.com\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Standup\r\n\
TRIGGER:-PT15M\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn parses_a_timed_event() {
        let event = Event::parse(TIMED).expect("a VEVENT");
        assert_eq!(event.uid, "standup-1");
        assert_eq!(event.summary, "Standup");
        assert_eq!(event.location, "Kitchen");
        assert_eq!(event.description, "Bring status, not slides");
        assert_eq!(event.organizer.as_deref(), Some("alice@example.com"));
        assert_eq!(event.attendees, vec!["bob@example.com".to_string()]);
        assert_eq!(event.recur(), Recur::Daily);
        assert_eq!(event.alarms, vec![Duration::minutes(15)]);
        assert!(!event.start.is_all_day());
    }

    #[test]
    fn resolves_utc_to_the_local_zone_consistently_with_local_now() {
        let event = Event::parse(TIMED).unwrap();
        // Whatever this machine's zone is, a UTC instant round-trips through it exactly.
        let expected = Utc.with_ymd_and_hms(2026, 9, 14, 17, 0, 0).unwrap().with_timezone(&Local);
        assert_eq!(event.start.instant(), expected);
    }

    #[test]
    fn parses_an_all_day_event() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:trip\r\nSUMMARY:Trip\r\n\
DTSTART;VALUE=DATE:20260920\r\nDTEND;VALUE=DATE:20260923\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = Event::parse(text).expect("a VEVENT");
        assert!(event.start.is_all_day());
        assert_eq!(event.start.date(), NaiveDate::from_ymd_opt(2026, 9, 20).unwrap());
        // DTEND is exclusive; the trip's last day is the 22nd, not the 23rd.
        assert_eq!(event.last_day(), NaiveDate::from_ymd_opt(2026, 9, 22).unwrap());
        assert!(event.spans(NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()));
        assert!(!event.spans(NaiveDate::from_ymd_opt(2026, 9, 23).unwrap()));
    }

    #[test]
    fn a_duration_stands_in_for_a_missing_dtend() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:x\r\n\
DTSTART:20260914T170000Z\r\nDURATION:PT30M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = Event::parse(text).unwrap();
        assert_eq!(event.end.instant() - event.start.instant(), Duration::minutes(30));
    }

    #[test]
    fn a_floating_time_reads_as_this_machines_local_zone() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSUMMARY:x\r\n\
DTSTART:20260914T090000\r\nDTEND:20260914T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = Event::parse(text).unwrap();
        let When::Time(start) = event.start else { panic!("expected a timed event") };
        assert_eq!(
            start.naive_local(),
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap().and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap())
        );
    }

    #[test]
    fn round_trips_a_timed_event_with_a_reminder() {
        let event = Event::parse(TIMED).unwrap();
        let again = Event::parse(&event.to_ical()).unwrap();
        assert_eq!(event.summary, again.summary);
        assert_eq!(event.location, again.location);
        assert_eq!(event.description, again.description);
        assert_eq!(event.start.instant(), again.start.instant());
        assert_eq!(event.end.instant(), again.end.instant());
        assert_eq!(event.recur(), again.recur());
        assert_eq!(event.alarms, again.alarms);
        // Organizer/attendees are read-only in the editor but must not be dropped on save.
        assert_eq!(again.organizer.as_deref(), Some("alice@example.com"));
        assert_eq!(again.attendees, event.attendees);
    }

    #[test]
    fn round_trips_an_all_day_event() {
        let event = Event::blank(
            When::Date(NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()),
            When::Date(NaiveDate::from_ymd_opt(2026, 12, 26).unwrap()),
        );
        let again = Event::parse(&event.to_ical()).unwrap();
        assert_eq!(again.start, event.start);
        assert_eq!(again.end, event.end);
        assert!(again.start.is_all_day());
    }

    #[test]
    fn escapes_commas_and_semicolons_on_the_way_out() {
        let mut event = Event::blank(
            When::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            When::Date(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()),
        );
        event.summary = "Comma, semicolon;".to_string();
        let text = event.to_ical();
        assert!(text.contains("SUMMARY:Comma\\, semicolon\\;"), "{text}");
        assert_eq!(Event::parse(&text).unwrap().summary, event.summary);
    }

    #[test]
    fn decodes_the_five_presets_and_falls_back_to_custom() {
        assert_eq!(Recur::decode(None), Recur::None);
        assert_eq!(Recur::decode(Some("FREQ=DAILY")), Recur::Daily);
        assert_eq!(Recur::decode(Some("FREQ=WEEKLY")), Recur::Weekly);
        assert_eq!(Recur::decode(Some("FREQ=MONTHLY")), Recur::Monthly);
        assert_eq!(Recur::decode(Some("FREQ=YEARLY")), Recur::Yearly);
        assert_eq!(Recur::decode(Some("FREQ=DAILY;COUNT=3")), Recur::Custom);
        assert_eq!(Recur::Daily.encode().as_deref(), Some("FREQ=DAILY"));
        assert_eq!(Recur::None.encode(), None);
    }

    #[test]
    fn folds_long_lines_so_they_survive_a_round_trip() {
        let mut event = Event::blank(
            When::Date(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            When::Date(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()),
        );
        event.description = "x".repeat(300);
        let text = event.to_ical();
        assert!(text.lines().all(|line| line.len() <= 75), "a line is too long");
        assert_eq!(Event::parse(&text).unwrap().description, "x".repeat(300));
    }
}
