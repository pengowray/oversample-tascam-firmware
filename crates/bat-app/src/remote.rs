//! The phone remote: the phone plays a tone and then a chime whose deployment ID holds a
//! command, to start or stop recording.
//!
//! The chime decoders cost too much to run all the time while recording, so the app looks for
//! the tone in the spectrum its analysis already makes, which costs one comparison per
//! spectrum column. The tone wakes the decoders for long enough to read a chime.
//!
//! The commands say what to do, not "press REC", so a chime read twice, or read on both
//! inputs, does no harm. The app only reports the command. The host carries it out, and tells
//! the app the outcome with [`BatApp::remote_result`], which beeps in the headphones.
//!
//! [`BatApp::remote_result`]: crate::BatApp::remote_result

use core::f32::consts::TAU;
use core::mem::MaybeUninit;

use chime::format::TimeMessage;
use chime::{Decoder, Event};
use crate::phone_time::Reading;

/// What a chime's deployment ID holds for a command, in the order the decoder gives it: these
/// seven bytes, then the command. The phone page's encoder reverses the bytes it is given.
const COMMAND_MAGIC: [u8; 7] = *b"BATXREC";
const COMMAND_START: u8 = 1;
const COMMAND_STOP: u8 = 2;

const CARRIER_HZ: f32 = chime::format::CARRIER_HZ as f32;
/// The tone counts in a spectrum column when the carrier's bin is this far above the bins 1.5 kHz
/// to each side. A Hann window's main lobe reaches 2 bins out, so those bins hold noise.
const TONE_ABOVE_DB: f32 = 10.0;
const NEIGHBOUR_BINS: usize = 4;
/// Each column with the tone adds 2 to a score and each column without it takes away 1. The
/// tone's symbols have short gaps, so up to a third of its columns can miss. The decoders wake
/// when the score reaches this, about 120 ms into a tone that shows in every column.
const WAKE_SCORE: u32 = 240;
/// The tone that started the wake can have up to 400 ms to run, the page then waits up to 2 s
/// for the next whole second, and the longest chime lasts 3.5 s.
const LISTEN_S: u64 = 6;
/// How long a result shows on the main screen.
const RESULT_SHOWN_S: u64 = 5;

/// What the phone asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCommand {
    Start,
    Stop,
}

/// What came of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResult {
    Started,
    AlreadyRecording,
    Stopped,
    AlreadyStopped,
    /// The HOLD switch is on, and the X8 would show its HOLD pop-up instead of recording.
    HoldOn(RemoteCommand),
    /// MANUAL's recording guide is on, and REC would open it instead of recording.
    GuideOn,
    /// A settings page is open in MANUAL, and the X8 ignores REC until it closes.
    PageOpen,
    /// The recorder did not reach the state asked for in time.
    NoResponse,
}

impl RemoteResult {
    fn beep(self) -> [(f32, u32); 2] {
        match self {
            RemoteResult::Started | RemoteResult::AlreadyRecording => [(880.0, NOTE_MS), (1_320.0, NOTE_MS)],
            RemoteResult::Stopped | RemoteResult::AlreadyStopped => [(1_320.0, NOTE_MS), (880.0, NOTE_MS)],
            _ => [(330.0, 3 * NOTE_MS), (0.0, 0)],
        }
    }
}

const NOTE_MS: u32 = 110;
/// In the units of the app's output. On the X8 the host's gain of 8 makes this about -16 dBFS.
const BEEP_AMPLITUDE: f32 = 0.02;
/// Each note's rise and fall, so it does not click.
const BEEP_RAMP_MS: u32 = 5;

/// The command in a chime's message, if it has one.
pub(crate) fn command_in(message: &TimeMessage) -> Option<RemoteCommand> {
    let id = message.deployment_id?;
    if id[..7] != COMMAND_MAGIC {
        return None;
    }
    match id[7] {
        COMMAND_START => Some(RemoteCommand::Start),
        COMMAND_STOP => Some(RemoteCommand::Stop),
        _ => None,
    }
}

/// The latest command read, for the Tools screen.
#[derive(Clone, Copy)]
pub(crate) struct LastCommand {
    pub(crate) command: RemoteCommand,
    /// Where the decoder finished reading it, in the app's samples.
    pub(crate) read_at: u64,
    pub(crate) level_dbfs: f32,
}

