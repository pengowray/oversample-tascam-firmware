//! The phone's location from chimes: kept for a take's GUANO `Loc Position`, and logged to
//! `BAT_LOCATIONS.CSV` beside the takes (docs/HOMEBREW.md, "Setting the clock").
//!
//! The context running the app adds locations; the GUANO hook (task 4, the recorder's own task)
//! reads them and writes the log, once a take, as a take's files close: the only place the patch
//! is known to use the card safely. A location read with no take afterwards is lost at power-off.

use core::fmt::Write;
use core::slice;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::PhoneTime;
use x8_patch::take_guano::{LOG_HEADER, LogLine};

use super::state::BLOCKS;
use super::{blocks_per_second, stock};

/// A location this old when a take starts is left out of the take's metadata.
const RECENT_S: u32 = 3600;

// The latest location, for GUANO: `SEQ` is odd while the others change.
static SEQ: AtomicU32 = AtomicU32::new(0);
static LATITUDE_E6: AtomicI32 = AtomicI32::new(0);
static LONGITUDE_E6: AtomicI32 = AtomicI32::new(0);
static READ_AT: AtomicU32 = AtomicU32::new(0);
/// The block count when the take in progress started (set by `open_take_hook`).
pub(super) static TAKE_STARTED: AtomicU32 = AtomicU32::new(0);
/// The log has not been written for the take in progress yet.
pub(super) static LOG_DUE: AtomicBool = AtomicBool::new(false);

/// Locations for the log, not written yet: one writer (the context running the app) and one reader
/// (task 4), so the two indices need no lock. A full ring drops new lines.
const RING: usize = 16;
static RING_LOCAL_S: [AtomicU32; RING] = [const { AtomicU32::new(0) }; RING];
static RING_TZ: [AtomicI32; RING] = [const { AtomicI32::new(0) }; RING];
static RING_LATITUDE: [AtomicI32; RING] = [const { AtomicI32::new(0) }; RING];
static RING_LONGITUDE: [AtomicI32; RING] = [const { AtomicI32::new(0) }; RING];
static WRITTEN: AtomicU32 = AtomicU32::new(0);
static ADDED: AtomicU32 = AtomicU32::new(0);

/// Keep a chime's location, from the context running the app.
pub(super) fn keep(time: &PhoneTime) {
    let Some((latitude, longitude)) = time.location else { return };
    let (latitude, longitude) = ((latitude * 1e6) as i32, (longitude * 1e6) as i32);
    SEQ.fetch_add(1, Release);
    LATITUDE_E6.store(latitude, Relaxed);
    LONGITUDE_E6.store(longitude, Relaxed);
    READ_AT.store(BLOCKS.load(Relaxed), Relaxed);
    SEQ.fetch_add(1, Release);
    let added = ADDED.load(Relaxed);
    if added.wrapping_sub(WRITTEN.load(Acquire)) as usize >= RING {
        return;
    }
    let i = added as usize % RING;
    RING_LOCAL_S[i].store(time.local_ms.div_euclid(1000).max(0) as u32, Relaxed);
    RING_TZ[i].store(time.timezone_minutes.into(), Relaxed);
    RING_LATITUDE[i].store(latitude, Relaxed);
    RING_LONGITUDE[i].store(longitude, Relaxed);
    ADDED.store(added.wrapping_add(1), Release);
}

/// The location for the take whose files are closing: one read during the take, or up to
/// `RECENT_S` before it started.
pub(super) fn for_take() -> Option<(i32, i32)> {
    loop {
        let seq = SEQ.load(Acquire);
        if seq == 0 {
            return None;
        }
        let (latitude, longitude, at) = (LATITUDE_E6.load(Relaxed), LONGITUDE_E6.load(Relaxed), READ_AT.load(Relaxed));
        if !seq.is_multiple_of(2) || SEQ.load(Acquire) != seq {
            continue;
        }
        let started = TAKE_STARTED.load(Relaxed);
        let now = BLOCKS.load(Relaxed);
        // Wrapping differences: the block count wraps after about a day.
        let since_read = now.wrapping_sub(at);
        let since_start = now.wrapping_sub(started);
        let recent = since_read <= since_start || since_read - since_start <= RECENT_S * blocks_per_second();
        return recent.then_some((latitude, longitude));
    }
}

