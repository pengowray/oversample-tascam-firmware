//! 192 kHz stereo over USB: the decisions that have no firmware addresses, so the host tests can
//! run them. docs/HOMEBREW.md, "192 kHz stereo over USB (2026-11-11)", has the stock code they
//! work with.
//!
//! The stock UAC2 audio function sends audio to the host on interface 2, in one of three
//! alternate settings: 1 is 8 channels of 3 bytes, 2 is 2 channels of 3 bytes, 3 is 8 channels of
//! 4 bytes. The packets are built in 416-byte buffers, and nothing in the stock code limits a
//! packet to that size. 8 channels above 96 kHz would overrun them, so the patch keeps alternate
//! settings 1 and 3 at 96 kHz or below.
//!
//! The patch adds alternate settings 4 (2 channels of 32-bit float) and 5 (4 channels of 32-bit
//! float), as copies of setting 3's descriptors with fewer channels. At 192 kHz they carry INPUT 3
//! and 4 (and 5 and 6) after the stock dual-gain merge, as the X8 records its 32-bit float
//! tracks. At 96 kHz and below the stock audio routine fills them by itself, in its own order:
//! the mix left and right, then INPUT 1 and 2.

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

/// The stock format byte (USB state + 0x48) for 32-bit float, as its handler sets for
/// alternate setting 3.
pub const FLOAT_FORMAT: u8 = 4;
/// The alternate settings the patch adds, and their channels of 32-bit float.
pub const NEW_ALTS: [(u8, u8); 2] = [(4, 2), (5, 4)];

/// The alternate setting to pass to the stock handler when the host asks for `alt` at
/// `host_rate`. 0 makes the stock handler do nothing, so no 8-channel stream starts above 96 kHz.
/// The stock handler sets bytes, channels and format for settings 1 to 3 only; for 4 and 5 it
/// keeps what `before_alt` wrote and queues its first packets at that size.
pub fn alt_to_use(alt: u8, host_rate: u32) -> u8 {
    if eight_channels(alt) && host_rate > HIGHEST_STOCK_RATE { 0 } else { alt }
}

/// For a setting the patch adds: the channels, bytes per sample and format byte to write into the
/// USB state before the stock handler runs, at any rate.
pub fn before_alt(alt: u8) -> Option<(u8, u8, u8)> {
    NEW_ALTS.iter().find(|&&(a, _)| a == alt).map(|&(_, channels)| (channels, 4, FLOAT_FORMAT))
}

/// The rate to give the stock code when the host sets `rate` while interface 2 is at `alt`.
/// 96 kHz is what the stock code itself sets for a rate it does not list.
pub fn rate_to_use(rate: u32, alt: u8) -> u32 {
    if eight_channels(alt) && rate > HIGHEST_STOCK_RATE { HIGHEST_STOCK_RATE } else { rate }
}

/// The channels of float the audio interrupt sends at 192 kHz: 2 on setting 4 and 4 on setting
/// 5, while the USB state still says so.
pub fn float_channels(alt: u8, channels: u8, bytes: u8) -> Option<u8> {
    before_alt(alt).and_then(|(want, b, _)| (channels == want && bytes == b).then_some(want))
}

/// A new setting's packet size, as `FUN_80df4a24` gives the stock settings at each bus reset: at
/// high speed one frame more than a microframe holds at 192 kHz; at full speed the 0x184 it gives
/// every setting.
pub fn new_alt_packet(channels: u8, high_speed: bool) -> u16 {
    if high_speed { largest_packet(channels, 4, RATE, true) as u16 } else { 0x184 }
}

/// Length of the stock configuration descriptor (configuration 0: mass storage and UAC2 audio).
pub const STOCK_CONFIG_LEN: usize = 386;
/// Where interface 2's alternate setting 3 starts in it, and interface 3 after it.
const ALT3_AT: usize = 225;
const IFACE3_AT: usize = 271;
/// Bytes of one alternate setting: interface, AS_GENERAL, FORMAT_TYPE, endpoint and its
/// class-specific endpoint descriptors.
pub const ALT_LEN: usize = 46;
/// Length of the configuration with the new settings.
pub const EXTENDED_CONFIG_LEN: usize = STOCK_CONFIG_LEN + NEW_ALTS.len() * ALT_LEN;
/// Offsets in an alternate setting: bAlternateSetting, bNrChannels and bmChannelConfig, and the
/// endpoint's wMaxPacketSize.
const ALT_NUMBER: usize = 3;
const NR_CHANNELS: usize = 19;
const CHANNEL_CONFIG: usize = 20;
pub const MAX_PACKET: usize = 35;

