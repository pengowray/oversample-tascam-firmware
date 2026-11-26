//! Runs the X8 from its cells when a USB supply is weak, instead of the stock question that leads to
//! POWER SAVE MODE. See docs/HOMEBREW.md, "USB power and POWER SAVE MODE", and
//! x8_patch::power_source for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use bat_app::{BatApp, Battery};
use x8_patch::power_source::{
    AUTO, BATTERY, LOW_READINGS, WeakSupply, back_to_auto, battery_eighths, low_readings, on_weak_supply, smooth_level,
};

use super::state::BLOCKS;
use super::{Handler, blocks_per_second, read_byte, stock};

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
    let (switch_on, switched) = (super::switches::on(super::switches::CELLS_ON_WEAK_USB), SWITCHED.load(Relaxed));
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

/// The cell level the title bar's battery shows, smoothed as the stock icon smooths it, and when
/// it was last moved. 0 until the first reading.
static SHOWN_LEVEL: AtomicU32 = AtomicU32::new(0);
static SHOWN_AT: AtomicU32 = AtomicU32::new(0);
/// The stock icon compares its level with the reading every 12 calls of task 2's 50 ms loop.
const SMOOTH_MS: u32 = 600;

/// Pass the app the battery as the stock title bars show it. From the context running the app,
/// before each redraw.
pub(super) fn show_battery(app: &mut BatApp<'static>) {
    if read_byte(stock::ON_CELLS_ONLY) != 1 {
        app.set_battery(Battery::External);
        return;
    }
    let now = BLOCKS.load(Relaxed);
    let reading = cell_level();
    let shown = SHOWN_LEVEL.load(Relaxed) as u8;
    if shown == 0 || now.wrapping_sub(SHOWN_AT.load(Relaxed)) >= SMOOTH_MS * blocks_per_second() / 1000 {
        // As the stock icon: 0xFF (and 0, no reading) keeps the level shown.
        let next = match reading {
            0 | 0xff => shown,
            _ if shown == 0 => reading,
            _ => smooth_level(shown, reading),
        };
        SHOWN_LEVEL.store(next.into(), Relaxed);
        SHOWN_AT.store(now, Relaxed);
    }
    // SAFETY: a constant table in the stock image.
    let ranges = unsafe { ptr::read_volatile(stock::BATTERY_ICON_RANGES) };
    let level = SHOWN_LEVEL.load(Relaxed) as u8;
    // No reading yet: full, as before.
    let eighths = battery_eighths(level, read_byte(stock::BATTERY_TYPE), &ranges).unwrap_or(8);
    app.set_battery(Battery::Cells(eighths));
}
