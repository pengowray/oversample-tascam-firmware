//! The phone time screen: what the chimes read, the phone's time and location from the latest
//! one, how the X8's clock compares, and a button that sets the X8's clock to the phone's time.

use core::fmt;

use chime::UnreadableReason;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use u8g2_fonts::{FontRenderer, fonts};

use super::{
    BODY, BODY_BOLD, CENTER, Fb, Fixed, LEFT, SMALL, VALUE, fill, mix_silence_lines, no_input_notice, rate_warning,
    rounded, text,
};
use crate::chime_test::{Channel, ChimeClock, Failure, FailureReason};
use crate::layout::{CLOCK_BACK, CLOCK_SET};
use crate::palette::*;
use crate::{BatApp, ClockState, Feed};

/// Large enough to read at arm's length with the phone in the other hand. Its digits are
/// all the same width, so the clock does not move as it ticks.
pub(super) const CLOCK: FontRenderer = FontRenderer::new::<fonts::u8g2_font_logisoso50_tn>();

pub(crate) const TITLE: &str = "PHONE TIME";
pub(super) const LABEL_READ: &str = "Phone time from the last chime";
pub(super) const LABEL_NONE: &str = "No chime read yet";
pub(super) const ROW_LABELS: [&str; 5] =
    ["Chimes read", "Chimes failed", "Last carrier dBFS", "Last SNR dB", "17-19 kHz now"];
pub(super) const SNR_NOT_MEASURED: &str = "not measured";
pub(super) const HELP: [&str; 2] =
    ["On the phone, press \"Play chimes for 1 minute\".", "Hold the phone steady near a mic, with the volume up."];
pub(super) const ANALYSIS: [&str; 3] = [
    "Bat analysis is stopped while this screen is open.",
    "The waterfall gets grey columns for this time.",
    "A recording in progress continues.",
];

pub(super) const SET_LABEL: &str = "Set X8 clock";
pub(super) const LOCATION_NONE: &str = "Location: not in this chime";
pub(super) const STATE_SETTING: &str = "Setting the X8 clock...";
pub(super) const STATE_AFTER_TAKE: &str = "X8 clock will be set when this take ends.";
pub(super) const STATE_FAILED: &str = "Could not set the X8 clock. Try again.";

const LABEL_Y: i32 = 44;
pub(crate) const CLOCK_Y: i32 = 102;
const DATE_Y: i32 = 126;
const AGE_Y: i32 = 144;
const LOCATION_Y: i32 = 160;
/// The table: a label and up to two columns of values.
const TABLE: Rectangle = Rectangle::new(Point::new(6, 168), Size::new(308, 124));
const TABLE_LABEL_X: i32 = 16;
pub(crate) const COLUMN_X: [i32; 2] = [124, 222];
const COLUMN_W: u32 = 84;
const HEADER_Y: i32 = 184;
pub(crate) const ROW_Y: i32 = 202;
pub(crate) const ROW_H: i32 = 18;
const BAR_H: u32 = 5;
/// The range of the 17 to 19 kHz bar, in dBFS. It reaches 0 so that an input close to
/// overload shows. It is grey at every level, because the level a chime needs is not known.
const BAR_RANGE_DB: (f32, f32) = (-100.0, 0.0);
pub(crate) const FAILURE_Y: i32 = 308;
/// Above the debug strip, which covers y 334 to 398.
const MIX_NOTICE_Y: [i32; 2] = [310, 326];
const HELP_Y: [i32; 2] = [326, 342];
const ANALYSIS_Y: [i32; 3] = [364, 380, 396];
/// What the X8's clock is doing, just above the buttons.
const STATE_Y: i32 = 418;

/// The names of the blocks being listened to, as Listen to names them.
pub(crate) fn columns(app: &BatApp) -> [Option<&'static str>; 2] {
    match (app.feed(), app.inputs()) {
        (_, [None, _]) => [None, None],
        (Feed::Mix, _) => [Some("Mix, left side"), Some("Mix, right side")],
        (Feed::Inputs, inputs) => inputs.map(|input| input.map(|input| input.long)),
    }
}

/// A length of time, in the unit that keeps the line short: seconds, minutes, hours, then days.
struct Span(u64);

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            s @ 0..60 => write!(f, "{s} s"),
            s @ 60..3600 => write!(f, "{} min", s / 60),
            s @ 3600..172_800 => write!(f, "{} h", s / 3600),
            s => write!(f, "{} days", s / 86_400),
        }
    }
}

