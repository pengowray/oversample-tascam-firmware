//! The phone's time and location, from the chimes the app reads: the phone remote's on the main
//! screen and any on the clock screen. The app passes each reading on to the host, which can set
//! its clock from it, log the location, and put both in a take's metadata.

use chime::format::TimeMessage;

/// A chime that gave the phone's time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    /// Seconds since 1970 UTC at the chime's first start symbol.
    pub(crate) unix_time: u32,
    pub(crate) timezone_minutes: i16,
    /// The app's sample at that start symbol.
    pub(crate) start_sample: i64,
    /// The app's sample where the decoder finished the chime.
    pub(crate) end_sample: u64,
    pub(crate) location: Option<(f64, f64)>,
}

impl Reading {
    pub(crate) fn new(message: &TimeMessage, start_sample: i64, end_sample: u64) -> Self {
        Self {
            unix_time: message.unix_time,
            timezone_minutes: message.timezone_minutes,
            start_sample,
            end_sample,
            location: message.latitude_longitude(),
        }
    }

    /// The phone's local time at the app's sample `now`, in ms since 1970-01-01 00:00 local.
    pub(crate) fn local_ms_at(&self, now: u64, rate_hz: u32) -> i64 {
        let elapsed_ms = (now as i64 - self.start_sample) * 1000 / rate_hz.max(1) as i64;
        (self.unix_time as i64 + self.timezone_minutes as i64 * 60) * 1000 + elapsed_ms
    }
}

/// The phone's time now, as the app passes it to the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhoneTime {
    /// The phone's local time when the app passed this on, in ms since 1970-01-01 00:00 local.
    pub local_ms: i64,
    /// The phone's offset from UTC, in minutes.
    pub timezone_minutes: i16,
    /// Latitude and longitude in degrees, when the chime carried them.
    pub location: Option<(f64, f64)>,
    /// Set the host's clock from it: the clock screen's button was pressed, or a phone remote
    /// chime was read with the setting for that on.
    pub set_clock: bool,
}

/// What the host's clock is doing, for the clock screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockState {
    /// Not asked to set it since the app started.
    Idle,
    /// Being set now.
    Setting,
    /// A take is in progress: the host sets the clock when it ends.
    AfterTake,
    /// Set, at this local time in seconds since 1970.
    Set(i64),
    /// The host tried and could not set it.
    Failed,
}
