//! Mail over the bridge: folders, messages, flags, compose, and Thunderbird's own index.
//!
//! The same shape as [`crate::contacts`], for the same reason: typed async functions over
//! [`Bridge`], errors flattened to `String` because an iced message must be `Clone` and there is
//! nothing to do with a bridge error but show it.
//!
//! Almost nothing here is ours. Threading is Gloda's `conversationID`, search is Gloda's own
//! full-text index, flags and tags are Thunderbird's and round-trip to IMAP, and rules are
//! `msgFilterRules.dat`. What this module adds is names, and the two or three places where a
//! Thunderbird call needs more than one round trip to answer a question worth asking.

use crate::base64;
use crate::mime;
use noctmalia_bridge::Bridge;
use serde::Deserialize;
use serde_json::{Value, json};

/// Errors reach the UI as text: iced messages must be `Clone`, and there is nothing to do with a
/// bridge error but show it.
pub type Result<T> = std::result::Result<T, String>;

fn failed<T>(result: std::result::Result<T, noctmalia_bridge::Error>) -> Result<T> {
    result.map_err(|error| error.to_string())
}

/// How many messages a folder is listed to before the rest is left alone.
///
/// `messages.list` pages at a hundred at a time, and every page is a round trip through a Python
/// shim, so an archive with a hundred thousand messages in it is a minute of nothing happening.
/// The list arrives page by page and is usable from the first one; this is where it stops asking.
pub const LIST_CAP: usize = 20_000;

// ── Accounts and folders ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Folder {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(rename = "accountId", default)]
    pub account: String,
    /// Thunderbird's own word for what the folder is for: `inbox`, `sent`, `drafts`, `trash`,
    /// `junk`, `archives`, `templates`, `outbox`. Newer builds put it in `specialUse` and keep
    /// `type` as a deprecated alias, so both are read and the first answer wins.
    #[serde(rename = "specialUse", default)]
    pub special_use: Vec<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(rename = "subFolders", default)]
    pub children: Vec<Folder>,
    #[serde(rename = "isFavorite", default)]
    pub favourite: bool,
}

impl Folder {
    /// What this folder is for, or "" for an ordinary one.
    pub fn purpose(&self) -> &str {
        self.special_use.first().map(String::as_str).or(self.kind.as_deref()).unwrap_or("")
    }

    /// Where a folder sorts in a rail. Inbox first, then the folders you act in, then everything
    /// else alphabetically, then the three nobody opens on purpose.
    pub fn rank(&self) -> u8 {
        match self.purpose() {
            "inbox" => 0,
            "drafts" => 1,
            "outbox" => 2,
            "sent" => 3,
            "archives" => 4,
            "" => 5,
            "templates" => 6,
            "junk" => 7,
            "trash" => 8,
            _ => 5,
        }
    }

    /// The glyph the rail marks it with.
    pub fn glyph(&self) -> char {
        use crate::ui::icon;
        match self.purpose() {
            "inbox" => icon::INBOX,
            "drafts" => icon::PENCIL,
            "outbox" | "sent" => icon::SEND,
            "archives" => icon::ARCHIVE,
            "templates" => icon::FILE,
            "junk" => icon::SHIELD_X,
            "trash" => icon::TRASH,
            _ => icon::FOLDER,
        }
    }

    /// This folder and everything under it, flattened, each with how deep it is.
    pub fn flatten(&self, depth: usize, into: &mut Vec<(usize, Folder)>) {
        into.push((depth, self.clone()));
        let mut children = self.children.clone();
        children
            .sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        for child in &children {
            child.flatten(depth + 1, into);
        }
    }
}

/// What a folder holds, which the folder itself does not say.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Counts {
    #[serde(rename = "totalMessageCount", default)]
    pub total: i64,
    #[serde(rename = "unreadMessageCount", default)]
    pub unread: i64,
    #[serde(rename = "newMessageCount", default)]
    pub new: i64,
}

pub async fn accounts(bridge: Bridge) -> Result<Vec<Account>> {
    failed(bridge.call("accounts.list", json!({ "includeSubFolders": true })).await)
}

