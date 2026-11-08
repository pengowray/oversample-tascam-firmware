use crate::DateTime;

/// What a GUANO writer needs from a WAV file's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The size field of the `RIFF` chunk: the file's length less 8, if nothing follows the chunk.
    pub riff_size: u32,
    pub sample_rate: u32,
    /// Bytes in one frame of all channels.
    pub block_align: u16,
    /// File offset of the first byte of audio.
    pub data_start: u32,
    pub data_size: u32,
    /// The `bext` chunk's OriginationDate and OriginationTime, if the file has a `bext` chunk with
    /// a real date and time in them. Recorders differ in which moment they store there.
    pub origination: Option<DateTime>,
}

/// Offsets in a `bext` chunk's body of OriginationDate ("yyyy-mm-dd") and OriginationTime
/// ("hh:mm:ss").
const BEXT_DATE: usize = 320;
const BEXT_TIME: usize = 330;

impl Header {
    /// File offset of the byte after the audio. A chunk added after the audio starts here, after a
    /// pad byte if this is odd.
    pub fn data_end(&self) -> Option<u32> {
        self.data_start.checked_add(self.data_size)
    }

    /// The audio's length in microseconds, to the nearest.
    pub fn length_us(&self) -> Option<u64> {
        if self.sample_rate == 0 || self.block_align == 0 {
            return None;
        }
        let frames = u64::from(self.data_size / u32::from(self.block_align));
        let rate = u64::from(self.sample_rate);
        Some((frames * 1_000_000 + rate / 2) / rate)
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a WAV header from the first bytes of a file, which must reach the `data` chunk's size
/// field. None for anything that isn't a RIFF WAVE file with `fmt ` before `data`.
pub fn read_header(bytes: &[u8]) -> Option<Header> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let riff_size = u32_at(bytes, 4)?;
    let mut format = None;
    let mut origination = None;
    let mut at = 12usize;
    loop {
        let id = bytes.get(at..at + 4)?;
        let size = u32_at(bytes, at + 4)?;
        let body = at + 8;
        match id {
            b"data" => {
                let (sample_rate, block_align) = format?;
                return Some(Header {
                    riff_size,
                    sample_rate,
                    block_align,
                    data_start: u32::try_from(body).ok()?,
                    data_size: size,
                    origination,
                });
            }
            b"fmt " => format = Some((u32_at(bytes, body + 4)?, u16_at(bytes, body + 12)?)),
            b"bext" => {
                let date = bytes.get(body + BEXT_DATE..body + BEXT_DATE + 10);
                let time = bytes.get(body + BEXT_TIME..body + BEXT_TIME + 8);
                origination = date.zip(time).and_then(|(date, time)| DateTime::parse(date, time));
            }
            _ => {}
        }
        // Chunks start on even offsets.
        at = body.checked_add(usize::try_from(size).ok()?)?.checked_add(size as usize & 1)?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::vec::Vec;

    /// A header laid out as the TASCAM Portacapture X8 writes one: `bext` (1116 bytes), `JUNK`
    /// (4372), `fmt ` and `data`, 5548 bytes before the audio.
    pub(crate) fn x8_header(data_size: u32, channels: u16, bits: u16, date: &[u8; 10], time: &[u8; 8]) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(5548 + data_size - 8).to_le_bytes());
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"bext");
        h.extend_from_slice(&1116u32.to_le_bytes());
        let mut bext = [0u8; 1116];
        bext[256..256 + 22].copy_from_slice(b"TASCAM Portacapture X8");
        bext[BEXT_DATE..BEXT_DATE + 10].copy_from_slice(date);
        bext[BEXT_TIME..BEXT_TIME + 8].copy_from_slice(time);
        h.extend_from_slice(&bext);
        h.extend_from_slice(b"JUNK");
        h.extend_from_slice(&4372u32.to_le_bytes());
        h.extend_from_slice(&[0; 4372]);
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        let format: u16 = if bits == 32 { 3 } else { 1 };
        let align = channels * bits / 8;
        h.extend_from_slice(&format.to_le_bytes());
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&192_000u32.to_le_bytes());
        h.extend_from_slice(&(192_000 * u32::from(align)).to_le_bytes());
        h.extend_from_slice(&align.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&data_size.to_le_bytes());
        assert_eq!(h.len(), 5548);
        h
    }

    /// 261106_0090_3-4.wav: 89,210,880 bytes of stereo float at 192 kHz.
    #[test]
    fn an_x8_take() {
        let bytes = x8_header(89_210_880, 2, 32, b"2026-11-06", b"19:08:08");
        let header = read_header(&bytes).unwrap();
        assert_eq!(header.data_start, 5548);
        assert_eq!(header.data_end(), Some(89_216_428));
        assert_eq!(header.riff_size + 8, 89_216_428);
        assert_eq!(header.sample_rate, 192_000);
        assert_eq!(header.block_align, 8);
        assert_eq!(header.length_us(), Some(58_080_000));
        assert_eq!(header.origination, DateTime::parse(b"2026-11-06", b"19:08:08"));
    }

    #[test]
    fn marks_in_the_header_are_skipped() {
        // Marks replace the start of JUNK with `cue ` and `LIST`, keeping the same total size.
        let mut bytes = x8_header(192_000 * 8, 2, 32, b"2026-11-06", b"19:08:08");
        let junk = 12 + 8 + 1116;
        bytes[junk..junk + 4].copy_from_slice(b"cue ");
        bytes[junk + 4..junk + 8].copy_from_slice(&100u32.to_le_bytes());
        bytes[junk + 108..junk + 112].copy_from_slice(b"JUNK");
        bytes[junk + 112..junk + 116].copy_from_slice(&(4372u32 - 108).to_le_bytes());
        let header = read_header(&bytes).unwrap();
        assert_eq!(header.data_start, 5548);
        assert_eq!(header.length_us(), Some(1_000_000));
    }

    #[test]
    fn an_empty_clock_gives_no_origination() {
        let bytes = x8_header(192_000 * 8, 2, 32, &[0; 10], &[0; 8]);
        assert_eq!(read_header(&bytes).unwrap().origination, None);
    }

    #[test]
    fn a_header_that_stops_short_is_refused() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-11-06", b"19:08:08");
        assert!(read_header(&bytes[..5000]).is_none());
        assert!(read_header(&bytes[..5547]).is_none());
        let mut not_wave = bytes.clone();
        not_wave[8..12].copy_from_slice(b"AVI ");
        assert!(read_header(&not_wave).is_none());
    }
}
