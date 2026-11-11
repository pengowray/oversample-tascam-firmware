//! The bat screen goes dark when the stock BACKLIGHT timer dims the backlight: the decision, with
//! no firmware addresses, so the host tests can run it. work/notes/backlight.md has the stock
//! timer.
//!
//! GENERAL SETTINGS > BACKLIGHT is a time in seconds (settings + 0x26, signed): 0 is OFF, 5 to 30
//! are the times, and below 0 is ALWAYS (-1, or 255 after POWER SAVE MODE has been on and off,
//! which saves it in one byte; 255 is then a real time). When the time passes with no touch,
//! button or wheel turn, task 1 dims the backlight to its lowest level and sets the dimmed flag.
//! The stock firmware never dims on external power.

/// What a pass of the DSP task does with the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redraw {
    Nothing,
    /// Dark: the recorder's state still reaches the app (REC can be pressed while dark), but
    /// nothing is drawn.
    StatusOnly,
    Draw,
}

/// The pass's redraw. `due_slow`: a redraw is due at the slowest rate, which a dark screen uses
/// for the status. `due`: a redraw is due at the rate for the app's state.
pub fn redraw(dark: bool, due_slow: bool, due: bool) -> Redraw {
    match (dark, due_slow, due) {
        (true, true, _) => Redraw::StatusOnly,
        (true, false, _) => Redraw::Nothing,
        (false, _, true) => Redraw::Draw,
        (false, _, false) => Redraw::Nothing,
    }
}

/// Whether the bat screen goes dark. `dimmed`: the stock dimmed flag. `backlight_s`: BACKLIGHT.
/// With OFF the backlight is dimmed again straight after every wake, so the screen keeps drawing.
/// The CPU test measures the drawing, so it keeps the screen too.
pub fn dark(dimmed: u8, backlight_s: i16, cpu_test: bool) -> bool {
    dimmed != 0 && backlight_s > 0 && !cpu_test
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_only_when_the_timer_dimmed_the_backlight() {
        assert!(dark(1, 30, false));
        assert!(dark(1, 255, false));
        assert!(!dark(0, 30, false));
    }

    /// In record standby a redraw is due at the full rate, 10 a second: a dark screen must still
    /// not draw over its black frame.
    #[test]
    fn a_dark_screen_never_draws() {
        for due_slow in [false, true] {
            for due in [false, true] {
                assert_ne!(redraw(true, due_slow, due), Redraw::Draw);
            }
        }
        assert_eq!(redraw(true, true, true), Redraw::StatusOnly);
        assert_eq!(redraw(false, false, true), Redraw::Draw);
        assert_eq!(redraw(false, true, false), Redraw::Nothing);
    }

    #[test]
    fn off_always_and_the_cpu_test_keep_the_screen() {
        assert!(!dark(1, 0, false));
        assert!(!dark(1, -1, false));
        assert!(!dark(1, 30, true));
    }
}
