//! The times in an exFAT file entry, as the patch stamps them: to the second, and with the UTC
//! offset of the X8's clock when it is known. See docs/HOMEBREW.md, "Time zone in exFAT file
//! entries". This part has no firmware addresses, so the host tests can run it.
//!
//! The first entry of a file's set holds three times. Each is a packed date and time in 2 s
//! steps. The created and modified times each have a byte that adds 0 to 1.99 s in 10 ms steps,
//! and each of the three has a UTC offset byte: 0x80 when the offset is given, plus a signed
//! count of 15-minute steps in the low 7 bits.

/// Bits of the stock time stamp function's flags: which times to write.
pub const CREATED: u32 = 1;
pub const MODIFIED: u32 = 2;
pub const ACCESSED: u32 = 4;
/// Use the time the file's record holds from its last modified stamp, not the clock.
pub const KEPT: u32 = 8;

pub const ENTRY_BYTES: usize = 32;

/// A time as an entry holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// Seconds / 2, then minutes from bit 5 and hours from bit 11.
    pub time: u16,
    /// Day, then the month from bit 5 and years since 1980 from bit 9.
    pub date: u16,
    /// 100 for an odd second: the count of 10 ms steps to add to `time`.
    pub ten_ms: u8,
}

impl Stamp {
    /// From what the stock clock read gives: seconds, minutes, hours, the day, months since
    /// January and years since 1900. A year before 1980 is stored as 1980, as the stock code does.
    pub fn from_clock(clock: &[u32; 6]) -> Stamp {
        let [second, minute, hour, day, month, year] = *clock;
        let years = year.saturating_sub(80);
        Stamp {
            time: (second / 2 | minute << 5 | hour << 11) as u16,
            date: (day | (month + 1) << 5 | years << 9) as u16,
            ten_ms: (second % 2 * 100) as u8,
        }
    }
}

/// The UTC offset byte for a clock that keeps local time `minutes` ahead of UTC. 0, which means
/// no offset is given, when the offset is not known or is not a whole number of 15-minute steps.
pub fn zone_byte(minutes: Option<i16>) -> u8 {
    match minutes {
        Some(minutes) if minutes % 15 == 0 && (-64 * 15..64 * 15).contains(&minutes) => {
            0x80 | (minutes / 15) as u8 & 0x7f
        }
        _ => 0,
    }
}

/// The offset in minutes that a UTC offset byte gives, if it gives one.
pub fn zone_minutes(byte: u8) -> Option<i16> {
    // The low 7 bits are a signed number: shift its sign bit to the top and back.
    (byte & 0x80 != 0).then(|| i16::from((byte << 1) as i8 >> 1) * 15)
}

/// The two bytes that keep an offset byte across power-off: the byte and its inverse, so that a
/// settings record that never held one (zeros) reads as no offset.
pub fn saved_zone(zone: u8) -> [u8; 2] {
    [zone, !zone]
}

/// The offset byte that [`saved_zone`] kept, or 0 for bytes it did not write.
pub fn zone_from_saved(saved: [u8; 2]) -> u8 {
    if saved[1] == !saved[0] && saved[0] & 0x80 != 0 { saved[0] } else { 0 }
}

