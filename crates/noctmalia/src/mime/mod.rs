//! What a message says, and what it gives away.
//!
//! Thunderbird has already done the hard part: `messages.getFull` hands over a decoded MIME tree,
//! so nothing here decodes base64, guesses a charset or unfolds a header. What is left is the two
//! decisions that are ours rather than the protocol's:
//!
//! - **Which part is the letter** ([`body`]), and then through [`plain`] or [`html`] into the one
//!   representation this program renders. Markdown, always, whatever arrived.
//! - **What the envelope admits to** ([`headers`]), which is the security surface: a display name
//!   wearing someone else's address, a relay chain that does not join up, a link whose words and
//!   destination disagree.

pub mod headers;
pub mod html;
pub mod plain;

use serde::Deserialize;

/// One node of Thunderbird's decoded MIME tree, as `messages.getFull` returns it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Part {
    #[serde(rename = "contentType", default)]
    pub content_type: Option<String>,
    /// The attachment filename, when the part has one.
    #[serde(default)]
    pub name: Option<String>,
    /// The MIME part number — `1.2` — which is how an attachment is asked for later.
    #[serde(rename = "partName", default)]
    pub part_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Decoded text, for a part that has any.
    #[serde(default)]
    pub body: Option<String>,
    /// Present on the root part, and on message/rfc822 parts.
    #[serde(default)]
    pub headers: Option<headers::Headers>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

impl Part {
    /// The media type, lowercased and without its parameters.
    pub fn media_type(&self) -> &str {
        self.content_type.as_deref().map_or("", |value| value.split(';').next().unwrap_or("").trim())
    }

    /// One `Content-Type` parameter, unquoted and lowercased.
    pub fn parameter(&self, name: &str) -> Option<String> {
        let content_type = self.content_type.as_deref()?;
        content_type.split(';').skip(1).find_map(|parameter| {
            let (key, value) = parameter.split_once('=')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().trim_matches('"').to_ascii_lowercase())
        })
    }

    fn is_multipart(&self) -> bool {
        self.media_type().starts_with("multipart/")
    }

    /// Whether this part is nothing but its `parts` — `multipart/*`, or a whole embedded message
    /// (`message/rfc822`: a forward, or the original beneath a bounce). Real mail nests these where
    /// the GreenMail fixture never did; treating one as a leaf left it with no body and an
    /// attachment labelled by its raw content type that nothing can actually fetch.
    fn is_container(&self) -> bool {
        self.is_multipart() || self.media_type() == "message/rfc822"
    }
}

/// What the letter arrived as. Kept so the reader can be told, and so the raw-source hatch knows
/// what it is showing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Flavour {
    #[default]
    Empty,
    Plain,
    Markdown,
    Html,
}

impl Flavour {
    pub fn label(self) -> &'static str {
        match self {
            Flavour::Empty => "empty",
            Flavour::Plain => "plain text",
            Flavour::Markdown => "markdown",
            Flavour::Html => "html",
        }
    }
}

/// A letter, as Markdown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Body {
    pub markdown: String,
    pub flavour: Flavour,
    /// Every image the sender wanted loaded. Nothing in this program loads one.
    pub images: Vec<String>,
    /// Images with nothing to show, which are there to report that the mail was opened.
    pub trackers: usize,
    /// Links whose words claim one destination and whose `href` is another.
    pub misleading: Vec<html::Misleading>,
    /// The message's HTML alternative, untouched — what "original formatting" mode renders
    /// instead of `markdown`. Present whenever the message *has* an HTML part anywhere, not only
    /// when [`pick`] chose to show it: a real plain-text alternative outranks HTML by design (see
    /// `pick`'s own doc comment), but that is a choice about the *default* view, not a claim that
    /// the sender's HTML doesn't exist. `trackers`/`images`/`misleading` above are computed from
    /// whichever text `pick` chose, regardless of mode, so detection doesn't vary by render
    /// choice. See `docs/html-mail-plan.md` Stream 4.1.
    pub raw_html: Option<String>,
}

/// A plain-text part below this length, next to an HTML one, is a placeholder rather than the
/// letter — "This message requires a mail reader that understands HTML".
const PLACEHOLDER: usize = 96;

/// The letter, converted.
pub fn body(root: &Part) -> Body {
    let mut body = match pick(root) {
        Some(part) => render(part),
        None => Body::default(),
    };
    if body.raw_html.is_none() {
        body.raw_html = find_html(root).and_then(|part| part.body.clone());
    }
    body
}

/// The first HTML part anywhere in the tree, independent of what [`pick`] chose — see
/// [`Body::raw_html`]. Walks the same containers `pick` does (so a forward's HTML is found too,
/// consistent with how its Markdown already is), but never picks between alternatives: the first
/// one encountered is the message's HTML, full stop.
fn find_html(part: &Part) -> Option<&Part> {
    if part.media_type() == "text/html" && part.name.is_none() {
        return Some(part);
    }
    if !part.is_container() {
        return None;
    }
    part.parts.iter().find_map(find_html)
}