/// Add settings 4 and 5 to interface 2 in the configuration descriptor in `config`, which holds
/// `len` bytes and has room for `EXTENDED_CONFIG_LEN`. Each is a copy of setting 3 (8 channels of
/// float) with its number, channel count and channel config changed; interface 3 moves up after
/// them, and the total length grows. Returns the new length, or None (and changes nothing) when
/// the descriptor is not the stock one.
pub fn extend_config(config: &mut [u8], len: usize) -> Option<usize> {
    if len != STOCK_CONFIG_LEN || config.len() < EXTENDED_CONFIG_LEN {
        return None;
    }
    let alt3 = &config[ALT3_AT..IFACE3_AT];
    // Setting 3's interface descriptor, AS_GENERAL with 8 channels of float, then interface 3.
    let stock = alt3[..4] == [9, 4, 2, 3]
        && alt3[9..12] == [16, 0x24, 1]
        && alt3[NR_CHANNELS] == 8
        && alt3[25..31] == [6, 0x24, 2, 1, 4, 32]
        && alt3[31..34] == [7, 5, 0x84]
        && config[IFACE3_AT..IFACE3_AT + 4] == [9, 4, 3, 0]
        && u16::from_le_bytes([config[2], config[3]]) as usize == STOCK_CONFIG_LEN;
    if !stock {
        return None;
    }
    let mut template = [0u8; ALT_LEN];
    template.copy_from_slice(alt3);
    config.copy_within(IFACE3_AT..STOCK_CONFIG_LEN, IFACE3_AT + NEW_ALTS.len() * ALT_LEN);
    for (k, &(alt, channels)) in NEW_ALTS.iter().enumerate() {
        let at = IFACE3_AT + k * ALT_LEN;
        let block = &mut config[at..at + ALT_LEN];
        block.copy_from_slice(&template);
        block[ALT_NUMBER] = alt;
        block[NR_CHANNELS] = channels;
        // 2 channels are front left and right, as setting 2; 4 have no positions, as setting 3's 8.
        let config_bits: u32 = if channels == 2 { 0x3 } else { 0 };
        block[CHANNEL_CONFIG..CHANNEL_CONFIG + 4].copy_from_slice(&config_bits.to_le_bytes());
    }
    config[2..4].copy_from_slice(&(EXTENDED_CONFIG_LEN as u16).to_le_bytes());
    Some(EXTENDED_CONFIG_LEN)
}

/// Where each new setting's wMaxPacketSize is in the extended descriptor.
pub fn new_alt_packet_offsets() -> [usize; 2] {
    core::array::from_fn(|k| IFACE3_AT + k * ALT_LEN + MAX_PACKET)
}

/// The stock dual-gain merge's state for one channel at the end of a block (state block at
/// 0x29cac + 0x48 x channel): `g` make-up gain (+0x04), `c` the high-gain path's scale (+0x0c)
/// and `w` the crossfade weight (+0x44). At 192 kHz `w` moves once a block of 4 frames, so it
/// holds for every frame of the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Merge {
    pub g: f32,
    pub c: f32,
    pub w: f32,
}

