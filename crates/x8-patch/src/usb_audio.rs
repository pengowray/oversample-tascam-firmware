//! USB audio: the decisions that have no firmware addresses, so the host tests can run them.
//! docs/HOMEBREW.md, "192 kHz stereo over USB (2026-11-11)" and "Channel names and an input report
//! over USB (2026-11-27)", has the stock code they work with.
//!
//! The stock UAC2 audio function sends audio to the host on interface 2, in one of three
//! alternate settings: 1 is 8 channels of 3 bytes, 2 is 2 channels of 3 bytes, 3 is 8 channels of
//! 4 bytes. The packets are built in 416-byte buffers, and nothing in the stock code limits a
//! packet to that size. 8 channels above 96 kHz would overrun them, so the patch moves the
//! buffers to 800 bytes each (`STRIDE_SITES`); until it has, it keeps settings 1 and 3 at 96 kHz
//! or below.
//!
//! At 192 kHz the patch sends what the stock audio routine sends at 96 kHz and below, in the same
//! order: the mix left and right, then INPUT 1 to 6 after the dual-gain merge, as many as the
//! setting has, captured each frame from the stock 192 kHz routine.
//!
//! The patch also names the 8 channels to the host with UAC2 channel name strings, and answers one
//! vendor request with a report of the input settings (`InputReport`).

use crate::take_guano::Inputs;

/// The highest rate the stock firmware offers over USB.
pub const HIGHEST_STOCK_RATE: u32 = 96_000;
/// The rate the patch adds.
pub const RATE: u32 = 192_000;
/// Size of each of the 4 buffers the stock code builds packets to the host in.
pub const MAX_IN_PACKET: u32 = 0x1a0;
/// Size of the ring between the audio routine and the USB callback (the word at 0x20004140, and
/// the constant in the callback's copy code).
pub const RING_BYTES: u32 = 0x3400;

/// The third sub-range of the clock's GET RANGE answer, {minimum, maximum, resolution} as
/// little-endian words: 96 kHz only in the stock firmware.
pub const STOCK_TOP_RANGE: [u8; 12] = range(HIGHEST_STOCK_RATE, HIGHEST_STOCK_RATE, 0);
/// The patch's third sub-range: 96 kHz to 192 kHz in steps of 96 kHz, so 96 and 192 kHz.
pub const WIDE_TOP_RANGE: [u8; 12] = range(HIGHEST_STOCK_RATE, RATE, HIGHEST_STOCK_RATE);

const fn range(min: u32, max: u32, res: u32) -> [u8; 12] {
    let (a, b, c) = (min.to_le_bytes(), max.to_le_bytes(), res.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3]]
}

/// Whether interface 2's alternate setting sends 8 channels.
pub fn eight_channels(alt: u8) -> bool {
    matches!(alt, 1 | 3)
}

/// The size of the packet buffers to the host: the stock 416 bytes, or 800 once the patch has
/// moved them (`big`).
pub fn buffer_size(big: bool) -> u32 {
    if big { BIG_IN_PACKET } else { MAX_IN_PACKET }
}

/// The alternate setting to pass to the stock handler when the host asks for `alt` at
/// `host_rate`. 0 makes the stock handler do nothing: with the stock buffers, no 8-channel stream
/// starts above 96 kHz.
pub fn alt_to_use(alt: u8, host_rate: u32, big: bool) -> u8 {
    if !big && eight_channels(alt) && host_rate > HIGHEST_STOCK_RATE { 0 } else { alt }
}

/// The rate to give the stock code when the host sets `rate` while interface 2 is at `alt`.
/// 96 kHz is what the stock code itself sets for a rate it does not list.
pub fn rate_to_use(rate: u32, alt: u8, big: bool) -> u32 {
    if !big && eight_channels(alt) && rate > HIGHEST_STOCK_RATE { HIGHEST_STOCK_RATE } else { rate }
}

/// The largest packet the stock callback builds: one frame more than a packet's share of the
/// rate. Packets go every 125 us microframe at high speed and every 1 ms at full speed.
pub fn largest_packet(channels: u8, bytes: u8, rate: u32, high_speed: bool) -> u32 {
    let per_second = if high_speed { 8_000 } else { 1_000 };
    channels as u32 * bytes as u32 * (rate / per_second + 1)
}

/// Whether packets of this format fit buffers of `buffer` bytes.
pub fn packet_fits_in(channels: u8, bytes: u8, rate: u32, high_speed: bool, buffer: u32) -> bool {
    largest_packet(channels, bytes, rate, high_speed) <= buffer
}

