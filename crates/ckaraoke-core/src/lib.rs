//! Core of CKaraoke (Rust port): `.ckr` packs, MIDI and NCN lyrics, and the song library.
//!
//! Nothing here plays audio or draws UI, so it builds and tests on any platform. The byte formats
//! (pack layout, the context-mixing codec, lyric timing) match the C++ build in `../../src`, so packs
//! written by either side open in the other.

pub mod cm;
pub mod crypto;
pub mod library;
pub mod lyrics;
pub mod midi;
pub mod pack;
pub mod song;
pub mod text;
pub mod tml;
pub mod player;

use std::fmt;

/// An error with a message meant for people. The texts are the same Thai strings the C++ app shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(e.to_string())
    }
}