pub async fn counts(bridge: Bridge, folder: String) -> Result<(String, Counts)> {
    let info = failed(bridge.call("folders.getFolderInfo", json!({ "folderId": folder })).await)?;
    Ok((folder, info))
}

// ── Messages ────────────────────────────────────────────────────────────────

/// A message as the index knows it: everything but the letter itself.
#[derive(Debug, Clone, Deserialize)]
pub struct Header {
    /// Thunderbird's per-session handle. Durable references use `message_id` and a folder.
    pub id: u64,
    #[serde(rename = "headerMessageId", default)]
    pub message_id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub recipients: Vec<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub flagged: bool,
    #[serde(default)]
    pub junk: bool,
    #[serde(default)]
    pub new: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub folder: Option<FolderRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FolderRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

impl Header {
    pub fn from(&self) -> mime::headers::Address {
        mime::headers::address(&self.author)
    }

    /// The subject with any number of `Re:` and `Fwd:` markers taken off, which is what a thread
    /// is named by and what two messages are compared on.
    pub fn stem(&self) -> &str {
        let mut rest = self.subject.trim();
        loop {
            let lowered = rest.to_ascii_lowercase();
            let marker = ["re:", "aw:", "fwd:", "fw:", "vs:", "sv:", "antw:"]
                .into_iter()
                .find(|marker| lowered.starts_with(marker));
            match marker {
                Some(marker) => rest = rest[marker.len()..].trim_start(),
                None => return rest,
            }
        }
    }

    pub fn when(&self) -> Option<chrono::DateTime<chrono::Local>> {
        moment(self.date.as_deref()?)
    }

    /// The date as an index column: the time if it arrived today, the day if this week, the date
    /// otherwise. A column of full timestamps is a column nobody reads.
    pub fn stamp(&self) -> String {
        let Some(when) = self.when() else { return String::new() };
        stamp(when, chrono::Local::now())
    }

    /// The date in full, for the pager, where there is room to be exact.
    pub fn full_date(&self) -> String {
        use chrono::format::strftime::StrftimeItems;
        let Some(when) = self.when() else { return self.date.clone().unwrap_or_default() };
        when.format_with_items(StrftimeItems::new("%a %-d %b %Y at %H:%M")).to_string()
    }
}

fn moment(text: &str) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    // A JS `Date` crosses as ISO 8601; a header Thunderbird passed through unchanged is RFC 2822.
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&chrono::Local));
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc2822(text) {
        return Some(parsed.with_timezone(&chrono::Local));
    }
    // A bare epoch in milliseconds, which is what a fixture is easiest to write with.
    let millis: i64 = text.parse().ok()?;
    chrono::Local.timestamp_millis_opt(millis).single()
}

/// The index column, given what time it is now.
fn stamp(when: chrono::DateTime<chrono::Local>, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::Datelike;
    use chrono::format::strftime::StrftimeItems;
    let format = |pattern| when.format_with_items(StrftimeItems::new(pattern)).to_string();
    let elapsed = now.signed_duration_since(when);
    if when.date_naive() == now.date_naive() {
        return format("%H:%M");
    }
    if elapsed.num_days() < 6 && elapsed.num_seconds() >= 0 {
        return format("%a %H:%M");
    }
    if when.year() == now.year() {
        return format("%-d %b");
    }
    format("%-d %b %Y")
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    /// Present while there is more to come.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub messages: Vec<Header>,
}

/// The first page of a folder.
pub async fn list(bridge: Bridge, folder: String) -> Result<Page> {
    failed(bridge.call("messages.list", json!({ "folderId": folder })).await)
}

/// The next page of one already started.
pub async fn page(bridge: Bridge, list_id: String) -> Result<Page> {
    failed(bridge.call("messages.continueList", json!({ "listId": list_id })).await)
}

/// Stops a listing we are no longer interested in, so Thunderbird can drop it.
pub async fn abort(bridge: Bridge, list_id: String) -> Result<()> {
    failed(bridge.call_raw("messages.abortList", json!({ "listId": list_id })).await)?;
    Ok(())
}