/// How the X8's clock compares with the phone's, and how long ago the latest chime was read.
pub(super) struct Age {
    /// The X8's clock less the phone's, in seconds, when the X8's is known.
    pub(super) x8_minus_phone: Option<i64>,
    pub(super) read_s_ago: u64,
}

impl fmt::Display for Age {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.x8_minus_phone {
            None => f.write_str("X8 clock time not known yet.")?,
            Some(0) => f.write_str("X8 clock same as phone.")?,
            Some(d) if d < 0 => write!(f, "X8 clock {} behind phone.", Span(d.unsigned_abs()))?,
            Some(d) => write!(f, "X8 clock {} ahead of phone.", Span(d.unsigned_abs()))?,
        }
        write!(f, " Chime read {} ago.", Span(self.read_s_ago))
    }
}

/// The location from the latest chime, in degrees: 5 decimals is about 1 m, the chime's precision.
struct Location(f64, f64);

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Location: {}, {}", Fixed::new(self.0, 5), Fixed::new(self.1, 5))
    }
}

/// The set button's line: what the X8's clock is doing.
struct StateLine(ClockState);

impl fmt::Display for StateLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ClockState::Idle => Ok(()),
            ClockState::Setting => f.write_str(STATE_SETTING),
            ClockState::AfterTake => f.write_str(STATE_AFTER_TAKE),
            ClockState::Failed => f.write_str(STATE_FAILED),
            ClockState::Set(seconds) => {
                let t = seconds.rem_euclid(86_400);
                write!(f, "X8 clock set to {:02}:{:02}:{:02}.", t / 3600, t / 60 % 60, t % 60)
            }
        }
    }
}

/// The date and UTC offset under the clock.
pub(super) struct DateLine(pub(super) ChimeClock);

impl fmt::Display for DateLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ChimeClock { local, timezone_minutes, .. } = self.0;
        let size = timezone_minutes.unsigned_abs();
        let sign = if timezone_minutes < 0 { '-' } else { '+' };
        write!(f, "{:04}-{:02}-{:02}  UTC{sign}{:02}:{:02}", local.year, local.month, local.day, size / 60, size % 60)
    }
}

/// Why the latest chime could not be read.
pub(super) struct FailureLine(pub(super) &'static str, pub(super) Failure);

impl fmt::Display for FailureLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let FailureLine(name, Failure { reason, bytes, .. }) = *self;
        write!(f, "{name}: ")?;
        match reason {
            FailureReason::FramesLost => f.write_str("input frames lost during the chime"),
            FailureReason::Decoder(UnreadableReason::SignalLost) => {
                write!(f, "carrier stopped after {bytes} complete byte{}", if bytes == 1 { "" } else { "s" })
            }
            FailureReason::Decoder(UnreadableReason::BadCrc) => f.write_str("CRC check failed"),
            FailureReason::Decoder(UnreadableReason::TooManyBytes) => f.write_str("chime was too long"),
            FailureReason::Decoder(UnreadableReason::ConflictingReads) => {
                f.write_str("two different readings passed CRC")
            }
            FailureReason::Decoder(UnreadableReason::MessageRefused) => f.write_str("message refused after CRC passed"),
            FailureReason::Decoder(UnreadableReason::BadSymbol) => f.write_str("symbol length not recognised"),
        }
    }
}

fn level_bar(fb: Fb, x: i32, y: i32, dbfs: f32) {
    let (low, high) = BAR_RANGE_DB;
    let share = ((dbfs - low) / (high - low)).clamp(0.0, 1.0);
    fill(fb, Rectangle::new(Point::new(x, y), Size::new(COLUMN_W, BAR_H)), BUTTON);
    fill(fb, Rectangle::new(Point::new(x, y), Size::new((COLUMN_W as f32 * share) as u32, BAR_H)), TEXT_MUTED);
}

