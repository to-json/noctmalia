//! noctmalia: a Noctalia-native front end for Thunderbird running headless as a daemon.
//!
//! - [`vcard`]: parsing and writing the contact format Thunderbird stores.
//! - [`contacts`]: address books and contacts over the bridge.
//! - [`font`]: the desktop's UI font.
//! - [`palette`]: following the colours the user's Noctalia shell is running.
//! - [`app`]: the rolodex window.

pub mod app;
pub mod contacts;
pub mod font;
pub mod palette;
pub mod vcard;