/// Write `now` as each of the times `flags` names, with its 10 ms byte and the UTC offset byte
/// `zone`. The other times, and their bytes, are left as they were.
#[inline(never)]
pub fn stamp(entry: &mut [u8; ENTRY_BYTES], now: &Stamp, flags: u32, zone: u8) {
    let mut put = |at: usize, ten_ms_at: Option<usize>, zone_at: usize| {
        entry[at..at + 2].copy_from_slice(&now.time.to_le_bytes());
        entry[at + 2..at + 4].copy_from_slice(&now.date.to_le_bytes());
        if let Some(ten_ms_at) = ten_ms_at {
            entry[ten_ms_at] = now.ten_ms;
        }
        entry[zone_at] = zone;
    };
    if flags & CREATED != 0 {
        put(0x08, Some(0x14), 0x16);
    }
    if flags & MODIFIED != 0 {
        put(0x0c, Some(0x15), 0x17);
    }
    if flags & ACCESSED != 0 {
        put(0x10, None, 0x18);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-28 17:47:58, the modified time of a take the X8 wrote, and the 4 bytes the card
    /// held for it (`fd 8d 7c 5d`).
    const CLOCK: [u32; 6] = [58, 47, 17, 28, 10, 126];

    #[test]
    fn packs_a_clock_reading_as_the_stock_code_does() {
        let stamp = Stamp::from_clock(&CLOCK);
        assert_eq!(stamp.time.to_le_bytes(), [0xfd, 0x8d]);
        assert_eq!(stamp.date.to_le_bytes(), [0x7c, 0x5d]);
        assert_eq!(stamp.ten_ms, 0);
    }

    #[test]
    fn an_odd_second_goes_in_the_10_ms_byte() {
        let odd = Stamp::from_clock(&[59, 47, 17, 28, 10, 126]);
        assert_eq!(odd.time, Stamp::from_clock(&CLOCK).time);
        assert_eq!(odd.ten_ms, 100);
    }

    #[test]
    fn a_year_before_1980_is_stored_as_1980() {
        assert_eq!(Stamp::from_clock(&[0, 0, 0, 1, 0, 70]).date, 1 | 1 << 5);
    }

    #[test]
    fn offset_bytes() {
        assert_eq!(zone_byte(None), 0x00);
        assert_eq!(zone_byte(Some(0)), 0x80);
        assert_eq!(zone_byte(Some(660)), 0xac);
        assert_eq!(zone_byte(Some(-210)), 0xf2);
        assert_eq!(zone_byte(Some(345)), 0x97);
        assert_eq!(zone_byte(Some(840)), 0xb8);
        assert_eq!(zone_byte(Some(-720)), 0xd0);
        // Not a whole number of steps, and out of the byte's range.
        assert_eq!(zone_byte(Some(7)), 0x00);
        assert_eq!(zone_byte(Some(960)), 0x00);
    }

    #[test]
    fn offset_bytes_read_back() {
        for minutes in [0, 660, -210, 345, 840, -720] {
            assert_eq!(zone_minutes(zone_byte(Some(minutes))), Some(minutes));
        }
        assert_eq!(zone_minutes(0x00), None);
        assert_eq!(zone_minutes(0x2c), None);
    }

    #[test]
    fn saved_offset_bytes() {
        assert_eq!(zone_from_saved(saved_zone(0xac)), 0xac);
        assert_eq!(zone_from_saved(saved_zone(0)), 0);
        assert_eq!(zone_from_saved([0, 0]), 0);
        assert_eq!(zone_from_saved([0xff, 0xff]), 0);
        assert_eq!(zone_from_saved([0xac, 0xac]), 0);
    }

    #[test]
    fn stamps_only_the_times_asked_for() {
        let mut entry = [0x11; ENTRY_BYTES];
        let now = Stamp { time: 0x8dfd, date: 0x5d7c, ten_ms: 100 };
        stamp(&mut entry, &now, MODIFIED | ACCESSED, 0xac);
        let mut expected = [0x11; ENTRY_BYTES];
        expected[0x0c..0x14].copy_from_slice(&[0xfd, 0x8d, 0x7c, 0x5d, 0xfd, 0x8d, 0x7c, 0x5d]);
        expected[0x15] = 100;
        expected[0x17] = 0xac;
        expected[0x18] = 0xac;
        assert_eq!(entry, expected);
    }

    #[test]
    fn stamps_all_three_for_a_new_file() {
        let mut entry = [0; ENTRY_BYTES];
        let now = Stamp { time: 0x8dfd, date: 0x5d7c, ten_ms: 100 };
        stamp(&mut entry, &now, CREATED | MODIFIED | ACCESSED, 0);
        assert_eq!(entry[0x08..0x14], [0xfd, 0x8d, 0x7c, 0x5d].repeat(3));
        assert_eq!(entry[0x14..0x19], [100, 100, 0, 0, 0]);
    }
}
