//! The bat screen goes blank some time after the stock BACKLIGHT timer has dimmed the backlight:
//! the decisions, with no firmware addresses, so the host tests can run them.
//! work/notes/backlight.md has the stock timer.
//!
//! GENERAL SETTINGS > BACKLIGHT is a time in seconds (settings + 0x26, signed): 0 is OFF (always
//! dim), 5 to 30 are the times, and below 0 is ALWAYS (-1, or 255 after POWER SAVE MODE has been on
//! and off, which saves it in one byte; 255 is then a real time). When the time passes with no
//! touch, button or wheel turn, task 1 dims the backlight to its lowest level and sets the dimmed
//! flag. The stock firmware never dims on external power.
//!
//! The dimmed screen can still be read, so the bat screen keeps drawing until no touch, button or
//! wheel turn has come for `BLANK_AFTER_S`, or `BLANK_AFTER_TAKE_S` while a take is open (the
//! owner's choice, 2026-11-12). It then goes black and stops drawing, which saves the drawing's
//! share of the processor.

/// Seconds with no touch, button or wheel turn before a dimmed bat screen goes blank.
pub const BLANK_AFTER_S: u32 = 90;
/// The same while a take is open (recording or paused).
pub const BLANK_AFTER_TAKE_S: u32 = 180;

/// What a pass of the DSP task does with the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redraw {
    Nothing,
    /// Blank: the recorder's state still reaches the app (REC can be pressed while blank), but
    /// nothing is drawn.
    StatusOnly,
    Draw,
}

/// The pass's redraw. `due_slow`: a redraw is due at the slowest rate, which a blank screen uses
/// for the status. `due`: a redraw is due at the rate for the app's state.
pub fn redraw(blank: bool, due_slow: bool, due: bool) -> Redraw {
    match (blank, due_slow, due) {
        (true, true, _) => Redraw::StatusOnly,
        (true, false, _) => Redraw::Nothing,
        (false, _, true) => Redraw::Draw,
        (false, _, false) => Redraw::Nothing,
    }
}

/// Whether the bat screen is blank. `dimmed`: the stock timer has dimmed the backlight.
/// `idle_s`: seconds since the last touch, button or wheel turn. `take_open`: recording or paused.
/// The CPU test measures the drawing, so it keeps the screen.
pub fn blank(dimmed: bool, idle_s: u32, take_open: bool, cpu_test: bool) -> bool {
    let after = if take_open { BLANK_AFTER_TAKE_S } else { BLANK_AFTER_S };
    dimmed && idle_s >= after && !cpu_test
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_after_90_s_or_3_min_in_a_take() {
        assert!(!blank(true, 89, false, false));
        assert!(blank(true, 90, false, false));
        assert!(!blank(true, 179, true, false));
        assert!(blank(true, 180, true, false));
    }

    /// With ALWAYS or on external power the stock timer never dims, so the screen stays.
    #[test]
    fn only_while_dimmed_and_not_in_the_cpu_test() {
        assert!(!blank(false, 1000, false, false));
        assert!(!blank(true, 1000, false, true));
    }

    /// In record standby a redraw is due at the full rate, 10 a second: a blank screen must still
    /// not draw over its black frame.
    #[test]
    fn a_blank_screen_never_draws() {
        for due_slow in [false, true] {
            for due in [false, true] {
                assert_ne!(redraw(true, due_slow, due), Redraw::Draw);
            }
        }
        assert_eq!(redraw(true, true, true), Redraw::StatusOnly);
        assert_eq!(redraw(false, false, true), Redraw::Draw);
        assert_eq!(redraw(false, true, false), Redraw::Nothing);
    }
}