/// Everything about one message that the index does not carry: the letter, what it is made of, and
/// what is odd about it.
#[derive(Debug, Clone)]
pub struct Letter {
    pub id: u64,
    pub body: mime::Body,
    pub attachments: Vec<mime::Attachment>,
    pub headers: mime::headers::Headers,
    pub findings: Vec<mime::headers::Finding>,
    pub unsubscribe: Option<String>,
}

pub async fn letter(bridge: Bridge, id: u64) -> Result<Letter> {
    let part: mime::Part = failed(bridge.call("messages.getFull", json!({ "messageId": id })).await)?;
    Ok(read(id, &part))
}

/// Everything a letter is, from a MIME tree Thunderbird has already decoded.
///
/// Split out of [`letter`] so that a test can hand it a message without a Thunderbird behind it —
/// which is how the claim that nothing remote reaches the screen is checked against the actual
/// widget tree rather than against the converter alone.
pub fn read(id: u64, part: &mime::Part) -> Letter {
    let body = mime::body(part);
    let attachments = mime::attachments(part);
    let headers = part.headers.clone().unwrap_or_default();
    let findings = mime::headers::examine(&headers, &body.misleading);
    let unsubscribe = mime::headers::unsubscribe(&headers);
    Letter { id, body, attachments, headers, findings, unsubscribe }
}

#[derive(Debug, Clone, Deserialize)]
struct Binary {
    #[serde(default)]
    base64: String,
}

/// The message exactly as it arrived. The escape hatch for everything the renderer flattened.
pub async fn raw(bridge: Bridge, id: u64) -> Result<String> {
    let binary: Binary = failed(bridge.call("messages.getRaw", json!({ "messageId": id })).await)?;
    let bytes = base64::decode(&binary.base64).ok_or_else(|| "the raw message did not decode".to_string())?;
    // A raw message is whatever bytes the sender sent, which need not be valid UTF-8 anywhere.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub async fn attachment(bridge: Bridge, id: u64, part: String) -> Result<Vec<u8>> {
    let binary: Binary =
        failed(bridge.call("messages.getAttachment", json!({ "messageId": id, "partName": part })).await)?;
    base64::decode(&binary.base64).ok_or_else(|| "the attachment did not decode".to_string())
}

/// What a flag change asks for. `None` leaves a flag alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub read: Option<bool>,
    pub flagged: Option<bool>,
    pub junk: Option<bool>,
}

pub async fn mark(bridge: Bridge, ids: Vec<u64>, flags: Flags) -> Result<()> {
    let mut properties = serde_json::Map::new();
    if let Some(read) = flags.read {
        properties.insert("read".into(), read.into());
    }
    if let Some(flagged) = flags.flagged {
        properties.insert("flagged".into(), flagged.into());
    }
    if let Some(junk) = flags.junk {
        properties.insert("junk".into(), junk.into());
    }
    if properties.is_empty() {
        return Ok(());
    }
    failed(bridge.call_raw("messages.update", json!({ "messageIds": ids, "properties": properties })).await)?;
    Ok(())
}

pub async fn tag(bridge: Bridge, ids: Vec<u64>, tags: Vec<String>) -> Result<()> {
    failed(bridge.call_raw("messages.update", json!({ "messageIds": ids, "properties": { "tags": tags } })).await)?;
    Ok(())
}

pub async fn archive(bridge: Bridge, ids: Vec<u64>) -> Result<()> {
    failed(bridge.call_raw("messages.archive", json!({ "messageIds": ids })).await)?;
    Ok(())
}

pub async fn discard(bridge: Bridge, ids: Vec<u64>) -> Result<()> {
    failed(
        bridge
            .call_raw("messages.delete", json!({ "messageIds": ids, "deletePermanently": false, "isUserAction": true }))
            .await,
    )?;
    Ok(())
}

pub async fn relocate(bridge: Bridge, ids: Vec<u64>, folder: String) -> Result<()> {
    failed(
        bridge.call_raw("messages.move", json!({ "messageIds": ids, "folderId": folder, "isUserAction": true })).await,
    )?;
    Ok(())
}

