//! The GUANO metadata the patch adds to the WAV files of takes from a bat tile (docs/HOMEBREW.md,
//! "GUANO metadata"). This part has no firmware addresses, so the host tests can run it.

use core::fmt;

use guano::{ChunkWriter, DateTime, Header};

/// What goes in the metadata besides the header's figures.
#[derive(Clone, Copy, Debug)]
pub struct Recorder<'a> {
    /// The stock firmware's version, as the X8 shows it.
    pub firmware: &'a str,
    /// The patch's build number, or 0 when unknown.
    pub patch_build: u32,
    /// The file's name, from [`guano::file_name`].
    pub file_name: Option<&'a str>,
    /// The phone's local time now, as seconds since 1970, when the X8's clock is to be set from it
    /// after this take: the start is then worked out from it instead of from the `bext` time.
    pub phone_now: Option<i64>,
    /// The UTC offset of the time the start is in, in minutes, when known: the phone's zone, once
    /// the clock keeps the phone's time.
    pub utc_offset_minutes: Option<i16>,
    /// The phone's location from a chime, in millionths of a degree (latitude, longitude), when
    /// recent enough for this take.
    pub location_e6: Option<(i32, i32)>,
}

/// Millionths of a degree as decimal degrees, with integer arithmetic: `-36.123456`.
pub struct Degrees(pub i32);

impl fmt::Display for Degrees {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let size = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:06}", size / 1_000_000, size % 1_000_000)
    }
}

/// One line of the location log: the phone's local time with its UTC offset, latitude and
/// longitude, comma-separated, as a spreadsheet reads it.
pub struct LogLine {
    pub local_seconds: i64,
    pub utc_offset_minutes: i16,
    pub location_e6: (i32, i32),
}

/// The location log's first line, written when the file is new.
pub const LOG_HEADER: &str = "time,latitude,longitude\n";

impl fmt::Display for LogLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(time) = DateTime::from_seconds(self.local_seconds) else { return Err(fmt::Error) };
        let (latitude, longitude) = self.location_e6;
        writeln!(f, "{},{},{}", Timestamp(time, Some(self.utc_offset_minutes)), Degrees(latitude), Degrees(longitude))
    }
}

/// A start time with its UTC offset, as ISO 8601 takes it: `2026-11-08T21:30:49+11:00`.
struct Timestamp(DateTime, Option<i16>);

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        if let Some(minutes) = self.1 {
            let sign = if minutes < 0 { '-' } else { '+' };
            let size = minutes.unsigned_abs();
            write!(f, "{sign}{:02}:{:02}", size / 60, size % 60)?;
        }
        Ok(())
    }
}