/// Whether the stock callback can build packets of this format in its stock buffers.
pub fn packet_fits(channels: u8, bytes: u8, rate: u32, high_speed: bool) -> bool {
    packet_fits_in(channels, bytes, rate, high_speed, MAX_IN_PACKET)
}

/// Whether the patch stops a stream before the stock callback builds its next packet: only above
/// 96 kHz, where the patch has made the rate possible, and only when the packet would not fit.
/// At 96 kHz and below the stock code decides, as before.
pub fn must_stop(channels: u8, bytes: u8, rate: u32, high_speed: bool, big: bool) -> bool {
    rate > HIGHEST_STOCK_RATE && !packet_fits_in(channels, bytes, rate, high_speed, buffer_size(big))
}

/// The packet size bus reset gives the 8-channel settings once the buffers are 800 bytes: their
/// largest packet at 192 kHz (setting 1: 8 x 3 bytes, setting 3: 8 x 4). The stock 312 and 416
/// cover 96 kHz only.
pub fn big_packet(alt: u8) -> Option<u16> {
    match alt {
        1 => Some(largest_packet(8, 3, RATE, true) as u16),
        3 => Some(largest_packet(8, 4, RATE, true) as u16),
        _ => None,
    }
}

/// Frames in the ring for a frame of `frame_bytes`, as the stock code counts them
/// (`FUN_00014c88`: the ring's 0x3400 bytes rounded down to whole frames).
pub fn ring_frames(frame_bytes: u32) -> u32 {
    RING_BYTES / frame_bytes
}

/// One frame as the stock routine has it for USB: the mix left and right, then INPUT 1 to 6.
pub type StockFrame = [f32; 8];

/// One sample as the stock 44.1 to 96 kHz routine packs it for USB (0x11B94 onward): as a float
/// for 4-byte samples (format byte 4), unclamped; for 3-byte samples clamped to +-1, times
/// 2147483647 as a double, truncated, and its top 3 bytes. Little-endian. Returns the bytes used.
pub fn stock_sample(value: f32, bytes: u8, out: &mut [u8]) -> usize {
    if bytes == 4 {
        out[..4].copy_from_slice(&value.to_le_bytes());
        4
    } else {
        out[..3].copy_from_slice(&stock_word(value).to_le_bytes()[1..]);
        3
    }
}

/// `(value.clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32`, as the stock routine computes it, in
/// single precision only: the patch has no double-precision hardware, and the library code for
/// it is about 1.7 KB. With `a = v x 2^31` (exact in an f32), the stock result is `trunc(a - v)`.
/// When `a` is a whole number that is `a - 1` for a positive `v` and `a + 1` for a negative one.
/// Otherwise `a`'s fraction is at least 128 times `|v|`, so subtracting `v` crosses no whole
/// number and the result is `trunc(a)`.
pub fn stock_word(value: f32) -> i32 {
    let v = if value.is_nan() { 0.0 } else { value.clamp(-1.0, 1.0) };
    let a = v * 2_147_483_648.0;
    if v == 1.0 {
        return i32::MAX;
    }
    let whole = a as i32;
    if whole as f32 == a {
        if v > 0.0 {
            whole - 1
        } else if v < 0.0 {
            whole + 1
        } else {
            0
        }
    } else {
        whole
    }
}

/// The 32-bit Thumb-2 instruction `insn` (two little-endian halfwords, as in memory) with its
/// modified immediate (i:imm3:imm8, as in `mov.w` and `add.w` with an immediate) replaced by
/// `value`. None when `value` cannot be encoded that way.
pub fn with_modified_imm(insn: [u8; 4], value: u32) -> Option<[u8; 4]> {
    let (i, imm3, imm8) = encode_modified_imm(value)?;
    let hw1 = u16::from_le_bytes([insn[0], insn[1]]) & !(1 << 10) | (i << 10);
    let hw2 = u16::from_le_bytes([insn[2], insn[3]]) & !(0x7 << 12 | 0xff) | (imm3 << 12) | imm8;
    let (a, b) = (hw1.to_le_bytes(), hw2.to_le_bytes());
    Some([a[0], a[1], b[0], b[1]])
}

