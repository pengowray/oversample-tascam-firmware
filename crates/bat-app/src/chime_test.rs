//! The clock screen's listening: while the screen is open, a decoder runs on each block of
//! input, and this keeps what they read. Setting the clock is up to the host.

use core::mem::MaybeUninit;

use chime::format::{LocalTime, TimeMessage};
use chime::{Decoder, Event, UnreadableReason};

use crate::phone_time::Reading;

/// What one block of input has given since the test started.
#[derive(Clone, Copy)]
pub(crate) struct Channel {
    pub(crate) read: u32,
    pub(crate) failed: u32,
    /// The level of the carrier during the latest chime read, in dBFS.
    pub(crate) carrier_dbfs: Option<f32>,
    /// The SNR of that chime in dB, NaN if the noise was not measured.
    pub(crate) snr_db: Option<f32>,
    /// The level in the 17 to 19 kHz band now, in dBFS.
    pub(crate) band_dbfs: f32,
}

/// Why a chime was not read.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum FailureReason {
    Decoder(UnreadableReason),
    /// It passed its CRC, but input frames were lost during it, so its length in samples
    /// does not tell how long ago it started, and its time would be wrong.
    FramesLost,
}

/// The latest chime that could not be read. A later chime read clears it.
#[derive(Clone, Copy)]
pub(crate) struct Failure {
    pub(crate) channel: usize,
    pub(crate) reason: FailureReason,
    pub(crate) bytes: usize,
}

/// The time now, going by the latest chime read.
#[derive(Clone, Copy)]
pub(crate) struct ChimeClock {
    pub(crate) local: LocalTime,
    pub(crate) timezone_minutes: i16,
    /// Seconds since the decoder finished reading that chime.
    pub(crate) read_s_ago: u64,
}

pub(crate) struct ChimeTest {
    rate_hz: u32,
    /// Whether the decoders in `Buffers` have been built.
    ready: bool,
    pub(crate) channels: [Channel; 2],
    reading: Option<Reading>,
    /// `reading` has not been passed on yet.
    fresh: bool,
    pub(crate) failure: Option<Failure>,
    /// The app's sample count just after the latest lost frames.
    last_jump: u64,
}

const NOTHING_YET: Channel = Channel { read: 0, failed: 0, carrier_dbfs: None, snr_db: None, band_dbfs: -120.0 };

impl ChimeTest {
    pub(crate) const fn new() -> Self {
        Self { rate_hz: 0, ready: false, channels: [NOTHING_YET; 2], reading: None, fresh: false, failure: None, last_jump: 0 }
    }

    /// Forget everything and build new decoders, whose sample counts start from 0.
    #[inline(never)]
    pub(crate) fn restart(&mut self, decoders: &mut [MaybeUninit<Decoder>; 2], rate_hz: u32) {
        *self = Self::new();
        self.rate_hz = rate_hz;
        self.ready = decoders.iter_mut().all(|slot| Decoder::init_in_place(slot, rate_hz).is_some());
    }

    /// Input frames were lost, and the app's sample count has moved on to `now`.
    pub(crate) fn input_jumped(&mut self, now: u64) {
        self.last_jump = now;
    }

    /// Feed the first `channels` blocks to their decoders. `sample` is the app's count at
    /// the start of the blocks. Not inlined, so the decoders stay out of `BatApp::process`,
    /// which the X8 patch runs from ITCM, where they would not fit.
    #[inline(never)]
    pub(crate) fn listen(&mut self, decoders: &mut [MaybeUninit<Decoder>; 2], blocks: [&[f32]; 2], channels: usize, sample: u64) {
        if !self.ready {
            return;
        }
        for (channel, (slot, block)) in decoders.iter_mut().zip(blocks).enumerate().take(channels) {
            // SAFETY: `ready` is set only after `restart` has built both decoders.
            let decoder = unsafe { slot.assume_init_mut() };
            decoder.push_block(block, |i, event| self.record(channel, event, sample + i as u64));
            self.channels[channel].band_dbfs = decoder.band_level_dbfs();
        }
    }

    fn record(&mut self, channel: usize, event: Event, now: u64) {
        match event {
            Event::Chime(chime) => {
                // The decoder's samples are counted from when it was built, and frames lost
                // since then are not in its count. The time since the chime started is the
                // same in both counts, unless frames were lost during the chime.
                let since_start = chime.end_sample as i64 - chime.start_sample;
                let start_sample = now as i64 - since_start;
                if start_sample < self.last_jump as i64 {
                    self.channels[channel].failed += 1;
                    self.failure = Some(Failure { channel, reason: FailureReason::FramesLost, bytes: chime.payload_len });
                    return;
                }
                let results = &mut self.channels[channel];
                results.read += 1;
                results.carrier_dbfs = Some(chime.level_dbfs);
                results.snr_db = Some(chime.snr_db);
                self.failure = None;
                if let Some(message) = TimeMessage::parse(&chime.payload[..chime.payload_len]) {
                    // The same chime on the other input gives the same time: pass it on once.
                    let repeat = self.reading.is_some_and(|last| last.unix_time == message.unix_time);
                    self.reading = Some(Reading::new(&message, start_sample, now));
                    self.fresh |= !repeat;
                }
            }
            Event::Unreadable(unreadable) => {
                self.channels[channel].failed += 1;
                let reason = FailureReason::Decoder(unreadable.reason);
                self.failure = Some(Failure { channel, reason, bytes: unreadable.bytes_received });
            }
        }
    }

    /// The latest reading, once: the app passes it on to the host.
    pub(crate) fn take_reading(&mut self) -> Option<Reading> {
        core::mem::take(&mut self.fresh).then_some(self.reading).flatten()
    }

    /// The time at sample `now`, if a chime has been read.
    pub(crate) fn clock(&self, now: u64) -> Option<ChimeClock> {
        let reading = self.reading?;
        let rate = self.rate_hz as i64;
        let elapsed = (now as i64 - reading.start_sample).div_euclid(rate);
        let local = reading.unix_time as i64 + reading.timezone_minutes as i64 * 60 + elapsed;
        Some(ChimeClock {
            local: LocalTime::from_epoch_seconds(local),
            timezone_minutes: reading.timezone_minutes,
            read_s_ago: now.saturating_sub(reading.end_sample) / self.rate_hz as u64,
        })
    }
}
