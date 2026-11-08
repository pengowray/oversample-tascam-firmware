//! GUANO metadata (https://github.com/riggsd/guano-spec) for takes from a bat tile: a `guan` chunk
//! added after the `data` chunk when the stock recorder closes a WAV file. The stock header can't
//! hold it: the space between `bext` and `fmt ` is kept for the take's marks, and the stock mark
//! writer refuses a file where that space holds anything else (docs/HOMEBREW.md, "GUANO
//! metadata"). This part has no firmware addresses, so the host tests can run it.

use core::fmt::{self, Write};

/// What the GUANO text needs from a WAV header as the stock recorder writes it: `RIFF`, `bext`,
/// `cue ` and `LIST` (only with marks), `JUNK`, `fmt `, then `data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub riff_size: u32,
    pub sample_rate: u32,
    /// Bytes in one frame of all channels.
    pub block_align: u16,
    /// File offset of the first byte of audio.
    pub data_start: u32,
    pub data_size: u32,
    /// The `bext` origination date and time. The stock recorder rewrites the header when it closes
    /// the file, so this is the end of the recording, to the second.
    pub written: Option<DateTime>,
}

/// A date and time with no time zone, as the X8's clock keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Days since 1970-01-01.
    days: i32,
    /// Seconds since midnight.
    seconds: u32,
}

const SECONDS_PER_DAY: i64 = 86_400;

/// Offsets in the `bext` chunk's body of the origination date ("yyyy-mm-dd") and time ("hh:mm:ss").
const BEXT_DATE: usize = 320;
const BEXT_TIME: usize = 330;

impl Header {
    /// File offset of the byte after the audio, where the chunk goes.
    pub fn data_end(&self) -> Option<u32> {
        self.data_start.checked_add(self.data_size)
    }

    /// The recording's length in microseconds, to the nearest.
    pub fn length_us(&self) -> Option<u64> {
        if self.sample_rate == 0 || self.block_align == 0 {
            return None;
        }
        let frames = u64::from(self.data_size / u32::from(self.block_align));
        let rate = u64::from(self.sample_rate);
        Some((frames * 1_000_000 + rate / 2) / rate)
    }

    /// When the recording started: the time the header was written, less the length.
    pub fn started(&self) -> Option<DateTime> {
        let length_s = (self.length_us()? + 500_000) / 1_000_000;
        self.written?.minus_seconds(length_s)
    }
}

impl DateTime {
    /// Reads "yyyy-mm-dd" and "hh:mm:ss". BWF allows any of `-_:. ` between the numbers.
    pub fn parse(date: &[u8], time: &[u8]) -> Option<Self> {
        let [y0, y1, y2, y3, _, m0, m1, _, d0, d1] = *date else { return None };
        let [h0, h1, _, n0, n1, _, s0, s1] = *time else { return None };
        let year = digits(&[y0, y1, y2, y3])? as i32;
        let month = digits(&[m0, m1])?;
        let day = digits(&[d0, d1])?;
        let (hour, minute, second) = (digits(&[h0, h1])?, digits(&[n0, n1])?, digits(&[s0, s1])?);
        let days_in_month = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
            2 => 28,
            _ => return None,
        };
        if day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(Self { days: days_from_civil(year, month, day), seconds: hour * 3600 + minute * 60 + second })
    }

    fn minus_seconds(self, seconds: u64) -> Option<Self> {
        let total = i64::from(self.days) * SECONDS_PER_DAY + i64::from(self.seconds) - i64::try_from(seconds).ok()?;
        Some(Self {
            days: i32::try_from(total.div_euclid(SECONDS_PER_DAY)).ok()?,
            seconds: total.rem_euclid(SECONDS_PER_DAY) as u32,
        })
    }
}

/// ISO 8601 without a UTC offset ("2026-11-06T19:07:10"): the X8 doesn't know its time zone.
impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.days);
        let (hour, minute, second) = (self.seconds / 3600, self.seconds / 60 % 60, self.seconds % 60);
        write!(f, "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
    }
}

