//! The parts of BAT PLAYBACK's card reader that need no firmware: which names are listed, the
//! paths the file functions take, what a file's header says, and where in a file to start
//! reading. See src/patch/reader.rs.

use bat_app::player::{Audio, FileInfo, NotPlayable, TOP_NAME_BYTES, XRI_BYTES};
use bat_dsp::wav::{Format, WavError};

/// The card, as the core file functions name it.
const DRIVE: &[u8] = b"C:";

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
    /// `C:`, which [`push`](Self::push) adds folders to.
    pub fn drive() -> Self {
        let mut path = Self { units: [0; PATH_UNITS], len: 0 };
        path.append(DRIVE);
        path
    }

    /// `C:\`, the top of the card, for listing its folders.
    pub fn card_top() -> Self {
        let mut path = Self::drive();
        path.append(b"\\");
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

/// Words in a request to list a folder at the top of the card: the name's length, then its
/// bytes, four to a word.
pub const ROOT_WORDS: usize = 1 + TOP_NAME_BYTES / 4;
const _: () = assert!(TOP_NAME_BYTES.is_multiple_of(4));

/// A folder name as [`ROOT_WORDS`] words, cut to [`TOP_NAME_BYTES`].
pub fn pack_root(name: &[u8]) -> [u32; ROOT_WORDS] {
    let len = name.len().min(TOP_NAME_BYTES);
    let mut words = [0; ROOT_WORDS];
    words[0] = len as u32;
    for (i, &b) in name[..len].iter().enumerate() {
        words[1 + i / 4] |= u32::from(b) << (8 * (i % 4));
    }
    words
}

/// The name in [`pack_root`]'s words, and its length.
pub fn unpack_root(words: &[u32; ROOT_WORDS]) -> ([u8; TOP_NAME_BYTES], usize) {
    let len = (words[0] as usize).min(TOP_NAME_BYTES);
    let mut name = [0; TOP_NAME_BYTES];
    for (i, b) in name[..len].iter_mut().enumerate() {
        *b = (words[1 + i / 4] >> (8 * (i % 4))) as u8;
    }
    (name, len)
}

/// Whether the folder choice lists a folder at the top of the card with this name and these
/// attributes: not hidden or a system folder, not one of the folders macOS makes (starting with
/// `.`), and a name that fits.
pub fn is_listed_folder(name: &[u8], hidden_or_system: bool) -> bool {
    !hidden_or_system && !name.is_empty() && name[0] != b'.' && name.len() <= TOP_NAME_BYTES
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

/// Bytes read after a file's audio, for a GUANO chunk there. The X8 patch's own chunk is under
/// 1 KB; other recorders may put other chunks first.
pub const TAIL_BYTES: u32 = 16 * 1024;

/// Where to read a file's tail, for the information view: the file offset of the sector holding
/// the first byte after the audio, and that byte's place in what is read. None if nothing
/// follows the audio, or the header does not say where the audio ends.
pub fn tail_read(header: &[u8], file_len: u32) -> Option<(u32, usize)> {
    let end = guano::read_header(header)?.data_end()?;
    // Chunks start on even offsets.
    let next = end.checked_add(end & 1)?;
    if next.checked_add(8)? > file_len {
        return None;
    }
    let base = next - next % SECTOR;
    Some((base, (next - base) as usize))
}

/// The information view's texts, written into `out`: the XRI text from the header's `bext`
/// chunk first, at most [`XRI_BYTES`], then the text of a `guan` chunk from the header or, if
/// there is none, from `tail` (the bytes read after the audio, and where the first chunk starts
/// in them). Returns the two lengths; texts that do not fit are cut, GUANO at its last line.
pub fn details_texts(header: &[u8], tail: Option<(&[u8], usize)>, out: &mut [u8]) -> (usize, usize) {
    let xri = guano::coding_history(header).unwrap_or(&[]);
    let xri = &xri[..xri.len().min(XRI_BYTES).min(out.len())];
    out[..xri.len()].copy_from_slice(xri);
    let text = match guano::guano_in_header(header) {
        Some(body) => &header[body],
        None => tail.and_then(|(bytes, at)| Some(&bytes[guano::find_chunk(bytes, at, b"guan")?])).unwrap_or(&[]),
    };
    let room = out.len() - xri.len();
    let text = if text.len() <= room {
        text
    } else {
        let cut = &text[..room];
        &cut[..cut.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1)]
    };
    out[xri.len()..xri.len() + text.len()].copy_from_slice(text);
    (xri.len(), text.len())
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
        assert_eq!(Path::card_top().as_units(), units("C:\\"));
        let mut path = Path::drive();
        assert!(path.push(b"SOUND"));
        assert_eq!(path.as_units(), units("C:\\SOUND"));
        assert!(path.push(b"FOLDER_0001") && path.push(b"260101_0001.wav"));
        assert_eq!(path.as_units(), units("C:\\SOUND\\FOLDER_0001\\260101_0001.wav"));
    }

    #[test]
    fn refuses_long_and_non_ascii_parts() {
        let mut path = Path::drive();
        assert!(path.push(b"SOUND"));
        let before = path;
        assert!(!path.push(&[b'a'; PATH_UNITS]));
        assert!(!path.push("caf\u{e9}.wav".as_bytes()));
        assert!(path == before);
        // The longest that fits leaves room for the ending 0.
        let room = PATH_UNITS - b"C:\\SOUND".len() - 2;
        assert!(path.push(&vec![b'a'; room]));
        assert_eq!(path.as_units().len(), PATH_UNITS);
    }

    #[test]
    fn folder_names_survive_packing() {
        for name in [&b""[..], b"SOUND", b"Xeno-canto downloads 2025", &[b'x'; TOP_NAME_BYTES]] {
            let (back, len) = unpack_root(&pack_root(name));
            assert_eq!(&back[..len], name);
        }
        let (_, len) = unpack_root(&pack_root(&[b'x'; 50]));
        assert_eq!(len, TOP_NAME_BYTES);
    }

    #[test]
    fn lists_ordinary_folders() {
        assert!(is_listed_folder(b"BATS", false));
        assert!(!is_listed_folder(b"System Volume Information", true));
        assert!(!is_listed_folder(b".Spotlight-V100", false));
        assert!(!is_listed_folder(&[b'x'; TOP_NAME_BYTES + 1], false));
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

    /// A file as the X8 writes one, with `frames` of 24-bit mono (an odd count makes the audio
    /// end on an odd offset), the X8's XRI text, and the bat patch's GUANO chunk after the audio
    /// with the RIFF size counting it, as `guano_hook` leaves it.
    fn x8_file(frames: u32, with_guano: bool) -> Vec<u8> {
        const XRI: &[u8] = b"T=XRI 008a;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=5:MIC;";
        let data = frames * 3;
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF\0\0\0\0WAVEbext");
        f.extend_from_slice(&1116u32.to_le_bytes());
        let mut bext = [0u8; 1116];
        bext[602..602 + XRI.len()].copy_from_slice(XRI);
        f.extend_from_slice(&bext);
        f.extend_from_slice(b"JUNK");
        f.extend_from_slice(&4372u32.to_le_bytes());
        f.extend_from_slice(&[0; 4372]);
        f.extend_from_slice(b"fmt \x10\0\0\0\x01\0\x01\0");
        f.extend_from_slice(&192_000u32.to_le_bytes());
        f.extend_from_slice(&(192_000u32 * 3).to_le_bytes());
        f.extend_from_slice(b"\x03\0\x18\0data");
        f.extend_from_slice(&data.to_le_bytes());
        assert_eq!(f.len(), 5548);
        f.resize(5548 + data as usize, 0x55);
        if with_guano {
            let header = guano::read_header(&f[..5548]).unwrap();
            let recorder = crate::take_guano::Recorder {
                firmware: "V2.03",
                patch_build: 258,
                file_name: Some("260315_0065.wav"),
                phone_now: None,
                utc_offset_minutes: None,
                location_e6: Some((-36_123_456, 146_000_007)),
                pauses: None,
            };
            let mut chunk = [0u8; 1024];
            let n = crate::take_guano::write_chunk(&header, &recorder, &mut chunk).unwrap();
            f.extend_from_slice(&chunk[..n]);
        }
        let riff = (f.len() - 8) as u32;
        f[4..8].copy_from_slice(&riff.to_le_bytes());
        f
    }

    /// What the reader does: the first 8 KB, then the tail from its sector.
    fn read_details(file: &[u8], out: &mut [u8]) -> (usize, usize) {
        let header = &file[..file.len().min(8192)];
        let tail = tail_read(header, file.len() as u32).map(|(base, at)| {
            let end = file.len().min(base as usize + TAIL_BYTES as usize);
            (&file[base as usize..end], at)
        });
        details_texts(header, tail, out)
    }

    #[test]
    fn details_of_an_x8_take_with_guano_after_the_audio() {
        for frames in [192_000, 192_001] {
            let file = x8_file(frames, true);
            let mut out = [0u8; 4096];
            let (xri, guano) = read_details(&file, &mut out);
            assert!(out[..xri].starts_with(b"T=XRI 008a;") && out[..xri].ends_with(b"SOURCE=5:MIC;"));
            let text = core::str::from_utf8(&out[xri..xri + guano]).unwrap();
            assert!(text.starts_with("GUANO|Version: 1.0\n"), "{frames} frames: {text:?}");
            assert!(text.contains("Loc Position: -36.123456 146.000007\n"), "{text}");
        }
    }

    #[test]
    fn details_of_a_take_without_guano_and_texts_cut_to_fit() {
        let file = x8_file(192_000, false);
        assert_eq!(tail_read(&file[..8192], file.len() as u32), None);
        let mut out = [0u8; 4096];
        let (xri, guano) = read_details(&file, &mut out);
        assert_eq!((xri > 0, guano), (true, 0));
        // A small buffer: GUANO stops at the end of a line.
        let file = x8_file(192_000, true);
        let mut out = [0u8; 160];
        let (xri, guano) = read_details(&file, &mut out);
        assert!(guano > 0 && out[xri + guano - 1] == b'\n' && xri + guano <= 160);
    }

    #[test]
    fn guano_before_the_audio_is_found_in_the_header() {
        let mut file = Vec::from(*b"RIFF\0\0\0\0WAVEguan\x0c\0\0\0Make: Elekonfmt ");
        file.extend_from_slice(b"\x10\0\0\0\x01\0\x01\0\0\xee\x02\0\0\xdc\x05\0\x02\0\x10\0data\x04\0\0\0\0\0\0\0");
        let mut out = [0u8; 256];
        let (xri, guano) = read_details(&file, &mut out);
        assert_eq!((xri, &out[..guano]), (0, &b"Make: Elekon"[..]));
    }
}