/// The modified immediate of a 32-bit Thumb-2 instruction.
pub fn modified_imm(insn: [u8; 4]) -> u32 {
    let hw1 = u16::from_le_bytes([insn[0], insn[1]]) as u32;
    let hw2 = u16::from_le_bytes([insn[2], insn[3]]) as u32;
    let field = ((hw1 >> 10) & 1) << 11 | ((hw2 >> 12) & 7) << 8 | (hw2 & 0xff);
    let imm8 = field & 0xff;
    match field >> 8 {
        0 => imm8,
        1 => imm8 << 16 | imm8,
        2 => imm8 << 24 | imm8 << 8,
        3 => imm8 * 0x0101_0101,
        _ => (0x80 | (field & 0x7f)).rotate_right(field >> 7),
    }
}

/// i, imm3 and imm8 for `value` as a rotated 8-bit constant (rotations 8 to 31), the form a
/// small multiple of a power of two takes.
fn encode_modified_imm(value: u32) -> Option<(u16, u16, u16)> {
    if value < 0x100 {
        return Some((0, 0, value as u16));
    }
    (8..32).find_map(|rot: u32| {
        let unrotated = value.rotate_left(rot);
        (unrotated & !0xff == 0 && unrotated & 0x80 != 0).then(|| {
            let field = rot << 7 | (unrotated & 0x7f);
            ((field >> 11) as u16, ((field >> 8) & 7) as u16, (field & 0xff) as u16)
        })
    })
}

/// The 4 packet buffers to the host, with room for 8 channels of 4 bytes at 192 kHz.
pub const BIG_IN_PACKET: u32 = 800;
/// The stock code's buffer stride as it appears in each instruction that sets it: the patch
/// replaces each with the same multiple of `BIG_IN_PACKET`.
pub const STRIDE_SITES: [(u32, [u8; 4], u32); 7] = [
    // FUN_00014ef0, the audio class callback: the stride it multiplies the buffer index by, and
    // the addresses of buffers 1 to 3 it compares with.
    (0x14fac, [0x4f, 0xf4, 0xd0, 0x71], 1),
    (0x14fce, [0x02, 0xf5, 0xd0, 0x74], 1),
    (0x14fd8, [0x02, 0xf5, 0x50, 0x76], 2),
    (0x14fe2, [0x02, 0xf5, 0x9c, 0x64], 3),
    // The interface 2 SET_INTERFACE handler: its first packets from buffers 1 to 3.
    (0x153b2, [0x07, 0xf5, 0xd0, 0x72], 1),
    (0x153e4, [0x07, 0xf5, 0x50, 0x72], 2),
    (0x15416, [0x07, 0xf5, 0x9c, 0x62], 3),
];

/// A Thumb-2 `bl` at `from` to `to` (both even), or None out of range.
pub fn thumb_bl(from: usize, to: usize) -> Option<[u16; 2]> {
    let offset = to as i64 - (from as i64 + 4);
    if !(-(1 << 24)..(1 << 24)).contains(&offset) {
        return None;
    }
    let o = offset as u32;
    let s = (o >> 24) & 1;
    let (i1, i2) = ((o >> 23) & 1, (o >> 22) & 1);
    let (j1, j2) = ((!(i1 ^ s)) & 1, (!(i2 ^ s)) & 1);
    let hw1 = 0xf000 | s << 10 | ((o >> 12) & 0x3ff);
    let hw2 = 0xd000 | j1 << 13 | j2 << 11 | ((o >> 1) & 0x7ff);
    Some([hw1 as u16, hw2 as u16])
}

/// The stock SET CUR check (`FUN_00014108`): whether `rate` is in the GET RANGE answer, counting
/// from each sub-range's minimum to its maximum in steps of its resolution.
pub fn listed(ranges: &[[u32; 3]], rate: u32) -> bool {
    ranges.iter().any(|&[min, max, res]| {
        let mut r = min;
        while r <= max {
            if r == rate {
                return true;
            }
            if res == 0 {
                break;
            }
            r += res;
        }
        false
    })
}

/// Where the audio function's descriptors (0x813B69FC in V2.03) start in the configuration
/// descriptor that `FUN_80df471c` builds at 0x2000F000.
pub const AUDIO_DESCRIPTORS_AT: usize = 32;
/// The three descriptors in the configuration that describe 8 channels: the capture terminal
/// (input terminal ID 4, iChannelNames at + 13) and the AS_GENERAL descriptors of interface 2's
/// alternate settings 1 and 3 (iChannelNames at + 15). For each: where the descriptor starts in
/// the configuration, where its iChannelNames byte is (bytes 108, 157 and 249), and its first 4
/// stock bytes. The 2-channel setting keeps its left and right channel config and no names.
pub const CHANNEL_NAME_SITES: [(usize, usize, [u8; 4]); 3] =
    [(95, 108, [0x11, 0x24, 0x02, 0x04]), (142, 157, [0x10, 0x24, 0x01, 0x02]), (234, 249, [0x10, 0x24, 0x01, 0x02])];