fn digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0, |n, &b| b.is_ascii_digit().then(|| n * 10 + u32::from(b - b'0')))
}

/// Days since 1970-01-01 for a date in the proleptic Gregorian calendar (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400) as u32;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era as i32 - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097) as u32;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era as i32 + era * 400 + i32::from(month <= 2);
    (year, month, day)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a WAV header from its first bytes, which must reach the `data` chunk's size. None for
/// anything that isn't a plain RIFF WAVE file with `fmt ` before `data`.
pub fn read_header(bytes: &[u8]) -> Option<Header> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let riff_size = u32_at(bytes, 4)?;
    let mut format = None;
    let mut written = None;
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
                    written,
                });
            }
            b"fmt " => format = Some((u32_at(bytes, body + 4)?, u16_at(bytes, body + 12)?)),
            b"bext" => {
                written = bytes.get(body + BEXT_DATE..body + BEXT_DATE + 10).zip(bytes.get(body + BEXT_TIME..body + BEXT_TIME + 8))
                    .and_then(|(date, time)| DateTime::parse(date, time));
            }
            _ => {}
        }
        // Chunks start on even offsets.
        at = body.checked_add(usize::try_from(size).ok()?)?.checked_add(size as usize & 1)?;
    }
}

/// The part of a stored path after the last `/`, `\` or `:`, if it is printable ASCII: anything
/// else is left out of the metadata rather than risk a line break or invalid UTF-8.
pub fn file_name(path: &[u8]) -> Option<&str> {
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let path = &path[..end];
    let start = path.iter().rposition(|&b| matches!(b, b'/' | b'\\' | b':')).map_or(0, |i| i + 1);
    let name = &path[start..];
    if name.is_empty() || !name.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    core::str::from_utf8(name).ok()
}

/// What goes in the GUANO text besides the header's figures.
#[derive(Clone, Copy, Debug)]
pub struct Recorder<'a> {
    /// The stock firmware's version, as the X8 shows it.
    pub firmware: &'a str,
    /// The patch's build number, or 0 when unknown.
    pub patch_build: u32,
    /// The file's name, from [`file_name`].
    pub file_name: Option<&'a str>,
}

