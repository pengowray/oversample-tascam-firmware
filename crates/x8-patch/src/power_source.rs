//! Running from the cells when a USB supply is weak, instead of POWER SAVE MODE: the decisions,
//! with no firmware addresses, so the host tests can run them. docs/HOMEBREW.md, "USB power and
//! POWER SAVE MODE (2026-11-11)", has the stock code they work with.
//!
//! With POWER SOURCE SELECT at AUTO and a USB supply that advertises less than 1.5 A, the stock
//! firmware asks "Is the AC adapter 1.5A or more?", and No turns on POWER SAVE MODE (44.1 and
//! 48 kHz only, no phantom power). The patch instead sets POWER SOURCE SELECT to BATTERY, which
//! takes no power from USB, when the cells are good enough for the stock firmware's own check.

/// POWER SOURCE SELECT (settings + 0x1D9).
pub const BATTERY: u8 = 0;
pub const AUTO: u8 = 1;

/// The stock thresholds for the cell level by BATTERY TYPE (alkaline, Ni-MH, lithium), at
/// 0x81726D6C. In BATTERY, when external power appears and the level is at or below the
/// threshold, the stock firmware switches to AUTO ("Battery Empty").
pub const STOCK_THRESHOLDS: [u8; 3] = [102, 102, 112];

/// Whether the cells can take over: a known battery type, a real reading (the battery task
/// keeps 0 and 0xFF for no reading), and a level above the stock threshold for that type.
pub fn cells_good(battery_type: u8, level: u8, thresholds: &[u8; 3]) -> bool {
    let Some(&threshold) = thresholds.get(battery_type as usize) else {
        return false;
    };
    level != 0 && level != 0xff && level > threshold
}

/// What the patch does with the stock "supply below 1.5 A" event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeakSupply {
    /// Let the stock question appear.
    Ask,
    /// Swallow the event and set POWER SOURCE SELECT to BATTERY.
    SwitchToCells,
    /// Swallow the event: the X8 already runs from the cells the patch chose.
    StayOnCells,
}

/// The answer to a weak USB supply. `switch_on`: the bat app's "Run from batteries on weak USB"
/// switch. `switched`: the patch set BATTERY and it is still set by the patch. `gave_up`: the patch
/// went back to AUTO since the X8 started, so the cells failed once and the stock question decides
/// until the next start.
pub fn on_weak_supply(
    switch_on: bool,
    power_source: u8,
    switched: bool,
    gave_up: bool,
    battery_type: u8,
    level: u8,
    thresholds: &[u8; 3],
) -> WeakSupply {
    if !switch_on || gave_up || !cells_good(battery_type, level, thresholds) {
        WeakSupply::Ask
    } else if power_source == AUTO {
        WeakSupply::SwitchToCells
    } else if switched {
        WeakSupply::StayOnCells
    } else {
        WeakSupply::Ask
    }
}

/// Whether to go back to AUTO, after the patch set BATTERY: when the setting is still BATTERY and
/// the cells are no longer good, as the stock firmware does when external power appears, or the
/// switch was turned off.
pub fn back_to_auto(
    switch_on: bool,
    switched: bool,
    power_source: u8,
    battery_type: u8,
    level: u8,
    thresholds: &[u8; 3],
) -> bool {
    switched && power_source == BATTERY && (!switch_on || !cells_good(battery_type, level, thresholds))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: [u8; 3] = STOCK_THRESHOLDS;

    use WeakSupply::*;

    fn answer(source: u8, switched: bool, gave_up: bool, battery_type: u8, level: u8) -> WeakSupply {
        on_weak_supply(true, source, switched, gave_up, battery_type, level, &T)
    }

    #[test]
    fn the_switch_off_leaves_the_stock_question() {
        assert_eq!(on_weak_supply(false, AUTO, false, false, 0, 120, &T), Ask);
        assert_eq!(on_weak_supply(false, BATTERY, true, false, 0, 120, &T), Ask);
        // Turned off while on the patch's cells: back to AUTO.
        assert!(back_to_auto(false, true, BATTERY, 0, 120, &T));
    }

    #[test]
    fn good_cells_take_over_from_a_weak_supply_in_auto() {
        assert_eq!(answer(AUTO, false, false, 0, 120), SwitchToCells);
        assert_eq!(answer(AUTO, false, false, 2, 113), SwitchToCells);
    }

    #[test]
    fn weak_or_missing_cells_leave_the_stock_question() {
        assert_eq!(answer(AUTO, false, false, 0, 102), Ask);
        assert_eq!(answer(AUTO, false, false, 2, 112), Ask);
        assert_eq!(answer(AUTO, false, false, 0, 0), Ask);
        assert_eq!(answer(AUTO, false, false, 0, 0xff), Ask);
        assert_eq!(answer(AUTO, false, false, 0, 20), Ask);
        assert_eq!(answer(AUTO, false, false, 3, 120), Ask);
    }

    /// After an unplug and replug the event can come again while the patch's BATTERY is set.
    #[test]
    fn stays_on_the_cells_the_patch_chose() {
        assert_eq!(answer(BATTERY, true, false, 0, 120), StayOnCells);
        // BATTERY set by the user: the stock code decides.
        assert_eq!(answer(BATTERY, false, false, 0, 120), Ask);
    }

    /// Once the cells have failed, unloaded cells recovering must not take over again.
    #[test]
    fn after_giving_up_the_stock_question_decides() {
        assert_eq!(answer(AUTO, false, true, 0, 120), Ask);
    }

    #[test]
    fn back_to_auto_only_after_the_patch_switched_and_the_cells_fade() {
        assert!(back_to_auto(true, true, BATTERY, 0, 100, &T));
        assert!(!back_to_auto(true, true, BATTERY, 0, 110, &T));
        assert!(!back_to_auto(true, false, BATTERY, 0, 100, &T));
        // The user set AUTO again: nothing to do.
        assert!(!back_to_auto(true, true, AUTO, 0, 100, &T));
    }
}