/// The string index of the first channel's name, which the patch writes in each iChannelNames.
/// UAC2 numbers the other channels' names on from it: 16 is the mix left, 17 the mix right, and
/// 18 to 23 are INPUT 1 to 6.
pub const FIRST_CHANNEL_NAME: u8 = 16;
/// The channels the names are for: the mix left and right, then INPUT 1 to 6.
pub const NAMED_CHANNELS: u8 = 8;
/// Size of the longest channel name's string descriptor, `Input 1: INT MIC`: bLength,
/// bDescriptorType, then the name in UTF-16.
pub const NAME_DESCRIPTOR_BYTES: usize = 2 + 2 * 16;
/// The USB descriptor type of a string descriptor.
const STRING_DESCRIPTOR: u8 = 3;

/// The vendor request the patch answers: device to host, vendor, to the device (`bmRequestType`
/// 0xC0), `bRequest` 0x58, `wValue` 1, `wIndex` 0. The setup packet's first 6 bytes; `wLength`
/// follows.
pub const REPORT_REQUEST: [u8; 6] = [0xc0, 0x58, 1, 0, 0, 0];
/// Bytes in version 1 of the report. A later version may add bytes after these.
pub const REPORT_BYTES: usize = 28;
pub const REPORT_VERSION: u8 = 1;

/// Where an input's signal comes from, as the stock XRI text names it. The number is the source's
/// code in the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Source {
    Unknown = 0,
    IntMic = 1,
    PlugIn = 2,
    ExtMic = 3,
    LineIn = 4,
    ExtIn = 5,
}

/// Part of a channel name in UTF-16LE, as the string descriptor has it, made at compile time:
/// the bytes and how many of them are used.
struct WideText {
    bytes: [u8; WIDE_TEXT_BYTES],
    len: usize,
}
/// Bytes for the longest part, `: INT MIC`.
const WIDE_TEXT_BYTES: usize = 18;

/// `text`, in ASCII, as UTF-16LE.
const fn wide(text: &[u8]) -> WideText {
    assert!(2 * text.len() <= WIDE_TEXT_BYTES);
    let mut bytes = [0; WIDE_TEXT_BYTES];
    let mut i = 0;
    while i < text.len() {
        assert!(text[i] < 0x80);
        bytes[2 * i] = text[i];
        i += 1;
    }
    WideText { bytes, len: 2 * text.len() }
}

/// The first part of each channel's name.
const CHANNEL_LABELS: [WideText; NAMED_CHANNELS as usize] = [
    wide(b"Mix L"),
    wide(b"Mix R"),
    wide(b"Input 1"),
    wide(b"Input 2"),
    wide(b"Input 3"),
    wide(b"Input 4"),
    wide(b"Input 5"),
    wide(b"Input 6"),
];
/// What follows an input's label: its source as the XRI text names it, in the order of `Source`.
const SOURCE_SUFFIXES: [WideText; 6] =
    [wide(b""), wide(b": INT MIC"), wide(b": PLUG IN"), wide(b": EXT MIC"), wide(b": LINE IN"), wide(b": EXT IN")];

/// The input settings the report gives the host, read from the X8's memory when the host asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputReport {
    /// The patch build, as the update screen shows it.
    pub build: u16,
    /// The settings that the GUANO metadata also has.
    pub inputs: Inputs,
    /// Settings + 0x258 to + 0x25B, one byte for each of inputs 3 to 6: 0 when the input is set to
    /// LINE IN, otherwise EXT MIC.
    pub line_or_mic: [u8; 4],
    /// Inputs 5 and 6 carry EXT IN (DSP state + 0x623 is 1).
    pub ext_in_on_5_6: bool,
    /// Tracks turned on in MANUAL, bit 0 for track 1.
    pub tracks_on: u8,
}

impl InputReport {
    /// The source of `input`, 0 for INPUT 1 to 5 for INPUT 6, by the rules of `FUN_80df6ba0`
    /// and `FUN_80dcfa78`.
    pub fn source(&self, input: usize) -> Source {
        match input {
            0 | 1 if self.inputs.built_in_mics => Source::IntMic,
            0 | 1 => Source::PlugIn,
            4 | 5 if self.ext_in_on_5_6 => Source::ExtIn,
            2..=5 if self.line_or_mic[input - 2] == 0 => Source::LineIn,
            2..=5 => Source::ExtMic,
            _ => Source::Unknown,
        }
    }

