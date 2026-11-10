//! The stock firmware's clock record: the 10 bytes its recorder task reads the clock chip into
//! (command 75) and sets the clock chip from (command 76). See docs/HOMEBREW.md, "Setting the
//! clock". This part has no firmware addresses, so the host tests can run it.

use chime::format::LocalTime;

pub const RECORD_BYTES: usize = 10;

/// What the patch needs from a record it read: the time, and two bytes that are kept so that a
/// record written back uses the same conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockReading {
    /// Local seconds since 1970.
    pub local_seconds: i64,
    /// The record's weekday less the true weekday (0 = Sunday), mod 7: the chip's numbering.
    pub weekday_offset: u8,
    /// Byte 6: the clock chip's AM/PM bit. The firmware runs the chip (a Seiko S-35390A or
    /// compatible) in 24-hour mode, where the chip accepts 0 or 1 here and sets the bit itself.
    pub hour_flag: u8,
}

/// Day of the week, 0 = Sunday, of a count of days since 1970-01-01 (a Thursday).
fn weekday(days: i64) -> u8 {
    (days + 4).rem_euclid(7) as u8
}

/// Reads a record: a two-digit year (u16, little-endian), month, day, weekday, a spare byte, then
/// the hour flag, hours, minutes and seconds. None for a date or time that cannot be.
pub fn read(record: &[u8; RECORD_BYTES]) -> Option<ClockReading> {
    let year = u16::from_le_bytes([record[0], record[1]]);
    let [month, day, wd, hour_flag, hour, minute, second] =
        [record[2], record[3], record[4], record[6], record[7], record[8], record[9]];
    if year > 99 || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let time = LocalTime { year: 2000 + i32::from(year), month, day, hour, minute, second };
    let local_seconds = time.to_epoch_seconds();
    // A day past the month's end rolls over in the conversion: refuse it.
    if LocalTime::from_epoch_seconds(local_seconds) != time {
        return None;
    }
    let weekday_offset = (wd + 7 - weekday(local_seconds.div_euclid(86_400))) % 7;
    Some(ClockReading { local_seconds, weekday_offset, hour_flag })
}

/// A record that sets the clock to `local_seconds`, with the conventions of a record read
/// earlier. None outside the years 2000 to 2099, which the two-digit year can hold.
pub fn write(local_seconds: i64, conventions: &ClockReading) -> Option<[u8; RECORD_BYTES]> {
    let t = LocalTime::from_epoch_seconds(local_seconds);
    if !(2000..=2099).contains(&t.year) {
        return None;
    }
    let year = ((t.year - 2000) as u16).to_le_bytes();
    let wd = (weekday(local_seconds.div_euclid(86_400)) + conventions.weekday_offset) % 7;
    Some([year[0], year[1], t.month, t.day, wd, 0, conventions.hour_flag, t.hour, t.minute, t.second])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-08 is a Sunday.
    const SUNDAY: [u8; RECORD_BYTES] = [26, 0, 11, 8, 0, 0, 0, 21, 30, 49];

    #[test]
    fn a_record_round_trips() {
        let reading = read(&SUNDAY).unwrap();
        assert_eq!(
            LocalTime::from_epoch_seconds(reading.local_seconds),
            LocalTime { year: 2026, month: 11, day: 8, hour: 21, minute: 30, second: 49 }
        );
        assert_eq!(reading.weekday_offset, 0);
        assert_eq!(write(reading.local_seconds, &reading), Some(SUNDAY));
    }

    /// A chip that counts Monday as 0 keeps doing so, and the hour flag is kept.
    #[test]
    fn conventions_carry_over() {
        let monday_first = [26, 0, 11, 8, 6, 0, 1, 21, 30, 49];
        let reading = read(&monday_first).unwrap();
        assert_eq!(reading.weekday_offset, 6);
        assert_eq!(reading.hour_flag, 1);
        // One day later, Monday: 0 in that numbering.
        let next = write(reading.local_seconds + 86_400, &reading).unwrap();
        assert_eq!(next, [26, 0, 11, 9, 0, 0, 1, 21, 30, 49]);
    }

    #[test]
    fn impossible_records_are_refused() {
        for bad in [
            [26, 0, 13, 8, 0, 0, 0, 21, 30, 49],
            [26, 0, 2, 30, 0, 0, 0, 21, 30, 49],
            [26, 0, 11, 8, 0, 0, 0, 24, 30, 49],
            [100, 0, 11, 8, 0, 0, 0, 21, 30, 49],
            [0; RECORD_BYTES],
        ] {
            assert_eq!(read(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn years_outside_the_record_are_refused() {
        let reading = read(&SUNDAY).unwrap();
        assert_eq!(write(946_684_799, &reading), None);
        assert!(write(946_684_800, &reading).is_some());
    }
}
