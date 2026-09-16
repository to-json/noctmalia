//! What a call over the bridge can fail with.
//!
//! Kept whole rather than flattened to text. An iced message has to be `Clone`, and this is —
//! which matters because [`Error::Bridge`]'s `NotConnected` is the one failure the interface
//! handles differently from every other: Thunderbird going away is already on screen as the
//! waiting page and already repaired by the resync its next hello triggers, so a toast for every
//! call that was in flight when it left is noise. [`Shell::report`](crate::shell::Shell::report)
//! is where that distinction is spent.

use std::fmt;

#[derive(Debug, Clone)]
pub enum Error {
    /// The bridge could not answer: not attached, timed out, refused, or Thunderbird said no.
    Bridge(noctmalia_bridge::Error),
    /// Something on this side of the bridge — a reply that did not decode into what was expected,
    /// a file that could not be written — where the text is all there is to do with it.
    Local(String),
}

impl Error {
    pub fn local(text: impl Into<String>) -> Error {
        Error::Local(text.into())
    }

    /// Whether this is Thunderbird not being there, as opposed to Thunderbird (or we) getting
    /// something wrong.
    pub fn is_disconnected(&self) -> bool {
        matches!(self, Error::Bridge(noctmalia_bridge::Error::NotConnected))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bridge(error) => write!(f, "{error}"),
            Error::Local(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Error {}

impl From<noctmalia_bridge::Error> for Error {
    fn from(error: noctmalia_bridge::Error) -> Error {
        Error::Bridge(error)
    }
}

impl From<String> for Error {
    fn from(text: String) -> Error {
        Error::Local(text)
    }
}

impl From<&str> for Error {
    fn from(text: &str) -> Error {
        Error::Local(text.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_not_connected_counts_as_disconnected() {
        assert!(Error::from(noctmalia_bridge::Error::NotConnected).is_disconnected());
        assert!(!Error::from(noctmalia_bridge::Error::Decode("x".into())).is_disconnected());
        assert!(!Error::local("disk full").is_disconnected());
    }

    #[test]
    fn a_bridge_error_keeps_its_own_wording() {
        let error = Error::from(noctmalia_bridge::Error::NotConnected);
        assert_eq!(error.to_string(), "Thunderbird is not connected");
        assert_eq!(Error::local("no").to_string(), "no");
    }
}