fn column_values(fb: Fb, x: i32, channel: &Channel) {
    let row = |i: i32| ROW_Y + i * ROW_H;
    text(fb, &SMALL, format_args!("{}", channel.read), x, row(0), LEFT, TEXT);
    text(fb, &SMALL, format_args!("{}", channel.failed), x, row(1), LEFT, TEXT);
    match channel.carrier_dbfs {
        Some(level) => text(fb, &SMALL, format_args!("{}", Fixed::new(level, 1)), x, row(2), LEFT, TEXT),
        None => text(fb, &SMALL, "--", x, row(2), LEFT, TEXT_MUTED),
    };
    match channel.snr_db {
        Some(snr) if snr.is_nan() => text(fb, &SMALL, SNR_NOT_MEASURED, x, row(3), LEFT, TEXT_MUTED),
        Some(snr) => text(fb, &SMALL, format_args!("{}", Fixed::signed(snr, 0)), x, row(3), LEFT, TEXT),
        None => text(fb, &SMALL, "--", x, row(3), LEFT, TEXT_MUTED),
    };
    text(fb, &SMALL, format_args!("{} dBFS", Fixed::new(channel.band_dbfs, 0)), x, row(4), LEFT, TEXT);
    level_bar(fb, x, row(4) + 4, channel.band_dbfs);
}

fn buttons(app: &BatApp, fb: Fb) {
    rounded(fb, CLOCK_BACK, 4, BUTTON);
    text(fb, &BODY_BOLD, "Back", CLOCK_BACK.center().x, CLOCK_BACK.top_left.y + 28, CENTER, TEXT);
    // Filled in the accent colour once a chime has given a time to set.
    let ready = app.phone.is_some();
    rounded(fb, CLOCK_SET, 4, if ready { ACCENT } else { BUTTON });
    let colour = if ready { TEXT } else { TEXT_MUTED };
    text(fb, &BODY_BOLD, SET_LABEL, CLOCK_SET.center().x, CLOCK_SET.top_left.y + 28, CENTER, colour);
    let colour = if app.clock_state() == ClockState::Failed { METER_YELLOW } else { TEXT_MUTED };
    text(fb, &SMALL, format_args!("{}", StateLine(app.clock_state())), 16, STATE_Y, LEFT, colour);
}

pub(super) fn draw(app: &BatApp, fb: Fb) {
    buttons(app, fb);
    // With the wrong sample rate or no input, nothing can be read, so these say why and
    // nothing else shows.
    if let Some(rate) = app.rate_warning() {
        rate_warning(fb, rate, app.input_kind());
        return;
    }
    if app.inputs()[0].is_none() {
        no_input_notice(fb);
        return;
    }

    let clock = app.chime.clock(app.samples);
    text(fb, &BODY, if clock.is_some() { LABEL_READ } else { LABEL_NONE }, 160, LABEL_Y, CENTER, TEXT_MUTED);
    match clock {
        Some(clock) => {
            let ChimeClock { local, read_s_ago, .. } = clock;
            let time = format_args!("{:02}:{:02}:{:02}", local.hour, local.minute, local.second);
            text(fb, &CLOCK, time, 160, CLOCK_Y, CENTER, TEXT);
            text(fb, &VALUE, format_args!("{}", DateLine(clock)), 160, DATE_Y, CENTER, TEXT);
            let x8_minus_phone = app.clocks_now().map(|(phone, x8)| x8 - phone);
            text(fb, &SMALL, format_args!("{}", Age { x8_minus_phone, read_s_ago }), 160, AGE_Y, CENTER, TEXT_MUTED);
            match app.phone.and_then(|reading| reading.location) {
                Some((latitude, longitude)) => text(
                    fb,
                    &SMALL,
                    format_args!("{}", Location(latitude, longitude)),
                    160,
                    LOCATION_Y,
                    CENTER,
                    TEXT_MUTED,
                ),
                None => text(fb, &SMALL, LOCATION_NONE, 160, LOCATION_Y, CENTER, TEXT_MUTED),
            };
        }
        None => {
            text(fb, &CLOCK, "--:--:--", 160, CLOCK_Y, CENTER, TEXT_MUTED);
        }
    }

    rounded(fb, TABLE, 4, PANEL);
    let names = columns(app);
    for (name, x) in names.iter().zip(COLUMN_X) {
        if let Some(name) = name {
            text(fb, &SMALL, *name, x, HEADER_Y, LEFT, TEXT_MUTED);
        }
    }
    for (i, label) in ROW_LABELS.iter().enumerate() {
        text(fb, &SMALL, *label, TABLE_LABEL_X, ROW_Y + i as i32 * ROW_H, LEFT, TEXT_MUTED);
    }
    for ((name, channel), x) in names.iter().zip(&app.chime.channels).zip(COLUMN_X) {
        if name.is_some() {
            column_values(fb, x, channel);
        }
    }

    if let Some((failure, name)) = app.chime.failure.and_then(|failure| Some((failure, names[failure.channel]?))) {
        text(fb, &SMALL, format_args!("{}", FailureLine(name, failure)), 16, FAILURE_Y, LEFT, METER_YELLOW);
    }

    match app.mix_silence() {
        // The same words as the notice on the main screen, because the app hears nothing.
        Some(silence) => {
            for (line, y) in mix_silence_lines(silence).iter().zip(MIX_NOTICE_Y) {
                text(fb, &BODY_BOLD, *line, 16, y, LEFT, TEXT);
            }
        }
        // Brighter until a chime is read, because this is what to do first.
        None => {
            let colour = if clock.is_some() { TEXT_MUTED } else { TEXT };
            for (line, y) in HELP.iter().zip(HELP_Y) {
                text(fb, &SMALL, *line, 16, y, LEFT, colour);
            }
        }
    }
    for (line, y) in ANALYSIS.iter().zip(ANALYSIS_Y) {
        text(fb, &SMALL, *line, 16, y, LEFT, TEXT_MUTED);
    }
}

