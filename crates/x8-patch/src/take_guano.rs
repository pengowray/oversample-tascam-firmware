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
    /// For a take from a bat tile: how many times it was paused, and for how long in all, in ms.
    pub pauses: Option<(u32, u32)>,
    /// The XRI text the X8 has just written in the file's header (the `bext` chunk's
    /// CodingHistory), which holds each input's settings. Read up to its first NUL.
    pub xri: Option<&'a [u8]>,
    /// Settings the XRI text leaves out, read from the X8's memory as the file closes.
    pub inputs: Option<Inputs>,
}

/// Input settings that the X8 does not write in the XRI text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// For tracks 1 to 6, the pair of inputs the track records: 1 INPUT 1-2, 2 INPUT 3-4,
    /// 3 INPUT 5-6. An odd track records the first input of the pair.
    pub track_sources: [u8; 6],
    /// Phantom power for inputs 3 to 6: 0 off.
    pub phantom: [u8; 4],
    /// PHANTOM VOLTAGE is 24 V, not 48 V.
    pub phantom_24v: bool,
    /// MIC TRIM for MIC L and MIC R: 0 to 6 for -3 to 3 dB.
    pub mic_trim: [u8; 2],
    /// Inputs 1 and 2 are the built-in mics, which MIC TRIM applies to.
    pub built_in_mics: bool,
    /// MS DECODE: 0 off, or the pair it decodes in the stereo mix: 1 INPUT 1-2, 2 3-4, 3 5-6.
    pub ms_decode: u8,
}

/// For each track of a file, the phantom power of the input the track records: `3:48V,4:OFF`.
/// The tracks are those in the `SOURCE` value of the file's XRI text (`3:EXT MIC,4:EXT MIC`).
struct Phantom<'a>(&'a [u8], &'a Inputs);

impl Phantom<'_> {
    /// Calls `each` with every track in the `SOURCE` value (0 to 5) and whether the input it
    /// records has phantom power on. Inputs 1 and 2, EXT IN and USB have none. Written as one
    /// plain loop over the bytes: the patch has little room, and iterators over text took 4 KB.
    #[inline(never)]
    fn tracks(&self, each: &mut dyn FnMut(u8, bool)) {
        let mut start = true;
        for &b in self.0 {
            if start && (b'1'..=b'6').contains(&b) {
                let track = b - b'1';
                let pair = self.1.track_sources[usize::from(track)];
                let on = matches!(pair, 2 | 3) && self.1.phantom[usize::from((pair - 2) * 2 + track % 2)] != 0;
                each(track, on);
            }
            start = b == b',';
        }
    }
}

impl fmt::Display for Phantom<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let volts = if self.1.phantom_24v { "24V" } else { "48V" };
        let mut result = Ok(());
        let mut comma = "";
        self.tracks(&mut |track, on| {
            let item = [comma.as_bytes().first().copied().unwrap_or(b' '), b'1' + track, b':'];
            let item = core::str::from_utf8(&item[usize::from(comma.is_empty())..]).unwrap_or_default();
            result = result.and_then(|()| f.write_str(item)).and_then(|()| f.write_str(if on { volts } else { "OFF" }));
            comma = ",";
        });
        result
    }
}

/// A MIC TRIM setting, 0 to 6, as its dB: `-3` to `3`.
struct Trim(u8);

impl fmt::Display for Trim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let db = [b'-', b'0' + self.0.abs_diff(3).min(9)];
        f.write_str(core::str::from_utf8(&db[usize::from(self.0 >= 3)..]).unwrap_or_default())
    }
}

/// The X8's XRI keys that describe the inputs, and the GUANO field each one goes in. The value is
/// copied as the X8 wrote it, one item for each channel of the file: `3:EXT MIC,4:EXT MIC`.
const INPUT_FIELDS: [(&str, &str); 4] = bat_app::player::XRI_GUANO_FIELDS;

/// Millionths of a degree as decimal degrees, with integer arithmetic: `-36.123456`.
pub struct Degrees(pub i32);

