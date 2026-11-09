//! Reading WAV files for playback: the header's format and where the audio is,
//! then whole frames of audio decoded to `f32`.
//!
//! [`parse`] reads the first bytes of a file (the player reads 8 KB). It walks
//! the RIFF chunks to `data`, skipping any others, such as the `bext` and
//! `JUNK` chunks that fill the X8's 5,548-byte header. [`decode`] then turns
//! bytes read from the audio into left and right samples, one chunk of the
//! file at a time.
//!
//! Supported: 16, 24 and 32-bit integer PCM and 32-bit float, with format 1
//! (PCM), 3 (IEEE float) or WAVE_FORMAT_EXTENSIBLE with the PCM or float
//! subformat. Not supported: 8-bit and 64-bit samples, other codecs, RF64 and
//! big-endian (RIFX) files.

/// Where a WAV file's audio is and how its samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: Encoding,
    /// Bytes in one frame (one sample of every channel).
    pub block_align: u16,
    /// File offset of the first byte of audio.
    pub data_start: u32,
    /// Bytes of audio from `data_start`: whole frames that are in the file.
    pub data_bytes: u32,
}

/// How each sample is stored. Integers are little-endian and signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
}

impl Encoding {
    /// Bytes in one sample of one channel.
    pub const fn bytes(self) -> u16 {
        match self {
            Encoding::Pcm16 => 2,
            Encoding::Pcm24 => 3,
            Encoding::Pcm32 | Encoding::Float32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// Not a RIFF WAVE file.
    NotWav,
    /// A WAV file in a format this module does not read: RF64, big-endian,
    /// 8 or 64-bit samples, a codec other than PCM or float, or a `fmt `
    /// chunk with impossible values or one that runs past the end of the file.
    Unsupported,
    /// No audio: the file ends before a `data` chunk, or its `data` chunk
    /// holds less than one frame.
    NoAudio,
    /// The `data` chunk starts beyond the bytes given to [`parse`]. Reading
    /// more of the file may help.
    HeaderTooLong,
}

impl Format {
    /// Frames of audio in the file.
    pub fn frames(&self) -> u32 {
        self.data_bytes / u32::from(self.block_align)
    }

    /// Length of the audio in microseconds, to the nearest.
    pub fn length_us(&self) -> u64 {
        let rate = u64::from(self.sample_rate);
        (u64::from(self.frames()) * 1_000_000 + rate / 2) / rate
    }
}

/// The last 14 bytes of the WAVE_FORMAT_EXTENSIBLE subformat GUIDs for PCM and
/// float. The first 2 bytes are the format code (1 or 3), and 2 zero bytes
/// follow it.
const GUID_TAIL: [u8; 12] = [0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Read a WAV file's format and the place of its audio from `header`, the
/// first bytes of the file, whose whole length is `file_len` bytes.
///
/// Chunks before `data` other than `fmt ` are skipped. The `data` chunk's
/// declared size is used unless it is 0 or 0xFFFFFFFF (left by a recorder that
/// stopped before writing it) or runs past the end of the file; then the audio
/// is taken to run to the end of the file. Either way it is rounded down to
/// whole frames.
pub fn parse(header: &[u8], file_len: u32) -> Result<Format, WavError> {
    let file_len = file_len as usize;
    match header.get(0..4) {
        Some(b"RIFF") => {}
        Some(b"RF64" | b"RIFX") => return Err(WavError::Unsupported),
        _ => return Err(WavError::NotWav),
    }
    if header.get(8..12) != Some(b"WAVE") {
        return Err(WavError::NotWav);
    }

    let mut fmt = None;
    let mut at = 12usize;
    loop {
        // A chunk header that is past the end of the file means there is no
        // `data` chunk; one past the end of `header` needs more of the file.
        if at.saturating_add(8) > file_len {
            return Err(WavError::NoAudio);
        }
        let (Some(id), Some(size)) = (header.get(at..at + 4), u32_at(header, at + 4)) else {
            return Err(WavError::HeaderTooLong);
        };
        let body = at + 8;
        if id == b"data" {
            let (sample_rate, channels, encoding, block_align) = fmt.ok_or(WavError::Unsupported)?;
            let in_file = file_len - body;
            let declared = size as usize;
            let bytes = if size == 0 || size == u32::MAX || declared > in_file { in_file } else { declared };
            let bytes = bytes - bytes % usize::from(block_align);
            if bytes == 0 {
                return Err(WavError::NoAudio);
            }
            return Ok(Format {
                sample_rate,
                channels,
                encoding,
                block_align,
                data_start: body as u32,
                data_bytes: bytes as u32,
            });
        }
        if id == b"fmt " {
            let end = body.saturating_add(size as usize);
            if end > file_len {
                return Err(WavError::Unsupported);
            }
            let fmt_body = header.get(body..end).ok_or(WavError::HeaderTooLong)?;
            fmt = Some(read_fmt(fmt_body)?);
        }
        // Chunks start on even offsets: a chunk of odd size has a pad byte.
        at = body.saturating_add(size as usize).saturating_add(size as usize & 1);
    }
}

/// Sample rate, channels, encoding and block align from the body of a `fmt `
/// chunk.
fn read_fmt(body: &[u8]) -> Result<(u32, u16, Encoding, u16), WavError> {
    let field = |at| u16_at(body, at).ok_or(WavError::Unsupported);
    let mut tag = field(0)?;
    let channels = field(2)?;
    let sample_rate = u32_at(body, 4).ok_or(WavError::Unsupported)?;
    let block_align = field(12)?;
    let bits = field(14)?;
    if tag == FORMAT_EXTENSIBLE {
        // cbSize, valid bits and channel mask come before the GUID.
        let guid = body.get(24..40).ok_or(WavError::Unsupported)?;
        if guid[2..4] != [0, 0] || guid[4..] != GUID_TAIL {
            return Err(WavError::Unsupported);
        }
        tag = u16::from_le_bytes([guid[0], guid[1]]);
    }
    // Samples with fewer valid bits than their container (such as 20 in 24)
    // are left-justified, so they decode correctly by their container size.
    let encoding = match (tag, bits) {
        (FORMAT_PCM, 16) => Encoding::Pcm16,
        (FORMAT_PCM, 24) => Encoding::Pcm24,
        (FORMAT_PCM, 32) => Encoding::Pcm32,
        (FORMAT_FLOAT, 32) => Encoding::Float32,
        _ => return Err(WavError::Unsupported),
    };
    if sample_rate == 0 || channels == 0 || u32::from(block_align) < u32::from(channels) * u32::from(encoding.bytes()) {
        return Err(WavError::Unsupported);
    }
    Ok((sample_rate, channels, encoding, block_align))
}

/// Decode whole frames from `bytes`, which starts at the start of a frame,
/// into `left` and `right`: at most as many frames as fit in both. Returns the
/// frames decoded. A part frame at the end of `bytes` is left alone, and a
/// `format` whose frames are too short for their samples decodes nothing.
///
/// Channel 0 goes to `left` and channel 1 to `right`. A mono file goes to
/// both, and channels after the second are left out. Integers are scaled to
/// -1.0 to 1.0 (divided by 2^(bits - 1)); floats are copied as they are.
pub fn decode(format: &Format, bytes: &[u8], left: &mut [f32], right: &mut [f32]) -> usize {
    // Where channel 1 starts in a frame; for mono, channel 0 again.
    let width = usize::from(format.encoding.bytes());
    let r = if format.channels >= 2 { width } else { 0 };
    // Frames too short for their samples, which `parse` never returns.
    if usize::from(format.block_align) < r + width {
        return 0;
    }
    let frames = bytes.chunks_exact(usize::from(format.block_align));
    let out = left.iter_mut().zip(right.iter_mut());
    let count = frames.len().min(out.len());
    match format.encoding {
        Encoding::Pcm16 => {
            const SCALE: f32 = 1.0 / 32_768.0;
            for (f, (l, rt)) in frames.zip(out) {
                *l = f32::from(i16::from_le_bytes([f[0], f[1]])) * SCALE;
                *rt = f32::from(i16::from_le_bytes([f[r], f[r + 1]])) * SCALE;
            }
        }
        Encoding::Pcm24 => {
            // The 24 bits go in the top of an i32, which is then scaled by
            // 2^-31: the same as the 24-bit value divided by 2^23.
            const SCALE: f32 = 1.0 / 2_147_483_648.0;
            for (f, (l, rt)) in frames.zip(out) {
                *l = i32::from_le_bytes([0, f[0], f[1], f[2]]) as f32 * SCALE;
                *rt = i32::from_le_bytes([0, f[r], f[r + 1], f[r + 2]]) as f32 * SCALE;
            }
        }
        Encoding::Pcm32 => {
            const SCALE: f32 = 1.0 / 2_147_483_648.0;
            for (f, (l, rt)) in frames.zip(out) {
                *l = i32::from_le_bytes([f[0], f[1], f[2], f[3]]) as f32 * SCALE;
                *rt = i32::from_le_bytes([f[r], f[r + 1], f[r + 2], f[r + 3]]) as f32 * SCALE;
            }
        }
        Encoding::Float32 => {
            for (f, (l, rt)) in frames.zip(out) {
                *l = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                *rt = f32::from_le_bytes([f[r], f[r + 1], f[r + 2], f[r + 3]]);
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Appends a chunk with its pad byte.
    fn chunk(file: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        file.extend_from_slice(id);
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(body);
        if body.len() % 2 == 1 {
            file.push(0);
        }
    }

    fn riff(chunks: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(b"WAVE");
        chunks(&mut f);
        let size = (f.len() - 8) as u32;
        f[4..8].copy_from_slice(&size.to_le_bytes());
        f
    }

    fn fmt16(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let align = channels * bits / 8;
        let mut b = Vec::new();
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        b.extend_from_slice(&align.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b
    }

    /// A 40-byte WAVE_FORMAT_EXTENSIBLE `fmt ` body with subformat `sub`.
    fn fmt40(sub: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut b = fmt16(FORMAT_EXTENSIBLE, channels, rate, bits);
        b.extend_from_slice(&22u16.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(&(if channels == 1 { 4u32 } else { 3 }).to_le_bytes());
        b.extend_from_slice(&sub.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&GUID_TAIL);
        assert_eq!(b.len(), 40);
        b
    }

    /// The `data` chunk's header with a declared size, and no audio after it.
    fn data_header(f: &mut Vec<u8>, declared: u32) {
        f.extend_from_slice(b"data");
        f.extend_from_slice(&declared.to_le_bytes());
    }

    /// A header as the X8 writes one: `bext` (1116 bytes), `JUNK` (4372),
    /// `fmt ` and `data`, 5548 bytes before the audio. From
    /// 260315_0066_3-4.wav: 42,614,784 bytes of stereo float at 192 kHz.
    #[test]
    fn an_x8_take() {
        let h = riff(|f| {
            chunk(f, b"bext", &[0; 1116]);
            chunk(f, b"JUNK", &[0; 4372]);
            chunk(f, b"fmt ", &fmt16(FORMAT_FLOAT, 2, 192_000, 32));
            data_header(f, 42_614_784);
        });
        assert_eq!(h.len(), 5548);
        let format = parse(&h, 42_620_332).unwrap();
        assert_eq!(
            format,
            Format {
                sample_rate: 192_000,
                channels: 2,
                encoding: Encoding::Float32,
                block_align: 8,
                data_start: 5548,
                data_bytes: 42_614_784,
            }
        );
        assert_eq!(format.frames(), 5_326_848);
        assert_eq!(format.length_us(), 27_744_000);
    }

    #[test]
    fn audio_followed_by_other_chunks_keeps_its_declared_size() {
        // The patch writes GUANO metadata after the audio.
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_FLOAT, 2, 192_000, 32));
            data_header(f, 8_000);
        });
        let format = parse(&h, 44 + 8_000 + 600).unwrap();
        assert_eq!((format.data_start, format.data_bytes), (44, 8_000));
    }

    #[test]
    fn float_with_fact_and_peak_chunks() {
        // 260223_0048_3-4 -- small selection.wav, as Audacity saved it.
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_FLOAT, 2, 192_000, 32));
            chunk(f, b"fact", &[0; 4]);
            chunk(f, b"PEAK", &[0; 24]);
            data_header(f, 1_925_184);
        });
        let format = parse(&h, 1_925_272).unwrap();
        assert_eq!(format.data_start, 88);
        assert_eq!(format.frames(), 240_648);
        assert_eq!(format.encoding, Encoding::Float32);
    }

