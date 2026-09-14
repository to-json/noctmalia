//! Reading the envelope, and saying what is odd about it.
//!
//! Two jobs. The small one is parsing the address forms mail uses, which nothing else in the
//! program should have to know about. The large one is [`examine`]: the checks that turn a message
//! into a short list of things worth knowing before you believe it.
//!
//! The rule those checks are written to is that **the mailhost is a witness, not a judge**. An
//! `Authentication-Results` header is a claim made by whichever machine handed us the message, and
//! we can neither verify nor replace it — so it is reported as what it is, attributed, and never
//! turned into a verdict of our own. What we *can* do in the client is compare a message against
//! itself: what the sender's name claims against the address behind it, what a link says against
//! where it goes, what the relay chain asserts against the rest of the relay chain. Those need no
//! trust at all, which is why they carry the loudest findings here.

use crate::mime::html::Misleading;
use serde::Deserialize;
use std::collections::BTreeMap;

/// The headers of one message, as Thunderbird hands them over: lowercased names, a list of values
/// each, unfolded and decoded.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct Headers(BTreeMap<String, Vec<String>>);

impl Headers {
    pub fn all(&self, name: &str) -> &[String] {
        let lowered = name.to_ascii_lowercase();
        self.0.get(&lowered).map_or(&[], Vec::as_slice)
    }

    pub fn first(&self, name: &str) -> Option<&str> {
        self.all(name).first().map(String::as_str)
    }

    /// Every header, in the order a raw message would have shown them — near enough, since what
    /// survives into this map is sorted rather than original. For the header view.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().flat_map(|(name, values)| values.iter().map(move |value| (name.as_str(), value.as_str())))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    fn of(pairs: &[(&str, &str)]) -> Headers {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, value) in pairs {
            map.entry(name.to_ascii_lowercase()).or_default().push((*value).to_string());
        }
        Headers(map)
    }
}

// ── Addresses ───────────────────────────────────────────────────────────────

/// One mailbox: what it calls itself, and where it actually is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub address: String,
}

impl Address {
    /// What to put on screen: the name if there is one, the address otherwise.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.address } else { &self.name }
    }

    /// The domain, lowercased, or empty if this is not an address.
    pub fn domain(&self) -> &str {
        self.address.rsplit_once('@').map_or("", |(_, domain)| domain)
    }

    /// The initials a placeholder avatar shows.
    pub fn initials(&self) -> String {
        let source = self.label();
        let mut initials = String::new();
        for word in source.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()) {
            if let Some(first) = word.chars().next() {
                initials.extend(first.to_uppercase());
            }
            if initials.chars().count() == 2 {
                break;
            }
        }
        if initials.is_empty() { "•".to_string() } else { initials }
    }
}

/// One address in any of the shapes mail writes them: `a@b`, `<a@b>`, `Name <a@b>`,
/// `"Name, with comma" <a@b>`.
pub fn address(raw: &str) -> Address {
    let raw = raw.trim();
    if let Some(open) = raw.rfind('<') {
        let close = raw[open..].find('>').map_or(raw.len(), |offset| open + offset);
        let name = unquote(raw[..open].trim());
        let address = raw[open + 1..close].trim().to_string();
        // `<a@b>` with nothing in front is an address, not a nameless name.
        return Address { name, address };
    }
    let mut address = Address { name: String::new(), address: raw.to_string() };
    // `a@b (Name)` is the other spelling, and still turns up from mailing lists.
    if let (Some(open), true) = (raw.find('('), raw.ends_with(')')) {
        address.name = raw[open + 1..raw.len() - 1].trim().to_string();
        address.address = raw[..open].trim().to_string();
    }
    address
}

/// A header holding a list of addresses. Commas inside a quoted name do not separate.
pub fn addresses(raw: &str) -> Vec<Address> {
    let mut found = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut angled = false;
    for (offset, character) in raw.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            ',' if !quoted && !angled => {
                let piece = raw[start..offset].trim();
                if !piece.is_empty() {
                    found.push(address(piece));
                }
                start = offset + character.len_utf8();
            }
            _ => {}
        }
    }
    let piece = raw[start..].trim();
    if !piece.is_empty() {
        found.push(address(piece));
    }
    found
}

fn unquote(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return trimmed[1..trimmed.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\");
    }
    trimmed.to_string()
}

// ── Findings ────────────────────────────────────────────────────────────────