    /// The sources of INPUT 1 to 6.
    pub fn sources(&self) -> [Source; 6] {
        core::array::from_fn(|input| self.source(input))
    }

    /// Version 1 of the report, all 28 bytes. docs/HOMEBREW.md has the table of bytes.
    pub fn bytes(&self) -> [u8; REPORT_BYTES] {
        let inputs = &self.inputs;
        let mut out = [0u8; REPORT_BYTES];
        out[0] = b'X';
        out[1] = b'8';
        out[2] = REPORT_VERSION;
        out[3] = REPORT_BYTES as u8;
        [out[4], out[5]] = self.build.to_le_bytes();
        out[6] = u8::from(inputs.built_in_mics) | u8::from(inputs.phantom_24v) << 1;
        out[7] = inputs.ms_decode;
        let sources = self.sources();
        for i in 0..6 {
            out[8 + i] = sources[i] as u8;
            out[18 + i] = inputs.track_sources[i];
        }
        for i in 0..4 {
            out[14 + i] = u8::from(inputs.phantom[i] != 0);
        }
        out[24] = self.tracks_on;
        out[25] = inputs.mic_trim[0];
        out[26] = inputs.mic_trim[1];
        out
    }
}

/// Whether a setup packet is the request for the report.
pub fn is_report_request(setup: &[u8; 8]) -> bool {
    setup[..6] == REPORT_REQUEST
}

