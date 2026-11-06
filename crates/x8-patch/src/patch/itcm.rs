use core::ptr;
use core::sync::atomic::Ordering::Relaxed;

use super::blocks_per_second;
use super::state::{BLOCKS, ITCM_CHECKED_AT, ITCM_OK, ITCM_SUM};

unsafe extern "C" {
    /// The hot code's place in ITCM and its copy in the patch (x8patch.ld).
    static __itcm_start: u8;
    static __itcm_end: u8;
    static __itcm_load: u8;
}

fn itcm_code() -> &'static [u32] {
    // SAFETY: linker symbols bounding the .itcm section, 8-byte aligned.
    unsafe {
        let start = &raw const __itcm_start as *const u32;
        let end = &raw const __itcm_end as *const u32;
        core::slice::from_raw_parts(start, end.offset_from(start) as usize)
    }
}

fn itcm_sum() -> u32 {
    itcm_code().iter().fold(0u32, |a, &w| a.rotate_left(1) ^ w)
}

/// Copy the hot code into ITCM. Must run before anything calls it.
pub(super) fn load_itcm() {
    let code = itcm_code();
    // SAFETY: the load copy is as long as the section; ITCM is writable.
    unsafe {
        let from = &raw const __itcm_load as *const u32;
        ptr::copy_nonoverlapping(from, code.as_ptr() as *mut u32, code.len());
        core::arch::asm!("dsb sy", "isb sy");
    }
    ITCM_SUM.store(itcm_sum(), Relaxed);
    ITCM_OK.store(true, Relaxed);
}

/// Once a second, check that the code in ITCM is unchanged; if anything has
/// written over it, stop running it for good.
pub(super) fn itcm_still_ok() -> bool {
    if !ITCM_OK.load(Relaxed) {
        return false;
    }
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(ITCM_CHECKED_AT.load(Relaxed)) >= blocks_per_second() {
        ITCM_CHECKED_AT.store(now, Relaxed);
        if itcm_sum() != ITCM_SUM.load(Relaxed) {
            ITCM_OK.store(false, Relaxed);
            return false;
        }
    }
    true
}
