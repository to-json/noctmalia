//! vCard 4.0 parsing and serialisation, enough for a contact editor.
//!
//! Thunderbird stores contacts as vCards and hands them over verbatim (`contacts.get`), so anything
//! we do not model still has to survive an edit: UID, REV, PHOTO, and the `X-` properties
//! Thunderbird keeps its own bookkeeping in. Every property we do not understand is kept as its
//! original line and written back unchanged.
//!
//! Not implemented: `group.PROP` grouping is preserved but never produced, and PHOTO values are
//! passed through rather than decoded.

use std::fmt::Write as _;

/// A property line: `[group.]NAME[;PARAM=VALUE]*:VALUE`.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    /// Parameters in file order. vCard 3.0's bare `;HOME` form is stored as `("TYPE", "HOME")`.
    pub params: Vec<(String, String)>,
    /// The value exactly as it appeared, still escaped and still `;`-joined.
    pub raw: String,
    /// `group` in `group.NAME`, kept so round-tripping does not break grouped properties.
    pub group: Option<String>,
}

impl Property {
    fn new(name: &str, value: String) -> Property {
        Property { name: name.to_string(), params: Vec::new(), raw: value, group: None }
    }

    fn with_type(name: &str, kind: &str, value: String) -> Property {
        let mut property = Property::new(name, value);
        if !kind.is_empty() {
            property.params.push(("TYPE".to_string(), kind.to_string()));
        }
        property
    }

    /// All values of a parameter, lowercased. `TYPE="voice,cell"` yields both.
    fn param_values(&self, name: &str) -> Vec<String> {
        self.params
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .flat_map(|(_, value)| value.split(','))
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
            .collect()
    }

    /// The single text value, unescaped.
    pub fn text(&self) -> String {
        unescape(&self.raw)
    }

    /// A structured value split on unescaped `;`, each component unescaped.
    pub fn components(&self) -> Vec<String> {
        split_unescaped(&self.raw, ';').iter().map(|part| unescape(part)).collect()
    }

    fn line(&self) -> String {
        let mut line = String::new();
        if let Some(group) = &self.group {
            let _ = write!(line, "{group}.");
        }
        line.push_str(&self.name);
        for (key, value) in &self.params {
            let quoted = value.contains([',', ';', ':']);
            if quoted {
                let _ = write!(line, ";{key}=\"{value}\"");
            } else {
                let _ = write!(line, ";{key}={value}");
            }
        }
        line.push(':');
        line.push_str(&self.raw);
        line
    }
}

/// One labelled value: an email address, a phone number, a link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// The first `TYPE` that says something a person cares about, e.g. `work`. May be empty.
    pub kind: String,
    pub value: String,
    /// `PREF=1`, or vCard 3.0's `TYPE=PREF`.
    pub preferred: bool,
}

/// A postal address (`ADR`), whose seven components are fixed by the spec.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Address {
    pub kind: String,
    pub po_box: String,
    pub extended: String,
    pub street: String,
    pub locality: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
}

impl Address {
    /// The address as displayed lines, skipping the components that are empty.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for part in [&self.po_box, &self.extended, &self.street] {
            if !part.is_empty() {
                lines.push(part.clone());
            }
        }
        let town = [&self.locality, &self.region, &self.postal_code]
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if !town.is_empty() {
            lines.push(town);
        }
        if !self.country.is_empty() {
            lines.push(self.country.clone());
        }
        lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines().is_empty()
    }
}

/// The name components of `N`: family, given, additional, prefixes, suffixes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Name {
    pub family: String,
    pub given: String,
    pub additional: String,
    pub prefixes: String,
    pub suffixes: String,
}

