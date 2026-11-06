//! Finds AudioMoth chimes in WAV files and compares the X8 clock with the phone.
//!
//! Usage: cargo run --release -p chime --example decode_wav -- FILE.wav [FILE.wav ...]
//!
//! The tool reads 16, 24 and 32-bit PCM and 32-bit float files with any number of
//! channels, streams them without loading them into memory, and runs one decoder
//! per channel. If a file has a BWF bext chunk, the tool also works out what the X8
//! clock read when each chime started and how far that is from the phone's time.
//!
//! The exit code is 0 if any chime was read, 1 if no chime was read, and 2 if any
//! file could not be read. A file that cannot be read does not stop the tool from
//! going on to the next file.

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;

use chime::format::{LocalTime, MAX_MESSAGE_BYTES, TimeMessage};
use chime::{Chime, Decoder, Event, Unreadable, UnreadableReason};
use hound::{SampleFormat, WavReader, WavSpec};

const EXIT_CHIME_READ: u8 = 0;
const EXIT_NO_CHIME: u8 = 1;
const EXIT_ERROR: u8 = 2;

const SECONDS_IN_DAY: f64 = 86_400.0;

fn main() -> ExitCode {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("Usage: decode_wav FILE.wav [FILE.wav ...]");
        return ExitCode::from(EXIT_ERROR);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut any_chime = false;
    let mut any_error = false;
    for path in &paths {
        match decode_file(Path::new(path), &mut out) {
            Ok(chimes_read) => any_chime |= chimes_read > 0,
            Err(error) => {
                eprintln!("{path}: {error}");
                any_error = true;
            }
        }
    }
    ExitCode::from(exit_code(any_error, any_chime))
}

fn exit_code(any_error: bool, any_chime: bool) -> u8 {
    if any_error {
        EXIT_ERROR
    } else if any_chime {
        EXIT_CHIME_READ
    } else {
        EXIT_NO_CHIME
    }
}

// -- Decoding a file ---------------------------------------------------------

/// Decodes one file and writes the report to `out`. Returns the number of chimes
/// read.
///
/// hound 3.5 does not skip the padding byte after an odd-sized chunk, so a file with
/// such a chunk before its data fails here with an error. The chunks in the X8
/// recording that has been checked all have even sizes.
fn decode_file(path: &Path, out: &mut impl Write) -> Result<usize, Box<dyn Error>> {
    let bext = read_bext(&mut BufReader::new(File::open(path)?))?;

    let reader = WavReader::open(path)?;
    let spec = reader.spec();
    let frames = reader.duration() as u64;
    if Decoder::new(spec.sample_rate).is_none() {
        return Err(format!(
            "the sample rate of {} Hz is below the 40000 Hz the decoder needs",
            spec.sample_rate
        )
        .into());
    }
    writeln!(out, "{}: {}", path.display(), describe_format(&spec, frames))?;

    let channels = spec.channels as usize;
    let mut decoders: Vec<Decoder> =
        (0..channels).map(|_| Decoder::new(spec.sample_rate).expect("rate was checked")).collect();
    let mut report = EventReport::new(spec.sample_rate, channels, bext.as_ref(), frames);

    let mut read_error = None;
    for (index, sample) in samples_as_f32(reader)?.enumerate() {
        let channel = index % channels;
        match sample {
            Ok(sample) => {
                if let Some(event) = decoders[channel].push(sample) {
                    report.write_event(out, channel, event)?;
                }
            }
            Err(error) => {
                read_error = Some(format!("the file could not be read to the end: {error}"));
                break;
            }
        }
    }

    // The band level has to be read before the silence below lowers it.
    let band_levels: Vec<f32> = decoders.iter().map(Decoder::band_level_dbfs).collect();

    // One second of silence lets a decoder finish a chime that the recording cut off.
    for _ in 0..spec.sample_rate {
        for (channel, decoder) in decoders.iter_mut().enumerate() {
            if let Some(event) = decoder.push(0.0) {
                report.write_event(out, channel, event)?;
            }
        }
    }

    for (channel, level) in band_levels.iter().enumerate() {
        if !report.event_seen[channel] {
            writeln!(out, "{}", describe_no_chime(channel, *level))?;
        }
    }

    match read_error {
        Some(message) => Err(message.into()),
        None => Ok(report.chimes_read),
    }
}

/// Writes the report lines for the decoder events of one file.
struct EventReport {
    sample_rate: u32,
    /// The X8 clock reading at sample 0, if the file's bext chunk gives one.
    clock_start: Option<f64>,
    /// Why the clock reading is missing. It is written once, after the first chime.
    missing_clock_note: Option<&'static str>,
    event_seen: Vec<bool>,
    chimes_read: usize,
}

