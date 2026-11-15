//! The phone remote: the phone plays a tone and then a chime whose deployment ID holds a
//! command, to set the clock, start or stop recording, or change the listening mode.
//!
//! The chime decoders cost too much to run all the time while recording, so the app looks for
//! the tone in the spectrum its analysis already makes, which costs one comparison per
//! spectrum column. The tone wakes the decoders for long enough to read a chime.
//!
//! While the Phone code switch is on, the app acts only on a command whose check was made from
//! the 6-digit code on the Phone page (format version 2, in `docs/PHONE_REMOTE.md`), and uses
//! no other chime's time or location on the main screen. The switch is off by default.
//!
//! The commands say what to do, not "press REC", so a chime read twice, or read on both
//! inputs, does no harm. The app changes the listening mode itself. It passes start and stop to the
//! host, which carries them out and tells the app the outcome with [`BatApp::remote_result`], and a
//! clock command to the host's clock with the chime's time. Each outcome beeps in the headphones.
//!
//! [`BatApp::remote_result`]: crate::BatApp::remote_result

use core::f32::consts::TAU;
use core::mem::MaybeUninit;

use crate::phone_time::Reading;
use chime::format::{REMOTE_CODE_DIGITS, TimeMessage};
use chime::{Decoder, Event};

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

/// What the phone asked for: the command in a chime's deployment ID (`chime::format`).
pub use chime::format::{ListeningMode, RemoteCommand, RemoteMessage};

use crate::Mode;

/// A phone command that the host carries out with its recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderCommand {
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
    HoldOn(RecorderCommand),
    /// MANUAL's recording guide is on, and REC would open it instead of recording.
    GuideOn,
    /// A settings page is open in MANUAL, and the X8 ignores REC until it closes.
    PageOpen,
    /// The recorder did not reach the state asked for in time.
    NoResponse,
    /// The chime has a phone remote message with a version, command or argument this build does
    /// not know: the phone page is newer than the patch.
    Unknown,
    /// The host set its clock to the phone's time.
    ClockSet,
    /// A take is under way: the host sets its clock when the take ends.
    ClockAfterTake,
    /// The host could not set its clock.
    ClockFailed,
    /// The app plays this listening mode now.
    ModeSet(Mode),
    /// This host does not offer the mode, so nothing changed.
    ModeNotOffered(Mode),
    /// The phone code is required, and the command has no check: nothing was done.
    CodeNeeded,
    /// The command's check was made from another code: nothing was done.
    CodeWrong,
    /// The command's check is right, but its time is not later than the time of the last
    /// command acted on, so it may be a recording played again: nothing was done.
    CodeOld,
}

impl RemoteResult {
    fn beep(self) -> [(f32, u32); 2] {
        match self {
            RemoteResult::Started | RemoteResult::AlreadyRecording => [(880.0, NOTE_MS), (1_320.0, NOTE_MS)],
            RemoteResult::Stopped | RemoteResult::AlreadyStopped => [(1_320.0, NOTE_MS), (880.0, NOTE_MS)],
            RemoteResult::ClockSet | RemoteResult::ClockAfterTake | RemoteResult::ModeSet(_) => {
                [(1_320.0, NOTE_MS), (0.0, 0)]
            }
            _ => [(330.0, 3 * NOTE_MS), (0.0, 0)],
        }
    }
}

const NOTE_MS: u32 = 110;
/// In the units of the app's output. On the X8 the host's gain of 8 makes this about -16 dBFS.
const BEEP_AMPLITUDE: f32 = 0.02;
/// Each note's rise and fall, so it does not click.
const BEEP_RAMP_MS: u32 = 5;

/// The latest phone remote message read, for the phone remote panel on the Record tab.
#[derive(Clone, Copy)]
pub(crate) struct LastCommand {
    pub(crate) message: RemoteMessage,
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
    /// The phone code, as 6 ASCII digits: made the first time the Phone code switch is turned
    /// on, and kept until the host is turned off.
    pub(crate) code: Option<[u8; REMOTE_CODE_DIGITS]>,
    /// Only commands with a check made from the code are acted on.
    pub(crate) code_required: bool,
    /// A hash of input samples, which the code is made from: the host has no random numbers,
    /// and the inputs' noise differs each time.
    noise: u32,
    /// The time in the last command with a check that was acted on.
    last_coded_unix: Option<u32>,
    /// The latest chime's time, not yet passed on, and whether it came with a clock command.
    reading: Option<(Reading, bool)>,
    pending: Option<RecorderCommand>,
    pending_mode: Option<ListeningMode>,
    /// A start command's choice for the Mute while recording switch, not yet passed on.
    pending_mute: Option<bool>,
    /// A clock command was read, and its outcome has not been reported yet.
    pub(crate) clock_pending: bool,
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
            code: None,
            code_required: false,
            noise: 0,
            last_coded_unix: None,
            reading: None,
            pending: None,
            pending_mode: None,
            pending_mute: None,
            clock_pending: false,
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

