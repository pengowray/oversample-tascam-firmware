//! Runs the X8 from its cells when a USB supply is weak, instead of the stock question that leads to
//! POWER SAVE MODE. See docs/HOMEBREW.md, "USB power and POWER SAVE MODE", and
//! x8_patch::power_source for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use x8_patch::power_source::{AUTO, BATTERY, LOW_READINGS, WeakSupply, back_to_auto, low_readings, on_weak_supply};

use super::{Handler, read_byte, stock};

/// The bat app's "Run from batteries on weak USB" switch, copied on each pass of the app. On at start,
/// as the app's setting is, so it applies before the bat screen first opens.
pub(super) static SWITCH_ON: AtomicBool = AtomicBool::new(true);
/// Set when the patch set POWER SOURCE SELECT to BATTERY, until it sets AUTO again.
static SWITCHED: AtomicBool = AtomicBool::new(false);
/// Set when the patch went back to AUTO because the cells failed: until the next start, the stock
/// question decides, so cells that recover once unloaded do not take over again.
static GAVE_UP: AtomicBool = AtomicBool::new(false);
/// Cell level messages in a row with a reading that is not good, while the patch has BATTERY set.
static LOW_IN_A_ROW: AtomicU32 = AtomicU32::new(0);

/// The battery task's newest cell level. Not the level task 1 keeps from the battery task's
/// messages, which can be about 5 minutes old while task 1 sees external power.
fn cell_level() -> u8 {
    // SAFETY: a halfword the battery task stores whole; only read here.
    unsafe { ptr::read_volatile(stock::CELL_LEVEL_NOW) as u8 }
}

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
/// with the switch on and good cells: the event is swallowed and, in AUTO, POWER SOURCE SELECT set
/// to BATTERY. A cell level message (kind 3) after that: AUTO again if the cells have not been good
/// for `LOW_READINGS` messages in a row or the switch was turned off, and the stock check asks its
/// question.
pub(super) fn on_event(kind_ptr: *mut u16, kind: u16, value: i32) {
    let (source, battery_type, level) = (read_byte(stock::POWER_SOURCE), read_byte(stock::BATTERY_TYPE), cell_level());
    let (switch_on, switched) = (SWITCH_ON.load(Relaxed), SWITCHED.load(Relaxed));
    if kind == stock::KIND_USB_SUPPLY && value == 0 {
        let answer =
            on_weak_supply(switch_on, source, switched, GAVE_UP.load(Relaxed), battery_type, level, &thresholds());
        if answer != WeakSupply::Ask {
            // SAFETY: the kind field of the message copy.
            unsafe { ptr::write_volatile(kind_ptr, stock::KIND_SWALLOWED) };
        }
        if answer == WeakSupply::SwitchToCells {
            post_power_source(BATTERY);
            SWITCHED.store(true, Relaxed);
            LOW_IN_A_ROW.store(0, Relaxed);
        }
    } else if kind == stock::KIND_CELL_LEVEL && switched {
        let low = low_readings(LOW_IN_A_ROW.load(Relaxed), battery_type, level, &thresholds());
        LOW_IN_A_ROW.store(low, Relaxed);
        if !back_to_auto(switch_on, switched, source, low) {
            return;
        }
        post_power_source(AUTO);
        SWITCHED.store(false, Relaxed);
        // Only failing cells end it for the session; with the switch turned on again, the next
        // weak supply is answered again.
        if low >= LOW_READINGS {
            GAVE_UP.store(true, Relaxed);
        }
        // SAFETY: the stock check's latch word; task 9 writes it too, and a lost write only
        // delays the question by one check (50 scans).
        unsafe { ptr::write_volatile(stock::SUPPLY_LATCH, 0) };
    }
}

/// For the Debug info strip: POWER SOURCE SELECT (0 BATTERY, 1 AUTO) and what the patch has done
/// since the start: `-` nothing, `S` set BATTERY for a weak supply, `G` went back to AUTO.
pub(super) fn debug_state() -> (u8, char) {
    let done = if SWITCHED.load(Relaxed) {
        'S'
    } else if GAVE_UP.load(Relaxed) {
        'G'
    } else {
        '-'
    };
    (read_byte(stock::POWER_SOURCE), done)
}

/// The POWER SOURCE SELECT value to save at power-off: AUTO when the patch set BATTERY, so the
/// next start uses USB power again.
pub(super) fn saved_power_source() -> Option<u8> {
    (SWITCHED.load(Relaxed) && read_byte(stock::POWER_SOURCE) == BATTERY).then_some(AUTO)
}