/// A writer into a byte slice that fails when it is full.
struct Bytes<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Write for Bytes<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.out.get_mut(self.len..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Writes the bytes to add at [`Header::data_end`]: a pad byte when the audio has an odd number of
/// bytes (RIFF chunks start on even offsets), then the `guan` chunk, its text padded with a line
/// break to an even length as the spec asks. Returns how many bytes, or None if `out` is too small.
pub fn write_chunk(header: &Header, recorder: &Recorder, out: &mut [u8]) -> Option<usize> {
    let pad = (header.data_size & 1) as usize;
    let text_start = pad + 8;
    let mut text = Bytes { out: out.get_mut(text_start..)?, len: 0 };
    write_text(header, recorder, &mut text).ok()?;
    if text.len % 2 == 1 {
        text.write_str("\n").ok()?;
    }
    let text_len = text.len;
    if pad == 1 {
        out[0] = 0;
    }
    out[pad..pad + 4].copy_from_slice(b"guan");
    out[pad + 4..pad + 8].copy_from_slice(&u32::try_from(text_len).ok()?.to_le_bytes());
    Some(text_start + text_len)
}

fn write_text(header: &Header, recorder: &Recorder, out: &mut impl Write) -> fmt::Result {
    // The spec puts GUANO|Version first; the order of the rest is free.
    writeln!(out, "GUANO|Version: 1.0")?;
    if let Some(started) = header.started() {
        writeln!(out, "Timestamp: {started}")?;
    }
    if let Some(us) = header.length_us() {
        writeln!(out, "Length: {}.{:06}", us / 1_000_000, us % 1_000_000)?;
    }
    writeln!(out, "Samplerate: {}", header.sample_rate)?;
    writeln!(out, "Make: TASCAM")?;
    writeln!(out, "Model: Portacapture X8")?;
    if recorder.patch_build == 0 {
        writeln!(out, "Firmware Version: {} with bat patch", recorder.firmware)?;
    } else {
        writeln!(out, "Firmware Version: {} with bat patch B{}", recorder.firmware, recorder.patch_build)?;
    }
    if let Some(name) = recorder.file_name {
        writeln!(out, "Original Filename: {name}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header laid out as the X8 writes one: `bext` (1116 bytes), `JUNK` (4372), `fmt ` and
    /// `data`, 5548 bytes before the audio.
    fn x8_header(data_size: u32, channels: u16, bits: u16, date: &[u8; 10], time: &[u8; 8]) -> std::vec::Vec<u8> {
        let mut h = std::vec::Vec::new();
        let total = 5548 + data_size;
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(total - 8).to_le_bytes());
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

    const RECORDER: Recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: Some("261106_0090_3-4.wav") };

    fn text(header: &Header, recorder: &Recorder) -> std::string::String {
        let mut out = [0u8; 1024];
        let n = write_chunk(header, recorder, &mut out).unwrap();
        let pad = (header.data_size & 1) as usize;
        assert_eq!(&out[pad..pad + 4], b"guan");
        let size = u32::from_le_bytes(out[pad + 4..pad + 8].try_into().unwrap()) as usize;
        assert_eq!(n, pad + 8 + size);
        assert_eq!(size % 2, 0);
        std::string::String::from_utf8(out[pad + 8..n].to_vec()).unwrap()
    }

    /// 261106_0090_3-4.wav: 89,210,880 bytes of stereo float at 192 kHz, header written at 19:08:08.
    #[test]
    fn a_take_from_the_x8() {
        let bytes = x8_header(89_210_880, 2, 32, b"2026-11-06", b"19:08:08");
        let header = read_header(&bytes).unwrap();
        assert_eq!(header.data_start, 5548);
        assert_eq!(header.data_end(), Some(89_216_428));
        assert_eq!(header.riff_size + 8, 89_216_428);
        assert_eq!(header.length_us(), Some(58_080_000));
        assert_eq!(
            text(&header, &RECORDER),
            "GUANO|Version: 1.0\n\
             Timestamp: 2026-11-06T19:07:10\n\
             Length: 58.080000\n\
             Samplerate: 192000\n\
             Make: TASCAM\n\
             Model: Portacapture X8\n\
             Firmware Version: V2.03 with bat patch B242\n\
             Original Filename: 261106_0090_3-4.wav\n"
        );
    }

    #[test]
    fn the_start_can_fall_on_the_day_before() {
        // 2 minutes of audio, closed 30 s after midnight on 1 March 2024 (a leap year).
        let bytes = x8_header(120 * 192_000 * 8, 2, 32, b"2024-03-01", b"00:00:30");
        let header = read_header(&bytes).unwrap();
        assert!(text(&header, &RECORDER).contains("Timestamp: 2024-02-29T23:58:30\n"));
        let bytes = x8_header(60 * 192_000 * 8, 2, 32, b"2027-01-01", b"00:00:10");
        let header = read_header(&bytes).unwrap();
        assert!(text(&header, &RECORDER).contains("Timestamp: 2026-12-31T23:59:10\n"));
    }

    #[test]
    fn an_unreadable_clock_leaves_the_timestamp_out() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-13-01", b"12:00:00");
        let t = text(&read_header(&bytes).unwrap(), &RECORDER);
        assert!(!t.contains("Timestamp"));
        assert!(t.starts_with("GUANO|Version: 1.0\nLength: 1.000000\n"));
    }

    /// 24-bit mono has 3 bytes a frame, so an odd number of frames leaves the data odd-sized and
    /// the chunk needs a pad byte before it.
    #[test]
    fn odd_sized_audio_gets_a_pad_byte() {
        let bytes = x8_header(3 * 192_001, 1, 24, b"2026-11-06", b"19:08:08");
        let header = read_header(&bytes).unwrap();
        let mut out = [0xffu8; 1024];
        let n = write_chunk(&header, &RECORDER, &mut out).unwrap();
        assert_eq!(out[0], 0);
        assert_eq!(&out[1..5], b"guan");
        assert_eq!((header.data_end().unwrap() as usize + n) % 2, 0, "the file ends on an even offset");
        assert!(text(&header, &RECORDER).contains("Length: 1.000005\n"));
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
    fn a_header_that_stops_short_is_refused() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-11-06", b"19:08:08");
        assert!(read_header(&bytes[..5000]).is_none());
        assert!(read_header(&bytes[..5547]).is_none());
        let mut not_wave = bytes.clone();
        not_wave[8..12].copy_from_slice(b"AVI ");
        assert!(read_header(&not_wave).is_none());
    }

    #[test]
    fn file_names_from_stored_paths() {
        assert_eq!(file_name(b"0:/MUSIC/261106_0090_3-4.wav\0junk"), Some("261106_0090_3-4.wav"));
        assert_eq!(file_name(b"A:\\261106_0090_MIX.wav\0"), Some("261106_0090_MIX.wav"));
        assert_eq!(file_name(b"261106_0090_5.wav"), Some("261106_0090_5.wav"));
        assert_eq!(file_name(b"0:/MUSIC/\0"), None);
        assert_eq!(file_name(b"bad\nname.wav\0"), None);
        assert_eq!(file_name(b"\xe9t\xe9.wav\0"), None);
    }

    #[test]
    fn without_a_build_or_name() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-11-06", b"19:08:08");
        let recorder = Recorder { firmware: "V2.03", patch_build: 0, file_name: None };
        let t = text(&read_header(&bytes).unwrap(), &recorder);
        assert!(t.contains("Firmware Version: V2.03 with bat patch\n"));
        assert!(!t.contains("Original Filename"));
    }

    #[test]
    fn too_small_a_buffer_is_refused() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-11-06", b"19:08:08");
        let header = read_header(&bytes).unwrap();
        assert!(write_chunk(&header, &RECORDER, &mut [0u8; 64]).is_none());
    }

    #[test]
    fn dates_round_trip() {
        for days in [-1, 0, 59, 365, 10_957, 20_398, 20_766, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(2026, 11, 6), 20_763);
    }
}

