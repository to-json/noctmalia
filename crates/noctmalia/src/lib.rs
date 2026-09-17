//! noctmalia: a Noctalia-native front end for Thunderbird running headless as a daemon.
//!
//! - [`vcard`]: parsing and writing the contact format Thunderbird stores.
//! - [`ical`]: parsing and writing the calendar event format Thunderbird stores.
//! - [`contacts`]: address books and contacts over the bridge.
//! - [`calendar`]: calendars and events over the bridge.
//! - [`mail`]: folders, messages, flags, compose and Thunderbird's own index, over the bridge.
//! - [`error`]: what a bridge call can fail with, kept whole so the shell can tell absence from failure.
//! - [`mime`]: what a message says, and what its envelope gives away.
//! - [`ui`]: the pieces every surface is drawn out of.
//! - [`thunderbird`]: the Thunderbird this program fetches, spawns, watches and stops.
//! - [`shell`]: what the surfaces share — the bridge, the width, the one banner.
//! - [`surfaces`]: mail, contacts and calendar.
//! - [`font`]: the desktop's UI font.
//! - [`palette`]: following the colours the user's Noctalia shell is running.
//! - [`app`]: the window, and whichever surface is in front of it.

pub mod app;
pub mod base64;
pub mod calendar;
pub mod commands;
pub mod config;
pub mod control;
pub mod error;
pub mod font;
pub mod ical;
pub mod mail;
pub mod mime;
pub use noctalia_iced::palette;
pub mod people;
pub mod shell;
pub mod surfaces;
pub mod thunderbird;
pub mod ui;
pub mod vcard;

pub use error::{Error, Result};
