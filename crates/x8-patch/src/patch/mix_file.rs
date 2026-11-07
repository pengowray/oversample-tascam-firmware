use core::ptr;
use core::sync::atomic::Ordering::Relaxed;

use crate::file_mask::without_master;

use super::state::{MIX_FILE_OFF, SCREEN_APP, TAKE_WITHOUT_MIX};
use super::{Handler, app_byte, read_byte, stock, system_mode};

/// Takes that the patch may change: MANUAL's normal recording to WAV. Other file
/// modes pair the master with a second format, and overdub and mix down write the
/// master on purpose (the render byte is non-zero).
fn plain_recording() -> bool {
    app_byte() == stock::C_MANUAL
        && read_byte(stock::FILE_FORMAT) < stock::FIRST_MP3_FORMAT
        && !matches!(read_byte(stock::FILE_MODE), 1 | 2)
        && read_byte(stock::RENDER_KIND) == 0
}

/// Takes the master out of the file mask. MANUAL's on/off words stay as they are,
/// so MANUAL's own tile still records its MIX file. Returns whether a master was
/// there to take out.
fn remove_master() -> bool {
    // SAFETY: the stock file mask and first slot byte. The stock code writes them
    // from the same tasks with no lock, and they are used while a file is open.
    unsafe {
        match without_master(ptr::read_volatile(stock::FILE_MASK)) {
            Some((mask, first)) => {
                ptr::write_volatile(stock::FILE_MASK, mask);
                ptr::write_volatile(stock::FIRST_FILE_SLOT, first);
                true
            }
            None => false,
        }
    }
}

/// Replaces the one call that opens a take's files for recording, in
/// `FUN_80dc99dc`, with r0 = the file names. The decision for the take is made
/// here, so a change of the switch cannot apply to a file that is already open.
pub(super) unsafe extern "C" fn open_take_hook(names: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 && read_byte(stock::FILES_OPEN) == 0 {
        let stale = TAKE_WITHOUT_MIX.swap(false, Relaxed);
        let wanted = MIX_FILE_OFF.load(Relaxed) && SCREEN_APP.load(Relaxed) == app_byte() as u32 && plain_recording();
        if stale || wanted {
            // SAFETY: the stock function that builds the mask. The stock open
            // function calls it from this same task.
            let build: Handler = unsafe { core::mem::transmute(stock::BUILD_FILE_MASK) };
            unsafe { build(0, 0, 0, 0) };
        }
        if wanted && remove_master() {
            TAKE_WITHOUT_MIX.store(true, Relaxed);
        }
    }
    // SAFETY: the stock function this call replaced, with its arguments.
    let open: Handler = unsafe { core::mem::transmute(stock::OPEN_TAKE_FILES) };
    unsafe { open(names, b, c, d) }
}

/// Replaces the call to the remaining time function at the end of the stock
/// `FUN_80db63d0`, which has just rebuilt the file mask from the on/off words.
/// The stock code rebuilds it during a take too, right after opening the files,
/// and a mask that named an unopened file would make the writers fail. Once the
/// take's files are closed, or for overdub or mix down, the rebuilt mask stands.
pub(super) unsafe extern "C" fn file_mask_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 && TAKE_WITHOUT_MIX.load(Relaxed) {
        if read_byte(stock::FILES_OPEN) != 0 && plain_recording() {
            remove_master();
        } else {
            TAKE_WITHOUT_MIX.store(false, Relaxed);
        }
    }
    // SAFETY: the stock function this call replaced, with its arguments.
    let remaining: Handler = unsafe { core::mem::transmute(stock::UPDATE_REMAINING_TIME) };
    unsafe { remaining(a, b, c, d) }
}
