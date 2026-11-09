//! The parts of BAT PLAYBACK's card reader that need no firmware: which names are listed, the
//! paths the file functions take, what a file's header says, and where in a file to start
//! reading. See src/patch/reader.rs.

use bat_app::player::{Audio, FileInfo, NotPlayable};
use bat_dsp::wav::{Format, WavError};

/// The folder the X8 records into, as an absolute path for the core file functions.
pub const SOUND: &[u8] = b"C:\\SOUND";

/// UTF-16 units in a path, with its ending 0.
pub const PATH_UNITS: usize = 256;

/// The card's sector size: reads that start on a sector go straight to the buffer by DMA.
pub const SECTOR: u32 = 512;

/// A path in UTF-16, ending in a 0 unit, as the core file functions take it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Path {
    units: [u16; PATH_UNITS],
    len: usize,
}

impl Path {
    /// `C:\SOUND`.
    pub fn sound() -> Self {
        let mut path = Self { units: [0; PATH_UNITS], len: 0 };
        path.append(SOUND);
        path
    }

    /// Add `\` and `part`. False, with the path unchanged, if `part` is not printable ASCII or the
    /// path would be too long.
    pub fn push(&mut self, part: &[u8]) -> bool {
        if self.len + 1 + part.len() >= PATH_UNITS || part.iter().any(|b| !(0x20..0x7f).contains(b)) {
            return false;
        }
        self.append(b"\\");
        self.append(part);
        true
    }

    fn append(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.units[self.len] = b as u16;
            self.len += 1;
        }
        self.units[self.len] = 0;
    }

    /// The units with the ending 0, for the file functions.
    pub fn as_units(&self) -> &[u16] {
        &self.units[..=self.len]
    }
}

/// Whether the list shows a file with this name: a name ending in `.wav`, in any case. Names
/// starting with `.` are left out: macOS leaves `._` files beside each file it copies, with
/// the same name after the dot, and they hold no audio.
pub fn is_wav_name(name: &[u8]) -> bool {
    name.len() > 4 && name[0] != b'.' && name[name.len() - 4..].eq_ignore_ascii_case(b".wav")
}

/// Bytes in one frame at most: 32 channels of 32 bits. The player decodes a block of frames at a
/// time from a buffer of a few KB, so files with larger frames are not played.
pub const MOST_BLOCK_ALIGN: u16 = 128;

/// [`bat_dsp::wav::parse`]'s result, with the formats the player cannot play as `Unsupported`:
/// no sample rate, or frames larger than [`MOST_BLOCK_ALIGN`].
pub fn checked(parsed: Result<Format, WavError>) -> Result<Format, WavError> {
    match parsed {
        Ok(format) if format.sample_rate == 0 || format.block_align > MOST_BLOCK_ALIGN => Err(WavError::Unsupported),
        other => other,
    }
}

/// What the list shows for a header that [`bat_dsp::wav::parse`] read.
pub fn header_info(parsed: Result<Format, WavError>) -> FileInfo {
    match parsed {
        Ok(format) => FileInfo::Playable(Audio {
            sample_rate: format.sample_rate,
            channels: format.channels,
            frames: format.frames(),
        }),
        Err(WavError::NotWav) => FileInfo::NotPlayable(NotPlayable::NotWav),
        Err(WavError::Unsupported) => FileInfo::NotPlayable(NotPlayable::Unsupported),
        Err(WavError::NoAudio) => FileInfo::NotPlayable(NotPlayable::NoAudio),
        Err(WavError::HeaderTooLong) => FileInfo::NotPlayable(NotPlayable::Unreadable),
    }
}

/// Where to start reading to play `format` from `frame`: the file offset of the sector the frame
/// is in, and the bytes from there to the frame. A frame past the end plays the last frame.
pub fn read_start(format: &Format, frame: u32) -> (u32, u32) {
    let frame = frame.min(format.frames().saturating_sub(1));
    let offset = format.data_start + frame * u32::from(format.block_align);
    let base = offset - offset % SECTOR;
    (base, offset - base)
}