impl Name {
    /// The name as a person would write it, for filling in a missing `FN`.
    pub fn joined(&self) -> String {
        [&self.prefixes, &self.given, &self.additional, &self.family, &self.suffixes]
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A contact. Modelled fields are editable; `rest` carries everything else through unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Card {
    pub formatted_name: String,
    pub name: Name,
    pub nickname: String,
    pub organisation: String,
    pub role: String,
    pub emails: Vec<Entry>,
    pub phones: Vec<Entry>,
    pub urls: Vec<Entry>,
    pub addresses: Vec<Address>,
    pub note: String,
    /// Properties we do not model, written back verbatim.
    pub rest: Vec<Property>,
}

impl Card {
    pub fn parse(text: &str) -> Card {
        let mut card = Card::default();
        for property in parse_properties(text) {
            let name = property.name.to_uppercase();
            match name.as_str() {
                "BEGIN" | "END" | "VERSION" => {}
                "FN" => card.formatted_name = property.text(),
                "N" => {
                    let parts = property.components();
                    let at = |index: usize| parts.get(index).cloned().unwrap_or_default();
                    card.name =
                        Name { family: at(0), given: at(1), additional: at(2), prefixes: at(3), suffixes: at(4) };
                }
                "NICKNAME" => card.nickname = property.text(),
                // ORG is structured (organisation;unit;…); the units matter to few people, so the
                // editor shows the whole thing joined and writes it back as one component.
                "ORG" => card.organisation = property.components().join(", "),
                "TITLE" => card.role = property.text(),
                "EMAIL" => card.emails.push(entry(&property)),
                "TEL" => card.phones.push(entry(&property)),
                "URL" => card.urls.push(entry(&property)),
                "ADR" => card.addresses.push(address(&property)),
                // NOTE repeats legally but Thunderbird writes one; later ones would be lost on save,
                // so keep the extras in `rest`.
                "NOTE" if card.note.is_empty() => card.note = property.text(),
                _ => card.rest.push(property),
            }
        }
        card
    }

    /// The name to show and sort by: `FN`, else the assembled `N`, else the first email address.
    pub fn display_name(&self) -> String {
        if !self.formatted_name.is_empty() {
            return self.formatted_name.clone();
        }
        let joined = self.name.joined();
        if !joined.is_empty() {
            return joined;
        }
        self.emails.first().map(|email| email.value.clone()).unwrap_or_default()
    }

    /// Sort key: family name first where there is one, so the list reads like a rolodex.
    pub fn sort_key(&self) -> String {
        let key = if self.name.family.is_empty() {
            self.display_name()
        } else {
            format!("{} {}", self.name.family, self.name.given)
        };
        key.to_lowercase()
    }

    /// The first one or two initials, for the avatar.
    pub fn initials(&self) -> String {
        let name = self.display_name();
        let mut initials: String = name.split_whitespace().filter_map(|word| word.chars().next()).take(2).collect();
        if initials.is_empty() {
            initials.push('?');
        }
        initials.to_uppercase()
    }

    pub fn to_vcard(&self) -> String {
        let mut properties = vec![Property::new("BEGIN", "VCARD".into()), Property::new("VERSION", "4.0".into())];

        let formatted = if self.formatted_name.is_empty() { self.name.joined() } else { self.formatted_name.clone() };
        properties.push(Property::new("FN", escape(&formatted)));
        let name = &self.name;
        let components = [&name.family, &name.given, &name.additional, &name.prefixes, &name.suffixes];
        if components.iter().any(|part| !part.is_empty()) {
            let value = components.iter().map(|part| escape(part)).collect::<Vec<_>>().join(";");
            properties.push(Property::new("N", value));
        }
        for (field, name) in [(&self.nickname, "NICKNAME"), (&self.organisation, "ORG"), (&self.role, "TITLE")] {
            if !field.is_empty() {
                properties.push(Property::new(name, escape(field)));
            }
        }
        for (list, name) in [(&self.emails, "EMAIL"), (&self.phones, "TEL"), (&self.urls, "URL")] {
            for entry in list.iter().filter(|entry| !entry.value.is_empty()) {
                let mut property = Property::with_type(name, &entry.kind, escape(&entry.value));
                if entry.preferred {
                    property.params.push(("PREF".to_string(), "1".to_string()));
                }
                properties.push(property);
            }
        }
        for address in self.addresses.iter().filter(|address| !address.is_empty()) {
            let components = [
                &address.po_box,
                &address.extended,
                &address.street,
                &address.locality,
                &address.region,
                &address.postal_code,
                &address.country,
            ];
            let value = components.iter().map(|part| escape(part)).collect::<Vec<_>>().join(";");
            properties.push(Property::with_type("ADR", &address.kind, value));
        }
        if !self.note.is_empty() {
            properties.push(Property::new("NOTE", escape(&self.note)));
        }
        properties.extend(self.rest.iter().cloned());
        properties.push(Property::new("END", "VCARD".into()));

        let mut out = String::new();
        for property in properties {
            out.push_str(&fold(&property.line()));
            out.push_str("\r\n");
        }
        out
    }
}

fn entry(property: &Property) -> Entry {
    let types = property.param_values("TYPE");
    // These say how the value is delivered, not what it is for; nobody wants to see them.
    let uninteresting = ["voice", "internet", "pref", "other"];
    let kind = types.iter().find(|kind| !uninteresting.contains(&kind.as_str())).cloned().unwrap_or_default();
    let preferred = types.iter().any(|kind| kind == "pref") || property.param_values("PREF").contains(&"1".into());
    // TEL values are often `tel:+15550100` URIs; the prefix is noise in a contact card.
    let value = property.text();
    let value = value.strip_prefix("tel:").unwrap_or(&value).to_string();
    Entry { kind, value, preferred }
}

fn address(property: &Property) -> Address {
    let parts = property.components();
    let at = |index: usize| parts.get(index).cloned().unwrap_or_default();
    let types = property.param_values("TYPE");
    Address {
        kind: types.first().cloned().unwrap_or_default(),
        po_box: at(0),
        extended: at(1),
        street: at(2),
        locality: at(3),
        region: at(4),
        postal_code: at(5),
        country: at(6),
    }
}

/// Unfolds and splits a vCard into properties.
pub fn parse_properties(text: &str) -> Vec<Property> {
    let mut properties = Vec::new();
    for line in unfold(text) {
        if line.trim().is_empty() {
            continue;
        }
        // The value may contain colons (URLs), so only the first one outside the parameters counts.
        let Some(colon) = value_colon(&line) else {
            continue;
        };
        let (head, value) = line.split_at(colon);
        let raw = value[1..].to_string();

        let mut parts = split_unescaped(head, ';');
        let mut first = parts.remove(0);
        let group = first.find('.').map(|dot| {
            let group = first[..dot].to_string();
            first = first[dot + 1..].to_string();
            group
        });

        let params = parts
            .into_iter()
            .map(|part| match part.split_once('=') {
                Some((key, value)) => (key.trim().to_string(), value.trim().trim_matches('"').to_string()),
                // vCard 3.0 writes bare type values: `TEL;HOME:…`.
                None => ("TYPE".to_string(), part.trim().to_string()),
            })
            .collect();

        properties.push(Property { name: first.trim().to_string(), params, raw, group });
    }
    properties
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
            Some(continuation) => {
                if let Some(last) = lines.last_mut() {
                    last.push_str(continuation);
                    continue;
                }
                lines.push(continuation.to_string());
            }
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
            // The leading space counts toward the folded line's length.
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

/// A minimal card for a new contact.
pub fn blank() -> Card {
    Card { emails: vec![Entry::default()], phones: vec![Entry::default()], ..Card::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCARD\r\n\
VERSION:4.0\r\n\
FN:Alice Chen\r\n\
N:Chen;Alice;Q;Dr.;PhD\r\n\
ORG:Noctalia\r\n\
TITLE:Engineer\r\n\
EMAIL;TYPE=work;PREF=1:alice@example.com\r\n\
EMAIL;TYPE=home:alice@home.example\r\n\
TEL;TYPE=\"voice,cell\":tel:+15550100\r\n\
ADR;TYPE=work:;;1 Long Street;Springfield;OR;97477;USA\r\n\
NOTE:Likes semicolons\\; dislikes commas\\, mostly\r\n\
UID:urn:uuid:1234\r\n\
X-THUNDERBIRD-THING:keep me\r\n\
END:VCARD\r\n";

    #[test]
    fn parses_the_modelled_fields() {
        let card = Card::parse(SAMPLE);
        assert_eq!(card.formatted_name, "Alice Chen");
        assert_eq!(card.name.family, "Chen");
        assert_eq!(card.name.given, "Alice");
        assert_eq!(card.name.prefixes, "Dr.");
        assert_eq!(card.organisation, "Noctalia");
        assert_eq!(card.role, "Engineer");
        assert_eq!(card.note, "Likes semicolons; dislikes commas, mostly");
    }

    #[test]
    fn picks_a_meaningful_type_and_strips_the_tel_uri() {
        let card = Card::parse(SAMPLE);
        assert_eq!(card.emails[0], Entry { kind: "work".into(), value: "alice@example.com".into(), preferred: true });
        assert_eq!(card.emails[1].kind, "home");
        // "voice" says nothing; "cell" does.
        assert_eq!(card.phones[0], Entry { kind: "cell".into(), value: "+15550100".into(), preferred: false });
    }

    #[test]
    fn parses_a_structured_address() {
        let card = Card::parse(SAMPLE);
        let address = &card.addresses[0];
        assert_eq!(address.street, "1 Long Street");
        assert_eq!(address.locality, "Springfield");
        assert_eq!(address.country, "USA");
        assert_eq!(address.lines(), ["1 Long Street", "Springfield OR 97477", "USA"]);
    }

    #[test]
    fn keeps_unmodelled_properties() {
        let card = Card::parse(SAMPLE);
        let names: Vec<&str> = card.rest.iter().map(|property| property.name.as_str()).collect();
        assert_eq!(names, ["UID", "X-THUNDERBIRD-THING"]);
        // A URI value keeps its own colons.
        assert_eq!(card.rest[0].raw, "urn:uuid:1234");
    }

    #[test]
    fn round_trips_without_losing_anything() {
        let card = Card::parse(SAMPLE);
        let again = Card::parse(&card.to_vcard());
        assert_eq!(card, again);
    }

    #[test]
    fn escapes_on_the_way_out() {
        let card = Card { formatted_name: "Semi; Colon".into(), note: "first\nsecond".into(), ..Card::default() };
        let text = card.to_vcard();
        assert!(text.contains("FN:Semi\\; Colon"), "{text}");
        assert!(text.contains("NOTE:first\\nsecond"), "{text}");
        assert_eq!(Card::parse(&text).note, "first\nsecond");
    }

    #[test]
    fn unfolds_continuation_lines() {
        let folded = "BEGIN:VCARD\r\nVERSION:4.0\r\nNOTE:one two\r\n  three\r\nEND:VCARD\r\n";
        assert_eq!(Card::parse(folded).note, "one two three");
    }

    #[test]
    fn folds_long_lines_so_they_survive_a_round_trip() {
        let card = Card { note: "x".repeat(300), ..Card::default() };
        let text = card.to_vcard();
        assert!(text.lines().all(|line| line.len() <= 75), "a line is too long");
        assert_eq!(Card::parse(&text).note, "x".repeat(300));
    }

    #[test]
    fn falls_back_through_fn_then_n_then_email() {
        let mut card = Card::default();
        card.emails.push(Entry { value: "only@example.com".into(), ..Entry::default() });
        assert_eq!(card.display_name(), "only@example.com");
        card.name.given = "Bo".into();
        card.name.family = "Builder".into();
        assert_eq!(card.display_name(), "Bo Builder");
        assert_eq!(card.sort_key(), "builder bo");
        assert_eq!(card.initials(), "BB");
        card.formatted_name = "Robert Builder".into();
        assert_eq!(card.display_name(), "Robert Builder");
    }
}