/// Test helpers: each line that has a limit, with its width and the room for it.
#[cfg(test)]
pub(crate) fn lines_with_widths() -> std::vec::Vec<(std::string::String, i32, i32)> {
    use std::string::ToString;
    use std::vec::Vec;

    let width = |font: &FontRenderer, line: &str| {
        font.get_rendered_dimensions(line, Point::zero(), u8g2_fonts::types::VerticalPosition::Baseline)
            .map_or(0, |d| d.advance.x)
    };
    let mut lines = Vec::new();
    let mut add = |font: &FontRenderer, line: &str, room: i32| lines.push((line.to_string(), width(font, line), room));
    let full = 320 - 2 * 16;
    add(&BODY, LABEL_READ, full);
    add(&BODY, LABEL_NONE, full);
    for line in HELP.iter().chain(&ANALYSIS) {
        add(&SMALL, line, full);
    }
    add(&VALUE, "2026-11-07  UTC+10:00", full);
    for x8_minus_phone in [None, Some(0), Some(-59), Some(-3599), Some(-172_799), Some(999 * 86_400)] {
        add(&SMALL, &Age { x8_minus_phone, read_s_ago: 3599 }.to_string(), full);
    }
    add(&SMALL, &Location(-89.99999, -179.99999).to_string(), full);
    add(&SMALL, LOCATION_NONE, full);
    for state in [ClockState::Setting, ClockState::AfterTake, ClockState::Failed, ClockState::Set(86_399)] {
        add(&SMALL, &StateLine(state).to_string(), full);
    }
    add(&BODY_BOLD, SET_LABEL, CLOCK_SET.size.width as i32 - 12);
    for label in ROW_LABELS {
        add(&SMALL, label, COLUMN_X[0] - TABLE_LABEL_X - 4);
    }
    let column = COLUMN_X[1] - COLUMN_X[0] - 4;
    for value in ["Mix, right side", "Mix, left side", "EXT IN right", "Input 3", SNR_NOT_MEASURED, "-100 dBFS"] {
        add(&SMALL, value, column);
    }
    let reasons = [
        FailureReason::Decoder(UnreadableReason::SignalLost),
        FailureReason::Decoder(UnreadableReason::BadCrc),
        FailureReason::Decoder(UnreadableReason::TooManyBytes),
        FailureReason::Decoder(UnreadableReason::ConflictingReads),
        FailureReason::Decoder(UnreadableReason::MessageRefused),
        FailureReason::Decoder(UnreadableReason::BadSymbol),
        FailureReason::FramesLost,
    ];
    for reason in reasons {
        let failure = Failure { channel: 0, reason, bytes: 22 };
        add(&SMALL, &FailureLine("Mix, right side", failure).to_string(), full);
    }
    lines
}

/// Test helpers: the screen's strings as text.
#[cfg(test)]
pub(crate) mod chime_text {
    use std::format;
    use std::string::{String, ToString};

    use chime::format::LocalTime;

    use super::{Age, ChimeClock, DateLine};

    pub(crate) fn age(seconds: u64) -> String {
        Age { x8_minus_phone: None, read_s_ago: seconds }.to_string()
    }

    pub(crate) fn compared(x8_minus_phone: i64, seconds: u64) -> String {
        Age { x8_minus_phone: Some(x8_minus_phone), read_s_ago: seconds }.to_string()
    }

    pub(crate) fn date(local_seconds: i64, timezone_minutes: i16) -> String {
        let local = LocalTime::from_epoch_seconds(local_seconds);
        format!("{}", DateLine(ChimeClock { local, timezone_minutes, read_s_ago: 0 }))
    }
}
