//! Calendars and events over the bridge.
//!
//! Thunderbird's calendar model, reached through the upstream calendar Experiment
//! (`docs/design.md` "Calendar"): a flat list of calendars, each holding events (and tasks, which
//! this surface does not show — see the plan this was built from). Items travel as raw ICAL text,
//! `format: "ical"`, which `tools/smoke.sh`'s "calendar round trip" proves a real headless
//! Thunderbird accepts, `RRULE` included; [`crate::ical`] is what makes sense of that text.

use crate::ical::Event;
use chrono::{DateTime, Local, Utc};
use noctmalia_bridge::Bridge;
use serde::Deserialize;
use serde_json::{Value, json};

/// One of Thunderbird's calendars. `color` is read but never drawn — the design language draws
/// only the sixteen palette roles, so `crate::surfaces::calendar` assigns each visible calendar a
/// role instead of trusting Thunderbird's arbitrary hex.
#[derive(Debug, Clone, Deserialize)]
pub struct Cal {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(rename = "readOnly", default)]
    pub read_only: bool,
}

/// An item as the bridge hands it over, before its ICAL text has been made sense of.
#[derive(Debug, Clone, Deserialize)]
struct ItemNode {
    id: String,
    #[serde(rename = "calendarId")]
    calendar_id: String,
    /// Set when this came back from an `expand: true` query: which occurrence of a recurring
    /// event this is. Thunderbird has already done the recurrence maths — see `docs/design.md`.
    #[serde(default)]
    instance: Option<String>,
    item: Value,
}

/// One occurrence of an event, ready for the surface to draw.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub calendar_id: String,
    pub instance: Option<String>,
    pub event: Event,
}

impl TryFrom<ItemNode> for Item {
    type Error = String;

    fn try_from(node: ItemNode) -> Result<Item> {
        let text = node.item.as_str().ok_or_else(|| format!("item {}: not ICAL text", node.id))?;
        let event = Event::parse(text).ok_or_else(|| format!("item {}: did not parse as a VEVENT", node.id))?;
        Ok(Item { id: node.id, calendar_id: node.calendar_id, instance: node.instance, event })
    }
}

/// Errors reach the UI as text: iced messages must be `Clone`, and there is nothing to do with a
/// bridge error but show it.
pub type Result<T> = std::result::Result<T, String>;

fn failed<T>(result: std::result::Result<T, noctmalia_bridge::Error>) -> Result<T> {
    result.map_err(|error| error.to_string())
}

pub async fn calendars(bridge: Bridge) -> Result<Vec<Cal>> {
    failed(bridge.call("calendar.calendars.query", json!({})).await)
}

pub async fn set_visible(bridge: Bridge, id: String, visible: bool) -> Result<()> {
    let params = json!({ "calendarId": id, "updateProperties": { "hidden": !visible } });
    failed(bridge.call_raw("calendar.calendars.update", params).await)?;
    Ok(())
}

/// Every event across `calendar_ids` that falls in `[start, end)`, expanded so a recurring event
/// arrives as one row per occurrence with the recurrence maths already done.
pub async fn items(
    bridge: Bridge,
    calendar_ids: Vec<String>,
    start: DateTime<Local>,
    end: DateTime<Local>,
) -> Result<Vec<Item>> {
    if calendar_ids.is_empty() {
        return Ok(Vec::new());
    }
    let params = json!({
        "calendarId": calendar_ids,
        "type": "event",
        "rangeStart": stamp(start),
        "rangeEnd": stamp(end),
        "expand": true,
        "returnFormat": "ical",
    });
    let nodes: Vec<ItemNode> = failed(bridge.call("calendar.items.query", params).await)?;
    // One item Thunderbird could not be made ICAL sense of should not blank the whole range.
    let mut items: Vec<Item> = nodes.into_iter().filter_map(|node| Item::try_from(node).ok()).collect();
    items.sort_by_key(|item| item.event.start.instant());
    Ok(items)
}

pub async fn create(bridge: Bridge, calendar_id: String, event: Event) -> Result<Item> {
    let params = json!({
        "calendarId": calendar_id,
        "type": "event",
        "format": "ical",
        "item": event.to_ical(),
        "returnFormat": "ical",
    });
    let node: ItemNode = failed(bridge.call("calendar.items.create", params).await)?;
    Item::try_from(node)
}

pub async fn update(bridge: Bridge, calendar_id: String, id: String, event: Event) -> Result<Item> {
    let params = json!({ "calendarId": calendar_id, "id": id, "format": "ical", "item": event.to_ical(), "returnFormat": "ical" });
    let node: ItemNode = failed(bridge.call("calendar.items.update", params).await)?;
    Item::try_from(node)
}

pub async fn remove(bridge: Bridge, calendar_id: String, id: String) -> Result<()> {
    let params = json!({ "calendarId": calendar_id, "id": id });
    failed(bridge.call_raw("calendar.items.remove", params).await)?;
    Ok(())
}

fn stamp(when: DateTime<Local>) -> String {
    when.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string()
}
