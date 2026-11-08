//! Pauses in takes from a bat tile: REC is kept from pausing a take unless the app's switch allows
//! it, and each pause that does happen (REC with the switch on, or the recorder's own pauses) is
//! counted and timed for GUANO, with a mark posted where it ended (docs/HOMEBREW.md, "Pauses").
//!
//! `track` runs at the top of the DSP task's loop, which keeps running, slowly, while PLAY shows
//! MANUAL's own screen.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use super::state::BLOCKS;
use super::{blocks_per_second, read_byte, stock};

/// The take in progress started from a bat tile (set by `open_take_hook`).
pub(super) static TAKE_BAT: AtomicBool = AtomicBool::new(false);
/// The app's switch: REC may pause a take. Off by default. The context running the app sets it.
pub(super) static PAUSE_WITH_REC: AtomicBool = AtomicBool::new(false);
/// REC was kept from pausing a take, for the app's notice (task 1 sets it, the app context clears it).
pub(super) static REFUSED: AtomicBool = AtomicBool::new(false);
/// Marks the patch posted and the key hook has not seen yet: it passes them to the recorder only,
/// not to the app, where MARK would capture a call for TE.
pub(super) static OWN_MARKS: AtomicU32 = AtomicU32::new(0);

static PAUSES: AtomicU32 = AtomicU32::new(0);
static PAUSED_BLOCKS: AtomicU32 = AtomicU32::new(0);
static PAUSED_SINCE: AtomicU32 = AtomicU32::new(0);
static IN_PAUSE: AtomicBool = AtomicBool::new(false);

const RECORDING: u8 = 3;

/// A take's files are opening: start counting again.
pub(super) fn take_started(bat_take: bool) {
    TAKE_BAT.store(bat_take, Relaxed);
    PAUSES.store(0, Relaxed);
    PAUSED_BLOCKS.store(0, Relaxed);
    IN_PAUSE.store(false, Relaxed);
}

/// Whether REC should be kept from the recording screen's handler: a bat take recording, with
/// the app's switch off.
pub(super) fn keep_rec_from_pausing() -> bool {
    TAKE_BAT.load(Relaxed) && !PAUSE_WITH_REC.load(Relaxed) && read_byte(stock::TRANSPORT) == RECORDING
}

/// Follow the recorder (the DSP task).
pub(super) fn track() {
    if !TAKE_BAT.load(Relaxed) {
        return;
    }
    let transport = read_byte(stock::TRANSPORT);
    let paused = matches!(transport, 4 | 6);
    let now = BLOCKS.load(Relaxed);
    if paused && !IN_PAUSE.load(Relaxed) {
        PAUSED_SINCE.store(now, Relaxed);
        IN_PAUSE.store(true, Relaxed);
    } else if !paused && IN_PAUSE.load(Relaxed) {
        IN_PAUSE.store(false, Relaxed);
        PAUSES.fetch_add(1, Relaxed);
        PAUSED_BLOCKS.fetch_add(now.wrapping_sub(PAUSED_SINCE.load(Relaxed)), Relaxed);
        if transport == RECORDING {
            // The recorded position is where the pause ended.
            OWN_MARKS.fetch_add(1, Relaxed);
            super::remote::post_button(stock::MARK);
        }
    }
}

/// The take's pauses so far, for GUANO: how many, and their total length in ms. A pause the take
/// ended in counts, up to now.
pub(super) fn for_take() -> Option<(u32, u32)> {
    if !TAKE_BAT.load(Relaxed) {
        return None;
    }
    let mut pauses = PAUSES.load(Relaxed);
    let mut blocks = PAUSED_BLOCKS.load(Relaxed);
    if IN_PAUSE.load(Relaxed) {
        pauses += 1;
        blocks = blocks.wrapping_add(BLOCKS.load(Relaxed).wrapping_sub(PAUSED_SINCE.load(Relaxed)));
    }
    Some((pauses, (blocks as u64 * 1000 / blocks_per_second() as u64) as u32))
}