/// Puts messages back where they came from.
///
/// A message's numeric id belongs to the session and to wherever it currently is, so undoing a move
/// cannot simply move the same ids back: the copy in the archive is a different message. What does
/// not change is `headerMessageId`, so this asks Thunderbird where each one ended up and moves
/// *that* home. Which is also why undo needs no state of ours beyond a list of strings.
pub async fn restore(bridge: Bridge, message_ids: Vec<String>, folder: String) -> Result<usize> {
    let mut found = Vec::new();
    for message_id in message_ids {
        // `autoPaginationTimeout: 0` or the call can sit there paginating rather than answering.
        let page: Page = failed(
            bridge.call("messages.query", json!({ "headerMessageId": message_id, "autoPaginationTimeout": 0 })).await,
        )?;
        // A message moved to the trash still matches, and so does the original if the move failed;
        // anything already home is not worth moving again.
        found.extend(
            page.messages
                .into_iter()
                .filter(|header| header.folder.as_ref().is_none_or(|at| at.id != folder))
                .map(|header| header.id),
        );
    }
    if found.is_empty() {
        return Ok(0);
    }
    let moved = found.len();
    relocate(bridge, found, folder).await?;
    Ok(moved)
}

/// Thunderbird's own search over the folder, for the fields the index already holds. The full-text
/// one is [`search`], which needs Gloda.
pub async fn query(bridge: Bridge, folder: Option<String>, text: String) -> Result<Vec<Header>> {
    let mut params = json!({ "subject": text.clone(), "autoPaginationTimeout": 0 });
    if let Some(folder) = folder {
        params["folderId"] = folder.into();
    }
    let page: Page = failed(bridge.call("messages.query", params).await)?;
    Ok(page.messages)
}

// ── Gloda: threading and search, which Thunderbird already has ──────────────

/// One conversation, as Gloda knows it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Conversation {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub subject: String,
    /// The `headerMessageId` of every message in it, oldest first.
    #[serde(default)]
    pub messages: Vec<String>,
}

/// Which conversation each of these messages is in.
///
/// Thunderbird has been threading the whole time — the gap was in the WebExtension API, not in
/// Thunderbird — so this asks Gloda rather than implementing JWZ over headers we would have to
/// fetch first. Gloda indexes asynchronously, so a message that has just arrived can briefly have
/// no conversation; that is a message on its own for a few seconds, not an error.
pub async fn conversations(bridge: Bridge, message_ids: Vec<String>) -> Result<Vec<Conversation>> {
    failed(bridge.call("gloda.conversations", json!({ "headerMessageIds": message_ids })).await)
}

/// Gloda's own ranked full-text search, which cannot be run from outside Thunderbird's process:
/// the index declares the `mozporter` tokenizer, which Gecko registers at runtime and stock SQLite
/// has never heard of.
pub async fn search(bridge: Bridge, text: String, limit: usize) -> Result<Vec<Header>> {
    failed(bridge.call("gloda.search", json!({ "query": text, "limit": limit })).await)
}

// ── Composing ───────────────────────────────────────────────────────────────

/// What to do with a draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deliver {
    Now,
    Draft,
}

/// A message on its way out.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub identity: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    /// Markdown, which is the one representation this program reads and writes.
    pub markdown: String,
    /// The message being replied to or forwarded, and how.
    pub about: Option<(u64, Reply)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Sender,
    All,
    Forward,
}

impl Reply {
    fn method(self) -> &'static str {
        match self {
            Reply::Sender => "replyToSender",
            Reply::All => "replyToAll",
            Reply::Forward => "forwardInline",
        }
    }
}

