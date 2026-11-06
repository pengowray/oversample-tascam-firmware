//! Streaming chime decoder.
//!
//! The decoder has four stages:
//!
//! 1. The front end mixes the input down from 18 kHz to complex baseband, low-pass
//!    filters it and decimates it to about 8 kHz.
//! 2. The detector compares the signal with itself 7.5 ms earlier. During the start
//!    symbols the two have opposite polarity whatever the symbol timing is. This
//!    comparison is cheap and has many false alarms in noise.
//! 3. The timing search adds up the last 50 ms over the 7.5 ms symbol period to find
//!    the symbol boundaries. Its score rejects the false alarms of the detector.
//! 4. The trellis walks the message on a grid of 2.5 ms buckets, because every symbol
//!    length is a multiple of 2.5 ms.
//!
//! # Why a trellis
//!
//! Deciding each symbol as it arrives is not enough. A wrong decision on a symbol
//! length adds or removes a symbol, which shifts all later bits by one, and the
//! Hamming code cannot correct that. So the trellis keeps the best path to each
//! position in a symbol, for each symbol of a byte, and only the stop symbols and the
//! CRC choose among the paths. This gains about 5 dB over deciding each symbol.
//!
//! Each path carries its own phase reference and its own decoded bytes, which keeps
//! the code small: there is no traceback, and there are no tentative decisions to undo.
//!
//! # Tracking
//!
//! Three things change during a chime, and the best path's decisions drive a tracker
//! for each:
//!
//! - The carrier frequency, because of clock errors and a moving phone.
//! - The symbol timing, because of clock errors and because the encoder rounds symbol
//!   lengths up to whole samples.
//! - The echoes. In a room, each reflection arrives with its own delay and phase. After
//!   a polarity flip the buckets therefore hold a mix of old and new polarity for up
//!   to tens of milliseconds, and a reference for the signal alone fails even at high
//!   SNR. An adaptive filter learns the echoes from the data, and each path removes
//!   the echoes that its own past polarities would cause.
//!
//! # Cost
//!
//! The struct holds about 11 KB and nothing is allocated. The front end costs 47
//! floating-point operations per input sample. The rest costs about 14 per baseband
//! sample while listening and about 170 per baseband sample during a message, which at
//! 192 kHz is 0.6 and 7 per input sample. The largest cost in one call of `push` is
//! the timing search, at about 6000 operations. One trellis step is about 3100.

use core::ops::{Add, AddAssign, Mul, Neg, Sub};

use bat_dsp::biquad::Butterworth4;

use crate::format::{
    self, CARRIER_HZ, CRC_BYTES, MAX_MESSAGE_BYTES, MAX_PAYLOAD_BYTES, START_STOP_US,
    START_SYMBOLS, STOP_SYMBOLS, SYMBOLS_PER_BYTE,
};
use crate::{Chime, Event, Unreadable, UnreadableReason};

// -- Front end ---------------------------------------------------------------

/// Lowest sample rate the decoder accepts, in Hz. The 18 kHz carrier and its upper
/// sideband must fit below half the sample rate.
const MIN_SAMPLE_RATE_HZ: u32 = 40_000;

const TARGET_BASEBAND_HZ: f64 = 8_000.0;

/// Cutoff of the 4th-order Butterworth low-pass filter on I and on Q, in Hz.
const FILTER_CUTOFF_HZ: f32 = 1_000.0;

/// Delay of the symbol edges through that filter, in seconds. It is the sum of
/// 1 / (Q * 2 pi * cutoff) over the filter's two sections.
const FILTER_DELAY_S: f64 = 0.000_416;

/// Added to the mixer output so that the filter states do not decay to subnormal
/// numbers during digital silence, which are slow on some processors.
const SUBNORMAL_GUARD: f32 = 1e-20;

/// Samples larger than this are treated as faults and replaced by 0, as are NaNs.
/// One NaN would otherwise stay in the filter states for ever.
const MAX_SAMPLE: f32 = 1e15;

// -- Baseband ring -----------------------------------------------------------

/// Baseband samples kept for the timing search and for the first buckets of a walk.
/// This is a power of two so that an index wraps with a mask. It holds 58 ms at the
/// highest possible baseband rate, 8.8 kHz.
const RING_LEN: usize = 512;
const RING_MASK: usize = RING_LEN - 1;

// -- Detector ----------------------------------------------------------------

/// The detector works on sums over 2.5 ms, taken every 1.25 ms. Each sum has a
/// fifth of the noise power of a baseband sample in the 2 kHz band, which keeps rho
/// large at low SNR.
const HALF_BUCKET_S: f64 = 0.001_25;

/// Number of 1.25 ms steps in the 7.5 ms lag.
const LAG_STEPS: usize = 6;

/// Number of 1.25 ms steps in one detector block of 5 ms.
const STEPS_PER_BLOCK: u32 = 4;

/// Number of 5 ms blocks in the detector's window of 50 ms.
const WINDOW_BLOCKS: usize = 10;

/// rho above this starts a timing search. The value is low because the timing
/// search rejects false detections.
const RHO_TRIGGER: f32 = 0.33;

/// rho below this in size means that the window has no chime or tone in it, so its
/// power counts as the noise floor.
const RHO_QUIET: f32 = 0.25;

/// Lowest baseband power per sample for a detection. It corresponds to a carrier of
/// about -127 dBFS RMS.
const POWER_FLOOR: f32 = 1e-13;

/// Number of 5 ms blocks between the trigger and the timing search, so that the
/// window holds several symbols of a loud chime that triggers early.
const SETTLE_BLOCKS: u32 = 6;

/// Weight of each new block in the noise floor estimates, about 100 ms.
const NOISE_WEIGHT: f32 = 0.05;

/// Lowest weight of each new block in the band level estimate, about 1 s.
const BAND_WEIGHT: f32 = 0.005;

/// Number of 5 ms blocks during which no new detection starts after a walk ends.
/// The last stop symbols and their reverb look like start symbols.
const HOLDOFF_AFTER_CHIME_BLOCKS: u32 = 60;
const HOLDOFF_AFTER_BAD_CHIME_BLOCKS: u32 = 40;

// -- Timing search -----------------------------------------------------------

/// Length of the window that the timing search folds, in seconds.
const FOLD_WINDOW_S: f64 = 0.050;

/// One slot for each baseband sample in a 7.5 ms period at 8.8 kHz.
const MAX_FOLD_SLOTS: usize = 67;

/// Lowest coherence that lets a walk start. Coherence is the power of the folded
/// signal over the power of all samples in the window, as a fraction of what a
/// clean run of start symbols gives.
const COHERENCE_GATE: f32 = 0.12;