    /// Turn the Phone code switch on or off at sample `now`. The first time it is turned on,
    /// make the code from the hash of the input so far and the time of the press. Not inlined:
    /// the wheel, a tap and the host each call it.
    #[inline(never)]
    pub(crate) fn set_code_required(&mut self, required: bool, now: u64) {
        self.code_required = required;
        if required && self.code.is_none() {
            // MurmurHash3's last step, so that every bit of the hash changes the code.
            let mut hash = self.noise ^ now as u32;
            hash = (hash ^ hash >> 16).wrapping_mul(0x85eb_ca6b);
            hash = (hash ^ hash >> 13).wrapping_mul(0xc2b2_ae35);
            hash ^= hash >> 16;
            // Each digit is the whole part of 10 times a fraction, which starts as the hash
            // over 2^32.
            let mut code = [0; REMOTE_CODE_DIGITS];
            for digit in &mut code {
                let tenfold = hash as u64 * 10;
                *digit = b'0' + (tenfold >> 32) as u8;
                hash = tenfold as u32;
            }
            self.code = Some(code);
        }
    }

    /// The code to show: while the code is required.
    pub(crate) fn code_shown(&self) -> Option<[u8; REMOTE_CODE_DIGITS]> {
        if self.code_required { self.code } else { None }
    }

    /// Add 1 sample in 64 of a block of input to the hash the code is made from, until the
    /// code is made. Inlined, because it runs in the app's processing, which the X8 patch runs
    /// from ITCM.
    #[inline(always)]
    pub(crate) fn add_noise(&mut self, block: &[f32]) {
        if self.code.is_none() {
            for sample in block.iter().step_by(64) {
                self.noise = (self.noise ^ sample.to_bits()).wrapping_mul(0x0100_0193);
            }
        }
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
    pub(crate) fn listen(
        &mut self,
        decoders: &mut [MaybeUninit<Decoder>; 2],
        blocks: [&[f32]; 2],
        channels: usize,
        now: u64,
    ) {
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
                let remote = message.remote();
                let same_time = self.last_unix == message.unix_time;
                if let Some(remote) = remote {
                    let same_chime = same_time && self.last.is_some_and(|last| last.message == remote);
                    self.last = Some(LastCommand { message: remote, read_at: now, level_dbfs: chime.level_dbfs });
                    self.last_unix = message.unix_time;
                    if same_chime {
                        return;
                    }
                    // Stop listening once the other input has had time to finish this chime.
                    self.listen_until = self.listen_until.min(now + self.rate_hz as u64 / 4);
                }
                let command = remote.and_then(RemoteMessage::command);
                if let Some(refused) = self.refusal(remote, &message) {
                    // A chime that is refused, or has no command, gives no time or location.
                    if remote.is_some() {
                        self.report(if command.is_some() { refused } else { RemoteResult::Unknown }, now);
                    }
                    return;
                }
                if !same_time {
                    // The decoder counts samples from when it was built; the time since the
                    // chime started is the same in both counts.
                    let start_sample = now as i64 - (chime.end_sample as i64 - chime.start_sample);
                    let sets_clock = command == Some(RemoteCommand::SetClock);
                    self.reading = Some((Reading::new(&message, start_sample, now), sets_clock));
                }
                if remote.is_some() {
                    match command {
                        Some(RemoteCommand::Start) => self.pending = Some(RecorderCommand::Start),
                        Some(RemoteCommand::StartWithMute(mute)) => {
                            self.pending_mute = Some(mute);
                            self.pending = Some(RecorderCommand::Start);
                        }
                        Some(RemoteCommand::Stop) => self.pending = Some(RecorderCommand::Stop),
                        Some(RemoteCommand::SetClock) => self.clock_pending = true,
                        Some(RemoteCommand::Listen(mode)) => self.pending_mode = Some(mode),
                        None => self.report(RemoteResult::Unknown, now),
                    }
                }
            }
            Event::Unreadable(_) => self.chimes_failed += 1,
        }
    }

    /// Why a chime is not used, while the code is required: it has no phone command, the
    /// command has no check, the check was made from another code, or the chime's time is not
    /// later than the time of the last command acted on. None for a chime to use, whose time
    /// is then kept for the next one. Not inlined, so the check's hash stays out of ITCM.
    #[inline(never)]
    fn refusal(&mut self, remote: Option<RemoteMessage>, message: &TimeMessage) -> Option<RemoteResult> {
        if !self.code_required {
            return None;
        }
        let (Some(remote), Some(code)) = (remote, self.code) else { return Some(RemoteResult::CodeNeeded) };
        if remote.check().is_none() {
            return Some(RemoteResult::CodeNeeded);
        }
        if !remote.check_matches(&code, message.unix_time, message.timezone_minutes) {
            return Some(RemoteResult::CodeWrong);
        }
        if self.last_coded_unix.is_some_and(|last| message.unix_time <= last) {
            return Some(RemoteResult::CodeOld);
        }
        self.last_coded_unix = Some(message.unix_time);
        None
    }

    /// The latest chime's time, once, and whether its chime was a clock command.
    pub(crate) fn take_reading(&mut self) -> Option<(Reading, bool)> {
        self.reading.take()
    }

    /// A command from the app itself (the CPU test), for the host to carry out as it does the
    /// phone's.
    pub(crate) fn command_from_app(&mut self, command: RecorderCommand) {
        self.pending = Some(command);
    }

    pub(crate) fn take_command(&mut self) -> Option<RecorderCommand> {
        self.pending.take()
    }

    pub(crate) fn take_mute(&mut self) -> Option<bool> {
        self.pending_mute.take()
    }

    pub(crate) fn take_mode(&mut self) -> Option<ListeningMode> {
        self.pending_mode.take()
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
