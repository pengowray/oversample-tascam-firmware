//! Times in exFAT file entries: to the second, with the UTC offset of the X8's clock when a
//! phone has set it (docs/HOMEBREW.md, "Time zone in exFAT file entries"). The stock code writes
//! even seconds and no offset, so Linux lists a take as if its local time were UTC.
//!
//! These hooks run in whichever task is using the card, from start-up and in every mode. They
//! use only the statics here and the clock's zone, which `clock.rs` keeps, loads and saves.

use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use x8_patch::entry_times::{self, ENTRY_BYTES, KEPT, MODIFIED, Stamp};

use super::stock;

/// The last modified stamp given to a file's record, date above time, and its 10 ms byte. The
/// record keeps the time without that byte, so a stamp that reuses the record's time gets the
/// byte from here when the times match.
static KEPT_TIME: AtomicU32 = AtomicU32::new(0);
static KEPT_TEN_MS: AtomicU32 = AtomicU32::new(0);

#[inline(never)]
fn zone() -> u8 {
    entry_times::zone_byte(super::clock::zone())
}

#[inline(never)]
fn clock_now() -> Stamp {
    let mut fields = [0u32; 9];
    // SAFETY: the stock function the file system reads the clock with, which fills 9 words.
    let read: unsafe extern "C" fn(*mut u32) -> i32 = unsafe { core::mem::transmute(stock::FILE_CLOCK) };
    unsafe { read(fields.as_mut_ptr()) };
    let [second, minute, hour, day, month, year, ..] = fields;
    Stamp::from_clock(&[second, minute, hour, day, month, year])
}

/// Replaces the stock time stamp function `FUN_80e0fcec(entry, file, flags)`: writes the times
/// that `flags` names into the first entry of a file's set, from the clock or, with `KEPT`, from
/// the file's record, and keeps a modified time in the record. `file` is 0 for a new folder.
pub(super) unsafe extern "C" fn stamp_hook(entry: usize, file: usize, flags: usize, _: usize) -> usize {
    let flags = flags as u32;
    let kept = (file + stock::FILE_MODIFIED_TIME) as *mut [u16; 2];
    let now = if flags & KEPT != 0 {
        // SAFETY: the open file's record; the stock function reads it here without a check too.
        let [time, date] = unsafe { ptr::read_volatile(kept) };
        let same = KEPT_TIME.load(Relaxed) == u32::from(date) << 16 | u32::from(time);
        Stamp { time, date, ten_ms: if same { KEPT_TEN_MS.load(Relaxed) as u8 } else { 0 } }
    } else {
        clock_now()
    };
    // SAFETY: the 32-byte entry the stock caller is filling in.
    entry_times::stamp(unsafe { &mut *(entry as *mut [u8; ENTRY_BYTES]) }, &now, flags, zone());
    if flags & MODIFIED != 0 && file != 0 {
        // SAFETY: as above.
        unsafe { ptr::write_volatile(kept, [now.time, now.date]) };
        KEPT_TIME.store(u32::from(now.date) << 16 | u32::from(now.time), Relaxed);
        KEPT_TEN_MS.store(now.ten_ms.into(), Relaxed);
    }
    0
}

/// Replaces the call in `FUN_80e0fef8`, which writes a new file's entries, that zeroes the 7
/// bytes after the first entry's UTC offsets. That function has just stamped the modified and
/// accessed times itself, to 2 s and with no offset: stamp them again. The created time, with
/// its 10 ms and offset bytes, is the caller's, and the patched function no longer zeroes those.
pub(super) unsafe extern "C" fn new_entry_hook(spare: usize, bytes: usize, _: usize, _: usize) -> usize {
    // SAFETY: the stock function this call went to, with its arguments.
    let zero: unsafe extern "C" fn(usize, usize) = unsafe { core::mem::transmute(stock::ZERO_BYTES) };
    unsafe { zero(spare, bytes) };
    // SAFETY: the spare bytes start 0x19 into the 32-byte entry the stock caller is filling in.
    let entry = unsafe { &mut *((spare - 0x19) as *mut [u8; ENTRY_BYTES]) };
    entry_times::stamp(entry, &clock_now(), MODIFIED | entry_times::ACCESSED, zone());
    0
}