impl fmt::Display for Degrees {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let size = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:06}", size / 1_000_000, size % 1_000_000)
    }
}

/// The XRI text's place in a header the X8 wrote: the `bext` chunk comes first, and CodingHistory
/// is the last 514 of its 1,116 bytes. The text ends at its first NUL; the bytes after it are 0.
/// [`guano::coding_history`] finds the same text in any WAV file, and takes more of the patch.
pub fn xri_in_header(header: &[u8]) -> Option<&[u8]> {
    const BEXT_AT: usize = 12;
    const TEXT: core::ops::Range<usize> = BEXT_AT + 8 + 602..BEXT_AT + 8 + 1116;
    header.get(TEXT).filter(|_| header[BEXT_AT..BEXT_AT + 8] == *b"bext\x5c\x04\0\0")
}

/// The GUANO field and the value for one `KEY=value` part of the XRI text, if it is one of
/// [`INPUT_FIELDS`]. Worked through as bytes and kept out of line: both keep the patch small.
#[inline(never)]
fn input_field(part: &[u8]) -> Option<(&'static str, &str)> {
    INPUT_FIELDS.iter().find_map(|&(key, field)| {
        let value = part.strip_prefix(key.as_bytes())?.strip_prefix(b"=")?;
        Some((field, core::str::from_utf8(value).ok()?))
    })
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
    // The bat patch's own fields, in its namespace after the standard ones. A take with no pauses
    // says so, since that is what tells a reader the audio has no gaps.
    if let Some((count, ms)) = recorder.pauses {
        chunk.field("X8Bat|Pauses", count).field("X8Bat|Paused", format_args!("{}.{:03}", ms / 1000, ms % 1000));
    }
    // The inputs' settings, as they were when the file closed. A MIX file's XRI text has none.
    let mut source = None;
    for part in recorder.xri.unwrap_or_default().split(|&b| b == b';' || b == 0) {
        if let Some((field, value)) = input_field(part) {
            chunk.field(field, value);
            if field == INPUT_FIELDS[0].1 {
                source = Some(value);
            }
        }
    }
    // What the XRI text leaves out, each only when it is on. Phantom power and mic trim go in a
    // track's file. MS DECODE changes only the stereo mix, so it goes in the MIX file.
    match (recorder.inputs, source) {
        (Some(inputs), Some(source)) => {
            let phantom = Phantom(source.as_bytes(), &inputs);
            let (mut any_on, mut mics) = (false, false);
            phantom.tracks(&mut |track, on| {
                any_on |= on;
                mics |= inputs.track_sources[usize::from(track)] == 1;
            });
            if any_on {
                chunk.field("X8Bat|Phantom", phantom);
            }
            let [left, right] = inputs.mic_trim;
            if mics && inputs.built_in_mics && (left, right) != (3, 3) {
                chunk.field("X8Bat|Mic Trim", format_args!("L:{} dB,R:{} dB", Trim(left), Trim(right)));
            }
        }
        (Some(Inputs { ms_decode: pair @ 1..=3, .. }), None) => {
            chunk.field("X8Bat|MS Decode", ["1-2", "3-4", "5-6"][usize::from(pair) - 1]);
        }
        _ => {}
    }
    chunk.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guano::DateTime;
    use std::string::String;

    const RECORDER: Recorder = Recorder {
        firmware: "V2.03",
        patch_build: 242,
        file_name: Some("261106_0090_3-4.wav"),
        phone_now: None,
        utc_offset_minutes: None,
        location_e6: None,
        pauses: None,
        xri: None,
        inputs: None,
    };

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
    fn pauses_are_counted_and_timed() {
        let recorder = Recorder { pauses: Some((2, 34_567)), ..RECORDER };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(t.ends_with("X8Bat|Pauses: 2\nX8Bat|Paused: 34.567\n"), "{t}");
    }

    /// The XRI text of 260430_0071_3-4.wav, and of its MIX file.
    const XRI: &[u8] = b"T=XRI 008c;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=3:EXT MIC,4:EXT MIC;\
INPUT GAIN=3:HIGH,4:HIGH;LEVEL=3:55,4:55;AUTO_LEVEL=3:OFF,4:OFF;,";
    const XRI_MIX: &[u8] = b"T=XRI 0034;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;,";

    #[test]
    fn input_settings_come_from_the_xri_text() {
        let recorder = Recorder { pauses: Some((0, 0)), xri: Some(XRI), ..RECORDER };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(
            t.ends_with(
                "X8Bat|Paused: 0.000\n\
                 X8Bat|Input Source: 3:EXT MIC,4:EXT MIC\n\
                 X8Bat|Input Gain: 3:HIGH,4:HIGH\n\
                 X8Bat|Input Level: 3:55,4:55\n\
                 X8Bat|Auto Level: 3:OFF,4:OFF\n"
            ),
            "{t}"
        );
        let mix = Recorder { xri: Some(XRI_MIX), ..RECORDER };
        assert!(!text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &mix).contains("X8Bat|"));
    }

    /// MANUAL's default sources, with everything else off.
    const INPUTS: Inputs = Inputs {
        track_sources: [1, 1, 2, 2, 3, 3],
        phantom: [0; 4],
        phantom_24v: false,
        mic_trim: [3, 3],
        built_in_mics: true,
        ms_decode: 0,
    };

    fn extra(xri: &[u8], inputs: Inputs) -> String {
        let recorder = Recorder { xri: Some(xri), inputs: Some(inputs), ..RECORDER };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        t.lines().filter(|l| ["X8Bat|Ph", "X8Bat|Mi", "X8Bat|MS"].iter().any(|k| l.starts_with(k))).collect()
    }

    #[test]
    fn phantom_power_is_listed_by_track_when_any_is_on() {
        assert_eq!(extra(XRI, INPUTS), "");
        assert_eq!(extra(XRI, Inputs { phantom: [1, 0, 0, 1], ..INPUTS }), "X8Bat|Phantom: 3:48V,4:OFF");
        let both = Inputs { phantom: [1, 1, 0, 0], phantom_24v: true, ..INPUTS };
        assert_eq!(extra(XRI, both), "X8Bat|Phantom: 3:24V,4:24V");
        // Tracks 3 and 4 recording inputs 5 and 6, and a mono file of track 6.
        let swapped = Inputs { track_sources: [1, 1, 3, 3, 2, 2], phantom: [0, 0, 0, 1], ..INPUTS };
        assert_eq!(extra(XRI, swapped), "X8Bat|Phantom: 3:OFF,4:48V");
        let mono = b"T=XRI 0050;XRI VER=0002;SOURCE=6:EXT MIC;INPUT GAIN=6:HIGH;LEVEL=6:41;AUTO_LEVEL=6:OFF;,";
        assert_eq!(extra(mono, Inputs { phantom: [0, 0, 0, 1], ..INPUTS }), "X8Bat|Phantom: 6:48V");
        // The built-in mics have no phantom power, whatever inputs 3 and 4 are set to.
        let mics = b"T=XRI 0050;SOURCE=1:INT MIC,2:INT MIC;INPUT GAIN=1:LOW,2:LOW;LEVEL=1:36,2:36;,";
        assert_eq!(extra(mics, Inputs { phantom: [1; 4], ..INPUTS }), "");
    }

    #[test]
    fn mic_trim_is_listed_for_the_built_in_mics_when_it_is_not_0() {
        let mics = b"T=XRI 0050;SOURCE=1:INT MIC,2:INT MIC;INPUT GAIN=1:LOW,2:LOW;LEVEL=1:36,2:36;,";
        let trimmed = Inputs { mic_trim: [1, 6], ..INPUTS };
        assert_eq!(extra(mics, trimmed), "X8Bat|Mic Trim: L:-2 dB,R:3 dB");
        assert_eq!(extra(mics, INPUTS), "");
        // Not for other inputs' files, nor with mics plugged into the EXT MIC jack.
        assert_eq!(extra(XRI, trimmed), "");
        let plugged = b"T=XRI 0050;SOURCE=1:PLUG IN,2:PLUG IN;,";
        assert_eq!(extra(plugged, Inputs { built_in_mics: false, ..trimmed }), "");
    }

    #[test]
    fn ms_decode_is_listed_in_the_mix_file() {
        let decoding = Inputs { ms_decode: 2, ..INPUTS };
        assert_eq!(extra(XRI_MIX, decoding), "X8Bat|MS Decode: 3-4");
        assert_eq!(extra(XRI_MIX, INPUTS), "");
        assert_eq!(extra(XRI, decoding), "");
    }

    /// A whole header as the X8 lays it out, read the way the patch reads it: the text is found
    /// at its fixed place, and the zero bytes after it add nothing.
    #[test]
    fn the_xri_text_is_read_from_its_place_in_the_header() {
        let mut bytes = std::vec![0u8; 5548];
        bytes[..20].copy_from_slice(b"RIFF\0\0\0\0WAVEbext\x5c\x04\0\0");
        bytes[622..622 + XRI.len()].copy_from_slice(XRI);
        let xri = xri_in_header(&bytes).unwrap();
        assert_eq!((xri.len(), guano::coding_history(&bytes)), (514, Some(XRI)));
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &Recorder { xri: Some(xri), ..RECORDER });
        assert!(t.ends_with("X8Bat|Input Level: 3:55,4:55\nX8Bat|Auto Level: 3:OFF,4:OFF\n"), "{t}");
        assert_eq!(t.matches("X8Bat|").count(), 4);
        // Another layout, or a header cut short, gives no text.
        bytes[16] = 0x5e;
        assert_eq!(xri_in_header(&bytes), None);
        assert_eq!(xri_in_header(&bytes[..1000]), None);
    }

    /// Every field at its longest fits the 1 KB the patch builds the chunk in.
    #[test]
    fn the_longest_chunk_fits() {
        let xri = b"T=XRI 00a0;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=5:EXT MIC,6:EXT MIC;\
INPUT GAIN=5:HIGH,6:HIGH;LEVEL=5:70,6:70;AUTO_LEVEL=5:OFF,6:OFF;,";
        let recorder = Recorder {
            file_name: Some("261106_0090F_5-6.wav"),
            phone_now: Some(1_793_995_808),
            utc_offset_minutes: Some(-210),
            location_e6: Some((-36_123_456, -146_000_007)),
            pauses: Some((u32::MAX, u32::MAX)),
            xri: Some(xri),
            inputs: Some(Inputs { phantom: [1; 4], mic_trim: [0, 0], ..INPUTS }),
            ..RECORDER
        };
        let mut out = [0u8; 0x400];
        let n = write_chunk(&header(u32::MAX - 5548, 8, b"2026-11-06", b"19:08:08"), &recorder, &mut out).unwrap();
        assert!(n < 600, "{n}");
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
        let recorder = Recorder {
            firmware: "V2.03",
            patch_build: 0,
            file_name: None,
            phone_now: None,
            utc_offset_minutes: None,
            location_e6: None,
            pauses: None,
            xri: None,
            inputs: None,
        };
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
        let recorder = Recorder {
            firmware: "V2.03",
            patch_build: 242,
            file_name: guano::file_name(path.as_bytes()),
            phone_now: None,
            utc_offset_minutes: None,
            location_e6: None,
            pauses: None,
            xri: xri_in_header(&head[..n]),
            inputs: None,
        };
        let mut out = [0u8; 0x400];
        let added = write_chunk(&header, &recorder, &mut out).unwrap();
        file.seek(SeekFrom::Start(u64::from(size))).unwrap();
        file.write_all(&out[..added]).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&(header.riff_size + added as u32).to_le_bytes()).unwrap();
    }
}