/// Writes the bytes to add at [`Header::data_end`] into `out`: a pad byte if the audio has an odd
/// number of bytes, then the `guan` chunk. Returns how many bytes, or None if `out` is too small.
///
/// `Timestamp` is the start of the recording: the `bext` origination time less the length. The X8
/// rewrites the header, `bext` included, as it closes the file, so the origination time is the end
/// of the recording, to the second. The X8 does not know its time zone, so there is no UTC offset.
pub fn write_chunk(header: &Header, recorder: &Recorder, out: &mut [u8]) -> Option<usize> {
    let mut chunk = ChunkWriter::new(out, header.data_end()?);
    let length_us = header.length_us();
    let end = match recorder.phone_now {
        Some(seconds) => DateTime::from_seconds(seconds),
        None => header.origination,
    };
    let started = end.zip(length_us).and_then(|(end, us)| end.minus_seconds((us + 500_000) / 1_000_000));
    if let Some(started) = started {
        chunk.field("Timestamp", Timestamp(started, recorder.utc_offset_minutes));
    }
    if let Some(us) = length_us {
        chunk.field("Length", format_args!("{}.{:06}", us / 1_000_000, us % 1_000_000));
    }
    chunk.field("Samplerate", header.sample_rate).field("Make", "TASCAM").field("Model", "Portacapture X8");
    if recorder.patch_build == 0 {
        chunk.field("Firmware Version", format_args!("{} with bat patch", recorder.firmware));
    } else {
        chunk.field("Firmware Version", format_args!("{} with bat patch B{}", recorder.firmware, recorder.patch_build));
    }
    if let Some(name) = recorder.file_name {
        chunk.field("Original Filename", name);
    }
    if let Some((latitude, longitude)) = recorder.location_e6 {
        chunk.field("Loc Position", format_args!("{} {}", Degrees(latitude), Degrees(longitude)));
    }
    chunk.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guano::DateTime;
    use std::string::String;

    const RECORDER: Recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: Some("261106_0090_3-4.wav"), phone_now: None, utc_offset_minutes: None, location_e6: None };

    /// A header as the X8 writes one, 5548 bytes before the audio.
    fn header(data_size: u32, block_align: u16, date: &[u8], time: &[u8]) -> Header {
        Header {
            riff_size: 5548 + data_size - 8,
            sample_rate: 192_000,
            block_align,
            data_start: 5548,
            data_size,
            origination: DateTime::parse(date, time),
        }
    }

    fn text(header: &Header, recorder: &Recorder) -> String {
        let mut out = [0u8; 1024];
        let n = write_chunk(header, recorder, &mut out).unwrap();
        let pad = (header.data_size & 1) as usize;
        assert_eq!(&out[pad..pad + 4], b"guan");
        String::from_utf8(out[pad + 8..n].to_vec()).unwrap()
    }

    /// 261106_0090_3-4.wav: 89,210,880 bytes of stereo float, header written at 19:08:08.
    #[test]
    fn a_take_from_the_x8() {
        assert_eq!(
            text(&header(89_210_880, 8, b"2026-11-06", b"19:08:08"), &RECORDER),
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

    /// A chime during the take: the X8's clock is 1 h 2 min slow, and the start comes from the
    /// phone's time at the close, with the phone's UTC offset.
    #[test]
    fn a_chime_during_the_take_corrects_the_start() {
        // 2026-11-06 20:10:08.
        let phone_now = 1_793_995_808;
        let recorder = Recorder { phone_now: Some(phone_now), utc_offset_minutes: Some(660), ..RECORDER };
        let t = text(&header(89_210_880, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(t.contains("Timestamp: 2026-11-06T20:09:10+11:00\n"), "{t}");
        let west = Recorder { utc_offset_minutes: Some(-210), ..RECORDER };
        let t = text(&header(89_210_880, 8, b"2026-11-06", b"19:08:08"), &west);
        assert!(t.contains("Timestamp: 2026-11-06T19:07:10-03:30\n"), "{t}");
    }

    #[test]
    fn a_recent_location_goes_in_and_log_lines_read_as_csv() {
        let recorder = Recorder { location_e6: Some((-36_123_456, 146_000_007)), ..RECORDER };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(t.contains("Loc Position: -36.123456 146.000007\n"), "{t}");
        let line = LogLine { local_seconds: 1_794_173_449, utc_offset_minutes: 660, location_e6: (-500_000, 0) };
        assert_eq!(std::format!("{line}"), "2026-11-08T21:30:49+11:00,-0.500000,0.000000\n");
    }

    #[test]
    fn the_start_can_fall_on_the_day_before() {
        let t = text(&header(120 * 192_000 * 8, 8, b"2024-03-01", b"00:00:30"), &RECORDER);
        assert!(t.contains("Timestamp: 2024-02-29T23:58:30\n"), "{t}");
    }

    #[test]
    fn an_unreadable_clock_leaves_the_timestamp_out() {
        let t = text(&header(192_000 * 8, 8, b"2026-13-01", b"12:00:00"), &RECORDER);
        assert!(t.starts_with("GUANO|Version: 1.0\nLength: 1.000000\n"), "{t}");
    }

    /// 24-bit mono has 3 bytes a frame, so an odd number of frames leaves the audio odd-sized.
    #[test]
    fn odd_sized_audio_gets_a_pad_byte() {
        let header = header(3 * 192_001, 3, b"2026-11-06", b"19:08:08");
        let mut out = [0xffu8; 1024];
        let n = write_chunk(&header, &RECORDER, &mut out).unwrap();
        assert_eq!(out[0], 0);
        assert_eq!(&out[1..5], b"guan");
        assert_eq!((header.data_end().unwrap() as usize + n) % 2, 0);
        assert!(text(&header, &RECORDER).contains("Length: 1.000005\n"));
    }

    #[test]
    fn without_a_build_or_name() {
        let recorder = Recorder { firmware: "V2.03", patch_build: 0, file_name: None, phone_now: None, utc_offset_minutes: None, location_e6: None };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(t.contains("Firmware Version: V2.03 with bat patch\n"));
        assert!(!t.contains("Original Filename"));
    }

    #[test]
    fn too_small_a_buffer_is_refused() {
        assert!(write_chunk(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &RECORDER, &mut [0u8; 64]).is_none());
    }

    /// Does what the patch does at the end of a take, on a copy of a real file:
    /// `X8_WAV=in.wav X8_WAV_OUT=out.wav cargo test -p x8-patch -- --ignored append_to_a_file`
    #[test]
    #[ignore]
    fn append_to_a_file() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let (Ok(input), Ok(output)) = (std::env::var("X8_WAV"), std::env::var("X8_WAV_OUT")) else { return };
        std::fs::copy(&input, &output).unwrap();
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&output).unwrap();
        let size = u32::try_from(file.metadata().unwrap().len()).unwrap();
        let mut head = [0u8; 0x2000];
        let n = file.read(&mut head).unwrap();
        let header = guano::read_header(&head[..n]).unwrap();
        assert_eq!(header.riff_size + 8, size);
        assert_eq!(header.data_end(), Some(size));
        let name = std::path::Path::new(&input).file_name().unwrap().to_str().unwrap();
        let path = std::format!("{name}\0");
        let recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: guano::file_name(path.as_bytes()), phone_now: None, utc_offset_minutes: None, location_e6: None };
        let mut out = [0u8; 0x400];
        let added = write_chunk(&header, &recorder, &mut out).unwrap();
        file.seek(SeekFrom::Start(u64::from(size))).unwrap();
        file.write_all(&out[..added]).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&(header.riff_size + added as u32).to_le_bytes()).unwrap();
    }
}