/// One input's merged sample, as the stock routine computes it and the X8 records it to a 32-bit
/// float track: `g x (B x w + A x c x (1 - w))`, with each word times the input's gain (+ or -
/// 2^-31, its sign the input's phase setting). `high` and `low` are the high- and low-gain
/// paths' words.
pub fn merged(high: i32, low: i32, input_gain: f32, m: Merge) -> f32 {
    let (a, b) = (high as f32 * input_gain, low as f32 * input_gain);
    m.g * (b * m.w + a * m.c * (1.0 - m.w))
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

    /// Configuration 0 as `FUN_80df471c` builds it: the 9-byte configuration descriptor and the
    /// 23-byte mass storage interface (their bytes as built do not matter here, only their
    /// length and wTotalLength), then the 354 bytes of audio descriptors at 0x813B69FC in V2.03.
    fn stock_config() -> std::vec::Vec<u8> {
        let mut config = std::vec![0u8; 32];
        config[..4].copy_from_slice(&[9, 2, 0x82, 0x01]);
        config.extend_from_slice(&STOCK_AUDIO);
        assert_eq!(config.len(), STOCK_CONFIG_LEN);
        config
    }

    const STOCK_AUDIO: [u8; 354] = [
        0x08, 0x0b, 0x01, 0x03, 0x01, 0x00, 0x20, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0x01, 0x01, 0x20,
        0x00, 0x09, 0x24, 0x01, 0x00, 0x02, 0x08, 0x4b, 0x00, 0x00, 0x08, 0x24, 0x0a, 0x07, 0x03, 0x03,
        0x00, 0x00, 0x0c, 0x24, 0x03, 0x02, 0x01, 0x01, 0x00, 0x04, 0x07, 0x00, 0x00, 0x00, 0x11, 0x24,
        0x02, 0x03, 0x01, 0x01, 0x00, 0x07, 0x02, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11,
        0x24, 0x02, 0x04, 0x01, 0x02, 0x00, 0x07, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0c, 0x24, 0x03, 0x05, 0x01, 0x03, 0x00, 0x03, 0x07, 0x00, 0x00, 0x00, 0x09, 0x04, 0x02, 0x00,
        0x00, 0x01, 0x02, 0x20, 0x00, 0x09, 0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x20, 0x00, 0x10, 0x24,
        0x01, 0x02, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x24,
        0x02, 0x01, 0x03, 0x18, 0x07, 0x05, 0x84, 0x05, 0x38, 0x01, 0x01, 0x08, 0x25, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x09, 0x04, 0x02, 0x02, 0x01, 0x01, 0x02, 0x20, 0x00, 0x10, 0x24, 0x01, 0x02,
        0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x02, 0x03, 0x00, 0x00, 0x00, 0x00, 0x06, 0x24, 0x02, 0x01,
        0x03, 0x18, 0x07, 0x05, 0x84, 0x05, 0x4e, 0x00, 0x01, 0x08, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x09, 0x04, 0x02, 0x03, 0x01, 0x01, 0x02, 0x20, 0x00, 0x10, 0x24, 0x01, 0x02, 0x00, 0x01,
        0x04, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x24, 0x02, 0x01, 0x04, 0x20,
        0x07, 0x05, 0x84, 0x05, 0xa0, 0x01, 0x01, 0x08, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09,
        0x04, 0x03, 0x00, 0x00, 0x01, 0x02, 0x20, 0x00, 0x09, 0x04, 0x03, 0x01, 0x02, 0x01, 0x02, 0x20,
        0x00, 0x10, 0x24, 0x01, 0x03, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x02, 0x03, 0x00, 0x00, 0x00,
        0x00, 0x06, 0x24, 0x02, 0x01, 0x03, 0x18, 0x07, 0x05, 0x03, 0x05, 0x4e, 0x00, 0x01, 0x08, 0x25,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x05, 0x83, 0x11, 0x04, 0x00, 0x04, 0x09, 0x04, 0x03,
        0x02, 0x02, 0x01, 0x02, 0x20, 0x00, 0x10, 0x24, 0x01, 0x03, 0x00, 0x01, 0x04, 0x00, 0x00, 0x00,
        0x02, 0x03, 0x00, 0x00, 0x00, 0x00, 0x06, 0x24, 0x02, 0x01, 0x04, 0x20, 0x07, 0x05, 0x03, 0x05,
        0x68, 0x00, 0x01, 0x08, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x05, 0x83, 0x11, 0x04,
        0x00, 0x04,
    ];

    /// Walk descriptors: (offset, type, bytes).
    fn walk(bytes: &[u8]) -> std::vec::Vec<(usize, u8, &[u8])> {
        let mut out = std::vec::Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let len = bytes[at] as usize;
            assert!(len >= 2 && at + len <= bytes.len(), "descriptor at {at}");
            out.push((at, bytes[at + 1], &bytes[at..at + len]));
            at += len;
        }
        out
    }

    #[test]
    fn two_float_settings_are_added_to_interface_2() {
        let stock = stock_config();
        let mut config = stock.clone();
        config.resize(1024, 0);
        assert_eq!(extend_config(&mut config, STOCK_CONFIG_LEN), Some(EXTENDED_CONFIG_LEN));
        let config = &config[..EXTENDED_CONFIG_LEN];
        assert_eq!(EXTENDED_CONFIG_LEN, 478);
        assert_eq!(u16::from_le_bytes([config[2], config[3]]), 478);
        // Everything before the new settings, and interface 3 after them, is as stock.
        assert_eq!(config[4..IFACE3_AT], stock[4..IFACE3_AT]);
        assert_eq!(config[IFACE3_AT + 2 * ALT_LEN..], stock[IFACE3_AT..]);
        let descriptors = walk(&config[32..]);
        let alts: std::vec::Vec<(u8, u8)> =
            descriptors.iter().filter(|d| d.1 == 4).map(|d| (d.2[2], d.2[3])).collect();
        assert_eq!(alts, [(1, 0), (2, 0), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (3, 0), (3, 1), (3, 2)]);
        // Settings 4 and 5: float, 4-byte samples, 2 and 4 channels, endpoint 0x84.
        for (k, &(alt, channels)) in NEW_ALTS.iter().enumerate() {
            let at = IFACE3_AT + k * ALT_LEN;
            let block = &config[at..at + ALT_LEN];
            assert_eq!(block[ALT_NUMBER], alt);
            assert_eq!(block[NR_CHANNELS], channels);
            assert_eq!(u32::from_le_bytes(block[15..19].try_into().unwrap()), 4, "bmFormats IEEE_FLOAT");
            assert_eq!(block[25..31], [6, 0x24, 2, 1, 4, 32]);
            assert_eq!(block[33], 0x84);
            assert_eq!(new_alt_packet_offsets()[k], at + MAX_PACKET);
        }
        // Anything but the stock descriptor is left alone.
        let mut other = stock.clone();
        other[IFACE3_AT + 3] = 1;
        other.resize(1024, 0);
        let before = other.clone();
        assert_eq!(extend_config(&mut other, STOCK_CONFIG_LEN), None);
        assert_eq!(other, before);
        let mut short = stock.clone();
        assert_eq!(extend_config(&mut short, STOCK_CONFIG_LEN), None, "no room");
    }

    #[test]
    fn the_new_settings_fit_at_every_rate() {
        assert_eq!(before_alt(4), Some((2, 4, FLOAT_FORMAT)));
        assert_eq!(before_alt(5), Some((4, 4, FLOAT_FORMAT)));
        assert_eq!(before_alt(3), None);
        assert_eq!(alt_to_use(4, 192_000), 4);
        assert_eq!(alt_to_use(5, 44_100), 5);
        assert_eq!(new_alt_packet(2, true), 200);
        assert_eq!(new_alt_packet(4, true), 400);
        assert!(new_alt_packet(4, true) as u32 <= MAX_IN_PACKET);
        for rate in [44_100, 48_000, 96_000, 192_000] {
            assert!(largest_packet(4, 4, rate, true) <= new_alt_packet(4, true) as u32);
        }
        assert_eq!(float_channels(5, 4, 4), Some(4));
        assert_eq!(float_channels(4, 2, 4), Some(2));
        assert_eq!(float_channels(4, 2, 3), None);
        assert_eq!(float_channels(3, 8, 4), None);
    }

    #[test]
    fn the_merge_follows_the_firmware_formula() {
        let full = 1i32 << 30; // half of full scale
        let gain = -1.0 / 2_147_483_648.0; // INPUT 3 and 4's default, inverted
        // D = 20 dB: c = 0.1, g = 10.
        let high_path = Merge { g: 10.0, c: 0.1, w: 0.0 };
        assert!((merged(full, 0, gain, high_path) + 0.5).abs() < 1e-6, "A alone, on A's scale");
        let low_path = Merge { w: 1.0, ..high_path };
        assert!((merged(0, full / 10, gain, low_path) + 0.5).abs() < 1e-4, "B, D dB up");
        // Above A's full scale only B can say how loud: 3 times full scale.
        assert!((merged(i32::MAX, 3 * (full / 5), gain, low_path) + 3.0).abs() < 1e-3);
        let half_way = Merge { w: 0.25, ..high_path };
        let want = 10.0 * (0.05 * gain * 0.25 * full as f32 + 0.5 * gain * 0.1 * 0.75 * full as f32 * 2.0);
        assert!((merged(full, full / 20, gain, half_way) - want).abs() < 1e-6);
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
        assert_eq!(rate_to_use(48_000, 1), 48_000);
        // Every combination the guard allows fits the buffers at high speed, in either order.
        let format = |alt: u8| match (alt, before_alt(alt)) {
            (_, Some((channels, bytes, _))) => (channels, bytes),
            (1, _) => (8, 3),
            (3, _) => (8, 4),
            _ => (2, 3),
        };
        for alt in 0..6u8 {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let (channels, bytes) = format(alt_to_use(alt, rate).max(if alt >= 4 { alt } else { 0 }));
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