type FileOpen = unsafe extern "C" fn(*const u8, *const u8) -> usize;
type FileClose = unsafe extern "C" fn(usize) -> i32;
type FileSize = unsafe extern "C" fn(usize, *mut u32) -> usize;
type FileSeek = unsafe extern "C" fn(usize, usize, u32, u32, i32) -> i32;
type FileWrite = unsafe extern "C" fn(*const u8, usize, usize) -> usize;

const NAME: &[u8] = b"BAT_LOCATIONS.CSV\0";
/// Where the lines are built: in the stock header buffer, after the GUANO chunk's place.
const LINES_AT: usize = 0x9000;
const LINES_BYTES: usize = 0x1000;

/// Appends the locations not written yet to the log, once a take, from the GUANO hook. Any failure
/// leaves them for the next take.
///
/// # Safety
/// Only from the GUANO hook, on the recorder's task, while it closes a take's files.
pub(super) unsafe fn write_log() {
    if !LOG_DUE.swap(false, Relaxed) {
        return;
    }
    let (written, added) = (WRITTEN.load(Relaxed), ADDED.load(Acquire));
    if written == added {
        return;
    }
    // SAFETY: stock functions, called as the stock recorder calls them, from its task.
    let (open, close, size, seek, write): (FileOpen, FileClose, FileSize, FileSeek, FileWrite) = unsafe {
        (
            core::mem::transmute::<usize, FileOpen>(stock::FILE_OPEN),
            core::mem::transmute::<usize, FileClose>(stock::FILE_CLOSE),
            core::mem::transmute::<usize, FileSize>(stock::FILE_SIZE),
            core::mem::transmute::<usize, FileSeek>(stock::FILE_SEEK),
            core::mem::transmute::<usize, FileWrite>(stock::FILE_WRITE),
        )
    };
    // SAFETY: the stock header buffer, which nothing else uses while the take closes.
    let out = unsafe { slice::from_raw_parts_mut(stock::HEADER_BUFFER.add(LINES_AT), LINES_BYTES) };
    let mut lines = Lines { out, len: 0 };
    // "r+" opens the log if it is there; "w+" makes it.
    let mut file = unsafe { open(NAME.as_ptr(), b"r+\0".as_ptr()) };
    if file == 0 {
        file = unsafe { open(NAME.as_ptr(), b"w+\0".as_ptr()) };
        if file == 0 || lines.write_str(LOG_HEADER).is_err() {
            return;
        }
    }
    let mut next = written;
    while next != added {
        let i = next as usize % RING;
        let line = LogLine {
            local_seconds: RING_LOCAL_S[i].load(Relaxed) as i64,
            utc_offset_minutes: RING_TZ[i].load(Relaxed) as i16,
            location_e6: (RING_LATITUDE[i].load(Relaxed), RING_LONGITUDE[i].load(Relaxed)),
        };
        if write!(lines, "{line}").is_err() {
            break;
        }
        next = next.wrapping_add(1);
    }
    let mut end = 0u32;
    let ok = unsafe {
        size(file, &mut end);
        seek(file, 0, end, 0, 0) >= 0 && write(lines.out.as_ptr(), lines.len, file) == lines.len
    };
    let closed = unsafe { close(file) } == 0;
    if ok && closed {
        WRITTEN.store(next, Release);
    }
}

/// A writer into the line buffer that fails when it is full.
struct Lines<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Write for Lines<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.out.get_mut(self.len..end).ok_or(core::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
