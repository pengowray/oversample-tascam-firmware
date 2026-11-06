//! Decoder for the AudioMoth chime, a data-over-sound format that sets a device
//! clock from a phone.
//!
//! A web page or app plays the chime through the phone's speaker and the
//! recorder decodes it from its microphone input. This crate holds the format
//! definitions and the decoder. It is `no_std` and never allocates, so a
//! firmware patch for the TASCAM Portacapture X8 can use it, and the
//! `decode_wav` example runs the same decoder on WAV files on a PC.
//!
//! # The format
//!
//! The carrier is 18 kHz. A chime is a run of symbols, and each symbol is a
//! burst of carrier with a sin^2 rise of 0.5 ms, a sustain and a sin^2 fall of
//! 0.5 ms. The envelope reaches zero between symbols and the carrier polarity
//! inverts at every symbol boundary. The length of a symbol, ramps included,
//! gives its meaning:
//!
//! | Symbol           | Length |
//! |------------------|--------|
//! | Data 0           | 5 ms   |
//! | Data 1           | 10 ms  |
//! | Start or stop    | 7.5 ms |
//!
//! A chime has 16 start symbols, then the data symbols, then 8 stop symbols.
//! The data is the payload bytes followed by a CRC-16/XMODEM, low byte first.
//! Each byte becomes 14 symbols: the 7-bit Hamming codewords of its low nibble
//! and its high nibble, interleaved bit by bit, low nibble first and each
//! codeword least significant bit first.
//!
//! The payload is a Unix time (4 bytes, little-endian) and a time zone offset
//! in minutes (2 bytes, little-endian, two's complement), optionally followed
//! by 7 bytes of location and then 8 bytes of deployment ID. The payload length
//! is not sent. A message ends when stop symbols appear at a byte boundary.
//!
//! A melody with notes from 523 to 784 Hz is mixed in at amplitude 1/2, and the
//! carrier has amplitude 1/4. The data is only in the carrier.
//!
//! The Unix time in the payload is the time at which the chime was scheduled to
//! start. The true time at any later moment is that time plus the time since
//! the first start symbol began, which is why [`Chime::start_sample`] is
//! reported.
//!
//! # Where the format comes from
//!
//! The format is the AudioMoth chime by Open Acoustic Devices
//! (<https://www.openacousticdevices.info>), published under the MIT licence in
//! the AudioMoth-Chime and AudioMoth-Firmware-Basic repositories, Copyright (c)
//! 2020 OpenAcousticDevices. Their JavaScript encoder is copied unchanged into
//! `web/recorder-setup/` with its licence in `web/recorder-setup/LICENSE-audiomoth.txt`.
//! The format constants and the Hamming codewords in [`format`] are taken from
//! that encoder.

#![no_std]

#[cfg(test)]
extern crate std;

mod decoder;
pub mod format;

pub use decoder::Decoder;
pub use format::{LocalTime, MAX_PAYLOAD_BYTES, TimeMessage};

/// A chime that was decoded and passed its CRC check.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chime {
    /// The payload bytes, without the CRC. Only the first `payload_len` bytes
    /// are valid.
    pub payload: [u8; MAX_PAYLOAD_BYTES],
    pub payload_len: usize,
    /// Input sample index where the first start symbol began. The time in the
    /// payload is the time of this sample.
    pub start_sample: u64,
    /// Input sample index when the decoder finished.
    pub end_sample: u64,
    /// Number of bits the Hamming code corrected in the whole message.
    pub corrected_bits: u32,
    /// RMS level of the 18 kHz carrier during the chime.
    pub level_dbfs: f32,
    pub snr_db: f32,
}

/// Why a chime could not be read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnreadableReason {
    /// The stop symbols arrived but the CRC check failed.
    BadCrc,
    /// The carrier stopped before the stop symbols.
    SignalLost,
    /// More bytes arrived than the longest message has.
    TooManyBytes,
    /// A symbol length matched none of the format's symbol lengths.
    BadSymbol,
}

/// A chime that was detected but could not be read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unreadable {
    /// Input sample index where the first start symbol began.
    pub start_sample: u64,
    /// Input sample index when the decoder gave up.
    pub end_sample: u64,
    pub reason: UnreadableReason,
    /// Number of complete bytes received, CRC bytes included.
    pub bytes_received: usize,
    /// RMS level of the 18 kHz carrier during the part that was received.
    pub level_dbfs: f32,
}

/// What [`Decoder::push`] reports when a chime ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Chime(Chime),
    Unreadable(Unreadable),
}
