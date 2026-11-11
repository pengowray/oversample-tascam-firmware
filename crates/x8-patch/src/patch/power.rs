//! Runs the X8 from its cells when a USB supply is weak, instead of the stock question that leads to
//! POWER SAVE MODE. See docs/HOMEBREW.md, "USB power and POWER SAVE MODE", and
//! x8_patch::power_source for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

use x8_patch::power_source::{AUTO, BATTERY, back_to_auto, run_from_cells};

use super::{Handler, read_byte, stock};

/// Set when the patch set POWER SOURCE SELECT to BATTERY, until it sets AUTO again.
static SWITCHED: AtomicBool = AtomicBool::new(false);

fn thresholds() -> [u8; 3] {
    // SAFETY: a constant table in the stock image.
    unsafe { ptr::read_volatile(stock::SOURCE_THRESHOLDS) }
}

fn post_power_source(value: u8) {
    // SAFETY: the stock function that posts a system message, called from task 1 as the stock
    // external power handler calls it.
    let post: Handler = unsafe { core::mem::transmute(stock::POST_SYSTEM_MESSAGE) };
    unsafe { post(stock::MSG_POWER_SOURCE, value as usize, 0, 0) };
}

/// Task 1, for each input event, before the stock handling. A weak USB supply (kind 8, value 0)
/// in AUTO with good cells: the event is swallowed and POWER SOURCE SELECT set to BATTERY. A new
/// cell level (kind 3) after that: AUTO again if the cells are no longer good.
pub(super) fn on_event(kind_ptr: *mut u16, kind: u16, value: i32) {
    let (source, battery_type, level) =
        (read_byte(stock::POWER_SOURCE), read_byte(stock::BATTERY_TYPE), read_byte(stock::CELL_LEVEL));
    if kind == stock::KIND_USB_SUPPLY && value == 0 && run_from_cells(source, battery_type, level, &thresholds()) {
        // SAFETY: the kind field of the message copy.
        unsafe { ptr::write_volatile(kind_ptr, stock::KIND_SWALLOWED) };
        post_power_source(BATTERY);
        SWITCHED.store(true, Relaxed);
    } else if kind == stock::KIND_CELL_LEVEL
        && back_to_auto(SWITCHED.load(Relaxed), source, battery_type, level, &thresholds())
    {
        post_power_source(AUTO);
        SWITCHED.store(false, Relaxed);
    }
}

/// The POWER SOURCE SELECT value to save at power-off: AUTO when the patch set BATTERY, so the
/// next start uses USB power again.
pub(super) fn saved_power_source() -> Option<u8> {
    (SWITCHED.load(Relaxed) && read_byte(stock::POWER_SOURCE) == BATTERY).then_some(AUTO)
}