/// The details a compose window takes, built from a draft.
///
/// **A draft goes out as plain text, and the plain text is the Markdown.** The plan called for
/// `multipart/alternative` with an HTML rendering beside the source; Thunderbird's compose API
/// settles it the other way, because `ComposeDetails.plainTextBody` is used *only* when
/// `isPlainText` is true — send HTML and Thunderbird derives the plain part from the HTML rather
/// than taking the one we wrote. That would mean the recipient never receives what was typed, only
/// a round trip through a renderer.
///
/// Which is fine, because a Markdown document is a plain-text document: that is what the format is
/// for. Somebody reading it in another client sees `**this**` and knows what it means; somebody
/// reading it here sees it rendered; and the bytes are the ones their correspondent chose.
fn details(draft: &Draft) -> Value {
    json!({
        "identityId": draft.identity,
        "to": draft.to,
        "cc": draft.cc,
        "bcc": draft.bcc,
        "subject": draft.subject,
        "plainTextBody": draft.markdown,
        "isPlainText": true,
    })
}

/// Sends a draft, or saves it.
///
/// A reply goes through a compose window even though there is a windowless `messages.send`, because
/// only `compose.beginReply` sets `In-Reply-To` and `References` — and a reply that does not thread
/// is a reply that shows up in the wrong place in everybody else's mail client. The spike in
/// `spike/boundary-probe` is where that was established.
pub async fn send(bridge: Bridge, draft: Draft, deliver: Deliver) -> Result<String> {
    let details = details(&draft);
    let mode = match deliver {
        Deliver::Now => "sendNow",
        Deliver::Draft => "draft",
    };
    let result: Value = match draft.about {
        Some((about, reply)) => failed(
            bridge
                .call(
                    "compose.reply",
                    json!({ "messageId": about, "type": reply.method(), "details": details, "mode": mode }),
                )
                .await,
        )?,
        None => failed(bridge.call("compose.begin", json!({ "details": details, "mode": mode })).await)?,
    };
    Ok(result.get("headerMessageId").and_then(Value::as_str).unwrap_or_default().to_string())
}

/// The identities a message could be sent from.
#[derive(Debug, Clone, Deserialize)]
pub struct Identity {
    pub id: String,
    #[serde(default)]
    pub email: String,
    #[serde(rename = "name", default)]
    pub name: String,
    #[serde(rename = "accountId", default)]
    pub account: String,
}

pub async fn identities(bridge: Bridge) -> Result<Vec<Identity>> {
    failed(bridge.call("identities.list", json!({})).await)
}

// ── Screening ───────────────────────────────────────────────────────────────

/// One of Thunderbird's own message filters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// A sentence, built by the Experiment out of the filter's terms.
    #[serde(default)]
    pub summary: String,
}

pub async fn rules(bridge: Bridge, account: String) -> Result<Vec<Rule>> {
    failed(bridge.call("filters.list", json!({ "accountId": account })).await)
}

/// What a proposed rule would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    /// The header to match on — `from`, `to`, `subject`, or `list-id`.
    pub header: String,
    pub value: String,
    /// Where matching mail goes. The path travels with the id because a WebExtension folder id is
    /// opaque and not every Thunderbird resolves one back to a folder from privileged code.
    pub folder: String,
    pub folder_path: String,
    pub folder_name: String,
}

pub async fn screen(bridge: Bridge, account: String, screen: Screen) -> Result<String> {
    let created: Value = failed(
        bridge
            .call(
                "filters.create",
                json!({
                    "accountId": account,
                    "name": screen.name,
                    "header": screen.header,
                    "value": screen.value,
                    "folderId": screen.folder,
                    "folderPath": screen.folder_path,
                }),
            )
            .await,
    )?;
    Ok(created.get("name").and_then(Value::as_str).unwrap_or(&screen.name).to_string())
}