/// How much a finding deserves to interrupt the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Worth knowing, not worth worrying about.
    Note,
    /// Something does not line up.
    Caution,
    /// The message is lying about itself in a way it takes effort to do by accident.
    Alarm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    /// A few words, for a badge.
    pub headline: &'static str,
    /// A sentence, for the panel.
    pub detail: String,
}

/// Characters that change the order text reads in, or that are not there at all. In a display name
/// they have exactly one use.
const DECEPTIVE: &[(char, &str)] = &[
    ('\u{202a}', "left-to-right embedding"),
    ('\u{202b}', "right-to-left embedding"),
    ('\u{202c}', "directional formatting"),
    ('\u{202d}', "left-to-right override"),
    ('\u{202e}', "right-to-left override"),
    ('\u{2066}', "left-to-right isolate"),
    ('\u{2067}', "right-to-left isolate"),
    ('\u{2068}', "first-strong isolate"),
    ('\u{200b}', "zero-width space"),
    ('\u{200c}', "zero-width non-joiner"),
    ('\u{200d}', "zero-width joiner"),
    ('\u{feff}', "zero-width no-break space"),
];

/// Everything odd about this message, loudest first.
pub fn examine(headers: &Headers, misleading: &[Misleading]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let from = headers.first("from").map(address).unwrap_or_default();

    identity(&from, &mut findings);
    reply_path(headers, &from, &mut findings);
    deception(headers, &from, &mut findings);
    authentication(headers, &from, &mut findings);
    relays(headers, &mut findings);
    for link in misleading {
        findings.push(Finding {
            level: Level::Alarm,
            headline: "Link goes elsewhere",
            detail: format!("A link reading “{}” actually points at {}.", link.shown, link.actual),
        });
    }

    findings.sort_by_key(|finding| std::cmp::Reverse(finding.level));
    findings
}

/// The sender's name against the sender's address.
fn identity(from: &Address, findings: &mut Vec<Finding>) {
    if from.name.is_empty() || from.address.is_empty() {
        return;
    }
    // A display name holding an address is either a mail client being unhelpful or somebody
    // counting on the address being the only part you read.
    if let Some(claimed) = addresses(&from.name).into_iter().find(|claimed| claimed.address.contains('@')) {
        if !claimed.address.eq_ignore_ascii_case(&from.address) {
            findings.push(Finding {
                level: Level::Alarm,
                headline: "Name is another address",
                detail: format!(
                    "The sender calls itself {} but the message is from {}.",
                    claimed.address, from.address
                ),
            });
        }
        return;
    }
    // The same trick without the `@`: a name that is a domain, and not this one.
    let name = from.name.trim().trim_end_matches('.');
    let looks_like_a_domain =
        name.contains('.') && !name.contains(' ') && name.split('.').all(|label| !label.is_empty());
    if looks_like_a_domain && !related(name, from.domain()) {
        findings.push(Finding {
            level: Level::Caution,
            headline: "Name is another domain",
            detail: format!("The sender calls itself {name} but the message is from {}.", from.domain()),
        });
    }
}

/// Where a reply would go, and where a bounce came from.
fn reply_path(headers: &Headers, from: &Address, findings: &mut Vec<Finding>) {
    if from.domain().is_empty() {
        return;
    }
    if let Some(reply) = headers.first("reply-to").map(address)
        && !reply.domain().is_empty()
        && !related(reply.domain(), from.domain())
    {
        findings.push(Finding {
            level: Level::Caution,
            headline: "Replies go elsewhere",
            detail: format!("A reply would go to {}, not to {}.", reply.address, from.domain()),
        });
    }
    // A mailing list legitimately rewrites the envelope sender, so this is worth saying and not
    // worth alarming about.
    if let Some(path) = headers.first("return-path").map(address) {
        let bounce = path.domain();
        if !bounce.is_empty() && !related(bounce, from.domain()) {
            findings.push(Finding {
                level: Level::Note,
                headline: "Sent by another domain",
                detail: format!("The envelope came from {bounce} on behalf of {}.", from.domain()),
            });
        }
    }
}

/// Characters chosen to make the text read as something other than what it is.
fn deception(headers: &Headers, from: &Address, findings: &mut Vec<Finding>) {
    let subject = headers.first("subject").unwrap_or_default();
    for (field, text) in [("sender's name", from.name.as_str()), ("subject", subject)] {
        if let Some((_, what)) = DECEPTIVE.iter().find(|(character, _)| text.contains(*character)) {
            findings.push(Finding {
                level: Level::Alarm,
                headline: "Hidden characters",
                detail: format!("The {field} contains a {what}, which changes how it reads."),
            });
        }
    }
}