/// The walk starts this many start symbols before the newest sample.
const WALK_BACK_SYMBOLS: f64 = 5.0;

// -- Trellis -----------------------------------------------------------------

const BUCKET_S: f64 = 0.0025;

/// Trellis states. Each state is a position in a symbol, and a path in a state has
/// just consumed the bucket at that position. S0 to S2 are the three buckets of a
/// start symbol and T0 to T2 those of a stop symbol. A data symbol has up to four
/// buckets, and the data states also record which symbol of its byte it is, because
/// a path can only stop at the end of a byte. A path that gains or loses a symbol
/// through noise is therefore in a different state from the correct path, and the
/// two never compete. Only the stop symbols and the CRC can choose between them.
const S0: usize = 0;
const S1: usize = 1;
const S2: usize = 2;
const T0: usize = 3;
const T1: usize = 4;
const T2: usize = 5;
const FIRST_DATA_STATE: usize = 6;
const STATES: usize = FIRST_DATA_STATE + SYMBOLS_PER_BYTE * 4;

/// The state for bucket `bucket` of symbol `symbol` of a byte.
const fn data_state(symbol: usize, bucket: usize) -> usize {
    FIRST_DATA_STATE + symbol * 4 + bucket
}

/// Weight of the older buckets in a path's phase reference, for each new bucket. A
/// short memory follows a moving phone better, and when the signal is clean it costs
/// nothing. A long memory gives a better reference when noise or echoes disturb the
/// buckets. Both of those lower the coherence that the timing search finds, so the
/// memory depends on that. 0.85 is about 7 buckets, or 17 ms, and 0.6 is about 2.5
/// buckets, or 6 ms.
const LONG_REFERENCE_MEMORY: f32 = 0.85;
const SHORT_REFERENCE_MEMORY: f32 = 0.6;
const COHERENCE_FOR_LONG_MEMORY: f32 = 0.72;
const COHERENCE_FOR_SHORT_MEMORY: f32 = 0.84;

/// A walk must confirm this many start symbols.
const MIN_START_SYMBOLS: u8 = 4;

/// The walk checks that the start symbols are there at this bucket.
const START_CHECK_BUCKET: u32 = 15;

/// A path with more start symbols than this is not reading a chime.
const MAX_START_SYMBOLS: u8 = START_SYMBOLS as u8 + 8;

/// A path must hold this many stop symbols before its CRC decides the message.
const STOP_SYMBOLS_TO_ACCEPT: u8 = 4;

/// Bytes a message needs before stop symbols can end it: the CRC and one more byte.
const MIN_BYTES_BEFORE_STOP: usize = CRC_BYTES + 1;

/// No walk lasts longer than this. The longest possible chime is 16 start symbols, 23
/// bytes of data symbols of 10 ms and 8 stop symbols, or about 3.4 s.
const MAX_WALK_BUCKETS: u32 = 1400;

/// Weight of each new bucket in the average bucket power, about 80 ms.
const POWER_WEIGHT: f32 = 1.0 / 32.0;

/// A bucket can raise the average as if its power were this many times the average,
/// so that a click does not raise the average for long.
const POWER_OUTLIER_LIMIT: f32 = 4.0;

/// The average bucket power is taken as the chime's level at this bucket.
const LEVEL_REFERENCE_BUCKET: u32 = 64;

/// A path can finish its stop symbols only if the average bucket power is above this
/// fraction of the chime's level. The average falls by about half in 60 ms.
const STOP_POWER_FRACTION: f32 = 0.6;

/// The signal counts as lost when the average bucket power falls below this fraction
/// of the chime's level, measured above the noise floor.
const LOST_FRACTION: f32 = 0.05;

// -- Timing tracking ---------------------------------------------------------

/// Weight of the older buckets in the timing error, for each new bucket.
const TIMING_MEMORY: f32 = 0.9;

/// Fraction of the timing error corrected at each polarity flip. At -5 dB SNR one flip
/// measures the timing to about 5 samples. This gain brings that to about 1 sample (125
/// us), and it follows the 44.1 kHz encoder's drift of 0.09 samples per data 0 with a
/// lag of about 2 samples at most.
const TIMING_GAIN: f32 = 0.05;

/// Largest correction at one flip, in baseband samples, so that a click does not move
/// the grid far.
const TIMING_MAX_STEP: f32 = 0.25;

// -- Carrier tracking --------------------------------------------------------

/// Change of the carrier frequency estimate, in radians per bucket, for a phase error
/// of 1 radian in the newest bucket, as a fraction of (1 - reference memory). The
/// phase error is scaled by (1 - reference memory). A loop with the path's reference
/// is critically damped when its gain is (1 - reference memory)^2 / 4.
const FREQUENCY_GAIN_FRACTION: f32 = 0.25;

/// Largest carrier frequency estimate, in radians per bucket: about 50 Hz.
const MAX_FREQUENCY: f32 = 0.8;

// -- Echoes ------------------------------------------------------------------

/// Number of earlier buckets whose echoes the trellis models: 30 ms.
const ECHO_TAPS: usize = 12;

/// Step size of the adaptive filter that learns the echoes, for each new bucket.
const ECHO_STEP: f32 = 0.04;

/// Step size for the filter's first tap, the direct signal, which must follow the
/// phase of the carrier.
const DIRECT_TAP_STEP: f32 = 0.15;

/// Noise power in each tap of that filter, as a fraction of the power of its error.
const ECHO_TAP_NOISE: f32 = 0.6 * ECHO_STEP;

/// Taps with less power than this many times the noise in a tap are set to zero, and
/// the power of larger taps is reduced by that amount.
const ECHO_TAP_MARGIN: f32 = 2.0;

/// Taps below this power relative to the direct signal are set to zero whatever the
/// noise is. At high SNR the filter's error is not noise but small differences between
/// the real symbol shapes and the model, and these make the taps wander.
const ECHO_TAP_FLOOR: f32 = 0.004;

/// The filter's output is used only when the power of its error is below this fraction
/// of the error of the direct signal alone, so only when the filter finds something in
/// the signal that it can predict. Without this test, the noise in the taps costs more
/// than 2 dB of sensitivity when there are no echoes, and when clipping makes the
/// carrier's amplitude follow the melody, the taps wander and break the message.
const ECHO_USEFUL_FRACTION: f32 = 0.8;

/// Weight of each new bucket in the power of the filter's error.
const ECHO_ERROR_WEIGHT: f32 = 0.02;

// -- Levels ------------------------------------------------------------------

/// Carrier power over the mean bucket power for a clean chime. The buckets at the
/// edges of a symbol hold its ramps, and the filter spreads the symbol edges.
const LEVEL_CORRECTION: f32 = 1.043;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cplx {
    re: f32,
    im: f32,
}