/// Renders one already-chosen text part.
fn render(part: &Part) -> Body {
    let text = part.body.as_deref().unwrap_or_default();
    match part.media_type() {
        "text/html" => {
            let converted = html::to_markdown(text);
            Body {
                markdown: converted.markdown,
                flavour: Flavour::Html,
                images: converted.images,
                trackers: converted.trackers,
                misleading: converted.misleading,
                raw_html: Some(text.to_string()),
            }
        }
        media => {
            let flowed = part.parameter("format").as_deref() == Some("flowed");
            let delete_space = part.parameter("delsp").as_deref() == Some("yes");
            let flavour = if media == "text/markdown" { Flavour::Markdown } else { Flavour::Plain };
            let markdown = plain::to_markdown(text, flowed, delete_space);
            let flavour = if markdown.is_empty() { Flavour::Empty } else { flavour };
            Body { markdown, flavour, ..Body::default() }
        }
    }
}

/// Which part of the tree is the letter.
///
/// `multipart/alternative` is where the decision lives, and the answer is mutt's: the plain part,
/// because it is what the sender wrote rather than what their marketing department laid out. The
/// exception is the plain part that is not a letter at all — a line of apology for the HTML one —
/// which is what [`PLACEHOLDER`] catches.
fn pick(part: &Part) -> Option<&Part> {
    let media = part.media_type();
    if media.starts_with("text/") {
        // An attached text file is an attachment, not the letter.
        return (media == "text/plain" || media == "text/html" || media == "text/markdown")
            .then_some(part)
            .filter(|part| part.name.is_none());
    }
    if !part.is_container() {
        return None;
    }

    if media == "multipart/alternative" {
        let candidates: Vec<&Part> = part.parts.iter().filter_map(pick).collect();
        let by = |wanted: &str| candidates.iter().copied().find(|part| part.media_type() == wanted);
        if let Some(markdown) = by("text/markdown") {
            return Some(markdown);
        }
        let plain = by("text/plain");
        let html = by("text/html");
        return match (plain, html) {
            (Some(plain), Some(html)) => {
                let length = plain.body.as_deref().unwrap_or_default().trim().len();
                Some(if length < PLACEHOLDER { html } else { plain })
            }
            // Later alternatives are richer, so with nothing to prefer, take the last.
            (plain, html) => plain.or(html).or_else(|| candidates.last().copied()),
        };
    }
    // `multipart/related` leads with the letter and follows with what it refers to; `mixed`,
    // `signed` and `report` lead with it and follow with attachments. Either way it is the first
    // part that turns out to be one.
    part.parts.iter().find_map(pick)
}

/// Something hanging off the message that is not the letter.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// The MIME part number, which is how it is fetched.
    pub part_name: String,
    pub name: String,
    pub content_type: String,
    pub size: u64,
}

impl Attachment {
    /// The size as a person reads it.
    pub fn human_size(&self) -> String {
        human_size(self.size)
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [("GB", 1 << 30), ("MB", 1 << 20), ("kB", 1 << 10), ("B", 1)];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            let value = bytes as f64 / scale as f64;
            return if scale == 1 || value >= 10.0 {
                format!("{} {unit}", value.round() as u64)
            } else {
                format!("{value:.1} {unit}")
            };
        }
    }
    "0 B".to_string()
}

/// Everything hanging off the message that is not the letter.
pub fn attachments(root: &Part) -> Vec<Attachment> {
    let letter = pick(root).and_then(|part| part.part_name.clone());
    let mut found = Vec::new();
    collect(root, letter.as_deref(), &mut found);
    found
}