/// Moves the mail already in this folder that the new rule would have caught.
///
/// A rule only ever applies to what arrives next, and a rule you made *because of* the forty
/// newsletters in front of you should deal with the forty newsletters in front of you. Done here
/// with `messages.query` and `messages.move` rather than by asking Thunderbird to run the filter
/// list: those are two documented calls that work the same on every build, and they give back an
/// exact count instead of a promise.
///
/// Only sender rules can be swept this way — `messages.query` has no way to ask about `List-Id` —
/// so a list rule returns nothing moved and takes effect from the next message.
pub async fn tidy(bridge: Bridge, folder: String, screen: Screen) -> Result<usize> {
    if screen.header != "from" {
        return Ok(0);
    }
    let page: Page = failed(
        bridge
            .call("messages.query", json!({ "folderId": folder, "author": screen.value, "autoPaginationTimeout": 0 }))
            .await,
    )?;
    let ids: Vec<u64> = page.messages.iter().map(|header| header.id).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let moved = ids.len();
    relocate(bridge, ids, screen.folder).await?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn header(subject: &str) -> Header {
        Header {
            id: 1,
            message_id: "x@example".into(),
            subject: subject.into(),
            author: "Ada <ada@example.com>".into(),
            recipients: Vec::new(),
            date: None,
            read: false,
            flagged: false,
            junk: false,
            new: false,
            tags: Vec::new(),
            size: 0,
            folder: None,
        }
    }

    #[test]
    fn a_subject_is_stripped_back_to_what_the_thread_is_about() {
        assert_eq!(header("Re: Re: Fwd: lunch").stem(), "lunch");
        assert_eq!(header("lunch").stem(), "lunch");
        // Other languages' mail clients write their own marker.
        assert_eq!(header("AW: Antw: lunch").stem(), "lunch");
        // A subject that is only markers is still a subject, and still empty.
        assert_eq!(header("Re:").stem(), "");
        // `Re` without the colon is a word.
        assert_eq!(header("Rest of it").stem(), "Rest of it");
    }

    #[test]
    fn the_index_column_says_the_least_that_identifies_the_moment() {
        let now = chrono::Local.with_ymd_and_hms(2026, 9, 14, 18, 0, 0).single().expect("a real moment");
        let at = |y, m, d, h, min| chrono::Local.with_ymd_and_hms(y, m, d, h, min, 0).single().expect("a real moment");
        assert_eq!(stamp(at(2026, 9, 14, 9, 5), now), "09:05", "today is a time");
        assert_eq!(stamp(at(2026, 9, 12, 9, 5), now), "Sat 09:05", "this week is a day");
        assert_eq!(stamp(at(2026, 3, 2, 9, 5), now), "2 Mar", "this year is a date");
        assert_eq!(stamp(at(2024, 3, 2, 9, 5), now), "2 Mar 2024", "further back needs the year");
    }

    #[test]
    fn every_shape_of_date_the_bridge_might_hand_over_parses() {
        assert!(moment("2026-09-14T12:34:56.000Z").is_some(), "a JS Date crosses as ISO 8601");
        assert!(moment("Mon, 14 Sep 2026 12:34:56 +0000").is_some(), "a header passed through unchanged");
        assert!(moment("1789394096000").is_some(), "what a fixture is easiest to write");
        assert!(moment("nonsense").is_none());
    }

    /// The rail's order is the order somebody actually works in.
    #[test]
    fn folders_sort_inbox_first_and_trash_last() {
        let folder = |special: &str| Folder {
            id: special.into(),
            name: special.into(),
            path: String::new(),
            account: String::new(),
            special_use: if special.is_empty() { Vec::new() } else { vec![special.into()] },
            kind: None,
            children: Vec::new(),
            favourite: false,
        };
        let mut all = [folder("trash"), folder(""), folder("sent"), folder("inbox"), folder("junk")];
        all.sort_by_key(Folder::rank);
        let order: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(order, ["inbox", "sent", "", "junk", "trash"]);
    }

    /// The deprecated spelling still has to work: `specialUse` is newer than the profiles we run
    /// against, and `type` is what a stand-in is likeliest to send.
    #[test]
    fn a_folder_says_what_it_is_for_in_either_spelling() {
        let old: Folder = serde_json::from_value(json!({"id": "1", "type": "inbox"})).expect("a folder");
        assert_eq!(old.purpose(), "inbox");
        let new: Folder = serde_json::from_value(json!({"id": "1", "specialUse": ["sent"]})).expect("a folder");
        assert_eq!(new.purpose(), "sent");
        let plain: Folder = serde_json::from_value(json!({"id": "1", "name": "Projects"})).expect("a folder");
        assert_eq!(plain.purpose(), "");
    }
}