impl Cplx {
    const ZERO: Cplx = Cplx { re: 0.0, im: 0.0 };

    fn power(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    /// The real part of self * conj(other).
    fn dot(self, other: Cplx) -> f32 {
        self.re * other.re + self.im * other.im
    }

    /// self * conj(other).
    fn mul_conj(self, other: Cplx) -> Cplx {
        Cplx {
            re: self.re * other.re + self.im * other.im,
            im: self.im * other.re - self.re * other.im,
        }
    }

    /// self * other.
    fn mul(self, other: Cplx) -> Cplx {
        Cplx {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

impl Add for Cplx {
    type Output = Cplx;
    fn add(self, other: Cplx) -> Cplx {
        Cplx { re: self.re + other.re, im: self.im + other.im }
    }
}

impl AddAssign for Cplx {
    fn add_assign(&mut self, other: Cplx) {
        self.re += other.re;
        self.im += other.im;
    }
}

impl Sub for Cplx {
    type Output = Cplx;
    fn sub(self, other: Cplx) -> Cplx {
        Cplx { re: self.re - other.re, im: self.im - other.im }
    }
}

impl Neg for Cplx {
    type Output = Cplx;
    fn neg(self) -> Cplx {
        Cplx { re: -self.re, im: -self.im }
    }
}

impl Mul<f32> for Cplx {
    type Output = Cplx;
    fn mul(self, gain: f32) -> Cplx {
        Cplx { re: self.re * gain, im: self.im * gain }
    }
}

fn round_to_u64(position: f64) -> u64 {
    (position + 0.5) as u64
}

fn power_to_db(power: f32) -> f32 {
    10.0 * libm::log10f(power.max(1e-12))
}

/// The echoes in a bucket from the earlier buckets of a path. Bit i of `history` is
/// the polarity of the bucket i + 1 buckets ago.
fn echo_of(taps: &[Cplx; ECHO_TAPS], history: u16) -> Cplx {
    let mut echo = Cplx::ZERO;
    for (index, &tap) in taps.iter().enumerate() {
        if (history >> index) & 1 == 1 {
            echo += tap;
        } else {
            echo = echo - tap;
        }
    }
    echo
}

/// One path through the trellis, with everything it has decoded so far.
#[derive(Clone, Copy)]
struct Path {
    /// Sum of the agreement of each bucket with the path's reference. Larger is
    /// better. Only differences between paths mean anything.
    metric: f32,
    /// Decaying sum of the path's buckets with the polarity of the path's current
    /// symbol. The phase of this sum is the phase the next bucket should have.
    reference: Cplx,
    /// Polarity of the path's latest buckets, newest in bit 0.
    history: u16,
    start_symbols: u8,
    stop_symbols: u8,
    /// Distance in baseband samples from the start of the walk to the first data
    /// symbol. Negative while the path has no data symbols.
    data_start: f32,
    /// The two Hamming codewords of the current byte, low nibble first.
    codewords: [u8; 2],
    /// Complete bytes, CRC included. One more than `MAX_MESSAGE_BYTES` means that
    /// the message is too long, and that byte is not stored.
    byte_count: u8,
    corrected_bits: u16,
    bytes: [u8; MAX_MESSAGE_BYTES],
}

impl Path {
    const DEAD: Path = Path {
        metric: f32::NEG_INFINITY,
        reference: Cplx::ZERO,
        history: 0,
        start_symbols: 0,
        stop_symbols: 0,
        data_start: -1.0,
        codewords: [0; 2],
        byte_count: 0,
        corrected_bits: 0,
        bytes: [0; MAX_MESSAGE_BYTES],
    };

    /// True when the data symbol that finishes this path's current byte can be the
    /// last data symbol of a message.
    fn can_stop_after_this_byte(&self) -> bool {
        let bytes_after = self.byte_count as usize + 1;
        (MIN_BYTES_BEFORE_STOP..=MAX_MESSAGE_BYTES).contains(&bytes_after)
    }

    /// Records a finished data symbol, number `symbol` of its byte, as a bit.
    fn push_bit(&mut self, symbol: usize, bit: u8) {
        self.codewords[symbol % 2] |= bit << (symbol / 2);
        if symbol == SYMBOLS_PER_BYTE - 1 {
            let (low, low_corrected) = format::hamming_decode(self.codewords[0]);
            let (high, high_corrected) = format::hamming_decode(self.codewords[1]);
            if (self.byte_count as usize) < MAX_MESSAGE_BYTES {
                self.bytes[self.byte_count as usize] = (high << 4) | low;
            }
            self.byte_count = self.byte_count.saturating_add(1);
            self.corrected_bits += low_corrected as u16 + high_corrected as u16;
            self.codewords = [0; 2];
        }
    }

    /// Writes this path, moved on by one bucket, to `next`. `metric` is the metric of
    /// the move. `flipped` means that the bucket is the first of a new symbol.
    fn extend_into(&self, next: &mut Path, bucket: Cplx, metric: f32, flipped: bool, memory: f32) {
        if metric == f32::NEG_INFINITY {
            next.metric = metric;
            return;
        }
        *next = *self;
        next.metric = metric;
        let polarity = if flipped { -self.reference } else { self.reference };
        next.reference = polarity * memory + bucket;
        next.history = (self.history << 1) | ((self.history & 1) ^ flipped as u16);
    }

    /// True when the bytes end with the correct CRC of the bytes before it.
    fn crc_is_good(&self) -> bool {
        let count = self.byte_count as usize;
        if !(CRC_BYTES + 1..=MAX_MESSAGE_BYTES).contains(&count) {
            return false;
        }
        // The CRC of zeros is zero, so a message of only zeros always passes. A path
        // that takes noise for short symbols produces just that.
        if self.bytes[..count].iter().all(|&byte| byte == 0) {
            return false;
        }
        let payload_len = count - CRC_BYTES;
        let crc = format::crc16(&self.bytes[..payload_len]);
        self.bytes[payload_len] == (crc & 0xFF) as u8 && self.bytes[payload_len + 1] == (crc >> 8) as u8
    }
}

/// How a walk ended.
#[derive(Clone, Copy)]
enum WalkEnd {
    Message(usize),
    Failed(UnreadableReason, usize),
    Silent,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Listening,
    /// A trigger fired, and the timing search runs when this many blocks have passed.
    Settling(u32),
    Walking,
}

/// Streaming chime decoder for one channel of audio.
pub struct Decoder {
    // Front end.
    mixer: Cplx,
    mixer_step: Cplx,
    filter_i: Butterworth4,
    filter_q: Butterworth4,
    decimation: u32,
    decimation_count: u32,
    input_samples: u64,
    sample_rate: f64,
    baseband_rate: f64,

    // Ring of baseband samples.
    ring: [Cplx; RING_LEN],
    baseband_samples: u64,

    // Detector.
    mode: Mode,
    half_bucket_len: u32,
    half_bucket_count: u32,
    half_bucket_sum: Cplx,
    previous_half_bucket: Cplx,
    lag_sums: [Cplx; LAG_STEPS],
    lag_index: usize,
    steps_in_block: u32,
    block_p: f32,
    block_q: f32,
    block_e: f32,
    block_band: f32,
    window_p: [f32; WINDOW_BLOCKS],
    window_q: [f32; WINDOW_BLOCKS],
    window_e: [f32; WINDOW_BLOCKS],
    window_band: [f32; WINDOW_BLOCKS],
    window_index: usize,
    holdoff_blocks: u32,

    // Level estimates. Both noise floors and `band_power` are baseband power per
    // sample. `noise_bucket` is the same for a sum over 2.5 ms, divided by the
    // squared number of samples in the sum.
    band_power: f32,
    band_blocks: u32,
    noise_band: f32,
    noise_bucket: f32,
    noise_blocks: u32,

    // Timing search.
    fold_slots: [Cplx; MAX_FOLD_SLOTS],

    // Walk.
    grid_edge: f64,
    walk_start: f64,
    bucket_index: u32,
    previous_half: Cplx,
    reference_memory: f32,
    carrier_phasor: Cplx,
    carrier_frequency: f32,
    /// The adaptive filter's raw estimates of the echoes.
    echo_estimates: [Cplx; ECHO_TAPS],
    /// The same estimates with noise reduced, as the trellis uses them.
    echo_taps: [Cplx; ECHO_TAPS],
    echo_error_power: f32,
    direct_error_power: f32,
    direct_tap: Cplx,
    timing_error: f32,
    timing_power: f32,
    path_buffers: [[Path; STATES]; 2],
    /// Which of `path_buffers` holds the current paths.
    current_paths: usize,
    power_average: f32,
    level_reference: f32,
    level_sum: f32,
    level_count: u32,
}

impl Decoder {
    /// Returns `None` below 40 kHz.
    pub fn new(sample_rate_hz: u32) -> Option<Decoder> {
        if sample_rate_hz < MIN_SAMPLE_RATE_HZ {
            return None;
        }
        let rate = sample_rate_hz as f64;
        let decimation = libm::round(rate / TARGET_BASEBAND_HZ) as u32;
        let baseband_rate = rate / decimation as f64;
        let filter = Butterworth4::lowpass(sample_rate_hz as f32, FILTER_CUTOFF_HZ);
        let step = 2.0 * core::f64::consts::PI * CARRIER_HZ as f64 / rate;
        Some(Decoder {
            mixer: Cplx { re: 1.0, im: 0.0 },
            mixer_step: Cplx { re: libm::cos(step) as f32, im: libm::sin(step) as f32 },
            filter_i: filter,
            filter_q: filter,
            decimation,
            decimation_count: 0,
            input_samples: 0,
            sample_rate: rate,
            baseband_rate,
            ring: [Cplx::ZERO; RING_LEN],
            baseband_samples: 0,
            mode: Mode::Listening,
            half_bucket_len: libm::round(HALF_BUCKET_S * baseband_rate) as u32,
            half_bucket_count: 0,
            half_bucket_sum: Cplx::ZERO,
            previous_half_bucket: Cplx::ZERO,
            lag_sums: [Cplx::ZERO; LAG_STEPS],
            lag_index: 0,
            steps_in_block: 0,
            block_p: 0.0,
            block_q: 0.0,
            block_e: 0.0,
            block_band: 0.0,
            window_p: [0.0; WINDOW_BLOCKS],
            window_q: [0.0; WINDOW_BLOCKS],
            window_e: [0.0; WINDOW_BLOCKS],
            window_band: [0.0; WINDOW_BLOCKS],
            window_index: 0,
            holdoff_blocks: 0,
            band_power: 0.0,
            band_blocks: 0,
            noise_band: 0.0,
            noise_bucket: 0.0,
            noise_blocks: 0,
            fold_slots: [Cplx::ZERO; MAX_FOLD_SLOTS],
            grid_edge: 0.0,
            walk_start: 0.0,
            bucket_index: 0,
            previous_half: Cplx::ZERO,
            reference_memory: LONG_REFERENCE_MEMORY,
            carrier_phasor: Cplx { re: 1.0, im: 0.0 },
            carrier_frequency: 0.0,
            echo_estimates: [Cplx::ZERO; ECHO_TAPS],
            echo_taps: [Cplx::ZERO; ECHO_TAPS],
            echo_error_power: 0.0,
            direct_error_power: 0.0,
            direct_tap: Cplx::ZERO,
            timing_error: 0.0,
            timing_power: 0.0,
            path_buffers: [[Path::DEAD; STATES]; 2],
            current_paths: 0,
            power_average: 0.0,
            level_reference: 0.0,
            level_sum: 0.0,
            level_count: 0,
        })
    }

    /// Feeds one sample of any scale, normally +-1.0. Returns an event when a chime
    /// ends.
    pub fn push(&mut self, sample: f32) -> Option<Event> {
        self.input_samples += 1;
        let sample = if sample.abs() < MAX_SAMPLE { sample } else { 0.0 };

        // Multiplying by exp(-j w t) moves the carrier to 0 Hz and halves its
        // amplitude. The term at -36 kHz does not pass the filters.
        let i = self.filter_i.process_sample(sample * self.mixer.re + SUBNORMAL_GUARD);
        let q = self.filter_q.process_sample(SUBNORMAL_GUARD - sample * self.mixer.im);
        self.mixer = self.mixer.mul(self.mixer_step);

        self.decimation_count += 1;
        if self.decimation_count < self.decimation {
            return None;
        }
        self.decimation_count = 0;

        // Without this the mixer's amplitude would drift through rounding. Near 1
        // this is the first-order form of dividing by the square root of the power.
        self.mixer = self.mixer * (0.5 * (3.0 - self.mixer.power()));

        self.push_baseband(Cplx { re: i, im: q })
    }

    /// Slow-moving RMS level of the 17 to 19 kHz band, in dBFS.
    pub fn band_level_dbfs(&self) -> f32 {
        power_to_db(2.0 * self.band_power).max(-120.0)
    }

    fn push_baseband(&mut self, z: Cplx) -> Option<Event> {
        self.ring[self.baseband_samples as usize & RING_MASK] = z;
        self.baseband_samples += 1;
        // At the start of a walk a few buckets are waiting. Taking one for each baseband
        // sample bounds the work in one call of push, and the walk soon catches up. A
        // walk that starts in this call waits for the next, because the timing search
        // is the larger cost.
        let was_walking = self.mode == Mode::Walking;
        self.update_detector(z);
        if was_walking
            && let Some(halves) = self.next_bucket()
            && let Some(end) = self.step_trellis(halves)
        {
            return self.finish_walk(end);
        }
        None
    }

    // -- Detector --------------------------------------------------------------

    fn update_detector(&mut self, z: Cplx) {
        self.half_bucket_sum += z;
        self.block_band += z.power();
        self.half_bucket_count += 1;
        if self.half_bucket_count < self.half_bucket_len {
            return;
        }
        self.half_bucket_count = 0;

        let sum = self.half_bucket_sum + self.previous_half_bucket;
        self.previous_half_bucket = self.half_bucket_sum;
        self.half_bucket_sum = Cplx::ZERO;
        let lagged = self.lag_sums[self.lag_index];
        self.lag_sums[self.lag_index] = sum;
        self.lag_index = (self.lag_index + 1) % LAG_STEPS;

        // The minus sign makes rho positive when the polarity has flipped.
        let product = sum.mul_conj(lagged);
        self.block_p -= product.re;
        self.block_q -= product.im;
        self.block_e += 0.5 * (sum.power() + lagged.power());

        self.steps_in_block += 1;
        if self.steps_in_block == STEPS_PER_BLOCK {
            self.steps_in_block = 0;
            self.close_detector_block();
        }
    }

    fn close_detector_block(&mut self) {
        let samples = (self.half_bucket_len * STEPS_PER_BLOCK) as f32;
        let samples_in_sum = (self.half_bucket_len * 2) as f32;
        let block_band = self.block_band / samples;

        // The oldest block in the window is the one about to be replaced. The noise
        // floor uses it because rho has had 50 ms to rise if a chime was starting.
        let oldest = self.window_index;
        let oldest_band = self.window_band[oldest] / samples;
        let oldest_bucket = self.window_e[oldest] / (STEPS_PER_BLOCK as f32 * samples_in_sum * samples_in_sum);

        self.window_p[oldest] = self.block_p;
        self.window_q[oldest] = self.block_q;
        self.window_e[oldest] = self.block_e;
        self.window_band[oldest] = self.block_band;
        self.window_index = (oldest + 1) % WINDOW_BLOCKS;
        self.block_p = 0.0;
        self.block_q = 0.0;
        self.block_e = 0.0;
        self.block_band = 0.0;

        self.band_blocks = self.band_blocks.saturating_add(1);
        let band_weight = (1.0 / self.band_blocks as f32).max(BAND_WEIGHT);
        self.band_power += (block_band - self.band_power) * band_weight;

        // Summing the window from its blocks each time means rounding errors from a
        // loud moment cannot stay in the sums.
        let sum_p: f32 = self.window_p.iter().sum();
        let sum_e: f32 = self.window_e.iter().sum();
        let window_power = sum_e / (WINDOW_BLOCKS as f32 * STEPS_PER_BLOCK as f32 * samples_in_sum * samples_in_sum);
        let rho = if window_power > POWER_FLOOR { sum_p / sum_e } else { 0.0 };

        if self.mode == Mode::Walking {
            return;
        }

        if rho.abs() < RHO_QUIET
            && window_power > POWER_FLOOR
            && oldest_band > 0.0
        {
            self.noise_blocks = self.noise_blocks.saturating_add(1);
            let weight = (1.0 / self.noise_blocks as f32).max(NOISE_WEIGHT);
            self.noise_band += (oldest_band - self.noise_band) * weight;
            self.noise_bucket += (oldest_bucket - self.noise_bucket) * weight;
        }

        match self.mode {
            Mode::Listening => {
                if self.holdoff_blocks > 0 {
                    self.holdoff_blocks -= 1;
                } else if rho >= RHO_TRIGGER {
                    self.mode = Mode::Settling(SETTLE_BLOCKS);
                }
            }
            Mode::Settling(blocks) => {
                if blocks > 1 {
                    self.mode = Mode::Settling(blocks - 1);
                } else {
                    self.mode = Mode::Listening;
                    self.search_timing();
                }
            }
            Mode::Walking => {}
        }
    }

    // -- Timing search ---------------------------------------------------------

    /// Finds the symbol boundaries in the newest 50 ms and starts a walk if the
    /// window looks like start symbols.
    ///
    /// The search flips the sign of every other 7.5 ms period of the window and adds
    /// the periods together slot by slot. With true boundaries at slot t in each
    /// period, the slots before t then all hold one polarity and the slots from t on
    /// hold the other. So t is the split that maximises the size of (sum of the
    /// slots from t on) minus (sum of the slots before t).
    ///
    /// The window is short enough for the phase to change little between periods
    /// unless the carrier is many Hz off. The detector's lag product gives the phase
    /// change per period, which the search removes.
    fn search_timing(&mut self) {
        let period = START_STOP_US as f64 * 1e-6 * self.baseband_rate;
        let window = ((FOLD_WINDOW_S * self.baseband_rate) as usize).min(self.baseband_samples as usize);
        let first = self.baseband_samples - window as u64;

        let sum_p: f32 = self.window_p.iter().sum();
        let sum_q: f32 = self.window_q.iter().sum();
        let phase_per_period = libm::atan2f(sum_q, sum_p);
        let rotation = Cplx { re: libm::cosf(phase_per_period), im: -libm::sinf(phase_per_period) };

        let slot_count = libm::ceil(period) as usize;
        let slots = &mut self.fold_slots[..slot_count];
        slots.fill(Cplx::ZERO);
        let mut position = 0.0f32;
        let period_f32 = period as f32;
        // The sign flip of each period is part of this phasor.
        let mut phasor = Cplx { re: 1.0, im: 0.0 };
        let mut energy = 0.0f32;
        for offset in 0..window {
            let z = self.ring[(first as usize + offset) & RING_MASK];
            slots[position as usize] += z.mul(phasor);
            energy += z.power();
            position += 1.0;
            if position >= period_f32 {
                position -= period_f32;
                phasor = -phasor.mul(rotation);
            }
        }

        let mut total = Cplx::ZERO;
        for &slot in slots.iter() {
            total += slot;
        }
        let mut before = Cplx::ZERO;
        let mut best_score = -1.0f32;
        let mut best_slot = 0usize;
        for (t, &slot) in slots.iter().enumerate() {
            let score = (total - before * 2.0).power();
            if score > best_score {
                best_score = score;
                best_slot = t;
            }
            before += slot;
        }

        let coherence = best_score / (window as f32 * energy.max(1e-30));
        if coherence < COHERENCE_GATE {
            return;
        }

        // Start the walk on the boundary 5 to 6 symbols before the newest sample. The
        // window is 6.67 symbols long, so that boundary is inside it.
        let newest = self.baseband_samples as f64;
        let boundary = first as f64 + best_slot as f64;
        let periods = libm::floor((newest - boundary) / period - WALK_BACK_SYMBOLS).max(0.0);
        let start = boundary + periods * period;
        let clean = (coherence - COHERENCE_FOR_LONG_MEMORY)
            / (COHERENCE_FOR_SHORT_MEMORY - COHERENCE_FOR_LONG_MEMORY);
        self.reference_memory = LONG_REFERENCE_MEMORY
            + (SHORT_REFERENCE_MEMORY - LONG_REFERENCE_MEMORY) * clean.clamp(0.0, 1.0);
        self.begin_walk(start, phase_per_period / 3.0);
    }

    // -- Walk ------------------------------------------------------------------

    fn begin_walk(&mut self, start: f64, carrier_frequency: f32) {
        self.mode = Mode::Walking;
        self.grid_edge = start;
        self.walk_start = start;
        self.bucket_index = 0;
        self.previous_half = Cplx::ZERO;
        self.carrier_phasor = Cplx { re: 1.0, im: 0.0 };
        self.carrier_frequency = carrier_frequency;
        self.echo_estimates = [Cplx::ZERO; ECHO_TAPS];
        self.echo_taps = [Cplx::ZERO; ECHO_TAPS];
        self.echo_error_power = 0.0;
        self.direct_error_power = 0.0;
        self.direct_tap = Cplx::ZERO;
        self.timing_error = 0.0;
        self.timing_power = 0.0;
        self.power_average = 0.0;
        self.level_reference = 0.0;
        self.level_sum = 0.0;
        self.level_count = 0;
        // A path "before" the walk, so that the first bucket is the first bucket of a
        // start symbol.
        self.path_buffers[0] = [Path::DEAD; STATES];
        self.path_buffers[0][S2].metric = 0.0;
        self.path_buffers[0][S2].history = 0b0111_0001_1100_0111;
        self.current_paths = 0;
    }

    /// Returns the next 2.5 ms bucket if all of its samples have arrived.
    fn next_bucket(&mut self) -> Option<(Cplx, Cplx)> {
        let bucket_len = BUCKET_S * self.baseband_rate;
        let start = round_to_u64(self.grid_edge);
        let middle = round_to_u64(self.grid_edge + 0.5 * bucket_len);
        let end = round_to_u64(self.grid_edge + bucket_len);
        if end > self.baseband_samples {
            return None;
        }
        let mut first_half = Cplx::ZERO;
        for index in start..middle {
            first_half += self.ring[index as usize & RING_MASK];
        }
        let mut second_half = Cplx::ZERO;
        for index in middle..end {
            second_half += self.ring[index as usize & RING_MASK];
        }
        Some((first_half, second_half))
    }

    /// Moves the grid on by one bucket, with a correction from the timing error.
    ///
    /// The encoder rounds symbol lengths up to whole samples, so a chime rendered at
    /// 44.1 kHz grows by 11 us for each data 0, and a 21-byte chime is about 2 ms
    /// longer than its nominal length. Clock errors add to this.
    ///
    /// The timing error is that of Gardner's detector, and `flip` says whether the
    /// best path has a polarity flip on the edge. `before` and `after` are the sums
    /// over the half buckets on each side of the edge. When the true flip is late by
    /// d samples, before + after = 2 d a in the direction of before - after, whose
    /// size is 2 w a for half buckets of w samples and carrier amplitude a. An edge
    /// with no flip holds no timing information, only noise, so it does not count.
    fn advance_grid(&mut self, before: Cplx, after: Cplx, flip: bool) {
        let bucket_len = BUCKET_S * self.baseband_rate;
        let mut step = 0.0;
        if flip {
            let sum = before + after;
            let difference = before - after;
            self.timing_error = self.timing_error * TIMING_MEMORY + sum.dot(difference);
            self.timing_power = self.timing_power * TIMING_MEMORY + difference.power();
            let half_len = 0.5 * bucket_len as f32;
            step = TIMING_GAIN * half_len * self.timing_error / self.timing_power.max(1e-30);
            step = step.clamp(-TIMING_MAX_STEP, TIMING_MAX_STEP);
        }
        self.grid_edge += bucket_len + step as f64;
    }

    /// Processes one bucket. Returns how the walk ended, if it did.
    fn step_trellis(&mut self, (first_half, second_half): (Cplx, Cplx)) -> Option<WalkEnd> {
        let first_half = first_half.mul_conj(self.carrier_phasor);
        let second_half = second_half.mul_conj(self.carrier_phasor);
        let bucket = first_half + second_half;
        let distance = (self.grid_edge - self.walk_start) as f32;

        let (buffer_a, buffer_b) = self.path_buffers.split_at_mut(1);
        let (old, new) = if self.current_paths == 0 {
            (&buffer_a[0], &mut buffer_b[0])
        } else {
            (&buffer_b[0], &mut buffer_a[0])
        };
        self.current_paths = 1 - self.current_paths;

        // Each path removes the echoes that its own past polarities would cause. A path
        // whose past fits the signal has less left over, and the second term rewards
        // that. With no echoes this term is the same for all paths.
        let echo_taps = self.echo_taps;
        let memory = self.reference_memory;
        let left_over_weight = 0.5 / (1.0 - memory);
        let mut cleaned = [Cplx::ZERO; STATES];
        let mut scores = [0.0f32; STATES];
        let mut penalties = [0.0f32; STATES];
        for state in 0..STATES {
            let path = &old[state];
            if path.metric == f32::NEG_INFINITY {
                continue;
            }
            let left_over = bucket - echo_of(&echo_taps, path.history);
            cleaned[state] = left_over;
            scores[state] = path.reference.dot(left_over);
            penalties[state] = left_over.power() * left_over_weight;
        }
        let stay = |state: usize| old[state].metric + scores[state] - penalties[state];
        let flip = |state: usize| old[state].metric - scores[state] - penalties[state];

        // Moves inside a symbol.
        for (from, to) in [(S0, S1), (S1, S2), (T0, T1), (T1, T2)] {
            old[from].extend_into(&mut new[to], cleaned[from], stay(from), false, memory);
        }
        for symbol in 0..SYMBOLS_PER_BYTE {
            for position in 0..3 {
                let from = data_state(symbol, position);
                old[from].extend_into(&mut new[from + 1], cleaned[from], stay(from), false, memory);
            }
        }

        // Moves to the first bucket of a new symbol.
        old[S2].extend_into(&mut new[S0], cleaned[S2], flip(S2), true, memory);
        new[S0].start_symbols = new[S0].start_symbols.saturating_add(1);

        for symbol in 0..SYMBOLS_PER_BYTE {
            let previous = (symbol + SYMBOLS_PER_BYTE - 1) % SYMBOLS_PER_BYTE;
            let after_0 = data_state(previous, 1);
            let after_1 = data_state(previous, 3);
            let next = &mut new[data_state(symbol, 0)];
            let (source, bit) = if flip(after_1) > flip(after_0) { (after_1, 1) } else { (after_0, 0) };
            let can_start_data = symbol == 0 && old[S2].start_symbols > 0;
            if can_start_data && flip(S2) > flip(source) {
                old[S2].extend_into(next, cleaned[S2], flip(S2), true, memory);
                next.data_start = distance;
            } else {
                old[source].extend_into(next, cleaned[source], flip(source), true, memory);
                next.push_bit(previous, bit);
            }
        }

        // A stop symbol follows a stop symbol, or a data symbol that finishes a byte.
        let last = SYMBOLS_PER_BYTE - 1;
        let after_0 = data_state(last, 1);
        let after_1 = data_state(last, 3);
        let mut best = flip(T2);
        let mut source = T2;
        if old[after_0].can_stop_after_this_byte() && flip(after_0) > best {
            best = flip(after_0);
            source = after_0;
        }
        if old[after_1].can_stop_after_this_byte() && flip(after_1) > best {
            best = flip(after_1);
            source = after_1;
        }
        old[source].extend_into(&mut new[T0], cleaned[source], best, true, memory);
        if source == T2 {
            new[T0].stop_symbols = new[T0].stop_symbols.saturating_add(1);
        } else {
            new[T0].push_bit(last, if source == after_1 { 1 } else { 0 });
        }

        // Keep the metrics small, so that adding to them stays accurate.
        let mut best_state = 0;
        for state in 1..STATES {
            if new[state].metric > new[best_state].metric {
                best_state = state;
            }
        }
        let best_metric = new[best_state].metric;
        for path in new.iter_mut() {
            path.metric -= best_metric;
        }

        let best_path = new[best_state];
        let half_echo = echo_of(&echo_taps, best_path.history >> 1) * 0.5;
        self.update_carrier_tracking(bucket - half_echo * 2.0, best_path.reference);
        self.update_echo_filter(bucket, best_path.history, best_state);

        // The first bucket of a symbol follows a polarity flip.
        let flip_on_edge = best_state == S0
            || best_state == T0
            || (best_state >= FIRST_DATA_STATE && (best_state - FIRST_DATA_STATE).is_multiple_of(4));
        if self.bucket_index > 0 {
            self.advance_grid(self.previous_half, first_half - half_echo, flip_on_edge);
        } else {
            self.advance_grid(Cplx::ZERO, Cplx::ZERO, false);
        }
        self.previous_half = second_half - half_echo;

        self.bucket_index += 1;
        self.update_power(bucket);
        self.check_walk_end(best_state)
    }

    /// Updates the carrier frequency and phase from the best path. `left_over` is the
    /// newest bucket without echoes, and `reference` is the path's reference, which now
    /// includes that bucket.
    ///
    /// The imaginary part below comes from the older buckets in the reference, because
    /// the bucket has no imaginary part relative to itself. With the reference's own
    /// tracking of the phase, the frequency makes a second-order loop.
    fn update_carrier_tracking(&mut self, left_over: Cplx, reference: Cplx) {
        let phase_error = left_over.mul_conj(reference).im / reference.power().max(1e-30);
        let gain = FREQUENCY_GAIN_FRACTION * (1.0 - self.reference_memory);
        let frequency = self.carrier_frequency + gain * phase_error.clamp(-1.0, 1.0);
        let frequency = frequency.clamp(-MAX_FREQUENCY, MAX_FREQUENCY);
        self.carrier_frequency = frequency;
        // cos and sin of a small angle, followed by the same correction of the amplitude
        // as the mixer. A small error in the step only looks like a small frequency error.
        let step = Cplx { re: 1.0 - 0.5 * frequency * frequency, im: frequency };
        let phasor = self.carrier_phasor.mul(step);
        self.carrier_phasor = phasor * (0.5 * (3.0 - phasor.power()));
    }

    /// Updates the echo filter from the best path's polarities.
    ///
    /// This is an ordinary adaptive filter from those polarities to the buckets. Its
    /// first tap is the direct signal, which is a separate estimate from the path's
    /// reference. If the filter used the reference, the two could share the direct
    /// signal in any proportion and the taps could drift until they broke the message.
    ///
    /// The start symbols repeat with opposite polarity every 3 buckets, so on them an
    /// echo from 3 or more buckets back looks the same as the direct signal or as an
    /// echo 1 or 2 buckets back. Only the first two taps learn from the start symbols.
    fn update_echo_filter(&mut self, bucket: Cplx, history: u16, best_state: usize) {
        let polarity_of = |age: usize| if (history >> age) & 1 == 1 { 1.0f32 } else { -1.0 };

        let mut predicted = self.direct_tap * polarity_of(0);
        for (tap, &estimate) in self.echo_estimates.iter().enumerate() {
            predicted += estimate * polarity_of(tap + 1);
        }
        let error = bucket - predicted;
        let direct_error = bucket - self.direct_tap * polarity_of(0);
        let count = self.bucket_index.max(1) as f32;
        let error_weight = (1.0 / count).max(ECHO_ERROR_WEIGHT);
        self.echo_error_power += (error.power() - self.echo_error_power) * error_weight;
        self.direct_error_power += (direct_error.power() - self.direct_error_power) * error_weight;
        self.direct_tap += error * (DIRECT_TAP_STEP * polarity_of(0));

        let tap_noise = ECHO_TAP_NOISE * self.echo_error_power;
        let tap_limit = (ECHO_TAP_MARGIN * tap_noise).max(ECHO_TAP_FLOOR * self.direct_tap.power());
        let in_start_symbols = best_state <= S2;
        for tap in 0..ECHO_TAPS {
            let estimate = &mut self.echo_estimates[tap];
            if tap < 2 || !in_start_symbols {
                *estimate += error * (ECHO_STEP * polarity_of(tap + 1));
            }
            let power = estimate.power();
            let gain = if power > tap_limit { 1.0 - tap_limit / power } else { 0.0 };
            self.echo_taps[tap] = *estimate * gain;
        }
        if self.echo_error_power > ECHO_USEFUL_FRACTION * self.direct_error_power {
            self.echo_taps = [Cplx::ZERO; ECHO_TAPS];
        }
    }

    fn update_power(&mut self, bucket: Cplx) {
        let samples = BUCKET_S * self.baseband_rate;
        let power = bucket.power() / (samples * samples) as f32;
        self.level_sum += power;
        self.level_count += 1;

        let limited = if self.bucket_index > 8 {
            power.min(POWER_OUTLIER_LIMIT * self.power_average)
        } else {
            power
        };
        let weight = (1.0 / self.bucket_index as f32).max(POWER_WEIGHT);
        self.power_average += (limited - self.power_average) * weight;
        if self.bucket_index == LEVEL_REFERENCE_BUCKET {
            self.level_reference = self.power_average;
        }
    }

    fn paths(&self) -> &[Path; STATES] {
        &self.path_buffers[self.current_paths]
    }

    fn check_walk_end(&self, best_state: usize) -> Option<WalkEnd> {
        let paths = self.paths();

        // The stop symbols of a path can start at three positions on the grid, and a
        // path with a wrong last symbol or two can still pass the CRC after the
        // Hamming correction. So the path must also lead the other stop positions.
        let stopping = &paths[T0];
        if stopping.metric > f32::NEG_INFINITY
            && stopping.metric >= paths[T1].metric
            && stopping.metric >= paths[T2].metric
            && stopping.stop_symbols >= STOP_SYMBOLS_TO_ACCEPT
            && stopping.crc_is_good()
        {
            return Some(WalkEnd::Message(T0));
        }

        let best = &paths[best_state];
        if self.bucket_index == START_CHECK_BUCKET && best.start_symbols < MIN_START_SYMBOLS {
            return Some(WalkEnd::Silent);
        }
        if best.byte_count as usize > MAX_MESSAGE_BYTES || self.bucket_index >= MAX_WALK_BUCKETS {
            return Some(WalkEnd::Failed(UnreadableReason::TooManyBytes, best_state));
        }
        // A path in the stop symbols can be following noise after the signal has gone, so
        // the power must also still be there, averaged over the last stop symbols.
        let in_stop_symbols = best_state == T0 || best_state == T1 || best_state == T2;
        if in_stop_symbols
            && best.stop_symbols >= STOP_SYMBOLS as u8
            && (self.power_average > STOP_POWER_FRACTION * self.level_reference
                || best.stop_symbols >= 2 * STOP_SYMBOLS as u8)
        {
            return Some(WalkEnd::Failed(UnreadableReason::BadCrc, best_state));
        }
        let in_start_symbols = best_state <= S2;
        if in_start_symbols && best.start_symbols > MAX_START_SYMBOLS {
            return Some(WalkEnd::Failed(UnreadableReason::BadSymbol, best_state));
        }
        if self.level_reference > 0.0 {
            let limit = self.noise_bucket + LOST_FRACTION * (self.level_reference - self.noise_bucket);
            if self.power_average < limit {
                // Stop symbols that end early still end a message. The longest message
                // can end without any.
                let could_be_complete = in_stop_symbols || best.byte_count as usize == MAX_MESSAGE_BYTES;
                if could_be_complete && best.crc_is_good() {
                    return Some(WalkEnd::Message(best_state));
                }
                // Stop symbols of an earlier chime can start a walk, and they end like
                // this with no bytes. There is nothing to report in that case.
                if best.byte_count == 0 {
                    return Some(WalkEnd::Silent);
                }
                return Some(WalkEnd::Failed(UnreadableReason::SignalLost, best_state));
            }
        }
        None
    }

    /// The input sample index of a grid position, in baseband samples.
    fn input_position(&self, baseband_position: f64) -> f64 {
        baseband_position * self.decimation as f64 + (self.decimation - 1) as f64
            - FILTER_DELAY_S * self.sample_rate
    }

    /// The input sample index where the first start symbol began, for a path.
    fn start_sample(&self, path: &Path) -> u64 {
        let base = if path.data_start >= 0.0 {
            let data_start = self.input_position(self.walk_start + path.data_start as f64);
            data_start - START_SYMBOLS as f64 * START_STOP_US as f64 * 1e-6 * self.sample_rate
        } else {
            self.input_position(self.walk_start)
        };
        round_to_u64(base.max(0.0))
    }

    /// RMS level of the carrier in dBFS and in power, from the walk's buckets. A
    /// carrier of amplitude A has baseband power A^2 / 4, while its RMS power is
    /// A^2 / 2.
    fn carrier_level(&self) -> (f32, f32) {
        let mean = self.level_sum / self.level_count.max(1) as f32;
        let power = 2.0 * LEVEL_CORRECTION * (mean - self.noise_bucket).max(mean * 0.01);
        (power_to_db(power), power)
    }

    fn finish_walk(&mut self, end: WalkEnd) -> Option<Event> {
        self.mode = Mode::Listening;
        // The detector's window holds the end of the walk, which may be stop symbols.
        self.window_p = [0.0; WINDOW_BLOCKS];
        self.window_q = [0.0; WINDOW_BLOCKS];
        self.window_e = [0.0; WINDOW_BLOCKS];
        self.window_band = [0.0; WINDOW_BLOCKS];
        self.holdoff_blocks = match end {
            WalkEnd::Message(_) => HOLDOFF_AFTER_CHIME_BLOCKS,
            WalkEnd::Failed(UnreadableReason::BadCrc, _) => HOLDOFF_AFTER_BAD_CHIME_BLOCKS,
            _ => 0,
        };

        let (level_dbfs, level_power) = self.carrier_level();
        match end {
            WalkEnd::Silent => None,
            WalkEnd::Message(state) => {
                let path = self.paths()[state];
                let payload_len = path.byte_count as usize - CRC_BYTES;
                let mut payload = [0u8; MAX_PAYLOAD_BYTES];
                payload[..payload_len].copy_from_slice(&path.bytes[..payload_len]);
                let noise = 2.0 * self.noise_band;
                let snr_db = power_to_db(level_power) - power_to_db(noise);
                Some(Event::Chime(Chime {
                    payload,
                    payload_len,
                    start_sample: self.start_sample(&path),
                    end_sample: self.input_samples,
                    corrected_bits: path.corrected_bits as u32,
                    level_dbfs,
                    snr_db,
                }))
            }
            WalkEnd::Failed(reason, state) => {
                let path = self.paths()[state];
                Some(Event::Unreadable(Unreadable {
                    start_sample: self.start_sample(&path),
                    end_sample: self.input_samples,
                    reason,
                    bytes_received: (path.byte_count as usize).min(MAX_MESSAGE_BYTES),
                    level_dbfs,
                }))
            }
        }
    }
}
