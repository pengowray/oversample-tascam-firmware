//! 192 kHz stereo over USB: the decisions that have no firmware addresses, so the host tests can
//! run them. docs/HOMEBREW.md, "192 kHz stereo over USB (2026-11-11)", has the stock code they
//! work with.
//!
//! The stock UAC2 audio function sends audio to the host on interface 2, in one of three
//! alternate settings: 1 is 8 channels of 3 bytes, 2 is 2 channels of 3 bytes, 3 is 8 channels of
//! 4 bytes. The packets are built in 416-byte buffers, and nothing in the stock code limits a
//! packet to that size. 8 channels above 96 kHz would overrun them, so the patch keeps 8 channels
//! at 96 kHz or below.

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

/// The alternate setting to pass to the stock handler when the host asks for `alt` at
/// `host_rate`. 0 makes the stock handler do nothing, so no 8-channel stream starts above 96 kHz.
pub fn alt_to_use(alt: u8, host_rate: u32) -> u8 {
    if eight_channels(alt) && host_rate > HIGHEST_STOCK_RATE { 0 } else { alt }
}

/// The rate to give the stock code when the host sets `rate` while interface 2 is at `alt`.
/// 96 kHz is what the stock code itself sets for a rate it does not list.
pub fn rate_to_use(rate: u32, alt: u8) -> u32 {
    if eight_channels(alt) && rate > HIGHEST_STOCK_RATE { HIGHEST_STOCK_RATE } else { rate }
}

/// The largest packet the stock callback builds: one frame more than a packet's share of the
/// rate. Packets go every 125 us microframe at high speed and every 1 ms at full speed.
pub fn largest_packet(channels: u8, bytes: u8, rate: u32, high_speed: bool) -> u32 {
    let per_second = if high_speed { 8_000 } else { 1_000 };
    channels as u32 * bytes as u32 * (rate / per_second + 1)
}

/// Whether the stock callback can build packets of this format in its buffers.
pub fn packet_fits(channels: u8, bytes: u8, rate: u32, high_speed: bool) -> bool {
    largest_packet(channels, bytes, rate, high_speed) <= MAX_IN_PACKET
}

/// Whether the patch stops a stream before the stock callback builds its next packet: only above
/// 96 kHz, where the patch has made the rate possible, and only when the packet would not fit.
/// At 96 kHz and below the stock code decides, as before.
pub fn must_stop(channels: u8, bytes: u8, rate: u32, high_speed: bool) -> bool {
    rate > HIGHEST_STOCK_RATE && !packet_fits(channels, bytes, rate, high_speed)
}

/// Frames in the ring for a frame of `frame_bytes`, as the stock code counts them.
pub fn ring_frames(frame_bytes: u32) -> u32 {
    RING_BYTES / frame_bytes
}

/// One sample as the host expects it: the top 3 bytes of the 32-bit input word for 3-byte
/// samples (as the stock packing does), or the whole word for 4-byte samples. Little-endian.
/// Returns the bytes used.
pub fn put_sample(word: i32, bytes: u8, out: &mut [u8]) -> usize {
    let le = word.to_le_bytes();
    if bytes == 3 {
        out[..3].copy_from_slice(&le[1..]);
        3
    } else {
        out[..4].copy_from_slice(&le);
        4
    }
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
        assert!(must_stop(8, 3, 192_000, true));
        assert!(must_stop(2, 3, 192_000, false));
        assert!(!must_stop(2, 3, 192_000, true));
        // Full speed at 96 kHz would not fit either, but that is the stock code's case.
        assert!(!must_stop(8, 3, 96_000, false));
        assert!(!must_stop(8, 4, 96_000, true));
    }

    #[test]
    fn eight_channels_stay_at_96_khz_or_below() {
        assert_eq!(alt_to_use(1, 192_000), 0);
        assert_eq!(alt_to_use(3, 192_000), 0);
        assert_eq!(alt_to_use(2, 192_000), 2);
        assert_eq!(alt_to_use(1, 96_000), 1);
        assert_eq!(alt_to_use(0, 192_000), 0);
        assert_eq!(rate_to_use(192_000, 1), 96_000);
        assert_eq!(rate_to_use(192_000, 3), 96_000);
        assert_eq!(rate_to_use(192_000, 2), 192_000);
        assert_eq!(rate_to_use(192_000, 0), 192_000);
        assert_eq!(rate_to_use(48_000, 1), 48_000);
        // Every combination the guard allows fits the buffers at high speed.
        for alt in 0..4u8 {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let (channels, bytes) = match alt {
                    1 => (8, 3),
                    3 => (8, 4),
                    _ => (2, 3),
                };
                if alt_to_use(alt, rate) == alt && rate_to_use(rate, alt) == rate {
                    assert!(packet_fits(channels, bytes, rate, true), "alt {alt} at {rate}");
                }
            }
        }
    }

    #[test]
    fn samples_are_packed_as_the_stock_code_does() {
        let mut out = [0u8; 4];
        assert_eq!(put_sample(0x1234_5678, 3, &mut out), 3);
        assert_eq!(out[..3], [0x56, 0x34, 0x12]);
        assert_eq!(put_sample(-256, 3, &mut out), 3);
        assert_eq!(out[..3], [0xff, 0xff, 0xff]);
        assert_eq!(put_sample(0x1234_5678, 4, &mut out), 4);
        assert_eq!(out, [0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn ring_holds_whole_frames() {
        assert_eq!(ring_frames(6), 2_218);
        assert_eq!(ring_frames(24), 554);
    }
}