pub(crate) struct Remote {
    /// The host can start and stop its recorder.
    pub(crate) available: bool,
    pub(crate) on: bool,
    /// The CPU test wants the decoders running.
    forced: bool,
    rate_hz: u32,
    carrier_bin: usize,
    tone_score: u32,
    /// The decoders in `Buffers` are ours and ready.
    built: bool,
    /// The decoders run until this sample.
    listen_until: u64,
    /// A wake that the next chunk has to act on.
    wake: bool,
    pub(crate) tones: u32,
    pub(crate) chimes_read: u32,
    pub(crate) chimes_failed: u32,
    pub(crate) last: Option<LastCommand>,
    /// The time in the latest command's chime, so its reading on the other input is ignored.
    last_unix: u32,
    /// The latest chime's time, not yet passed on.
    reading: Option<Reading>,
    pending: Option<RemoteCommand>,
    result: Option<(RemoteResult, u64)>,
    beep: Beep,
}

impl Remote {
    pub(crate) const fn new() -> Self {
        Self {
            available: false,
            on: false,
            forced: false,
            rate_hz: 0,
            carrier_bin: 0,
            tone_score: 0,
            built: false,
            listen_until: 0,
            wake: false,
            tones: 0,
            chimes_read: 0,
            chimes_failed: 0,
            last: None,
            last_unix: 0,
            reading: None,
            pending: None,
            result: None,
            beep: Beep::new(),
        }
    }

    pub(crate) fn configure(&mut self, rate_hz: u32, bin_hz: f32) {
        self.rate_hz = rate_hz;
        self.carrier_bin = libm::roundf(CARRIER_HZ / bin_hz) as usize;
    }

    /// Whether the tone is being looked for.
    pub(crate) fn enabled(&self) -> bool {
        self.available && self.on
    }

    /// One spectrum column, in dB. Inlined, because it runs in the app's analysis, which the X8
    /// patch runs from ITCM.
    #[inline(always)]
    pub(crate) fn column(&mut self, db: &[f32]) {
        let bin = self.carrier_bin;
        let carrier = db[bin];
        let either_side = 0.5 * (db[bin - NEIGHBOUR_BINS] + db[bin + NEIGHBOUR_BINS]);
        if carrier - either_side >= TONE_ABOVE_DB {
            self.tone_score += 2;
            if self.tone_score >= WAKE_SCORE {
                self.tone_score = 0;
                self.wake = true;
            }
        } else {
            self.tone_score = self.tone_score.saturating_sub(1);
        }
    }

    /// Whether the decoders have work to do at sample `now`.
    #[inline(always)]
    pub(crate) fn wants_decoders(&self, now: u64) -> bool {
        self.wake || self.forced || (self.enabled() && now < self.listen_until)
    }

    /// The chime test has taken the decoders.
    pub(crate) fn decoders_taken(&mut self) {
        self.built = false;
        self.listen_until = 0;
        self.wake = false;
    }

    pub(crate) fn set_forced(&mut self, forced: bool) {
        self.forced = forced;
        if !forced {
            self.built = false;
        }
    }

    /// Feed the first `channels` blocks to the decoders. Not inlined, so the decoders stay out
    /// of `BatApp::process`, which the X8 patch runs from ITCM, where they would not fit.
    #[inline(never)]
    pub(crate) fn listen(&mut self, decoders: &mut [MaybeUninit<Decoder>; 2], blocks: [&[f32]; 2], channels: usize, now: u64) {
        if self.wake {
            self.wake = false;
            if now >= self.listen_until {
                self.tones += 1;
            }
            self.listen_until = now + LISTEN_S * self.rate_hz as u64;
        }
        if !self.built {
            let rate = self.rate_hz;
            self.built = decoders.iter_mut().all(|slot| Decoder::init_in_place(slot, rate).is_some());
            if !self.built {
                return;
            }
        }
        for (slot, block) in decoders.iter_mut().zip(blocks).take(channels) {
            // SAFETY: `built` is set only after both decoders have been built, and is cleared
            // when the chime test builds its own.
            let decoder = unsafe { slot.assume_init_mut() };
            decoder.push_block(block, |i, event| self.event(event, now + i as u64));
        }
    }