impl EventReport {
    fn new(sample_rate: u32, channels: usize, bext: Option<&Bext>, frames: u64) -> Self {
        let clock_start = bext.and_then(|fields| clock_at_sample_zero(fields, sample_rate, frames));
        let missing_clock_note = match (bext, clock_start) {
            (None, _) => Some("The file has no bext chunk, so the X8 clock reading is unknown."),
            (Some(_), None) => {
                Some("The bext chunk has no usable date and time, so the X8 clock reading is unknown.")
            }
            (Some(_), Some(_)) => None,
        };
        Self {
            sample_rate,
            clock_start,
            missing_clock_note,
            event_seen: vec![false; channels],
            chimes_read: 0,
        }
    }

    fn write_event(&mut self, out: &mut impl Write, channel: usize, event: Event) -> io::Result<()> {
        self.event_seen[channel] = true;
        match event {
            Event::Chime(chime) => {
                self.chimes_read += 1;
                writeln!(out, "{}", describe_chime(channel, self.sample_rate, &chime))?;
                let Some(message) = TimeMessage::parse(&chime.payload[..chime.payload_len]) else {
                    return Ok(());
                };
                if let Some(start) = self.clock_start {
                    let line = describe_clock(&message, chime.start_sample, self.sample_rate, start);
                    writeln!(out, "{line}")?;
                }
                if let Some(note) = self.missing_clock_note.take() {
                    writeln!(out, "{note}")?;
                }
            }
            Event::Unreadable(unreadable) => {
                writeln!(out, "{}", describe_unreadable(channel, self.sample_rate, &unreadable))?;
            }
        }
        Ok(())
    }
}

/// Returns the samples of all channels, interleaved, with PCM scaled to +-1.0.
fn samples_as_f32<R: Read + 'static>(
    reader: WavReader<R>,
) -> Result<Box<dyn Iterator<Item = hound::Result<f32>>>, String> {
    let spec = reader.spec();
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => Ok(Box::new(reader.into_samples::<f32>())),
        (SampleFormat::Int, bits @ (16 | 24 | 32)) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            Ok(Box::new(reader.into_samples::<i32>().map(move |sample| sample.map(|s| s as f32 * scale))))
        }
        (format, bits) => Err(format!("{bits}-bit {format:?} samples are not supported")),
    }
}

// -- Report lines ------------------------------------------------------------

fn plural(count: u64, singular: &str) -> String {
    if count == 1 { format!("1 {singular}") } else { format!("{count} {singular}s") }
}

/// For example "192000 Hz, 2 channels, 32-bit float, 27.7 s".
fn describe_format(spec: &WavSpec, frames: u64) -> String {
    let encoding = match spec.sample_format {
        SampleFormat::Float => "float",
        SampleFormat::Int => "PCM",
    };
    format!(
        "{} Hz, {}, {}-bit {}, {:.1} s",
        spec.sample_rate,
        plural(spec.channels as u64, "channel"),
        spec.bits_per_sample,
        encoding,
        frames as f64 / spec.sample_rate as f64
    )
}

/// For example "UTC+11:00" or "UTC-05:00".
fn format_time_zone(minutes: i16) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let magnitude = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", magnitude / 60, magnitude % 60)
}

fn describe_chime(channel: usize, sample_rate: u32, chime: &Chime) -> String {
    let start_seconds = chime.start_sample as f64 / sample_rate as f64;
    let prefix = format!("Channel {} at {start_seconds:.3} s:", channel + 1);
    let signal = format!(
        "Signal {:.1} dBFS, SNR {:.0} dB, {} corrected.",
        chime.level_dbfs,
        chime.snr_db,
        plural(chime.corrected_bits as u64, "bit")
    );

    let payload = &chime.payload[..chime.payload_len];
    let Some(message) = TimeMessage::parse(payload) else {
        let hex: String = payload.iter().map(|byte| format!("{byte:02x}")).collect();
        return format!(
            "{prefix} chime read, but its {}-byte payload is not a time message: {hex}. {signal}",
            payload.len()
        );
    };

    let contents = match (message.location.is_some(), message.deployment_id.is_some()) {
        (false, false) => String::new(),
        (true, false) => format!("Payload length {} bytes, with location. ", payload.len()),
        (false, true) => format!("Payload length {} bytes, with deployment ID. ", payload.len()),
        (true, true) => format!("Payload length {} bytes, with location and deployment ID. ", payload.len()),
    };
    format!(
        "{prefix} phone time {} {} (Unix {}). {contents}{signal}",
        message.local(),
        format_time_zone(message.timezone_minutes),
        message.unix_time
    )
}

