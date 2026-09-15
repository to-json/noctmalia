//! Contacts and address books over the bridge.
//!
//! Thunderbird's own model is a flat list of address books, each holding contacts and mailing
//! lists. Contacts are vCards; see [`crate::vcard`].

use crate::vcard::Card;
use noctmalia_bridge::Bridge;
use serde::Deserialize;
use serde_json::json;

/// An address book. `readOnly` books are typically LDAP directories or collected addresses.
#[derive(Debug, Clone, Deserialize)]
pub struct AddressBook {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "readOnly", default)]
    pub read_only: bool,
    #[serde(default)]
    pub remote: bool,
}

/// A contact as the bridge hands it over. The bridge normalises MV2's `properties.vCard` up to
/// `vCard`, so this is the same shape on either manifest version.
#[derive(Debug, Clone, Deserialize)]
struct ContactNode {
    id: String,
    #[serde(rename = "parentId", default)]
    parent_id: Option<String>,
    #[serde(rename = "vCard", default)]
    vcard: Option<String>,
}

/// A contact, parsed.
#[derive(Debug, Clone)]
pub struct Contact {
    pub id: String,
    pub book: Option<String>,
    pub card: Card,
}

impl From<ContactNode> for Contact {
    fn from(node: ContactNode) -> Contact {
        let card = Card::parse(node.vcard.as_deref().unwrap_or_default());
        Contact { id: node.id, book: node.parent_id, card }
    }
}

/// Errors reach the UI as text: iced messages must be `Clone`, and there is nothing to do with a
/// bridge error but show it.
pub type Result<T> = std::result::Result<T, String>;

fn failed<T>(result: std::result::Result<T, noctmalia_bridge::Error>) -> Result<T> {
    result.map_err(|error| error.to_string())
}

pub async fn books(bridge: Bridge) -> Result<Vec<AddressBook>> {
    failed(bridge.call("addressBooks.list", json!({ "complete": false })).await)
}

/// Every contact in `book`, or across all books when it is `None`.
///
/// Thunderbird has no "all books" list call, so this fans out. Books are few and a rolodex is small
/// enough that listing them all beats maintaining a cache.
pub async fn list(bridge: Bridge, book: Option<String>, books: Vec<AddressBook>) -> Result<Vec<Contact>> {
    let wanted: Vec<String> = match book {
        Some(id) => vec![id],
        None => books.iter().map(|book| book.id.clone()).collect(),
    };
    let mut contacts = Vec::new();
    for id in wanted {
        let nodes: Vec<ContactNode> = failed(bridge.call("contacts.list", json!({ "parentId": id })).await)?;
        contacts.extend(nodes.into_iter().map(Contact::from));
    }
    Ok(sorted(contacts))
}

/// Thunderbird's own substring search over name and email.
pub async fn search(bridge: Bridge, query: String, book: Option<String>) -> Result<Vec<Contact>> {
    let params = match &book {
        Some(id) => json!({ "parentId": id, "searchString": query }),
        None => json!({ "searchString": query }),
    };
    let nodes: Vec<ContactNode> = failed(bridge.call("contacts.quickSearch", params).await)?;
    Ok(sorted(nodes.into_iter().map(Contact::from).collect()))
}

pub async fn create(bridge: Bridge, book: String, vcard: String) -> Result<String> {
    failed(bridge.call("contacts.create", json!({ "parentId": book, "vCard": vcard })).await)
}

pub async fn update(bridge: Bridge, id: String, vcard: String) -> Result<String> {
    failed(bridge.call_raw("contacts.update", json!({ "contactId": id, "vCard": vcard })).await)?;
    Ok(id)
}

pub async fn delete(bridge: Bridge, id: String) -> Result<()> {
    failed(bridge.call_raw("contacts.delete", json!({ "contactId": id })).await)?;
    Ok(())
}

fn sorted(mut contacts: Vec<Contact>) -> Vec<Contact> {
    contacts.sort_by_key(|contact| contact.card.sort_key());
    contacts
}
