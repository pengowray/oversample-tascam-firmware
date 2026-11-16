//! 192 kHz stereo over USB: the decisions that have no firmware addresses, so the host tests can
//! run them. docs/HOMEBREW.md, "192 kHz stereo over USB (2026-11-11)", has the stock code they
//! work with.
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
}