/// Describes the X8 clock at the start of a chime. `clock_start` is the clock
/// reading at sample 0 from [`clock_at_sample_zero`].
///
/// The time in a chime is the time at which the phone scheduled it to start, so a
/// delay in the phone's audio output appears here as the X8 clock being ahead.
fn describe_clock(message: &TimeMessage, start_sample: u64, sample_rate: u32, clock_start: f64) -> String {
    let clock = clock_start + start_sample as f64 / sample_rate as f64;
    let phone = message.unix_time as f64 + message.timezone_minutes as f64 * 60.0;
    let difference = clock - phone;
    let magnitude = difference.abs();

    let comparison = if magnitude < 0.05 {
        "The X8 clock matches the phone to within 0.1 s.".to_string()
    } else {
        let direction = if difference < 0.0 { "behind" } else { "ahead of" };
        format!("The X8 clock is {magnitude:.1} s{} {direction} the phone.", describe_long_duration(magnitude))
    };
    format!("X8 clock at that moment: {}. {comparison}", LocalTime::from_epoch_seconds(clock.floor() as i64))
}

/// For a difference of an hour or more, the same value in days, hours and minutes.
fn describe_long_duration(seconds: f64) -> String {
    if seconds < 3600.0 {
        return String::new();
    }
    let total_minutes = (seconds / 60.0) as u64;
    let (days, hours, minutes) = (total_minutes / 1440, total_minutes / 60 % 24, total_minutes % 60);
    if days > 0 {
        format!(" ({}, {hours} h {minutes} min)", plural(days, "day"))
    } else {
        format!(" ({hours} h {minutes} min)")
    }
}

fn describe_unreadable(channel: usize, sample_rate: u32, unreadable: &Unreadable) -> String {
    let start_seconds = unreadable.start_sample as f64 / sample_rate as f64;
    let bytes = plural(unreadable.bytes_received as u64, "byte");
    let reason = match unreadable.reason {
        UnreadableReason::BadCrc => format!("CRC check failed after {bytes}"),
        UnreadableReason::SignalLost => format!("signal lost after {bytes}"),
        UnreadableReason::TooManyBytes => {
            format!("no stop symbols after {bytes}, and the longest message is {MAX_MESSAGE_BYTES} bytes")
        }
        UnreadableReason::BadSymbol => format!("a symbol had an invalid length after {bytes}"),
    };
    format!(
        "Channel {} at {start_seconds:.3} s: chime heard but unreadable: {reason}. Signal {:.1} dBFS.",
        channel + 1,
        unreadable.level_dbfs
    )
}

fn describe_no_chime(channel: usize, band_level_dbfs: f32) -> String {
    format!("Channel {}: no chime found. 18 kHz band level {band_level_dbfs:.1} dBFS.", channel + 1)
}

// -- BWF bext chunk ----------------------------------------------------------

const BEXT_DATE_OFFSET: usize = 320;
const BEXT_TIME_OFFSET: usize = 330;
const BEXT_TIME_REFERENCE_OFFSET: usize = 338;
const BEXT_FIELDS_END: usize = BEXT_TIME_REFERENCE_OFFSET + 8;

/// The fields of a BWF bext chunk that hold the recorder's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bext {
    /// OriginationDate as text, for example "2026-03-15".
    origination_date: [u8; 10],
    /// OriginationTime as text, for example "09:00:40".
    origination_time: [u8; 8],
    /// TimeReference: samples since midnight at the first sample of the file.
    time_reference: u64,
}

/// Walks the RIFF chunks and reads the bext chunk. Returns `Ok(None)` if the input
/// is not a RIFF file, or has no bext chunk, or its bext chunk is too short to hold
/// these fields.
fn read_bext<R: Read + Seek>(reader: &mut R) -> io::Result<Option<Bext>> {
    let mut header = [0u8; 12];
    if !read_or_eof(reader, &mut header)? || &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut chunk_header = [0u8; 8];
    while read_or_eof(reader, &mut chunk_header)? {
        let size = u32::from_le_bytes([chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7]]) as u64;
        if &chunk_header[0..4] == b"bext" {
            if (size as usize) < BEXT_FIELDS_END {
                return Ok(None);
            }
            let mut fields = [0u8; BEXT_FIELDS_END];
            if !read_or_eof(reader, &mut fields)? {
                return Ok(None);
            }
            let mut origination_date = [0u8; 10];
            origination_date.copy_from_slice(&fields[BEXT_DATE_OFFSET..BEXT_DATE_OFFSET + 10]);
            let mut origination_time = [0u8; 8];
            origination_time.copy_from_slice(&fields[BEXT_TIME_OFFSET..BEXT_TIME_OFFSET + 8]);
            let mut time_reference = [0u8; 8];
            time_reference.copy_from_slice(&fields[BEXT_TIME_REFERENCE_OFFSET..BEXT_FIELDS_END]);
            return Ok(Some(Bext {
                origination_date,
                origination_time,
                time_reference: u64::from_le_bytes(time_reference),
            }));
        }
        // A chunk with an odd size is followed by one padding byte.
        reader.seek(SeekFrom::Current((size + (size & 1)) as i64))?;
    }
    Ok(None)
}

