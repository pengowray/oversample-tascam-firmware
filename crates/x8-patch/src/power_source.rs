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

/// Whether to answer a weak USB supply by running from the cells.
pub fn run_from_cells(power_source: u8, battery_type: u8, level: u8, thresholds: &[u8; 3]) -> bool {
    power_source == AUTO && cells_good(battery_type, level, thresholds)
}

/// Whether to go back to AUTO, after the patch set BATTERY: when the setting is still BATTERY and
/// the cells are no longer good, as the stock firmware does when external power appears.
pub fn back_to_auto(switched: bool, power_source: u8, battery_type: u8, level: u8, thresholds: &[u8; 3]) -> bool {
    switched && power_source == BATTERY && !cells_good(battery_type, level, thresholds)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: [u8; 3] = STOCK_THRESHOLDS;

    #[test]
    fn good_cells_take_over_from_a_weak_supply_in_auto() {
        assert!(run_from_cells(AUTO, 0, 120, &T));
        assert!(run_from_cells(AUTO, 2, 113, &T));
    }

    #[test]
    fn weak_or_missing_cells_leave_the_stock_question() {
        assert!(!run_from_cells(AUTO, 0, 102, &T));
        assert!(!run_from_cells(AUTO, 2, 112, &T));
        assert!(!run_from_cells(AUTO, 0, 0, &T));
        assert!(!run_from_cells(AUTO, 0, 0xff, &T));
        assert!(!run_from_cells(AUTO, 0, 20, &T));
    }

    #[test]
    fn only_from_auto_and_a_known_battery_type() {
        assert!(!run_from_cells(BATTERY, 0, 120, &T));
        assert!(!run_from_cells(AUTO, 3, 120, &T));
    }

    #[test]
    fn back_to_auto_only_after_the_patch_switched_and_the_cells_fade() {
        assert!(back_to_auto(true, BATTERY, 0, 100, &T));
        assert!(!back_to_auto(true, BATTERY, 0, 110, &T));
        assert!(!back_to_auto(false, BATTERY, 0, 100, &T));
        // The user set AUTO again: nothing to do.
        assert!(!back_to_auto(true, AUTO, 0, 100, &T));
    }
}
