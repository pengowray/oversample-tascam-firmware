use core::fmt;

/// A date and time with no time zone, as a recorder's clock keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Days since 1970-01-01.
    days: i32,
    /// Seconds since midnight.
    seconds: u32,
}

const SECONDS_PER_DAY: i64 = 86_400;

impl DateTime {
    /// Reads "yyyy-mm-dd" and "hh:mm:ss", as in a `bext` chunk. BWF allows any of `-_:. ` between
    /// the numbers. None for anything that isn't a real date and time.
    pub fn parse(date: &[u8], time: &[u8]) -> Option<Self> {
        let [y0, y1, y2, y3, _, m0, m1, _, d0, d1] = *date else { return None };
        let [h0, h1, _, n0, n1, _, s0, s1] = *time else { return None };
        let year = digits(&[y0, y1, y2, y3])? as i32;
        let month = digits(&[m0, m1])?;
        let day = digits(&[d0, d1])?;
        let (hour, minute, second) = (digits(&[h0, h1])?, digits(&[n0, n1])?, digits(&[s0, s1])?);
        let days_in_month = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
            2 => 28,
            _ => return None,
        };
        if day == 0 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(Self { days: days_from_civil(year, month, day), seconds: hour * 3600 + minute * 60 + second })
    }

    /// The date and time `seconds` after 1970-01-01 00:00, with no time zone applied.
    pub fn from_seconds(seconds: i64) -> Option<Self> {
        Some(Self {
            days: i32::try_from(seconds.div_euclid(SECONDS_PER_DAY)).ok()?,
            seconds: seconds.rem_euclid(SECONDS_PER_DAY) as u32,
        })
    }

    /// The date and time `seconds` earlier.
    pub fn minus_seconds(self, seconds: u64) -> Option<Self> {
        let total = i64::from(self.days) * SECONDS_PER_DAY + i64::from(self.seconds) - i64::try_from(seconds).ok()?;
        Some(Self {
            days: i32::try_from(total.div_euclid(SECONDS_PER_DAY)).ok()?,
            seconds: total.rem_euclid(SECONDS_PER_DAY) as u32,
        })
    }
}

/// ISO 8601 with no UTC offset ("2026-11-06T19:07:10"), as GUANO's `Timestamp` takes a local time.
impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.days);
        let (hour, minute, second) = (self.seconds / 3600, self.seconds / 60 % 60, self.seconds % 60);
        write!(f, "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
    }
}

fn digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0, |n, &b| b.is_ascii_digit().then(|| n * 10 + u32::from(b - b'0')))
}

/// Days since 1970-01-01 for a date in the proleptic Gregorian calendar (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400) as u32;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era as i32 - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097) as u32;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era as i32 + era * 400 + i32::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: &[u8], time: &[u8]) -> DateTime {
        DateTime::parse(date, time).unwrap()
    }

    #[test]
    fn subtracting_crosses_days_months_and_years() {
        assert_eq!(at(b"2026-11-06", b"19:08:08").minus_seconds(58).unwrap().to_string(), "2026-11-06T19:07:10");
        assert_eq!(at(b"2024-03-01", b"00:00:30").minus_seconds(120).unwrap().to_string(), "2024-02-29T23:58:30");
        assert_eq!(at(b"2027-01-01", b"00:00:10").minus_seconds(60).unwrap().to_string(), "2026-12-31T23:59:10");
    }

    #[test]
    fn from_seconds_matches_parse() {
        assert_eq!(DateTime::from_seconds(1_794_173_449), Some(at(b"2026-11-08", b"21:30:49")));
    }

    #[test]
    fn other_separators_are_accepted() {
        assert_eq!(at(b"2026_11_06", b"19.08.08").to_string(), "2026-11-06T19:08:08");
    }

    #[test]
    fn impossible_dates_and_times_are_refused() {
        for (date, time) in [
            (&b"2026-13-01"[..], &b"12:00:00"[..]),
            (b"2026-02-29", b"12:00:00"),
            (b"2026-04-31", b"12:00:00"),
            (b"2026-11-00", b"12:00:00"),
            (b"2026-11-06", b"24:00:00"),
            (b"2026-11-06", b"12:60:00"),
            (b"\0\0\0\0\0\0\0\0\0\0", b"\0\0\0\0\0\0\0\0"),
            (b"2026-11-6", b"12:00:00"),
        ] {
            assert_eq!(DateTime::parse(date, time), None, "{date:?} {time:?}");
        }
        assert!(DateTime::parse(b"2000-02-29", b"12:00:00").is_some());
        assert!(DateTime::parse(b"1900-02-29", b"12:00:00").is_none());
    }

    #[test]
    fn days_round_trip() {
        for days in [-1, 0, 59, 365, 10_957, 20_398, 20_766, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(2026, 11, 6), 20_763);
    }
}
