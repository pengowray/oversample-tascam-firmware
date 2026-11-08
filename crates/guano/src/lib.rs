//! GUANO metadata for WAV files (https://github.com/riggsd/guano-spec), for recorders and firmware
//! patches that write it as a file is closed.
//!
//! - [`read_header`] reads what a writer needs from the first few KB of a WAV file: where the audio
//!   is and how long it is, the sample rate, and the `bext` chunk's origination date and time.
//! - [`ChunkWriter`] builds a `guan` chunk in a byte buffer, following the spec's rules: the
//!   `GUANO|Version` field first, `\n` line breaks, and the text padded with whitespace to an even
//!   length. A pad byte goes first when the chunk starts at an odd file offset.
//!
//! `no_std`, no allocation, so the TASCAM Portacapture X8's firmware patch can use it.

#![cfg_attr(not(test), no_std)]

mod chunk;
mod date_time;
mod header;

pub use chunk::{ChunkWriter, file_name};
pub use date_time::DateTime;
pub use header::{Header, read_header};
