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

/// Cell level readings in a row that are not good before the patch goes back to AUTO: about 7 s
/// of the battery task's messages. The stock firmware, on its cells with no external power,
/// counts the same number of low readings before each step towards its low battery power-off.
/// One reading does not decide, because the patch reads the battery task's newest reading, which
/// is not smoothed.
pub const LOW_READINGS: u32 = 100;

/// The count of readings in a row that are not good, after this one.
pub fn low_readings(so_far: u32, battery_type: u8, level: u8, thresholds: &[u8; 3]) -> u32 {
    if cells_good(battery_type, level, thresholds) { 0 } else { so_far.saturating_add(1) }
}

/// Whether to go back to AUTO, after the patch set BATTERY: when the setting is still BATTERY and
/// the cells have not been good for `LOW_READINGS` readings in a row, or the switch was turned
/// off.
pub fn back_to_auto(switch_on: bool, switched: bool, power_source: u8, low_readings: u32) -> bool {
    switched && power_source == BATTERY && (!switch_on || low_readings >= LOW_READINGS)
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
        // Turned off while on the patch's cells: back to AUTO, with good cells too.
        assert!(back_to_auto(false, true, BATTERY, 0));
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
        assert!(back_to_auto(true, true, BATTERY, LOW_READINGS));
        assert!(!back_to_auto(true, true, BATTERY, LOW_READINGS - 1));
        assert!(!back_to_auto(true, false, BATTERY, LOW_READINGS));
        // The user set AUTO again: nothing to do.
        assert!(!back_to_auto(true, true, AUTO, LOW_READINGS));
    }

    /// A dip under load for a few readings must not end the session on the cells.
    #[test]
    fn low_readings_count_only_in_a_row() {
        let mut count = 0;
        for _ in 0..LOW_READINGS - 1 {
            count = low_readings(count, 0, 100, &T);
        }
        assert!(!back_to_auto(true, true, BATTERY, count));
        count = low_readings(count, 0, 110, &T);
        assert_eq!(count, 0);
        for _ in 0..LOW_READINGS {
            count = low_readings(count, 0, 102, &T);
        }
        assert!(back_to_auto(true, true, BATTERY, count));
        // No cells (a reading under 36) counts as not good too.
        assert_eq!(low_readings(0, 0, 20, &T), 1);
        assert_eq!(low_readings(u32::MAX, 0, 20, &T), u32::MAX);
    }
}