/// Fills `buffer`, or returns `Ok(false)` if the input ends first.
fn read_or_eof<R: Read>(reader: &mut R, buffer: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(buffer) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reads ASCII digits as a number.
fn parse_digits(text: &[u8]) -> Option<u32> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(text.iter().fold(0, |value, digit| value * 10 + (digit - b'0') as u32))
}

/// Reads OriginationDate and OriginationTime. The separators can be any character,
/// as the BWF specification allows.
fn parse_origination(bext: &Bext) -> Option<LocalTime> {
    let date = &bext.origination_date;
    let time = &bext.origination_time;
    let local = LocalTime {
        year: parse_digits(&date[0..4])? as i32,
        month: parse_digits(&date[5..7])? as u8,
        day: parse_digits(&date[8..10])? as u8,
        hour: parse_digits(&time[0..2])? as u8,
        minute: parse_digits(&time[3..5])? as u8,
        second: parse_digits(&time[6..8])? as u8,
    };
    let valid = (1..=12).contains(&local.month)
        && (1..=31).contains(&local.day)
        && local.hour < 24
        && local.minute < 60
        && local.second < 60;
    valid.then_some(local)
}

/// The X8 clock reading at the first sample of the file, as seconds since
/// 1970-01-01 00:00:00 with no time zone applied. Returns `None` if the bext chunk
/// has no usable date and time.
///
/// TimeReference gives the time of day of the first sample, and OriginationDate
/// gives the date. In the one X8 recording available (web/samples/0066.wav, 27.74 s
/// long), TimeReference reads 09:00:11.99 and OriginationTime reads 09:00:40, so
/// OriginationDate and OriginationTime appear to be written at the end of the
/// recording. A recording that crosses midnight would then have an OriginationDate
/// one day later than its first sample. No such recording has been checked, so this
/// function does not assume which end of the recording they describe. It tries the
/// day before, the same day and the day after OriginationDate, and picks the one
/// that puts the origination date and time closest to the span of the recording.
fn clock_at_sample_zero(bext: &Bext, sample_rate: u32, frames: u64) -> Option<f64> {
    // A TimeReference of 0 counts as not set, because a recording is very unlikely
    // to start on the exact sample of midnight.
    if bext.time_reference == 0 {
        return None;
    }
    let origination = parse_origination(bext)?.to_epoch_seconds() as f64;
    let origination_midnight = (origination / SECONDS_IN_DAY).floor() * SECONDS_IN_DAY;
    let time_of_day = (bext.time_reference as f64 / sample_rate as f64).rem_euclid(SECONDS_IN_DAY);
    let duration = frames as f64 / sample_rate as f64;

    let mut best: Option<(f64, f64)> = None;
    for day_offset in [0.0, -1.0, 1.0] {
        let start = origination_midnight + day_offset * SECONDS_IN_DAY + time_of_day;
        let end = start + duration;
        let distance = (start - origination).max(origination - end).max(0.0);
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, start));
        }
    }
    best.map(|(_, start)| start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::WavWriter;
    use std::io::Cursor;
    use std::path::PathBuf;

    const RATE: u32 = 192_000;

    /// Builds the bytes of a bext chunk's payload. The size is the size of the
    /// chunk in the X8 recording, 1116 bytes.
    fn bext_payload(date: &[u8; 10], time: &[u8; 8], time_reference: u64) -> Vec<u8> {
        let mut payload = vec![0u8; 1116];
        payload[..22].copy_from_slice(b"TASCAM Portacapture X8");
        payload[320..330].copy_from_slice(date);
        payload[330..338].copy_from_slice(time);
        payload[338..346].copy_from_slice(&time_reference.to_le_bytes());
        payload[346] = 1;
        payload
    }

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            bytes.push(0);
        }
        bytes
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&body);
        bytes
    }

    /// The fmt chunk of a 32-bit float file, as the X8 writes it.
    fn float_fmt(channels: u16, rate: u32) -> Vec<u8> {
        let block = channels * 4;
        let mut payload = 3u16.to_le_bytes().to_vec();
        payload.extend_from_slice(&channels.to_le_bytes());
        payload.extend_from_slice(&rate.to_le_bytes());
        payload.extend_from_slice(&(rate * block as u32).to_le_bytes());
        payload.extend_from_slice(&block.to_le_bytes());
        payload.extend_from_slice(&32u16.to_le_bytes());
        chunk(b"fmt ", &payload)
    }

    /// A file with the chunk layout and chunk sizes of an X8 recording: bext, JUNK,
    /// fmt, data.
    fn x8_style_file(bext: &[u8], channels: u16, frames: usize) -> Vec<u8> {
        let data = vec![0u8; frames * channels as usize * 4];
        riff(&[chunk(b"bext", bext), chunk(b"JUNK", &[0u8; 4372]), float_fmt(channels, RATE), chunk(b"data", &data)])
    }

    fn temp_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!("chime_decode_wav_test_{}_{name}", std::process::id()))
    }

    fn bext(date: &[u8; 10], time: &[u8; 8], time_reference: u64) -> Bext {
        Bext { origination_date: *date, origination_time: *time, time_reference }
    }

    fn local_from_clock(seconds: f64) -> LocalTime {
        LocalTime::from_epoch_seconds(seconds.floor() as i64)
    }

    fn message(unix_time: u32, timezone_minutes: i16) -> TimeMessage {
        TimeMessage { unix_time, timezone_minutes, location: None, deployment_id: None }
    }

    fn chime_with_payload(payload: &[u8]) -> Chime {
        let mut bytes = [0u8; 21];
        bytes[..payload.len()].copy_from_slice(payload);
        Chime {
            payload: bytes,
            payload_len: payload.len(),
            start_sample: 674_304,
            end_sample: 900_000,
            corrected_bits: 2,
            level_dbfs: -41.2,
            snr_db: 34.4,
        }
    }

    #[test]
    fn read_bext_finds_the_fields_in_x8_layout() {
        let bytes = x8_style_file(&bext_payload(b"2026-03-15", b"09:00:40", 6_223_101_696), 2, 10);
        let parsed = read_bext(&mut Cursor::new(&bytes)).unwrap().unwrap();
        assert_eq!(parsed, bext(b"2026-03-15", b"09:00:40", 6_223_101_696));
    }

    #[test]
    fn read_bext_steps_over_odd_chunks_before_bext() {
        let bytes = riff(&[
            chunk(b"JUNK", &[0u8; 7]),
            float_fmt(1, RATE),
            chunk(b"bext", &bext_payload(b"2026-11-06", b"16:40:02", 42)),
            chunk(b"data", &[0u8; 8]),
        ]);
        let parsed = read_bext(&mut Cursor::new(&bytes)).unwrap().unwrap();
        assert_eq!(parsed.time_reference, 42);
    }

    #[test]
    fn read_bext_finds_a_chunk_after_the_data() {
        let bytes = riff(&[
            float_fmt(1, RATE),
            chunk(b"data", &[0u8; 8]),
            chunk(b"bext", &bext_payload(b"2026-11-06", b"16:40:02", 42)),
        ]);
        assert_eq!(read_bext(&mut Cursor::new(&bytes)).unwrap().unwrap().time_reference, 42);
    }

    #[test]
    fn read_bext_returns_none_when_there_is_no_usable_chunk() {
        let no_bext = riff(&[float_fmt(1, RATE), chunk(b"data", &[0u8; 8])]);
        assert_eq!(read_bext(&mut Cursor::new(&no_bext)).unwrap(), None);

        let short_bext = riff(&[chunk(b"bext", &[0u8; 100]), float_fmt(1, RATE)]);
        assert_eq!(read_bext(&mut Cursor::new(&short_bext)).unwrap(), None);

        let cut_off = &riff(&[chunk(b"bext", &bext_payload(b"2026-11-06", b"16:40:02", 42))])[..200];
        assert_eq!(read_bext(&mut Cursor::new(cut_off)).unwrap(), None);

        assert_eq!(read_bext(&mut Cursor::new(b"not a wav file".as_slice())).unwrap(), None);
        assert_eq!(read_bext(&mut Cursor::new(b"".as_slice())).unwrap(), None);
    }

    #[test]
    fn hound_reads_a_file_with_bext_and_junk_before_fmt() {
        let bytes = x8_style_file(&bext_payload(b"2026-03-15", b"09:00:40", 1), 2, 10);
        let reader = WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_format, SampleFormat::Float);
        assert_eq!(reader.duration(), 10);
    }

    #[test]
    fn clock_matches_the_x8_recording() {
        // The values in web/samples/0066.wav: 5326848 frames, or 27.74 s.
        let fields = bext(b"2026-03-15", b"09:00:40", 6_223_101_696);
        let start = clock_at_sample_zero(&fields, RATE, 5_326_848).unwrap();
        assert_eq!(
            local_from_clock(start),
            LocalTime { year: 2026, month: 3, day: 15, hour: 9, minute: 0, second: 11 }
        );
        let fraction = start - start.floor();
        assert!((fraction - 0.99).abs() < 0.01, "fraction {fraction}");
    }

    #[test]
    fn clock_handles_a_recording_that_crosses_midnight() {
        // The first sample is at 23:59:50 and OriginationDate is the day after,
        // and the recording lasts 20 s.
        let time_reference = (23 * 3600 + 59 * 60 + 50) as u64 * RATE as u64;
        let fields = bext(b"2027-01-01", b"00:00:10", time_reference);
        let start = clock_at_sample_zero(&fields, RATE, 20 * RATE as u64).unwrap();
        assert_eq!(
            local_from_clock(start),
            LocalTime { year: 2026, month: 12, day: 31, hour: 23, minute: 59, second: 50 }
        );
    }

    #[test]
    fn clock_handles_origination_written_at_the_start() {
        // OriginationTime is the truncated start time here: 09:00:11 for a start at
        // 09:00:11.99. That must not count as crossing midnight.
        let fields = bext(b"2026-03-15", b"09:00:11", 6_223_101_696);
        let start = clock_at_sample_zero(&fields, RATE, 5_326_848).unwrap();
        assert_eq!(local_from_clock(start).day, 15);
    }

    #[test]
    fn clock_is_none_without_usable_fields() {
        assert_eq!(clock_at_sample_zero(&bext(b"2026-03-15", b"09:00:40", 0), RATE, 100), None);
        assert_eq!(clock_at_sample_zero(&bext(&[0; 10], &[0; 8], 42), RATE, 100), None);
        assert_eq!(clock_at_sample_zero(&bext(b"2026-13-15", b"09:00:40", 42), RATE, 100), None);
        assert_eq!(clock_at_sample_zero(&bext(b"2026-03-15", b"25:00:40", 42), RATE, 100), None);
    }

    #[test]
    fn date_separators_can_be_any_character() {
        let fields = bext(b"2026:03:15", b"09.00.40", 6_223_101_696);
        assert_eq!(
            parse_origination(&fields).unwrap(),
            LocalTime { year: 2026, month: 3, day: 15, hour: 9, minute: 0, second: 40 }
        );
    }

    #[test]
    fn chime_report_for_a_time_message() {
        // Unix 1793000000 with a time zone of UTC+11:00 is 2026-10-26 18:33:20 local.
        let chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);
        assert_eq!(
            describe_chime(0, RATE, &chime),
            "Channel 1 at 3.512 s: phone time 2026-10-26 18:33:20 UTC+11:00 (Unix 1793000000). \
             Signal -41.2 dBFS, SNR 34 dB, 2 bits corrected."
        );
    }

    #[test]
    fn chime_report_states_the_length_of_longer_payloads() {
        let mut payload = vec![0x40, 0x02, 0xDF, 0x6A, 0x2C, 0xFF];
        payload.extend_from_slice(&[0; 7]);
        let line = describe_chime(1, RATE, &chime_with_payload(&payload));
        assert!(line.starts_with("Channel 2 at 3.512 s: phone time 2026-10-26 04:01:20 UTC-03:32"), "{line}");
        assert!(line.contains("Payload length 13 bytes, with location. Signal"), "{line}");

        payload.push(0);
        let line = describe_chime(0, RATE, &chime_with_payload(&payload));
        assert!(line.contains("Payload length 14 bytes, with deployment ID. Signal"), "{line}");

        payload.extend_from_slice(&[0; 7]);
        let line = describe_chime(0, RATE, &chime_with_payload(&payload));
        assert!(line.contains("Payload length 21 bytes, with location and deployment ID. Signal"), "{line}");
    }

    #[test]
    fn chime_report_for_a_payload_that_is_not_a_time_message() {
        let mut chime = chime_with_payload(&[0xAB, 0xCD, 0x01]);
        chime.corrected_bits = 1;
        assert_eq!(
            describe_chime(0, RATE, &chime),
            "Channel 1 at 3.512 s: chime read, but its 3-byte payload is not a time message: abcd01. \
             Signal -41.2 dBFS, SNR 34 dB, 1 bit corrected."
        );
    }

    #[test]
    fn clock_report_says_behind_ahead_or_matches() {
        // Phone local time: 1793000000 + 660 minutes = 1793039600 seconds.
        let phone = message(1_793_000_000, 660);
        let phone_local = 1_793_039_600.0;
        let at_start = |offset: f64| describe_clock(&phone, RATE as u64, RATE, phone_local - 1.0 + offset);

        assert_eq!(
            at_start(-7.0),
            "X8 clock at that moment: 2026-10-26 18:33:13. The X8 clock is 7.0 s behind the phone."
        );
        assert_eq!(
            at_start(2.25),
            "X8 clock at that moment: 2026-10-26 18:33:22. The X8 clock is 2.2 s ahead of the phone."
        );
        assert_eq!(
            at_start(0.01),
            "X8 clock at that moment: 2026-10-26 18:33:20. The X8 clock matches the phone to within 0.1 s."
        );
    }

    #[test]
    fn clock_report_spells_out_long_differences() {
        let phone = message(1_793_000_000, 0);
        let behind = describe_clock(&phone, 0, RATE, 1_793_000_000.0 - 90_000.0);
        assert!(behind.ends_with("90000.0 s (1 day, 1 h 0 min) behind the phone."), "{behind}");
        let ahead = describe_clock(&phone, 0, RATE, 1_793_000_000.0 + 7_260.0);
        assert!(ahead.ends_with("7260.0 s (2 h 1 min) ahead of the phone."), "{ahead}");
    }

    #[test]
    fn unreadable_reports() {
        let mut unreadable = Unreadable {
            start_sample: 998_400,
            end_sample: 1_200_000,
            reason: UnreadableReason::BadCrc,
            bytes_received: 8,
            level_dbfs: -61.0,
        };
        assert_eq!(
            describe_unreadable(1, RATE, &unreadable),
            "Channel 2 at 5.200 s: chime heard but unreadable: CRC check failed after 8 bytes. Signal -61.0 dBFS."
        );
        unreadable.reason = UnreadableReason::SignalLost;
        unreadable.bytes_received = 1;
        assert!(describe_unreadable(0, RATE, &unreadable).contains("signal lost after 1 byte."));
        unreadable.reason = UnreadableReason::TooManyBytes;
        unreadable.bytes_received = 24;
        assert!(describe_unreadable(0, RATE, &unreadable).contains(
            "no stop symbols after 24 bytes, and the longest message is 23 bytes."
        ));
        unreadable.bytes_received = 1;
        unreadable.reason = UnreadableReason::BadSymbol;
        assert!(describe_unreadable(0, RATE, &unreadable).contains("a symbol had an invalid length after 1 byte."));
    }

    #[test]
    fn no_chime_report() {
        assert_eq!(describe_no_chime(0, -72.0), "Channel 1: no chime found. 18 kHz band level -72.0 dBFS.");
    }

    #[test]
    fn format_line_and_time_zones() {
        let spec = WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 32, sample_format: SampleFormat::Float };
        assert_eq!(describe_format(&spec, 5_326_848), "192000 Hz, 2 channels, 32-bit float, 27.7 s");
        let spec = WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: SampleFormat::Int };
        assert_eq!(describe_format(&spec, 47_520), "48000 Hz, 1 channel, 16-bit PCM, 1.0 s");

        assert_eq!(format_time_zone(660), "UTC+11:00");
        assert_eq!(format_time_zone(-300), "UTC-05:00");
        assert_eq!(format_time_zone(345), "UTC+05:45");
        assert_eq!(format_time_zone(0), "UTC+00:00");
    }

    #[test]
    fn event_report_writes_the_clock_line_after_each_chime() {
        let fields = bext(b"2026-10-26", b"18:33:40", 6_223_101_696);
        let mut report = EventReport::new(RATE, 2, Some(&fields), 192_000 * 30);
        let chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);

        let mut output = Vec::new();
        report.write_event(&mut output, 1, Event::Chime(chime)).unwrap();
        report.write_event(&mut output, 1, Event::Chime(chime)).unwrap();

        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].starts_with("Channel 2 at 3.512 s: phone time "), "{text}");
        // TimeReference 09:00:11.99 is hours behind the phone's 18:33:20 local time.
        assert!(lines[1].starts_with("X8 clock at that moment: 2026-10-26 09:00:15. The X8 clock is "), "{text}");
        assert!(lines[1].ends_with(" behind the phone."), "{text}");
        assert_eq!(report.chimes_read, 2);
        assert_eq!(report.event_seen, [false, true]);
    }

    #[test]
    fn event_report_says_once_why_the_clock_is_unknown() {
        let mut report = EventReport::new(RATE, 1, None, 192_000);
        let chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches("The file has no bext chunk").count(), 1, "{text}");
        assert_eq!(text.lines().count(), 3, "{text}");

        let unusable = bext(&[0; 10], &[0; 8], 0);
        let mut report = EventReport::new(RATE, 1, Some(&unusable), 192_000);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("The bext chunk has no usable date and time"), "{text}");
    }

    #[test]
    fn event_report_has_no_clock_line_for_other_payloads_and_unreadable_chimes() {
        let fields = bext(b"2026-10-26", b"18:33:40", 6_223_101_696);
        let mut report = EventReport::new(RATE, 1, Some(&fields), 192_000 * 30);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime_with_payload(&[1, 2, 3]))).unwrap();
        let unreadable = Unreadable {
            start_sample: 0,
            end_sample: 1,
            reason: UnreadableReason::SignalLost,
            bytes_received: 0,
            level_dbfs: -60.0,
        };
        report.write_event(&mut output, 0, Event::Unreadable(unreadable)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(!text.contains("X8 clock"), "{text}");
        assert_eq!(report.chimes_read, 1);
    }

    /// Uses the real X8 recording, which git ignores. Run it with:
    /// `cargo test -p chime --example decode_wav -- --ignored`
    #[test]
    #[ignore = "needs web/samples/0066.wav, which is not in the repository"]
    fn real_x8_recording() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/samples/0066.wav");
        let fields = read_bext(&mut BufReader::new(File::open(path).unwrap())).unwrap().unwrap();
        assert_eq!(fields, bext(b"2026-03-15", b"09:00:40", 6_223_101_696));

        let reader = WavReader::open(path).unwrap();
        assert_eq!(reader.spec().sample_rate, RATE);
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_format, SampleFormat::Float);
        let frames = reader.duration() as u64;
        assert_eq!(frames, 5_326_848);

        let start = clock_at_sample_zero(&fields, RATE, frames).unwrap();
        assert_eq!(
            local_from_clock(start),
            LocalTime { year: 2026, month: 3, day: 15, hour: 9, minute: 0, second: 11 }
        );
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(false, true), 0);
        assert_eq!(exit_code(false, false), 1);
        assert_eq!(exit_code(true, true), 2);
        assert_eq!(exit_code(true, false), 2);
    }

    #[test]
    fn decode_file_reports_a_silent_x8_style_file() {
        let path = temp_path("silent.wav");
        let frames = RATE as usize / 4;
        std::fs::write(&path, x8_style_file(&bext_payload(b"2026-03-15", b"09:00:40", 1), 2, frames)).unwrap();

        let mut output = Vec::new();
        let result = decode_file(&path, &mut output);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(result.unwrap(), 0);
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with(": 192000 Hz, 2 channels, 32-bit float, 0.2 s"), "{text}");
        assert!(lines[1].starts_with("Channel 1: no chime found. 18 kHz band level "), "{text}");
        assert!(lines[2].starts_with("Channel 2: no chime found. 18 kHz band level "), "{text}");
    }

    #[test]
    fn decode_file_fails_for_files_it_cannot_read() {
        assert!(decode_file(Path::new("/nonexistent/file.wav"), &mut Vec::new()).is_err());

        let path = temp_path("not_a_wav.wav");
        std::fs::write(&path, b"this is not a wav file").unwrap();
        let result = decode_file(&path, &mut Vec::new());
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn decode_file_fails_below_the_lowest_sample_rate() {
        let path = temp_path("low_rate.wav");
        let spec = WavSpec { channels: 1, sample_rate: 8_000, bits_per_sample: 16, sample_format: SampleFormat::Int };
        WavWriter::create(&path, spec).unwrap().finalize().unwrap();
        let result = decode_file(&path, &mut Vec::new());
        std::fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().to_string().contains("below the 40000 Hz"));
    }

    #[test]
    fn decode_file_reports_a_read_error_for_a_cut_off_file() {
        let mut bytes = x8_style_file(&bext_payload(b"2026-03-15", b"09:00:40", 1), 1, 1000);
        bytes.truncate(bytes.len() - 100);
        let path = temp_path("cut_off.wav");
        std::fs::write(&path, bytes).unwrap();
        let mut output = Vec::new();
        let result = decode_file(&path, &mut output);
        std::fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().to_string().contains("could not be read to the end"));
        assert!(String::from_utf8(output).unwrap().contains("Channel 1: no chime found."));
    }

    #[test]
    fn samples_are_scaled_to_plus_minus_one() {
        let cases: [(u16, SampleFormat, f64); 4] = [
            (16, SampleFormat::Int, 16_384.0),
            (24, SampleFormat::Int, 4_194_304.0),
            (32, SampleFormat::Int, 1_073_741_824.0),
            (32, SampleFormat::Float, 0.5),
        ];
        for (bits, sample_format, half_scale) in cases {
            let spec = WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: bits, sample_format };
            let path = temp_path(&format!("scale_{bits}_{sample_format:?}.wav"));
            let mut writer = WavWriter::create(&path, spec).unwrap();
            for value in [half_scale, -half_scale] {
                match sample_format {
                    SampleFormat::Float => writer.write_sample(value as f32).unwrap(),
                    SampleFormat::Int => writer.write_sample(value as i32).unwrap(),
                }
            }
            writer.finalize().unwrap();

            let reader = WavReader::open(&path).unwrap();
            let samples: Vec<f32> = samples_as_f32(reader).unwrap().map(|s| s.unwrap()).collect();
            std::fs::remove_file(&path).unwrap();
            assert_eq!(samples, [0.5, -0.5], "{bits}-bit {sample_format:?}");
        }
    }

    #[test]
    fn eight_bit_samples_are_an_error() {
        let path = temp_path("eight_bit.wav");
        let spec = WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 8, sample_format: SampleFormat::Int };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        writer.write_sample(1i8).unwrap();
        writer.finalize().unwrap();
        let reader = WavReader::open(&path).unwrap();
        let result = samples_as_f32(reader);
        std::fs::remove_file(&path).unwrap();
        assert!(result.err().unwrap().contains("not supported"));
    }
}