/// Does what the patch does at the end of a take, on a copy of a real file:
/// `X8_WAV=in.wav X8_WAV_OUT=out.wav cargo test -p x8-patch --bin x8-patch -- --ignored append_to_a_file`
#[cfg(test)]
mod real_file {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    #[test]
    #[ignore]
    fn append_to_a_file() {
        let (Ok(input), Ok(output)) = (std::env::var("X8_WAV"), std::env::var("X8_WAV_OUT")) else { return };
        std::fs::copy(&input, &output).unwrap();
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&output).unwrap();
        let size = u32::try_from(file.metadata().unwrap().len()).unwrap();
        let mut head = [0u8; 0x2000];
        let n = file.read(&mut head).unwrap();
        let header = read_header(&head[..n]).unwrap();
        assert_eq!(header.riff_size + 8, size);
        assert_eq!(header.data_end(), Some(size));
        let name = std::path::Path::new(&input).file_name().unwrap().to_str().unwrap();
        let path = std::format!("0:/MUSIC/{name}\0");
        let recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: file_name(path.as_bytes()) };
        let mut out = [0u8; 0x400];
        let added = write_chunk(&header, &recorder, &mut out).unwrap();
        file.seek(SeekFrom::Start(u64::from(size))).unwrap();
        file.write_all(&out[..added]).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&(header.riff_size + added as u32).to_le_bytes()).unwrap();
    }
}