fn collect(part: &Part, letter: Option<&str>, found: &mut Vec<Attachment>) {
    if part.is_container() {
        for child in &part.parts {
            collect(child, letter, found);
        }
        return;
    }
    if part.part_name.as_deref() == letter || part.part_name.is_none() {
        return;
    }
    let named = part.name.as_deref().map(str::trim).filter(|name| !name.is_empty());
    let media = part.media_type();
    // A part is an attachment if it has a filename, or if it is something a letter is not made of.
    if named.is_none() && (media.starts_with("text/") || media.is_empty()) {
        return;
    }
    found.push(Attachment {
        part_name: part.part_name.clone().unwrap_or_default(),
        name: named.unwrap_or(media).to_string(),
        content_type: media.to_string(),
        size: part.size.unwrap_or(0),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn part(value: serde_json::Value) -> Part {
        serde_json::from_value(value).expect("a part the test wrote")
    }

    fn alternative(plain: &str, html: &str) -> Part {
        part(json!({
            "contentType": "multipart/alternative", "partName": "",
            "parts": [
                {"contentType": "text/plain", "partName": "1", "body": plain},
                {"contentType": "text/html", "partName": "2", "body": html},
            ]
        }))
    }

    #[test]
    fn a_plain_alternative_wins_because_it_is_what_the_sender_wrote() {
        let message = alternative(
            "The actual letter, written out at some length by a person who had \
             something to say and said it in plain text, as people do.",
            "<p>marketing</p>",
        );
        let body = body(&message);
        assert_eq!(body.flavour, Flavour::Plain);
        assert!(body.markdown.starts_with("The actual letter"));
    }

    /// The plain alternative winning is a choice about the *default* view — it doesn't mean the
    /// sender's HTML is gone. "Original formatting" needs it regardless of which part `pick` chose.
    #[test]
    fn a_plain_alternative_winning_does_not_lose_the_html_it_beat() {
        let message = alternative(
            "The actual letter, written out at some length by a person who had \
             something to say and said it in plain text, as people do.",
            "<p>marketing</p>",
        );
        let body = body(&message);
        assert_eq!(body.flavour, Flavour::Plain, "plain still wins the default view");
        assert_eq!(body.raw_html.as_deref(), Some("<p>marketing</p>"));
    }

    /// ...unless the plain part is an apology for the HTML one, which is not a letter.
    #[test]
    fn a_plain_part_that_is_only_an_apology_loses_to_the_html_one() {
        let message = alternative("This email requires HTML.", "<p>The <b>actual</b> letter</p>");
        let body = body(&message);
        assert_eq!(body.flavour, Flavour::Html);
        assert_eq!(body.markdown, "The **actual** letter");
    }

    #[test]
    fn markdown_outranks_both_because_somebody_meant_it() {
        let message = part(json!({
            "contentType": "multipart/alternative", "partName": "",
            "parts": [
                {"contentType": "text/plain", "partName": "1", "body": "a long plain fallback that is a whole letter"},
                {"contentType": "text/markdown", "partName": "2", "body": "# heading\n\nand **this**"},
            ]
        }));
        let body = body(&message);
        assert_eq!(body.flavour, Flavour::Markdown);
        assert_eq!(body.markdown, "# heading\n\nand **this**");
    }

    #[test]
    fn a_letter_inside_related_inside_mixed_is_still_the_letter() {
        let message = part(json!({
            "contentType": "multipart/mixed", "partName": "",
            "parts": [
                {"contentType": "multipart/related", "partName": "1", "parts": [
                    {"contentType": "text/html", "partName": "1.1", "body": "<p>hello</p>"},
                    {"contentType": "image/png", "partName": "1.2", "name": "logo.png", "size": 2048},
                ]},
                {"contentType": "application/pdf", "partName": "2", "name": "invoice.pdf", "size": 51200},
            ]
        }));
        assert_eq!(body(&message).markdown, "hello");
        let found = attachments(&message);
        assert_eq!(found.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["logo.png", "invoice.pdf"]);
        assert_eq!(found[1].part_name, "2");
        assert_eq!(found[1].human_size(), "50 kB");
    }

    /// A forward or a bounce wraps the original message whole, headers and all, as
    /// `message/rfc822` rather than `multipart/*` — real mail nests this where the fixture never
    /// did, and it used to leave the letter empty with an unfetchable "attachment" in its place.
    #[test]
    fn a_letter_inside_a_forwarded_message_is_still_the_letter() {
        let message = part(json!({
            "contentType": "multipart/mixed", "partName": "",
            "parts": [{"contentType": "message/rfc822", "partName": "1", "parts": [
                {"contentType": "multipart/alternative", "partName": "1.1", "parts": [
                    {"contentType": "text/plain", "partName": "1.1.1", "body": "The original letter, written out at some \
                        length by a person who had something to say and said it in plain text, as people do."},
                    {"contentType": "text/html", "partName": "1.1.2", "body": "<p>marketing</p>"},
                ]},
                {"contentType": "application/pdf", "partName": "1.2", "name": "invoice.pdf", "size": 1024},
            ]}]
        }));
        assert!(body(&message).markdown.starts_with("The original letter"));
        let found = attachments(&message);
        assert_eq!(found.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["invoice.pdf"]);
    }

    #[test]
    fn a_message_with_nothing_readable_in_it_is_empty_rather_than_a_panic() {
        assert_eq!(body(&Part::default()), Body::default());
        let only_attachment = part(json!({
            "contentType": "multipart/mixed", "partName": "",
            "parts": [{"contentType": "application/octet-stream", "partName": "1", "name": "thing.bin", "size": 1}]
        }));
        assert_eq!(body(&only_attachment).flavour, Flavour::Empty);
        assert_eq!(attachments(&only_attachment).len(), 1);
    }

    #[test]
    fn content_type_parameters_reach_the_converter() {
        let flowed = part(json!({
            "contentType": "text/plain; charset=UTF-8; format=Flowed; delsp=YES", "partName": "1",
            "body": "one \ntwo"
        }));
        assert_eq!(flowed.parameter("format").as_deref(), Some("flowed"));
        assert_eq!(body(&flowed).markdown, "onetwo", "delsp=yes eats the space that marked the flow");
    }

    #[test]
    fn sizes_read_the_way_a_person_reads_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(900), "900 B");
        assert_eq!(human_size(1536), "1.5 kB");
        assert_eq!(human_size(51200), "50 kB");
        assert_eq!(human_size(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(human_size(15 * 1024 * 1024), "15 MB");
    }
}