/// What the receiving server says it could and could not verify — reported as its claim, since it
/// is not ours to make or to check.
fn authentication(headers: &Headers, from: &Address, findings: &mut Vec<Finding>) {
    let results = headers.all("authentication-results");
    if results.is_empty() {
        if !from.domain().is_empty() {
            findings.push(Finding {
                level: Level::Note,
                headline: "Nothing checked it",
                detail: "No server on the way here recorded an SPF, DKIM or DMARC result.".to_string(),
            });
        }
        return;
    }
    let joined = results.join(" ").to_ascii_lowercase();
    let reporter = joined.split_whitespace().next().unwrap_or("the receiving server").trim_end_matches(';').to_string();
    // A headline per check rather than one for all of them: three badges reading "Failed a check"
    // say less than one reading "DMARC failed", and take three times the room to do it.
    for (method, name, failed, unverified) in [
        ("dmarc=", "DMARC", "DMARC failed", "DMARC unverified"),
        ("dkim=", "DKIM", "DKIM failed", "DKIM unverified"),
        ("spf=", "SPF", "SPF failed", "SPF unverified"),
    ] {
        let Some(at) = joined.find(method) else { continue };
        let verdict = joined[at + method.len()..].split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
        let (level, headline) = match verdict {
            "fail" | "permerror" => (Level::Alarm, failed),
            "softfail" | "temperror" | "none" => (Level::Caution, unverified),
            _ => continue,
        };
        findings.push(Finding {
            level,
            headline,
            detail: format!("{reporter} reports {name} {verdict} for this message."),
        });
    }
}

/// Whether the relay chain joins up.
///
/// `Received` headers are stacked newest first, and each says which host it took the message
/// `from` and which host it arrived `by`. Read down the stack, one hop's `by` should be the next
/// one's `from`. A chain that does not join has had a hop forged into it — or, much more often, a
/// relay that writes its headers loosely, which is why this is a caution and not an alarm.
fn relays(headers: &Headers, findings: &mut Vec<Finding>) {
    let hops: Vec<(String, String)> = headers.all("received").iter().map(|hop| via(hop)).collect();
    if hops.len() < 2 {
        return;
    }
    for pair in hops.windows(2) {
        let (_, by) = &pair[1];
        let (from, _) = &pair[0];
        if by.is_empty() || from.is_empty() {
            continue;
        }
        if !related(host(from), host(by)) {
            findings.push(Finding {
                level: Level::Caution,
                headline: "Relay chain is broken",
                detail: format!("A hop says it came from {from}, but the hop below it handed off at {by}."),
            });
            return;
        }
    }
}

/// The `from` and `by` hosts of one `Received` header.
fn via(hop: &str) -> (String, String) {
    let word_after = |keyword: &str| {
        let lowered = format!(" {keyword} ");
        let padded = format!(" {} ", hop.split_whitespace().collect::<Vec<_>>().join(" "));
        let at = padded.find(&lowered)?;
        let rest = &padded[at + lowered.len()..];
        Some(rest.split_whitespace().next().unwrap_or("").trim_matches(['(', ')', ';']).to_string())
    };
    (word_after("from").unwrap_or_default(), word_after("by").unwrap_or_default())
}

/// The registrable-ish part of a host: the last two labels, which is what makes
/// `mx1.mail.example.com` and `out3.example.com` the same organisation without a public suffix list.
fn host(name: &str) -> &str {
    let name = name.trim().trim_end_matches('.');
    let mut labels = name.rsplitn(3, '.');
    match (labels.next(), labels.next()) {
        (Some(top), Some(second)) => &name[name.len() - top.len() - second.len() - 1..],
        _ => name,
    }
}

/// Whether two hosts belong to the same place, as far as anything without a suffix list can tell.
fn related(left: &str, right: &str) -> bool {
    let (left, right) = (host(left).to_ascii_lowercase(), host(right).to_ascii_lowercase());
    left == right || left.is_empty() || right.is_empty()
}

/// Where this message says it can be unsubscribed from, if it says so at all.
///
/// The `mailto:` form is preferred over the `https:` one: it needs no browser, gives the sender no
/// confirmation that the address is read by a person, and is the one a mail client can act on.
pub fn unsubscribe(headers: &Headers) -> Option<String> {
    let header = headers.first("list-unsubscribe")?;
    let links: Vec<&str> =
        header.split(',').map(|link| link.trim().trim_start_matches('<').trim_end_matches('>')).collect();
    let pick = links
        .iter()
        .find(|link| link.starts_with("mailto:"))
        .or_else(|| links.iter().find(|link| link.starts_with("https://")))?;
    Some((*pick).to_string())
}