/// Writes the string descriptor for string `index` into `out` and returns its length, if `index`
/// is one of the channel names: `Mix L`, `Mix R`, then `Input 1: INT MIC` to `Input 6: EXT IN`,
/// with each input's source from `sources`. None for any other index.
pub fn channel_name(index: u8, sources: &[Source; 6], out: &mut [u8; NAME_DESCRIPTOR_BYTES]) -> Option<usize> {
    let channel = index.wrapping_sub(FIRST_CHANNEL_NAME) as usize;
    let label = CHANNEL_LABELS.get(channel)?;
    let source = match channel.checked_sub(2) {
        Some(input) => *sources.get(input)?,
        None => Source::Unknown,
    };
    let suffix = &SOURCE_SUFFIXES[source as usize];
    let size = 2 + label.len + suffix.len;
    out[0] = size as u8;
    out[1] = STRING_DESCRIPTOR;
    out[2..2 + label.len].copy_from_slice(&label.bytes[..label.len]);
    out[2 + label.len..size].copy_from_slice(&suffix.bytes[..suffix.len]);
    Some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(bytes: [u8; 12]) -> [u32; 3] {
        let w = |k: usize| u32::from_le_bytes([bytes[k], bytes[k + 1], bytes[k + 2], bytes[k + 3]]);
        [w(0), w(4), w(8)]
    }

    /// The bytes at 0x2000022e in the unpacked DTCM run (work/ram/dtcm-20000000.bin).
    #[test]
    fn stock_range_is_the_dumped_bytes() {
        assert_eq!(STOCK_TOP_RANGE, [0x00, 0x77, 0x01, 0x00, 0x00, 0x77, 0x01, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn the_buffer_stride_sites_take_800_byte_buffers() {
        for (address, stock, multiple) in STRIDE_SITES {
            assert_eq!(modified_imm(stock), MAX_IN_PACKET * multiple, "0x{address:x}");
            // Re-encoding the stock value gives the stock bytes.
            assert_eq!(with_modified_imm(stock, MAX_IN_PACKET * multiple), Some(stock), "0x{address:x}");
            let new = with_modified_imm(stock, BIG_IN_PACKET * multiple).expect("encodable");
            assert_eq!(modified_imm(new), BIG_IN_PACKET * multiple);
            // Only the immediate changes: the registers and the opcode stay.
            let mask = |b: [u8; 4]| {
                let hw1 = u16::from_le_bytes([b[0], b[1]]) & !(1 << 10);
                let hw2 = u16::from_le_bytes([b[2], b[3]]) & !(0x7000 | 0xff);
                (hw1, hw2)
            };
            assert_eq!(mask(new), mask(stock));
        }
        // Checked against capstone: 600 is 4ff41678 as mov.w r8, 800 is 4ff44873 as mov.w r3.
        assert_eq!(with_modified_imm([0x4f, 0xf4, 0x9c, 0x78], 600), Some([0x4f, 0xf4, 0x16, 0x78]));
        assert_eq!(with_modified_imm([0x4f, 0xf4, 0xd0, 0x73], 800), Some([0x4f, 0xf4, 0x48, 0x73]));
        assert_eq!(with_modified_imm([0x4f, 0xf4, 0xd0, 0x73], 801), None);
        assert!(packet_fits_in(8, 4, 192_000, true, BIG_IN_PACKET));
        assert!(packet_fits_in(8, 3, 192_000, true, BIG_IN_PACKET));
    }

    #[test]
    fn the_wide_list_offers_192_khz_and_nothing_else_new() {
        let stock = [[44_100, 44_100, 0], [48_000, 48_000, 0], words(STOCK_TOP_RANGE)];
        let wide = [[44_100, 44_100, 0], [48_000, 48_000, 0], words(WIDE_TOP_RANGE)];
        for rate in [44_100, 48_000, 96_000] {
            assert!(listed(&stock, rate) && listed(&wide, rate));
        }
        assert!(!listed(&stock, 192_000));
        assert!(listed(&wide, 192_000));
        for rate in [88_200, 144_000, 176_400, 288_000, 384_000] {
            assert!(!listed(&wide, rate), "{rate}");
        }
    }

    /// The stock descriptor sizes, 13 frames each, fit at 96 kHz; the patch's stereo size (150 bytes)
    /// fits at 192 kHz, and 8 channels do not.
    #[test]
    fn packet_sizes() {
        assert_eq!(largest_packet(8, 3, 96_000, true), 312);
        assert_eq!(largest_packet(8, 4, 96_000, true), 416);
        assert_eq!(largest_packet(2, 3, 96_000, true), 78);
        assert_eq!(largest_packet(2, 3, 192_000, true), 150);
        assert!(packet_fits(8, 4, 96_000, true));
        assert!(packet_fits(2, 3, 192_000, true));
        assert!(!packet_fits(8, 3, 192_000, true));
        assert!(!packet_fits(8, 4, 192_000, true));
        // At full speed a packet holds 1 ms, so even stereo at 192 kHz does not fit.
        assert!(!packet_fits(2, 3, 192_000, false));
    }

    #[test]
    fn streams_stop_only_above_96_khz() {
        assert!(must_stop(8, 3, 192_000, true, false));
        assert!(must_stop(2, 3, 192_000, false, false));
        assert!(!must_stop(2, 3, 192_000, true, false));
        // With 800-byte buffers 8 channels fit at 192 kHz, 3 or 4 bytes.
        assert!(!must_stop(8, 3, 192_000, true, true));
        assert!(!must_stop(8, 4, 192_000, true, true));
        // Full speed at 96 kHz would not fit either, but that is the stock code's case.
        assert!(!must_stop(8, 3, 96_000, false, false));
        assert!(!must_stop(8, 4, 96_000, true, false));
    }

    #[test]
    fn eight_channels_wait_for_the_big_buffers() {
        for big in [false, true] {
            assert_eq!(alt_to_use(1, 192_000, big), if big { 1 } else { 0 });
            assert_eq!(alt_to_use(3, 192_000, big), if big { 3 } else { 0 });
            assert_eq!(rate_to_use(192_000, 3, big), if big { 192_000 } else { 96_000 });
            assert_eq!(alt_to_use(2, 192_000, big), 2);
            assert_eq!(alt_to_use(1, 96_000, big), 1);
            assert_eq!(rate_to_use(48_000, 1, big), 48_000);
            // Every combination allowed fits the buffers at high speed, in either order.
            for alt in 0..4u8 {
                let (channels, bytes) = match alt {
                    1 => (8, 3),
                    3 => (8, 4),
                    _ => (2, 3),
                };
                for rate in [44_100, 48_000, 96_000, 192_000] {
                    if alt_to_use(alt, rate, big) == alt && rate_to_use(rate, alt, big) == rate {
                        assert!(packet_fits_in(channels, bytes, rate, true, buffer_size(big)), "alt {alt} at {rate}");
                    }
                }
            }
        }
        assert_eq!(big_packet(1), Some(600));
        assert_eq!(big_packet(3), Some(800));
        assert_eq!(big_packet(2), None);
    }

    #[test]
    fn calls_are_encoded_as_the_stock_ones() {
        // The 192 kHz routine's recorder step call, and the same call moved to the capture stub
        // (checked with capstone).
        assert_eq!(thumb_bl(0x127ee, 0x177c8), Some([0xf004, 0xffeb]));
        assert_eq!(thumb_bl(0x127ee, 0x31a94), Some([0xf01f, 0xf951]));
        assert_eq!(thumb_bl(0, 1 << 25), None);
    }

    #[test]
    fn samples_are_packed_as_the_stock_code_does() {
        let mut out = [0u8; 4];
        // Float: as is, above 1.0 too.
        assert_eq!(stock_sample(1.5, 4, &mut out), 4);
        assert_eq!(out, 1.5f32.to_le_bytes());
        // 24-bit: clamped, scaled to 2^31 - 1, truncated, top 3 bytes.
        assert_eq!(stock_sample(1.5, 3, &mut out), 3);
        assert_eq!(out[..3], [0xff, 0xff, 0x7f]);
        assert_eq!(stock_sample(-1.0, 3, &mut out), 3);
        assert_eq!(out[..3], [0x00, 0x00, 0x80]);
        assert_eq!(stock_sample(0.5, 3, &mut out), 3);
        assert_eq!(out[..3], (((0.5f64 * 2_147_483_647.0) as i32).to_le_bytes())[1..]);
        // The single-precision word equals the stock double-precision one for every f32 tried:
        // edges, powers of two, tiny values, and a spread across the whole range.
        let stock = |v: f32| (v.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0) as i32;
        let mut values = std::vec![0.0f32, -0.0, 1.0, -1.0, 1.5, -1.5, f32::MIN_POSITIVE, 1e-30, -1e-30];
        for e in -40..1 {
            let p = 2f32.powi(e);
            values.extend([p, -p, p * 1.000_001, -p * 0.999_999]);
        }
        let mut x = 0x1234_5678u32;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            values.push(f32::from_bits(x & 0xbfff_ffff)); // magnitude below 2
        }
        for v in values {
            assert_eq!(stock_word(v), stock(v), "{v:e}");
        }
        assert_eq!(stock_sample(0.0, 3, &mut out), 3);
        assert_eq!(out[..3], [0, 0, 0]);
    }

    #[test]
    fn ring_holds_whole_frames() {
        assert_eq!(ring_frames(6), 2_218);
        assert_eq!(ring_frames(24), 554);
    }

    /// The three stock descriptors, from the audio descriptors at 0x813B69FC in V2.03 (at 63, 110
    /// and 202 there), with bNrChannels 8 and iChannelNames 0.
    #[test]
    fn channel_name_sites_are_the_stock_descriptors() {
        let capture: [u8; 17] = [0x11, 0x24, 0x02, 0x04, 0x01, 0x02, 0x00, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0];
        let general: [u8; 16] = [0x10, 0x24, 0x01, 0x02, 0x00, 0x01, 0x01, 0, 0, 0, 0x08, 0, 0, 0, 0, 0];
        let stock: [(usize, &[u8]); 3] = [(63, &capture), (110, &general), (202, &general)];
        for (&(start, names, lead), &(at, descriptor)) in CHANNEL_NAME_SITES.iter().zip(&stock) {
            assert_eq!(start, AUDIO_DESCRIPTORS_AT + at);
            assert_eq!(descriptor[..4], lead);
            assert_eq!(descriptor[0] as usize, descriptor.len());
            // iChannelNames is the byte before bmControls (2 bytes) and iTerminal in the input
            // terminal, and the last byte of AS_GENERAL.
            let field = if descriptor.len() == 17 { 13 } else { 15 };
            assert_eq!(names, start + field);
            assert_eq!(descriptor[field], 0);
            // bNrChannels: 8.
            assert_eq!(descriptor[if descriptor.len() == 17 { 8 } else { 10 }], 8);
        }
        assert_eq!(CHANNEL_NAME_SITES.map(|(_, names, _)| names), [108, 157, 249]);
        // Alternate setting 1's endpoint, 164, follows its AS_GENERAL and its 6-byte format type.
        assert_eq!(CHANNEL_NAME_SITES[1].0 + 16 + 6, 164);
        assert_eq!(CHANNEL_NAME_SITES[2].0 + 16 + 6, 256);
    }

    fn report() -> InputReport {
        InputReport {
            build: 289,
            inputs: Inputs {
                track_sources: [1, 1, 2, 2, 3, 4],
                phantom: [1, 0, 0, 1],
                phantom_24v: true,
                mic_trim: [2, 5],
                built_in_mics: true,
                ms_decode: 2,
            },
            line_or_mic: [1, 0, 1, 0],
            ext_in_on_5_6: false,
            tracks_on: 0b10_1101,
        }
    }

    #[test]
    fn sources_follow_the_stock_rules() {
        use Source::*;
        let mut r = report();
        assert_eq!(r.sources(), [IntMic, IntMic, ExtMic, LineIn, ExtMic, LineIn]);
        r.inputs.built_in_mics = false;
        r.ext_in_on_5_6 = true;
        assert_eq!(r.sources(), [PlugIn, PlugIn, ExtMic, LineIn, ExtIn, ExtIn]);
        r.line_or_mic = [0, 7, 0, 0];
        assert_eq!(r.sources(), [PlugIn, PlugIn, LineIn, ExtMic, ExtIn, ExtIn]);
        assert_eq!(r.source(6), Unknown);
    }

    #[test]
    fn the_report_has_every_byte_in_its_place() {
        let bytes = report().bytes();
        assert_eq!(
            bytes,
            [
                b'X', b'8', 1, 28, // magic, version, length
                0x21, 0x01, // build 289
                0b11, 2, // built-in mics and 24 V, MS DECODE on INPUT 3-4
                1, 1, 3, 4, 3, 4, // sources of INPUT 1 to 6
                1, 0, 0, 1, // phantom power on INPUT 3 to 6
                1, 1, 2, 2, 3, 4, // INPUT SELECT for tracks 1 to 6
                0b10_1101, 2, 5, 0, // tracks on, MIC TRIM, spare
            ]
        );
        let mut r = report();
        r.inputs.phantom = [0, 2, 0, 0];
        r.inputs.phantom_24v = false;
        r.inputs.built_in_mics = false;
        let bytes = r.bytes();
        assert_eq!(bytes[6], 0);
        assert_eq!(bytes[14..18], [0, 1, 0, 0]);
        assert_eq!(bytes[8..10], [2, 2]);
    }

    #[test]
    fn only_the_report_request_matches() {
        let request = |b: [u8; 6]| [b[0], b[1], b[2], b[3], b[4], b[5], 28, 0];
        assert!(is_report_request(&request(REPORT_REQUEST)));
        // wLength does not matter.
        assert!(is_report_request(&[0xc0, 0x58, 1, 0, 0, 0, 0xff, 0xff]));
        for other in [
            [0x40, 0x58, 1, 0, 0, 0], // host to device
            [0xc1, 0x58, 1, 0, 0, 0], // to an interface
            [0xa0, 0x58, 1, 0, 0, 0], // class
            [0xc0, 0x57, 1, 0, 0, 0],
            [0xc0, 0x58, 2, 0, 0, 0],
            [0xc0, 0x58, 1, 1, 0, 0],
            [0xc0, 0x58, 1, 0, 1, 0],
            [0xc0, 0x58, 1, 0, 0, 1],
        ] {
            assert!(!is_report_request(&request(other)), "{other:x?}");
        }
    }

    fn name(index: u8, sources: &[Source; 6]) -> Option<std::string::String> {
        let mut out = [0xaa; NAME_DESCRIPTOR_BYTES];
        let size = channel_name(index, sources, &mut out)?;
        assert_eq!(out[0] as usize, size);
        assert_eq!(out[1], 3);
        let units: std::vec::Vec<u16> =
            out[2..size].chunks(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        Some(std::string::String::from_utf16(&units).unwrap())
    }

    #[test]
    fn channels_are_named_from_their_sources() {
        use Source::*;
        let sources = [IntMic, PlugIn, ExtMic, LineIn, ExtIn, ExtIn];
        let names: std::vec::Vec<_> = (16..24).map(|i| name(i, &sources).unwrap()).collect();
        assert_eq!(
            names,
            [
                "Mix L",
                "Mix R",
                "Input 1: INT MIC",
                "Input 2: PLUG IN",
                "Input 3: EXT MIC",
                "Input 4: LINE IN",
                "Input 5: EXT IN",
                "Input 6: EXT IN"
            ]
        );
        assert_eq!(name(18, &[Unknown; 6]).as_deref(), Some("Input 1"));
        // The longest name fills the descriptor.
        let mut out = [0; NAME_DESCRIPTOR_BYTES];
        assert_eq!(channel_name(18, &sources, &mut out), Some(NAME_DESCRIPTOR_BYTES));
        for index in [0, 1, 4, 5, 15, 24, 255] {
            assert_eq!(name(index, &sources), None, "{index}");
        }
    }
}
