//! The bat app's switches that task 1 and the stock hooks act on, as bits of one word. The context
//! running the app writes the whole word on each pass (`copy_from_app`); the others only read it.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use bat_app::BatApp;

/// MIX file off, while BAT DETECTOR is open: `mix_file::open_take_hook` leaves out the master file.
pub(super) const MIX_FILE_OFF: u32 = 1 << 0;
/// GUANO metadata on: `mix_file::open_take_hook` marks the take for GUANO.
pub(super) const GUANO: u32 = 1 << 1;
/// Run from batteries on weak USB (`power`).
pub(super) const CELLS_ON_WEAK_USB: u32 = 1 << 2;
/// Tests > Backlight off when dimmed (`backlight`).
pub(super) const BACKLIGHT_OFF: u32 = 1 << 3;
/// Pause with REC: REC may pause a take (`pauses`).
pub(super) const PAUSE_WITH_REC: u32 = 1 << 4;

/// At start, as the app's defaults are, so that Run from batteries on weak USB applies before the
/// bat screen first opens. Task 1 then sets that bit from the saved settings (`keys`).
static SWITCHES: AtomicU32 = AtomicU32::new(GUANO | CELLS_ON_WEAK_USB);

pub(super) fn on(switch: u32) -> bool {
    SWITCHES.load(Relaxed) & switch != 0
}

/// Task 1, once at start, before the app runs, so nothing else writes the word yet.
pub(super) fn set_cells_on_weak_usb(on: bool) {
    let others = SWITCHES.load(Relaxed) & !CELLS_ON_WEAK_USB;
    SWITCHES.store(if on { others | CELLS_ON_WEAK_USB } else { others }, Relaxed);
}

/// The context running the app, on each pass. `mix_file_off`: the app's MIX file switch is off
/// and BAT DETECTOR is open.
pub(super) fn copy_from_app(app: &BatApp<'static>, mix_file_off: bool) {
    let bit = |switch: u32, on: bool| if on { switch } else { 0 };
    SWITCHES.store(
        bit(MIX_FILE_OFF, mix_file_off)
            | bit(GUANO, app.guano())
            | bit(CELLS_ON_WEAK_USB, app.cells_on_weak_usb())
            | bit(BACKLIGHT_OFF, app.backlight_off())
            | bit(PAUSE_WITH_REC, app.pause_with_rec()),
        Relaxed,
    );
}
