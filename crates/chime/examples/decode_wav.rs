//! Finds AudioMoth chimes in WAV files and compares the X8 clock with the phone.
//!
//! Usage: cargo run --release -p chime --example decode_wav -- FILE.wav [FILE.wav ...]
//!
//! The tool reads 16, 24 and 32-bit PCM and 32-bit float files with any number of
//! channels, streams them without loading them into memory, and runs one decoder
//! per channel. If a file has a BWF bext chunk, the tool also works out what the X8
//! clock read when each chime started and how far that is from the phone's time, to
//! about 1 s.
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

const EXIT_CHIME_READ: u8 = 0;
const EXIT_NO_CHIME: u8 = 1;
const EXIT_ERROR: u8 = 2;

const SECONDS_IN_DAY: f64 = 86_400.0;

/// A TimeReference above this many days of samples is not a real one. Split files can
/// go past one day, because TimeReference adds up over the parts of a take.
const MAX_TIME_REFERENCE_DAYS: f64 = 7.0;

/// OriginationTime further than this from the recording's time span casts doubt on the
/// X8 clock reading. In 73 X8 takes, 4 were further than this from the end of the take.
/// Gaps of 2 to 4.5 s occur in ordinary takes.
const ORIGINATION_GAP_LIMIT_S: f64 = 5.0;

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
fn decode_file(path: &Path, out: &mut impl Write) -> Result<usize, Box<dyn Error>> {
    let mut file = BufReader::new(File::open(path)?);
    let chunks = read_chunks(&mut file)?;
    let spec = chunks.format.ok_or("the file is not a WAV file with a fmt chunk")??;
    let (data_start, data_size) = chunks.data.ok_or("the file has no data chunk")?;
    // An empty recording can have other chunks after its data chunk, and those end at the
    // end of the file. Samples read as chunks end anywhere else.
    let file_size = file.seek(SeekFrom::End(0))?;
    if data_size == 0 && chunks.end.abs_diff(file_size) > 1 {
        return Err(format!("the data chunk declares a size of 0, but {} bytes follow it", file_size - data_start).into());
    }
    let bext = chunks.bext;
    let frames = data_size / (spec.bytes_per_sample() * spec.channels as u64);
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
    file.seek(SeekFrom::Start(data_start))?;
    for (index, sample) in samples_as_f32(file, &spec, frames * spec.channels as u64).enumerate() {
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
    /// Why the clock reading is missing or in doubt. It is written once, after the first
    /// chime.
    clock_note: Option<String>,
    event_seen: Vec<bool>,
    chimes_read: usize,
}

impl EventReport {
    fn new(sample_rate: u32, channels: usize, bext: Option<&Bext>, frames: u64) -> Self {
        let (clock_start, clock_note) = match bext.map(|fields| clock_at_sample_zero(fields, sample_rate, frames)) {
            None => (None, Some("The file has no bext chunk, so the X8 clock reading is unknown.".to_string())),
            Some(Err(problem)) => (None, Some(format!("{problem}, so the X8 clock reading is unknown."))),
            Some(Ok(clock)) if clock.origination_gap > ORIGINATION_GAP_LIMIT_S => (
                Some(clock.start),
                Some(format!(
                    "OriginationTime in the bext chunk is {:.0} s outside the time span of this recording, so the X8 clock reading may be wrong.",
                    clock.origination_gap
                )),
            ),
            Some(Ok(clock)) => (Some(clock.start), None),
        };
        Self {
            sample_rate,
            clock_start,
            clock_note,
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
                if let Some(note) = self.clock_note.take() {
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

/// Returns `count` samples from `reader`, with PCM scaled to +-1.0. The reader must be at
/// the first sample.
fn samples_as_f32<R: Read>(mut reader: R, spec: &WavSpec, count: u64) -> impl Iterator<Item = io::Result<f32>> {
    let (format, size) = (spec.sample_format, spec.bytes_per_sample() as usize);
    let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
    (0..count).map(move |_| {
        let mut bytes = [0u8; 4];
        // Filling the top of the word gives a 16 or 24-bit sample its sign.
        reader.read_exact(&mut bytes[4 - size..])?;
        Ok(match format {
            SampleFormat::Float => f32::from_le_bytes(bytes),
            SampleFormat::Int => (i32::from_le_bytes(bytes) >> (32 - 8 * size)) as f32 * scale,
        })
    })
}

// -- Sample format -----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum SampleFormat {
    Int,
    Float,
}

/// The fields of a fmt chunk that the tool uses.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WavSpec {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    sample_format: SampleFormat,
}

impl WavSpec {
    fn bytes_per_sample(&self) -> u64 {
        self.bits_per_sample as u64 / 8
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

/// A number with the given decimal places, and with no minus sign if it rounds to zero.
fn fixed(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    match text.strip_prefix('-') {
        Some(size) if size.chars().all(|c| c == '0' || c == '.') => size.to_string(),
        _ => text,
    }
}

fn describe_chime(channel: usize, sample_rate: u32, chime: &Chime) -> String {
    let start_seconds = fixed(chime.start_sample as f64 / sample_rate as f64, 3);
    let prefix = format!("Channel {} at {start_seconds} s:", channel + 1);
    let snr = if chime.snr_db.is_nan() {
        "SNR not measured".to_string()
    } else {
        format!("SNR {} dB", fixed(chime.snr_db as f64, 0))
    };
    let signal = format!(
        "Signal {:.1} dBFS, {snr}, {} corrected.",
        chime.level_dbfs,
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
///
/// Everything here is in whole seconds, from one rounded value, because a bext chunk gives
/// the X8 clock only to about 1 s. In 73 X8 takes, TimeReference was a whole second minus
/// a fixed 12 or 76 ms, wherever in a second Record was pressed. So the clock could be up
/// to about 1 s ahead of what TimeReference says. This is inferred from those takes, and
/// no test has shown it. To settle it, record several takes of chimes from one phone. If
/// this is right, the difference moves by up to 1 s between takes.
fn describe_clock(message: &TimeMessage, start_sample: i64, sample_rate: u32, clock_start: f64) -> String {
    let clock = (clock_start + start_sample as f64 / sample_rate as f64).round() as i64;
    let phone = message.unix_time as i64 + message.timezone_minutes as i64 * 60;
    let difference = clock - phone;
    let magnitude = difference.unsigned_abs();

    let comparison = if magnitude <= 1 {
        "The X8 clock matches the phone to within about 1 s.".to_string()
    } else {
        let direction = if difference < 0 { "behind" } else { "ahead of" };
        format!(
            "The X8 clock is {magnitude} s{} {direction} the phone, accurate to about 1 s.",
            describe_long_duration(magnitude)
        )
    };
    format!("X8 clock at that moment: {}. {comparison}", LocalTime::from_epoch_seconds(clock))
}

/// For a difference of an hour or more, the same value in days, hours and minutes, to the
/// nearest minute.
fn describe_long_duration(seconds: u64) -> String {
    if seconds < 3600 {
        return String::new();
    }
    let total_minutes = (seconds + 30) / 60;
    let (days, hours, minutes) = (total_minutes / 1440, total_minutes / 60 % 24, total_minutes % 60);
    if days > 0 {
        format!(" ({}, {hours} h {minutes} min)", plural(days, "day"))
    } else {
        format!(" ({hours} h {minutes} min)")
    }
}

fn describe_unreadable(channel: usize, sample_rate: u32, unreadable: &Unreadable) -> String {
    let start_seconds = fixed(unreadable.start_sample as f64 / sample_rate as f64, 3);
    let bytes = plural(unreadable.bytes_received as u64, "byte");
    let reason = match unreadable.reason {
        UnreadableReason::BadCrc => format!("CRC check failed after {bytes}"),
        UnreadableReason::SignalLost => format!("signal lost after {bytes}"),
        UnreadableReason::TooManyBytes if unreadable.bytes_received > MAX_MESSAGE_BYTES => {
            format!("no stop symbols after {bytes}, and the longest message is {MAX_MESSAGE_BYTES} bytes")
        }
        UnreadableReason::TooManyBytes => {
            format!("no stop symbols after {bytes}, and the longest chime lasts about 3.4 s")
        }
        UnreadableReason::BadSymbol => format!("a symbol had an invalid length after {bytes}"),
        UnreadableReason::ConflictingReads => {
            format!("two different readings passed the CRC check after {bytes}")
        }
        UnreadableReason::MessageRefused => {
            format!(
                "the message passed the CRC check after {bytes}, but the decoder refused it: \
                 too many corrected bits, only zeros, or a time zone out of range"
            )
        }
    };
    format!(
        "Channel {} at {start_seconds} s: chime heard but unreadable: {reason}. Signal {:.1} dBFS.",
        channel + 1,
        unreadable.level_dbfs
    )
}

fn describe_no_chime(channel: usize, band_level_dbfs: f32) -> String {
    format!(
        "Channel {}: no chime found. 18 kHz band level near the end of the file: {band_level_dbfs:.1} dBFS (running average, 1 s time constant).",
        channel + 1
    )
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

/// What a file's RIFF chunks say. Hound is not used for this because it does not skip
/// the padding byte after a chunk of odd size, and some recorders write a bext chunk of
/// odd size before the data.
#[derive(Debug, Default)]
struct Chunks {
    /// The fmt chunk, or why it cannot be used.
    format: Option<Result<WavSpec, String>>,
    /// Where the samples start, and how many bytes they fill.
    data: Option<(u64, u64)>,
    bext: Option<Bext>,
    /// Where the last chunk ends, which can be past the end of the file.
    end: u64,
}

/// Walks the RIFF chunks. An input that is not a RIFF file has no chunks. A chunk that the
/// input cuts off counts as missing, apart from the data chunk. If there is more than one
/// data chunk, the first holds the samples.
fn read_chunks<R: Read + Seek>(reader: &mut R) -> io::Result<Chunks> {
    let mut chunks = Chunks::default();
    let mut header = [0u8; 12];
    if !read_or_eof(reader, &mut header)? || &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(chunks);
    }
    let mut position = 12u64;
    let mut chunk_header = [0u8; 8];
    while read_or_eof(reader, &mut chunk_header)? {
        let size = u32::from_le_bytes([chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7]]) as u64;
        let id = &chunk_header[0..4];
        if id == b"fmt " {
            let mut fields = [0u8; 40];
            let wanted = (size as usize).min(fields.len());
            if read_or_eof(reader, &mut fields[..wanted])? {
                chunks.format = Some(parse_fmt(&fields[..wanted]));
            }
        } else if id == b"bext" && size as usize >= BEXT_FIELDS_END {
            let mut fields = [0u8; BEXT_FIELDS_END];
            if read_or_eof(reader, &mut fields)? {
                chunks.bext = Some(parse_bext(&fields));
            }
        } else if id == b"data" && chunks.data.is_none() {
            chunks.data = Some((position + 8, size));
        }
        // A chunk with an odd size is followed by one padding byte.
        position += 8 + size + (size & 1);
        reader.seek(SeekFrom::Start(position))?;
    }
    chunks.end = position;
    Ok(chunks)
}

/// Reads the fields of a fmt chunk.
fn parse_fmt(fields: &[u8]) -> Result<WavSpec, String> {
    if fields.len() < 16 {
        return Err("the fmt chunk is too short".to_string());
    }
    let u16_at = |offset: usize| u16::from_le_bytes([fields[offset], fields[offset + 1]]);
    let mut tag = u16_at(0);
    // WAVE_FORMAT_EXTENSIBLE keeps the format in the first two bytes of its SubFormat.
    if tag == 0xFFFE && fields.len() >= 26 {
        tag = u16_at(24);
    }
    let block_align = u16_at(12) as u64;
    let spec = WavSpec {
        channels: u16_at(2),
        sample_rate: u32::from_le_bytes([fields[4], fields[5], fields[6], fields[7]]),
        bits_per_sample: u16_at(14),
        sample_format: match tag {
            1 => SampleFormat::Int,
            3 => SampleFormat::Float,
            _ => return Err(format!("format tag {tag} is not supported")),
        },
    };
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) | (SampleFormat::Int, 16 | 24 | 32) if spec.channels > 0 => {
            if block_align == spec.bytes_per_sample() * spec.channels as u64 {
                Ok(spec)
            } else {
                Err(format!(
                    "a block align of {block_align} bytes with {} of {}-bit samples is not supported",
                    plural(spec.channels as u64, "channel"),
                    spec.bits_per_sample
                ))
            }
        }
        (format, bits) => Err(format!("{bits}-bit {format:?} samples in {} channels are not supported", spec.channels)),
    }
}

fn parse_bext(fields: &[u8; BEXT_FIELDS_END]) -> Bext {
    let mut origination_date = [0u8; 10];
    origination_date.copy_from_slice(&fields[BEXT_DATE_OFFSET..BEXT_DATE_OFFSET + 10]);
    let mut origination_time = [0u8; 8];
    origination_time.copy_from_slice(&fields[BEXT_TIME_OFFSET..BEXT_TIME_OFFSET + 8]);
    let mut time_reference = [0u8; 8];
    time_reference.copy_from_slice(&fields[BEXT_TIME_REFERENCE_OFFSET..BEXT_FIELDS_END]);
    Bext { origination_date, origination_time, time_reference: u64::from_le_bytes(time_reference) }
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
    // Converting to seconds and back changes a date or time that does not exist, such as
    // 31 February or 25:00.
    (LocalTime::from_epoch_seconds(local.to_epoch_seconds()) == local).then_some(local)
}

/// The X8 clock reading at the first sample of a file.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClockStart {
    /// Seconds since 1970-01-01 00:00:00 with no time zone applied.
    start: f64,
    /// How far OriginationDate and OriginationTime are from the recording's time span, in
    /// seconds.
    origination_gap: f64,
}

/// Works out the X8 clock reading at the first sample of the file. If the bext chunk
/// cannot give one, the error says why.
///
/// TimeReference gives the time of day of the first sample, and OriginationDate gives the
/// date. In 73 X8 takes, OriginationDate and OriginationTime appear to be written at the
/// end of the recording: 56 are within 1 s of it. But 13 are more than 1.5 s after it and
/// one is 1076 s after it, for a reason that is not known. A recording that crosses
/// midnight would have an OriginationDate one day later than its first sample. No such
/// recording has been checked, so this function does not assume which end of the recording
/// they describe. It tries the day before, the same day and the day after OriginationDate,
/// and picks the one that puts the origination date and time closest to the span of the
/// recording.
fn clock_at_sample_zero(bext: &Bext, sample_rate: u32, frames: u64) -> Result<ClockStart, &'static str> {
    // A TimeReference of 0 counts as not set, because a recording is very unlikely
    // to start on the exact sample of midnight.
    if bext.time_reference == 0 {
        return Err("The bext chunk's TimeReference is 0");
    }
    let time_reference = bext.time_reference as f64 / sample_rate as f64;
    if time_reference > MAX_TIME_REFERENCE_DAYS * SECONDS_IN_DAY {
        return Err("The bext chunk's TimeReference is more than 7 days");
    }
    let origination =
        parse_origination(bext).ok_or("The bext chunk has no usable date and time")?.to_epoch_seconds() as f64;
    let origination_midnight = (origination / SECONDS_IN_DAY).floor() * SECONDS_IN_DAY;
    let time_of_day = time_reference.rem_euclid(SECONDS_IN_DAY);
    let duration = frames as f64 / sample_rate as f64;

    let mut best = ClockStart { start: 0.0, origination_gap: f64::INFINITY };
    for day_offset in [0.0, -1.0, 1.0] {
        let start = origination_midnight + day_offset * SECONDS_IN_DAY + time_of_day;
        let end = start + duration;
        let origination_gap = (start - origination).max(origination - end).max(0.0);
        if origination_gap < best.origination_gap {
            best = ClockStart { start, origination_gap };
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::WavWriter;
    use std::io::Cursor;

    /// The bext chunk, as the walk finds it.
    fn read_bext<R: Read + Seek>(reader: &mut R) -> io::Result<Option<Bext>> {
        Ok(read_chunks(reader)?.bext)
    }
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
    fn reads_a_file_with_bext_and_junk_before_fmt() {
        let bytes = x8_style_file(&bext_payload(b"2026-03-15", b"09:00:40", 1), 2, 10);
        let chunks = read_chunks(&mut Cursor::new(&bytes)).unwrap();
        let spec = chunks.format.unwrap().unwrap();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_format, SampleFormat::Float);
        let (start, size) = chunks.data.unwrap();
        assert_eq!(size, 80);
        assert_eq!(start as usize, bytes.len() - 80);
    }

    #[test]
    fn a_chunk_of_odd_size_before_the_data_is_stepped_over() {
        let samples: Vec<u8> = [0.5f32, -0.25].iter().flat_map(|s| s.to_le_bytes()).collect();
        let bytes = riff(&[float_fmt(1, RATE), chunk(b"bext", &[0u8; 603]), chunk(b"data", &samples)]);
        let path = temp_path("odd_chunk.wav");
        std::fs::write(&path, &bytes).unwrap();
        let mut output = Vec::new();
        let result = decode_file(&path, &mut output);
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert!(String::from_utf8(output).unwrap().contains("192000 Hz, 1 channel, 32-bit float"));

        let chunks = read_chunks(&mut Cursor::new(&bytes)).unwrap();
        let (start, size) = chunks.data.unwrap();
        let mut reader = Cursor::new(&bytes);
        reader.seek(SeekFrom::Start(start)).unwrap();
        let spec = chunks.format.unwrap().unwrap();
        let values: Vec<f32> = samples_as_f32(reader, &spec, size / 4).map(|s| s.unwrap()).collect();
        assert_eq!(values, [0.5, -0.25]);
    }

    #[test]
    fn the_format_of_an_extensible_fmt_chunk_is_in_its_subformat() {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes());
        fmt.extend_from_slice(&8u16.to_le_bytes());
        fmt.extend_from_slice(&RATE.to_le_bytes());
        fmt.extend_from_slice(&(RATE * 32).to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&0u32.to_le_bytes());
        fmt.extend_from_slice(&3u16.to_le_bytes());
        fmt.extend_from_slice(&[0u8; 14]);
        let bytes = riff(&[chunk(b"fmt ", &fmt), chunk(b"data", &[0u8; 32])]);
        let spec = read_chunks(&mut Cursor::new(&bytes)).unwrap().format.unwrap().unwrap();
        assert_eq!(spec.sample_format, SampleFormat::Float);
        assert_eq!(spec.channels, 8);
        assert_eq!(spec.bits_per_sample, 32);
    }

    #[test]
    fn formats_that_are_not_supported_are_errors() {
        let mut fmt = 2u16.to_le_bytes().to_vec();
        fmt.extend_from_slice(&[0u8; 14]);
        assert!(parse_fmt(&fmt).unwrap_err().contains("format tag 2"));
        assert!(parse_fmt(&[0u8; 10]).unwrap_err().contains("too short"));
    }

    #[test]
    fn clock_matches_the_x8_recording() {
        // The values in web/samples/0066.wav: 5326848 frames, or 27.74 s.
        let fields = bext(b"2026-03-15", b"09:00:40", 6_223_101_696);
        let start = clock_at_sample_zero(&fields, RATE, 5_326_848).unwrap().start;
        assert_eq!(
            local_from_clock(start),
            LocalTime { year: 2026, month: 3, day: 15, hour: 9, minute: 0, second: 11 }
        );
    }

    #[test]
    fn clock_handles_a_recording_that_crosses_midnight() {
        // The first sample is at 23:59:50 and OriginationDate is the day after,
        // and the recording lasts 20 s.
        let time_reference = (23 * 3600 + 59 * 60 + 50) as u64 * RATE as u64;
        let fields = bext(b"2027-01-01", b"00:00:10", time_reference);
        let start = clock_at_sample_zero(&fields, RATE, 20 * RATE as u64).unwrap().start;
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
        let start = clock_at_sample_zero(&fields, RATE, 5_326_848).unwrap().start;
        assert_eq!(local_from_clock(start).day, 15);
    }

    #[test]
    fn clock_is_an_error_without_usable_fields() {
        let error = |fields: Bext| clock_at_sample_zero(&fields, RATE, 100).unwrap_err();
        assert!(error(bext(b"2026-03-15", b"09:00:40", 0)).contains("TimeReference is 0"));
        assert!(error(bext(&[0; 10], &[0; 8], 42)).contains("no usable date and time"));
        assert!(error(bext(b"2026-13-15", b"09:00:40", 42)).contains("no usable date and time"));
        assert!(error(bext(b"2026-03-15", b"25:00:40", 42)).contains("no usable date and time"));
        assert!(error(bext(b"2026-02-31", b"09:00:40", 42)).contains("no usable date and time"));
        assert!(error(bext(b"2026-03-15", b"09:00:40", (1 << 63) + 12_345)).contains("more than 7 days"));
    }

    #[test]
    fn a_time_reference_above_one_day_wraps() {
        // TimeReference adds up over split files, so it can pass midnight.
        let time_reference = (86_400 + 60) * RATE as u64;
        let clock = clock_at_sample_zero(&bext(b"2026-03-16", b"00:01:30", time_reference), RATE, 30 * RATE as u64);
        let start = clock.unwrap().start;
        assert_eq!(local_from_clock(start), LocalTime { year: 2026, month: 3, day: 16, hour: 0, minute: 1, second: 0 });
    }

    #[test]
    fn origination_time_far_from_the_recording_gives_one_note() {
        let chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);
        // The recording is from 18:33:10 to 18:33:40, and OriginationTime is 300 s later.
        let time_reference = (18 * 3600 + 33 * 60 + 10) as u64 * RATE as u64;
        let fields = bext(b"2026-10-26", b"18:38:40", time_reference);
        let mut report = EventReport::new(RATE, 1, Some(&fields), 30 * RATE as u64);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches("X8 clock at that moment").count(), 2, "{text}");
        assert_eq!(
            text.matches(
                "OriginationTime in the bext chunk is 300 s outside the time span of this recording, \
                 so the X8 clock reading may be wrong."
            )
            .count(),
            1,
            "{text}"
        );

        // A gap of 4 s is ordinary.
        let fields = bext(b"2026-10-26", b"18:33:44", time_reference);
        let mut report = EventReport::new(RATE, 1, Some(&fields), 30 * RATE as u64);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        assert!(!String::from_utf8(output).unwrap().contains("OriginationTime"));
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
        let at_start = |offset: f64| describe_clock(&phone, RATE as i64, RATE, phone_local - 1.0 + offset);

        assert_eq!(
            at_start(-7.0),
            "X8 clock at that moment: 2026-10-26 18:33:13. The X8 clock is 7 s behind the phone, accurate to about 1 s."
        );
        assert_eq!(
            at_start(2.25),
            "X8 clock at that moment: 2026-10-26 18:33:22. The X8 clock is 2 s ahead of the phone, accurate to about 1 s."
        );
        assert_eq!(
            at_start(0.01),
            "X8 clock at that moment: 2026-10-26 18:33:20. The X8 clock matches the phone to within about 1 s."
        );
        // As in a real X8 take: a whole second minus 12 ms, one second behind the phone.
        assert_eq!(
            at_start(-1.012),
            "X8 clock at that moment: 2026-10-26 18:33:19. The X8 clock matches the phone to within about 1 s."
        );
    }

    #[test]
    fn a_chime_before_the_first_sample_uses_the_moment_it_started() {
        let phone = message(1_793_000_000, 0);
        let line = describe_clock(&phone, -3 * RATE as i64, RATE, 1_793_000_000.0 + 3.0);
        assert!(line.contains("matches the phone"), "{line}");
    }

    #[test]
    fn clock_report_spells_out_long_differences() {
        let phone = message(1_793_000_000, 0);
        let behind = describe_clock(&phone, 0, RATE, 1_793_000_000.0 - 90_000.0);
        assert!(behind.ends_with("90000 s (1 day, 1 h 0 min) behind the phone, accurate to about 1 s."), "{behind}");
        let ahead = describe_clock(&phone, 0, RATE, 1_793_000_000.0 + 7_260.0);
        assert!(ahead.ends_with("7260 s (2 h 1 min) ahead of the phone, accurate to about 1 s."), "{ahead}");
    }

    #[test]
    fn long_differences_come_from_the_same_rounded_seconds_in_both_directions() {
        let phone = message(1_793_000_000, 0);
        // The X8's TimeReference is a whole second minus 12 ms.
        let hours = |sign: f64, count: f64| {
            describe_clock(&phone, 0, RATE, 1_793_000_000.0 + sign * count * 3600.0 - 0.012)
        };
        assert!(hours(1.0, 1.0).contains("3600 s (1 h 0 min) ahead of"), "{}", hours(1.0, 1.0));
        assert!(hours(-1.0, 1.0).contains("3600 s (1 h 0 min) behind"), "{}", hours(-1.0, 1.0));
        assert!(hours(1.0, 2.0).contains("7200 s (2 h 0 min) ahead of"), "{}", hours(1.0, 2.0));
        assert!(hours(1.0, 237.0 * 24.0).contains("20476800 s (237 days, 0 h 0 min)"), "{}", hours(1.0, 5688.0));
    }

    #[test]
    fn numbers_that_round_to_zero_have_no_minus_sign() {
        assert_eq!(fixed(-0.3, 0), "0");
        assert_eq!(fixed(-0.0002, 3), "0.000");
        assert_eq!(fixed(-0.6, 0), "-1");
        assert_eq!(fixed(-0.09, 3), "-0.090");
        assert_eq!(fixed(10.0, 0), "10");
    }

    #[test]
    fn a_missing_snr_is_stated() {
        let mut chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);
        chime.snr_db = f32::NAN;
        let line = describe_chime(0, RATE, &chime);
        assert!(line.ends_with("Signal -41.2 dBFS, SNR not measured, 2 bits corrected."), "{line}");
        chime.snr_db = -0.3;
        let line = describe_chime(0, RATE, &chime);
        assert!(line.contains(", SNR 0 dB,"), "{line}");
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
        unreadable.reason = UnreadableReason::TooManyBytes;
        unreadable.bytes_received = 20;
        assert!(describe_unreadable(0, RATE, &unreadable).contains("no stop symbols after 20 bytes, and the longest chime lasts"));
        unreadable.reason = UnreadableReason::MessageRefused;
        unreadable.bytes_received = 8;
        assert!(describe_unreadable(0, RATE, &unreadable).contains("the message passed the CRC check after 8 bytes, but"));
    }

    #[test]
    fn no_chime_report() {
        assert_eq!(
            describe_no_chime(0, -72.0),
            "Channel 1: no chime found. 18 kHz band level near the end of the file: -72.0 dBFS (running average, 1 s time constant)."
        );
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
        let fields = bext(b"2026-10-26", b"09:00:40", 6_223_101_696);
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
        assert!(lines[1].starts_with("X8 clock at that moment: 2026-10-26 09:00:16. The X8 clock is "), "{text}");
        assert!(lines[1].ends_with(" behind the phone, accurate to about 1 s."), "{text}");
        assert_eq!(report.chimes_read, 2);
        assert_eq!(report.event_seen, [false, true]);
    }

    #[test]
    fn event_report_says_when_time_reference_is_0() {
        let chime = chime_with_payload(&[0x40, 0x02, 0xDF, 0x6A, 0x94, 0x02]);
        let fields = bext(b"2026-10-26", b"00:00:30", 0);
        let mut report = EventReport::new(RATE, 1, Some(&fields), 192_000);
        let mut output = Vec::new();
        report.write_event(&mut output, 0, Event::Chime(chime)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("The bext chunk's TimeReference is 0, so the X8 clock reading is unknown."), "{text}");
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

        let unusable = bext(&[0; 10], &[0; 8], 42);
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

        let chunks = read_chunks(&mut BufReader::new(File::open(path).unwrap())).unwrap();
        let spec = chunks.format.unwrap().unwrap();
        assert_eq!(spec.sample_rate, RATE);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_format, SampleFormat::Float);
        let frames = chunks.data.unwrap().1 / 8;
        assert_eq!(frames, 5_326_848);

        let clock = clock_at_sample_zero(&fields, RATE, frames).unwrap();
        assert_eq!(
            local_from_clock(clock.start),
            LocalTime { year: 2026, month: 3, day: 15, hour: 9, minute: 0, second: 11 }
        );
        assert!(clock.origination_gap < ORIGINATION_GAP_LIMIT_S, "{clock:?}");
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
    fn the_first_data_chunk_holds_the_samples() {
        let bytes = riff(&[float_fmt(1, RATE), chunk(b"data", &[0u8; 16]), chunk(b"data", &[0u8; 8])]);
        let (start, size) = read_chunks(&mut Cursor::new(&bytes)).unwrap().data.unwrap();
        assert_eq!(size, 16);
        assert_eq!(start as usize, bytes.len() - 16 - 8 - 8);
    }

    #[test]
    fn a_data_chunk_of_size_0_with_bytes_after_it_is_an_error() {
        let mut bytes = riff(&[float_fmt(1, RATE), chunk(b"data", &[])]);
        let path = temp_path("data_size_0.wav");
        std::fs::write(&path, &bytes).unwrap();
        let empty = decode_file(&path, &mut Vec::new());
        bytes.extend_from_slice(&[0u8; 400]);
        std::fs::write(&path, &bytes).unwrap();
        let with_samples = decode_file(&path, &mut Vec::new());
        assert_eq!(empty.unwrap(), 0);
        // 400 zero bytes read as chunks of size 0, so these samples look like chunks.
        assert_eq!(with_samples.unwrap(), 0);

        // Samples that are not zero read as chunks with sizes that end past the file.
        let mut bytes = riff(&[float_fmt(1, RATE), chunk(b"data", &[])]);
        bytes.extend_from_slice(&[0x55; 400]);
        std::fs::write(&path, &bytes).unwrap();
        let with_samples = decode_file(&path, &mut Vec::new());

        // An empty recording with another chunk after its data chunk is not an error.
        let bytes = riff(&[float_fmt(1, RATE), chunk(b"data", &[]), chunk(b"LIST", &[0u8; 354])]);
        std::fs::write(&path, &bytes).unwrap();
        let with_a_chunk_after = decode_file(&path, &mut Vec::new());
        std::fs::remove_file(&path).unwrap();
        assert!(with_samples.unwrap_err().to_string().contains("size of 0, but 400 bytes follow"));
        assert_eq!(with_a_chunk_after.unwrap(), 0);
    }

    #[test]
    fn a_block_align_that_does_not_fit_the_samples_is_an_error() {
        let mut fields = 1u16.to_le_bytes().to_vec();
        fields.extend_from_slice(&1u16.to_le_bytes());
        fields.extend_from_slice(&48_000u32.to_le_bytes());
        fields.extend_from_slice(&192_000u32.to_le_bytes());
        fields.extend_from_slice(&4u16.to_le_bytes());
        fields.extend_from_slice(&24u16.to_le_bytes());
        assert!(parse_fmt(&fields).unwrap_err().contains("block align of 4 bytes"));
        fields[12] = 3;
        assert_eq!(parse_fmt(&fields).unwrap().bits_per_sample, 24);
    }

    #[test]
    fn decode_file_fails_below_the_lowest_sample_rate() {
        let path = temp_path("low_rate.wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 8_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
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
        let cases: [(u16, hound::SampleFormat, f64); 4] = [
            (16, hound::SampleFormat::Int, 16_384.0),
            (24, hound::SampleFormat::Int, 4_194_304.0),
            (32, hound::SampleFormat::Int, 1_073_741_824.0),
            (32, hound::SampleFormat::Float, 0.5),
        ];
        for (bits, sample_format, half_scale) in cases {
            let hound_spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: bits, sample_format };
            let path = temp_path(&format!("scale_{bits}_{sample_format:?}.wav"));
            let mut writer = WavWriter::create(&path, hound_spec).unwrap();
            for value in [half_scale, -half_scale] {
                match sample_format {
                    hound::SampleFormat::Float => writer.write_sample(value as f32).unwrap(),
                    hound::SampleFormat::Int => writer.write_sample(value as i32).unwrap(),
                }
            }
            writer.finalize().unwrap();

            let mut file = BufReader::new(File::open(&path).unwrap());
            let chunks = read_chunks(&mut file).unwrap();
            let spec = chunks.format.unwrap().unwrap();
            let (start, size) = chunks.data.unwrap();
            file.seek(SeekFrom::Start(start)).unwrap();
            let count = size / spec.bytes_per_sample();
            let samples: Vec<f32> = samples_as_f32(file, &spec, count).map(|s| s.unwrap()).collect();
            std::fs::remove_file(&path).unwrap();
            assert_eq!(samples, [0.5, -0.5], "{bits}-bit {sample_format:?}");
        }
    }

    #[test]
    fn eight_bit_samples_are_an_error() {
        let path = temp_path("eight_bit.wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 8, sample_format: hound::SampleFormat::Int };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        writer.write_sample(1i8).unwrap();
        writer.finalize().unwrap();
        let result = decode_file(&path, &mut Vec::new());
        std::fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }
}
