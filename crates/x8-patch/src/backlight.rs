//! How often the bat screen is redrawn, with no firmware addresses, so the host tests can run it.
//! work/notes/backlight.md has the stock BACKLIGHT timer.
//!
//! GENERAL SETTINGS > BACKLIGHT is a time in seconds (settings + 0x26, signed): 0 is OFF (always
//! dim), 5 to 30 are the times, and below 0 is ALWAYS (-1, or 255 after POWER SAVE MODE has been on
//! and off, which saves it in one byte; 255 is then a real time). When the time passes with no
//! touch, button or wheel turn, task 1 dims the backlight to its lowest level and sets the dimmed
//! flag. The stock firmware never dims on external power.
//!
//! The dimmed screen can still be read, so the bat screen stays on and is redrawn less often,
//! which saves most of the drawing's share of the processor. The patch never writes the backlight
//! and never blanks the screen (B268 did, after 90 s; that saved little more and was dropped on
//! 2026-11-13).

pub const REDRAWS_PER_SECOND: u32 = 10;
/// While the recorder is recording: the stock interrupt and the SD card writes then leave the
/// app the least time. Drawing took 21% of wall time at 10 a second (B231).
pub const REDRAWS_PER_SECOND_RECORDING: u32 = 5;
/// While the app is idle (`BatApp::idle`: MANUAL's mix is muted, so the waterfall stands still)
/// or the stock BACKLIGHT timer has dimmed the backlight.
pub const REDRAWS_PER_SECOND_SLOW: u32 = 2;

/// Redraws a second. `dimmed`: the stock timer has dimmed the backlight. `input_since_redraw`: a
/// touch, button or wheel turn has reached the app since the last redraw, so its effect is drawn
/// at the full rate. The CPU test measures the drawing, so it keeps the usual rates.
pub fn redraws_per_second(recording: bool, idle: bool, dimmed: bool, input_since_redraw: bool, cpu_test: bool) -> u32 {
    if (idle || dimmed) && !input_since_redraw && !cpu_test {
        REDRAWS_PER_SECOND_SLOW
    } else if recording {
        REDRAWS_PER_SECOND_RECORDING
    } else {
        REDRAWS_PER_SECOND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_while_dimmed_or_idle() {
        assert_eq!(redraws_per_second(false, false, false, false, false), REDRAWS_PER_SECOND);
        assert_eq!(redraws_per_second(true, false, false, false, false), REDRAWS_PER_SECOND_RECORDING);
        assert_eq!(redraws_per_second(false, false, true, false, false), REDRAWS_PER_SECOND_SLOW);
        assert_eq!(redraws_per_second(true, false, true, false, false), REDRAWS_PER_SECOND_SLOW);
        assert_eq!(redraws_per_second(false, true, false, false, false), REDRAWS_PER_SECOND_SLOW);
    }

    /// With BACKLIGHT at OFF the light is always dim, so a touch must still be drawn at once.
    #[test]
    fn input_and_the_cpu_test_keep_the_usual_rates() {
        assert_eq!(redraws_per_second(false, false, true, true, false), REDRAWS_PER_SECOND);
        assert_eq!(redraws_per_second(true, true, true, true, false), REDRAWS_PER_SECOND_RECORDING);
        assert_eq!(redraws_per_second(false, false, true, false, true), REDRAWS_PER_SECOND);
        assert_eq!(redraws_per_second(true, false, true, false, true), REDRAWS_PER_SECOND_RECORDING);
    }
}
