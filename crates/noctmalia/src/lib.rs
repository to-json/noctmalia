//! noctmalia: a Noctalia-native front end for Thunderbird running headless as a daemon.
//!
//! - [`vcard`]: parsing and writing the contact format Thunderbird stores.
//! - [`contacts`]: address books and contacts over the bridge.
//! - [`mail`]: folders, messages, flags, compose and Thunderbird's own index, over the bridge.
//! - [`mime`]: what a message says, and what its envelope gives away.
//! - [`ui`]: the pieces every surface is drawn out of.
//! - [`shell`]: what the surfaces share — the bridge, the width, the one banner.
//! - [`surfaces`]: mail and contacts.
//! - [`font`]: the desktop's UI font.
//! - [`palette`]: following the colours the user's Noctalia shell is running.
//! - [`app`]: the window, and whichever surface is in front of it.

pub mod app;
pub mod base64;
pub mod contacts;
pub mod font;
pub mod mail;
pub mod mime;
pub mod palette;
pub mod shell;
pub mod surfaces;
pub mod ui;
pub mod vcard;