    #[test]
    fn extensible_24_bit_after_an_odd_sized_bext() {
        // SteppedSine_384kHz_v1: bext of 651 bytes and a pad byte, then an
        // extensible fmt with the PCM subformat.
        let h = riff(|f| {
            chunk(f, b"bext", &[0; 651]);
            chunk(f, b"fmt ", &fmt40(FORMAT_PCM, 1, 384_000, 24));
            chunk(f, b"fact", &[0; 4]);
            data_header(f, 20_620_800);
        });
        let format = parse(&h, 20_621_540).unwrap();
        assert_eq!(format.data_start, 740);
        assert_eq!((format.encoding, format.channels, format.block_align), (Encoding::Pcm24, 1, 3));
        assert_eq!(format.frames(), 6_873_600);
    }

    #[test]
    fn extensible_float_with_a_list_chunk() {
        // XC936939: 96 kHz stereo float.
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt40(FORMAT_FLOAT, 2, 96_000, 32));
            chunk(f, b"fact", &[0; 4]);
            chunk(f, b"LIST", &[0; 26]);
            data_header(f, 7_616_512);
        });
        let format = parse(&h, 7_616_626).unwrap();
        assert_eq!((format.data_start, format.encoding), (114, Encoding::Float32));
        assert_eq!(format.frames(), 952_064);
    }

    #[test]
    fn fmt_of_18_bytes() {
        let mut body = fmt16(FORMAT_PCM, 1, 250_000, 16);
        body.extend_from_slice(&0u16.to_le_bytes());
        let h = riff(|f| {
            chunk(f, b"fmt ", &body);
            data_header(f, 1_000);
        });
        let format = parse(&h, 46 + 1_000).unwrap();
        assert_eq!((format.data_start, format.data_bytes, format.encoding), (46, 1_000, Encoding::Pcm16));
    }

    #[test]
    fn unknown_or_oversized_data_sizes_use_what_is_in_the_file() {
        for declared in [0, u32::MAX, 1_000_000] {
            let h = riff(|f| {
                chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 2, 192_000, 24));
                data_header(f, declared);
            });
            // 1001 bytes in the file: 166 whole 6-byte frames.
            let format = parse(&h, 44 + 1_001).unwrap();
            assert_eq!(format.data_bytes, 996, "declared {declared}");
        }
    }

    #[test]
    fn a_declared_size_is_rounded_down_to_whole_frames() {
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 2, 192_000, 16));
            data_header(f, 1_003);
        });
        assert_eq!(parse(&h, 10_000).unwrap().data_bytes, 1_000);
    }

    #[test]
    fn errors() {
        let good = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 1, 384_000, 16));
            data_header(f, 0);
        });
        assert_eq!(parse(&good, 44 + 100).map(|f| f.data_bytes), Ok(100));
        // Data declared 0 and nothing after it: 20260130_171457.wav was 398
        // bytes, but an empty file is also possible.
        assert_eq!(parse(&good, 44), Err(WavError::NoAudio));
        assert_eq!(parse(&good, 45), Err(WavError::NoAudio));

        assert_eq!(parse(b"", 0), Err(WavError::NotWav));
        assert_eq!(parse(b"OggS\0\0\0\0WAVE", 1000), Err(WavError::NotWav));
        assert_eq!(parse(b"RIFF\0\0\0\0AVI ", 1000), Err(WavError::NotWav));
        let mut rf64 = good.clone();
        rf64[0..4].copy_from_slice(b"RF64");
        assert_eq!(parse(&rf64, 1000), Err(WavError::Unsupported));

        let with = |body: Vec<u8>| {
            riff(|f| {
                chunk(f, b"fmt ", &body);
                data_header(f, 100);
            })
        };
        let parse_fmt = |body: Vec<u8>| {
            let h = with(body);
            parse(&h, h.len() as u32 + 100)
        };
        assert_eq!(parse_fmt(fmt16(FORMAT_PCM, 1, 48_000, 8)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt16(FORMAT_FLOAT, 1, 48_000, 64)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt16(2, 1, 48_000, 16)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt16(FORMAT_PCM, 0, 48_000, 16)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt16(FORMAT_PCM, 1, 0, 16)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt40(2, 1, 48_000, 16)), Err(WavError::Unsupported));
        assert_eq!(parse_fmt(fmt16(FORMAT_EXTENSIBLE, 1, 48_000, 16)), Err(WavError::Unsupported));
        let mut bad_align = fmt16(FORMAT_PCM, 2, 48_000, 16);
        bad_align[12] = 2;
        assert_eq!(parse_fmt(bad_align), Err(WavError::Unsupported));

        // `data` before `fmt `.
        let h = riff(|f| data_header(f, 100));
        assert_eq!(parse(&h, 200), Err(WavError::Unsupported));
        // No `data` chunk before the end of the file.
        let h = riff(|f| chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 1, 48_000, 16)));
        assert_eq!(parse(&h, h.len() as u32), Err(WavError::NoAudio));
    }

    #[test]
    fn data_beyond_the_header_bytes_needs_more_of_the_file() {
        // XC809855 has a 21 KB `PAD` chunk, so its audio starts at 23,944.
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 2, 48_000, 24));
            chunk(f, b"bext", &[0; 858]);
            chunk(f, b"plst", &[0; 1204]);
            chunk(f, b"iXML", &[0; 756]);
            chunk(f, b"PAD ", &[0; 21_050]);
            data_header(f, 4_377_600);
        });
        let file_len = 4_401_544;
        assert_eq!(parse(&h[..8192], file_len), Err(WavError::HeaderTooLong));
        let format = parse(&h, file_len).unwrap();
        assert_eq!(format.data_start, 23_944);
        // A cut inside the `fmt ` chunk, or inside a chunk header.
        assert_eq!(parse(&h[..30], file_len), Err(WavError::HeaderTooLong));
        assert_eq!(parse(&h[..14], file_len), Err(WavError::HeaderTooLong));
    }

    #[test]
    fn huge_chunk_sizes_do_not_overflow() {
        let h = riff(|f| {
            chunk(f, b"fmt ", &fmt16(FORMAT_PCM, 1, 48_000, 16));
            f.extend_from_slice(b"JUNK");
            f.extend_from_slice(&u32::MAX.to_le_bytes());
        });
        assert_eq!(parse(&h, u32::MAX), Err(WavError::NoAudio));
        assert_eq!(parse(&h, 100), Err(WavError::NoAudio));
        // A `fmt ` chunk that runs past the end of the file, or only past
        // the end of the header.
        let mut big_fmt = riff(|f| chunk(f, b"fmt ", &[0; 16]));
        big_fmt[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&big_fmt, u32::MAX), Err(WavError::Unsupported));
        big_fmt[16..20].copy_from_slice(&10_000u32.to_le_bytes());
        assert_eq!(parse(&big_fmt, 1_000), Err(WavError::Unsupported));
        assert_eq!(parse(&big_fmt, 100_000), Err(WavError::HeaderTooLong));
    }

    fn format(encoding: Encoding, channels: u16) -> Format {
        let block_align = channels * encoding.bytes();
        Format { sample_rate: 192_000, channels, encoding, block_align, data_start: 44, data_bytes: 0 }
    }

    #[test]
    fn decodes_each_encoding_to_minus_one_to_one() {
        let pcm16: Vec<u8> = [i16::MIN, i16::MAX, 0, -16_384].iter().flat_map(|v| v.to_le_bytes()).collect();
        let pcm24: Vec<u8> =
            [-8_388_608i32, 8_388_607, 1, -1].iter().flat_map(|v| v.to_le_bytes()[..3].to_vec()).collect();
        let pcm32: Vec<u8> = [i32::MIN, i32::MAX, 0, 1 << 30].iter().flat_map(|v| v.to_le_bytes()).collect();
        let float: Vec<u8> = [-1.0f32, 0.25, 1.5, -0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let cases: [(Encoding, &[u8], [f32; 4]); 4] = [
            (Encoding::Pcm16, &pcm16, [-1.0, 32_767.0 / 32_768.0, 0.0, -0.5]),
            (Encoding::Pcm24, &pcm24, [-1.0, 8_388_607.0 / 8_388_608.0, 1.0 / 8_388_608.0, -1.0 / 8_388_608.0]),
            (Encoding::Pcm32, &pcm32, [-1.0, 1.0, 0.0, 0.5]),
            (Encoding::Float32, &float, [-1.0, 0.25, 1.5, 0.0]),
        ];
        for (encoding, bytes, expect) in cases {
            let (mut l, mut r) = ([9.0; 3], [9.0; 3]);
            assert_eq!(decode(&format(encoding, 2), bytes, &mut l, &mut r), 2);
            assert_eq!([l[0], r[0], l[1], r[1]], expect, "{encoding:?}");
            assert_eq!((l[2], r[2]), (9.0, 9.0));
        }
    }

    #[test]
    fn mono_goes_to_both_sides_and_extra_channels_are_left_out() {
        let mono: Vec<u8> = [100i16, -200, 300].iter().flat_map(|v| v.to_le_bytes()).collect();
        let (mut l, mut r) = ([0.0; 4], [0.0; 4]);
        assert_eq!(decode(&format(Encoding::Pcm16, 1), &mono, &mut l, &mut r), 3);
        assert_eq!(l, r);
        assert_eq!(l[1], -200.0 / 32_768.0);

        let four: Vec<u8> = (1..=8i16).flat_map(|v| (v * 1000).to_le_bytes()).collect();
        assert_eq!(decode(&format(Encoding::Pcm16, 4), &four, &mut l, &mut r), 2);
        assert_eq!(l[..2], [1_000.0 / 32_768.0, 5_000.0 / 32_768.0]);
        assert_eq!(r[..2], [2_000.0 / 32_768.0, 6_000.0 / 32_768.0]);
    }

    #[test]
    fn decodes_whole_frames_that_fit() {
        let bytes: Vec<u8> = (0..20u8).collect();
        let f = format(Encoding::Pcm24, 2);
        let (mut l, mut r) = ([0.0; 10], [0.0; 10]);
        // 20 bytes hold 3 whole 6-byte frames.
        assert_eq!(decode(&f, &bytes, &mut l, &mut r), 3);
        assert_eq!(decode(&f, &bytes, &mut l[..2], &mut r), 2);
        assert_eq!(decode(&f, &bytes, &mut l, &mut r[..1]), 1);
        assert_eq!(decode(&f, &bytes[..5], &mut l, &mut r), 0);
    }

    #[test]
    fn extensible_with_wider_blocks_than_samples() {
        // Some writers pad each frame: 2 channels of 16 bits in 8 bytes.
        let mut f = format(Encoding::Pcm16, 2);
        f.block_align = 8;
        let bytes: Vec<u8> = [1000i16, 2000, 0, 0, 3000, 4000, 0, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let (mut l, mut r) = ([0.0; 2], [0.0; 2]);
        assert_eq!(decode(&f, &bytes, &mut l, &mut r), 2);
        assert_eq!(l, [1000.0 / 32_768.0, 3000.0 / 32_768.0]);
        assert_eq!(r, [2000.0 / 32_768.0, 4000.0 / 32_768.0]);
    }
}

/// Checks against real files, not pass or fail tests. Run with `cargo test -p
/// bat-dsp --release wav::measurements -- --ignored --nocapture`.
#[cfg(test)]
mod measurements {
    use super::*;

    #[test]
    #[ignore]
    /// Parse the first 8 KB of every WAV file in ~/sync/default/bats and its
    /// subfolders, and decode the first second. Prints the files that do not
    /// parse or decode to silence, then a count of each format.
    fn parse_local_recordings() {
        use std::collections::BTreeMap;
        use std::io::Read;
        let Some(home) = std::env::var_os("HOME") else { return };
        let mut dirs = std::vec![std::path::Path::new(&home).join("sync/default/bats")];
        let mut formats: BTreeMap<std::string::String, usize> = BTreeMap::new();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for path in entries.flatten().map(|e| e.path()) {
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")) {
                    continue;
                }
                let mut file = std::fs::File::open(&path).unwrap();
                let len = file.metadata().unwrap().len() as u32;
                let mut head = std::vec![0u8; 8192];
                let n = file.read(&mut head).unwrap();
                let name = path.file_name().unwrap().to_string_lossy();
                let f = match parse(&head[..n], len) {
                    Ok(f) => f,
                    Err(e) => {
                        std::println!("{name}: {e:?}");
                        continue;
                    }
                };
                let take = (f.sample_rate as usize * f.block_align as usize).min(f.data_bytes as usize);
                let bytes = std::fs::read(&path).unwrap();
                let audio = &bytes[f.data_start as usize..f.data_start as usize + take];
                let frames = take / f.block_align as usize;
                let (mut l, mut r) = (std::vec![0.0; frames], std::vec![0.0; frames]);
                assert_eq!(decode(&f, audio, &mut l, &mut r), frames);
                if l.iter().chain(&r).all(|&v| v == 0.0) {
                    std::println!("{name}: silent first second");
                }
                let key = std::format!("{} Hz, {} ch, {:?}", f.sample_rate, f.channels, f.encoding);
                *formats.entry(key).or_default() += 1;
            }
        }
        for (format, count) in formats {
            std::println!("{count:4} files: {format}");
        }
    }
}