    /// The decoders have finished listening: the next wake starts them from nothing, because
    /// their sample counts and noise levels do not carry over the time they were not fed.
    pub(crate) fn sleep_if_done(&mut self, now: u64) {
        if self.built && !self.forced && !self.wake && now >= self.listen_until {
            self.built = false;
        }
    }

    fn event(&mut self, event: Event, now: u64) {
        match event {
            Event::Chime(chime) => {
                let Some(message) = TimeMessage::parse(&chime.payload[..chime.payload_len]) else { return };
                self.chimes_read += 1;
                // The decoder counts samples from when it was built; the time since the chime
                // started is the same in both counts.
                let start_sample = now as i64 - (chime.end_sample as i64 - chime.start_sample);
                if self.last_unix != message.unix_time {
                    self.reading = Some(Reading::new(&message, start_sample, now));
                }
                let Some(command) = command_in(&message) else { return };
                let same_chime = self.last.is_some_and(|last| last.command == command) && self.last_unix == message.unix_time;
                self.last = Some(LastCommand { command, read_at: now, level_dbfs: chime.level_dbfs });
                self.last_unix = message.unix_time;
                if !same_chime {
                    self.pending = Some(command);
                    // Stop listening once the other input has had time to finish this chime.
                    self.listen_until = self.listen_until.min(now + self.rate_hz as u64 / 4);
                }
            }
            Event::Unreadable(_) => self.chimes_failed += 1,
        }
    }

    /// The latest chime's time, once.
    pub(crate) fn take_reading(&mut self) -> Option<Reading> {
        self.reading.take()
    }

    pub(crate) fn take_command(&mut self) -> Option<RemoteCommand> {
        self.pending.take()
    }

    pub(crate) fn report(&mut self, result: RemoteResult, now: u64) {
        self.result = Some((result, now + RESULT_SHOWN_S * self.rate_hz as u64));
        self.beep.start(result.beep(), self.rate_hz);
    }

    /// The result to show at sample `now`.
    pub(crate) fn result_shown(&self, now: u64) -> Option<RemoteResult> {
        self.result.filter(|&(_, until)| now < until).map(|(result, _)| result)
    }

    /// Tones heard, chimes read and chimes that failed, since the host started.
    pub(crate) fn counts(&self) -> (u32, u32, u32) {
        (self.tones, self.chimes_read, self.chimes_failed)
    }

    pub(crate) fn listening(&self, now: u64) -> bool {
        self.enabled() && now < self.listen_until
    }

    #[inline(always)]
    pub(crate) fn beeping(&self) -> bool {
        self.beep.note < 2
    }

    pub(crate) fn add_beep(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.beep.add(self.rate_hz, left, right);
    }
}

/// Up to two notes in the headphones.
struct Beep {
    notes: [(f32, u32); 2],
    /// The note playing, 2 when none is.
    note: usize,
    at: u32,
    phase: f32,
}

impl Beep {
    const fn new() -> Self {
        Self { notes: [(0.0, 0); 2], note: 2, at: 0, phase: 0.0 }
    }

    fn start(&mut self, notes: [(f32, u32); 2], rate_hz: u32) {
        let frames = |(hz, ms): (f32, u32)| (hz, ms * rate_hz / 1000);
        self.notes = notes.map(frames);
        self.note = 0;
        self.at = 0;
    }

    /// Not inlined, so that it stays out of ITCM: it runs for a fraction of a second.
    #[inline(never)]
    fn add(&mut self, rate_hz: u32, left: &mut [f32], right: &mut [f32]) {
        let ramp = (BEEP_RAMP_MS * rate_hz / 1000) as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let Some(&(hz, frames)) = self.notes.get(self.note) else { return };
            if self.at >= frames {
                self.note += 1;
                self.at = 0;
                continue;
            }
            self.phase = (self.phase + TAU * hz / rate_hz as f32) % TAU;
            let edge = (self.at.min(frames - self.at) as f32 / ramp).min(1.0);
            let x = BEEP_AMPLITUDE * edge * libm::sinf(self.phase);
            *l += x;
            *r += x;
            self.at += 1;
        }
    }
}