/// The file's frame being heard, for the seek bar: the resampler's output frame `j` (counted
/// from where playing started, at `start`) is the file at `start + j × in_rate / out_rate`, and
/// the last `queued` frames written are still waiting to be heard.
pub fn heard_frame(start: u32, out_done: u64, queued: u32, in_rate: u32, out_rate: u32) -> u32 {
    let heard = out_done.saturating_sub(u64::from(queued));
    let frames = heard * u64::from(in_rate) / u64::from(out_rate.max(1));
    (u64::from(start) + frames).min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use bat_dsp::wav::Encoding;

    fn units(text: &str) -> Vec<u16> {
        text.bytes().map(u16::from).chain([0]).collect()
    }

    #[test]
    fn builds_paths_with_backslashes() {
        let mut path = Path::sound();
        assert_eq!(path.as_units(), units("C:\\SOUND"));
        assert!(path.push(b"FOLDER_0001") && path.push(b"260101_0001.wav"));
        assert_eq!(path.as_units(), units("C:\\SOUND\\FOLDER_0001\\260101_0001.wav"));
    }

    #[test]
    fn refuses_long_and_non_ascii_parts() {
        let mut path = Path::sound();
        let before = path;
        assert!(!path.push(&[b'a'; PATH_UNITS]));
        assert!(!path.push("caf\u{e9}.wav".as_bytes()));
        assert!(path == before);
        // The longest that fits leaves room for the ending 0.
        let room = PATH_UNITS - SOUND.len() - 2;
        assert!(path.push(&vec![b'a'; room]));
        assert_eq!(path.as_units().len(), PATH_UNITS);
    }

    #[test]
    fn lists_wav_names_in_any_case() {
        assert!(is_wav_name(b"260101_0001.wav"));
        assert!(is_wav_name(b"XC123 - Pipistrellus.WAV"));
        assert!(!is_wav_name(b".wav"));
        assert!(!is_wav_name(b"._260101_0001.wav"));
        assert!(!is_wav_name(b"notes.txt"));
        assert!(!is_wav_name(b"take.wav.bak"));
    }

    fn format() -> Format {
        Format {
            sample_rate: 48_000,
            channels: 2,
            encoding: Encoding::Pcm24,
            block_align: 6,
            data_start: 5_548,
            data_bytes: 6 * 1_000,
        }
    }

    #[test]
    fn header_info_names_the_reason() {
        assert_eq!(
            header_info(Ok(format())),
            FileInfo::Playable(Audio { sample_rate: 48_000, channels: 2, frames: 1_000 })
        );
        assert_eq!(header_info(Err(WavError::NotWav)), FileInfo::NotPlayable(NotPlayable::NotWav));
        assert_eq!(header_info(Err(WavError::HeaderTooLong)), FileInfo::NotPlayable(NotPlayable::Unreadable));
    }

    #[test]
    fn refuses_formats_the_player_cannot_play() {
        assert_eq!(checked(Ok(format())), Ok(format()));
        assert_eq!(checked(Ok(Format { sample_rate: 0, ..format() })), Err(WavError::Unsupported));
        assert_eq!(checked(Ok(Format { block_align: 132, ..format() })), Err(WavError::Unsupported));
        assert_eq!(checked(Err(WavError::NoAudio)), Err(WavError::NoAudio));
    }

    #[test]
    fn reading_starts_on_the_sector_of_the_frame() {
        // Frame 0 is at 5,548, in the sector at 5,120.
        assert_eq!(read_start(&format(), 0), (5_120, 428));
        // Frame 100 is at 6,148.
        assert_eq!(read_start(&format(), 100), (6_144, 4));
        // Past the end: the last frame, at 5,548 + 6 x 999 = 11,542.
        assert_eq!(read_start(&format(), 5_000), (11_264, 278));
    }

    #[test]
    fn heard_frame_follows_the_output() {
        // 48 kHz into 192 kHz: 4 output frames a file frame.
        assert_eq!(heard_frame(1_000, 400, 0, 48_000, 192_000), 1_100);
        assert_eq!(heard_frame(1_000, 400, 200, 48_000, 192_000), 1_050);
        assert_eq!(heard_frame(1_000, 100, 200, 48_000, 192_000), 1_000);
    }
}