/// Headers worth putting in front of somebody, in the order they are worth reading.
///
/// Not all of them: a raw header view is `\`, and this is the short list that answers "who really
/// sent this and what did the machines think of it".
pub const INTERESTING: &[&str] = &[
    "from",
    "reply-to",
    "return-path",
    "sender",
    "to",
    "cc",
    "date",
    "subject",
    "message-id",
    "in-reply-to",
    "list-id",
    "list-unsubscribe",
    "authentication-results",
    "received-spf",
    "dkim-signature",
    "x-mailer",
    "user-agent",
    "precedence",
    "auto-submitted",
    "x-spam-status",
    "x-spam-score",
    "x-spam-flag",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(findings: &[Finding]) -> Vec<(Level, &'static str)> {
        findings.iter().map(|finding| (finding.level, finding.headline)).collect()
    }

    #[test]
    fn every_shape_of_address_parses() {
        assert_eq!(address("a@b.example"), Address { name: String::new(), address: "a@b.example".into() });
        assert_eq!(address("<a@b.example>"), Address { name: String::new(), address: "a@b.example".into() });
        assert_eq!(address("Ada <a@b.example>"), Address { name: "Ada".into(), address: "a@b.example".into() });
        assert_eq!(
            address("\"Lovelace, Ada\" <a@b.example>"),
            Address { name: "Lovelace, Ada".into(), address: "a@b.example".into() }
        );
        assert_eq!(address("a@b.example (Ada)"), Address { name: "Ada".into(), address: "a@b.example".into() });
    }

    #[test]
    fn a_comma_inside_a_name_does_not_split_the_list() {
        let list = addresses("\"Lovelace, Ada\" <a@b.example>, Bob <b@c.example>, d@e.example");
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "Lovelace, Ada");
        assert_eq!(list[2].address, "d@e.example");
    }

    #[test]
    fn initials_come_from_the_name_and_fall_back_to_the_address() {
        assert_eq!(address("Ada Lovelace <a@b.example>").initials(), "AL");
        assert_eq!(address("ada@example.com").initials(), "AE", "the address is all there is");
        assert_eq!(Address::default().initials(), "•");
    }

    /// The loudest check, and the one that needs nobody's word for it.
    #[test]
    fn a_display_name_wearing_another_address_is_an_alarm() {
        let headers = Headers::of(&[
            ("From", "\"billing@paypal.com\" <collect@mail.ru>"),
            ("Authentication-Results", "mx.example.net; spf=pass; dkim=pass; dmarc=pass"),
        ]);
        let findings = examine(&headers, &[]);
        assert_eq!(levels(&findings), [(Level::Alarm, "Name is another address")]);
        assert!(findings[0].detail.contains("collect@mail.ru"));
    }

    #[test]
    fn a_display_name_that_is_a_bare_domain_is_a_caution_unless_it_is_the_real_one() {
        let lying = Headers::of(&[("From", "paypal.com <collect@mail.ru>")]);
        assert!(levels(&examine(&lying, &[])).contains(&(Level::Caution, "Name is another domain")));

        let honest = Headers::of(&[("From", "paypal.com <service@mail.paypal.com>")]);
        let findings = examine(&honest, &[]);
        assert!(!findings.iter().any(|f| f.headline == "Name is another domain"), "{findings:?}");
    }

    #[test]
    fn a_reply_to_somewhere_else_is_worth_saying_and_a_bounce_address_is_only_worth_noting() {
        let headers = Headers::of(&[
            ("From", "Bank <no-reply@bank.example>"),
            ("Reply-To", "helpdesk@free-mail.example"),
            ("Return-Path", "<bounce-123@sendgrid.example>"),
            ("Authentication-Results", "mx.example.net; dmarc=pass"),
        ]);
        let found = levels(&examine(&headers, &[]));
        assert!(found.contains(&(Level::Caution, "Replies go elsewhere")));
        assert!(found.contains(&(Level::Note, "Sent by another domain")));
        // Loudest first, so the badge row reads in the right order.
        assert_eq!(found.first(), Some(&(Level::Caution, "Replies go elsewhere")));
    }

    #[test]
    fn a_subdomain_is_the_same_organisation_and_does_not_raise_anything() {
        let headers = Headers::of(&[
            ("From", "Bank <no-reply@mail.bank.example>"),
            ("Reply-To", "support@bank.example"),
            ("Return-Path", "<bounce@out3.bank.example>"),
            ("Authentication-Results", "mx.example.net; dmarc=pass"),
        ]);
        assert_eq!(examine(&headers, &[]), Vec::new());
    }

    #[test]
    fn a_failed_check_is_reported_as_the_claim_of_whoever_made_it() {
        let headers = Headers::of(&[
            ("From", "Bank <no-reply@bank.example>"),
            ("Authentication-Results", "mx.example.net; spf=fail smtp.mailfrom=bank.example; dkim=none; dmarc=fail"),
        ]);
        let found = levels(&examine(&headers, &[]));
        assert!(found.contains(&(Level::Alarm, "SPF failed")));
        assert!(found.contains(&(Level::Alarm, "DMARC failed")));
        assert!(found.contains(&(Level::Caution, "DKIM unverified")));
        // Attributed, never asserted.
        let findings = examine(&headers, &[]);
        assert!(findings.iter().all(|f| !f.headline.contains("SPF") || f.detail.starts_with("mx.example.net reports")));
    }

    #[test]
    fn a_message_nothing_vouched_for_says_so_quietly() {
        let headers = Headers::of(&[("From", "Someone <a@b.example>")]);
        assert_eq!(levels(&examine(&headers, &[])), [(Level::Note, "Nothing checked it")]);
    }

    #[test]
    fn a_relay_chain_that_does_not_join_up_is_noticed_and_one_that_does_is_not() {
        let broken = Headers::of(&[
            ("From", "a@b.example"),
            ("Authentication-Results", "mx.example.net; dmarc=pass"),
            ("Received", "from unknown.ru (unknown.ru [1.2.3.4]) by mx.example.net with ESMTP"),
            ("Received", "from sender.example (sender.example [5.6.7.8]) by relay.elsewhere.example with ESMTP"),
        ]);
        assert!(levels(&examine(&broken, &[])).contains(&(Level::Caution, "Relay chain is broken")));

        let sound = Headers::of(&[
            ("From", "a@b.example"),
            ("Authentication-Results", "mx.example.net; dmarc=pass"),
            ("Received", "from relay.example (relay.example [1.2.3.4]) by mx.example.net with ESMTP"),
            ("Received", "from sender.example (sender.example [5.6.7.8]) by mx2.relay.example with ESMTP"),
        ]);
        assert_eq!(examine(&sound, &[]), Vec::new(), "a sound chain says nothing");
    }

    #[test]
    fn a_name_written_to_read_backwards_is_an_alarm() {
        let headers = Headers::of(&[
            ("From", "\"Invoice \u{202e}fdp.exe\" <a@b.example>"),
            ("Authentication-Results", "mx.example.net; dmarc=pass"),
        ]);
        assert!(levels(&examine(&headers, &[])).contains(&(Level::Alarm, "Hidden characters")));
    }

    #[test]
    fn a_link_that_goes_somewhere_other_than_it_says_is_an_alarm() {
        let headers = Headers::of(&[("From", "a@b.example"), ("Authentication-Results", "m; dmarc=pass")]);
        let misleading = [Misleading { shown: "bank.example".into(), actual: "https://evil.example/go".into() }];
        assert_eq!(levels(&examine(&headers, &misleading)), [(Level::Alarm, "Link goes elsewhere")]);
    }

    #[test]
    fn unsubscribing_prefers_the_address_over_the_web_page() {
        let both = Headers::of(&[("List-Unsubscribe", "<https://list.example/u?x=1>, <mailto:u@list.example>")]);
        assert_eq!(unsubscribe(&both).as_deref(), Some("mailto:u@list.example"));
        let web = Headers::of(&[("List-Unsubscribe", "<https://list.example/u?x=1>")]);
        assert_eq!(unsubscribe(&web).as_deref(), Some("https://list.example/u?x=1"));
        assert_eq!(unsubscribe(&Headers::default()), None);
    }

    #[test]
    fn hosts_are_compared_by_the_organisation_they_belong_to() {
        assert_eq!(host("mx1.mail.example.com"), "example.com");
        assert_eq!(host("example.com"), "example.com");
        assert_eq!(host("localhost"), "localhost");
        assert!(related("mx1.example.com", "out3.example.com"));
        assert!(!related("example.com", "example.net"));
    }
}
