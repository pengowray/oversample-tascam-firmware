//! The CPU test's logs, appended to `CPU_TEST.TXT` beside the takes.
//!
//! The context running the app takes each log as text into `spare::CPU_LOG` as a run ends. The
//! GUANO hook (task 4, the recorder's own task) writes what is there as the next take's files
//! close: the only time the patch is known to use the card safely. A run that records its own
//! take ends it, so its log is written seconds later. Logs that wait for a take add up in the
//! buffer, and one that does not fit is cut short. Logs not written are lost at power-off.

use core::fmt::Write;
use core::slice;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::BatApp;

use super::guano_file::{PATH_UNITS, path};
use super::location::{FileClose, FileOpen, FileSeek, FileSize, FileWrite, LINES_AT, LINES_BYTES};
use super::{spare, stock};

/// Nothing waiting.
const IDLE: u32 = 0;
/// Text waiting for a take to close.
const READY: u32 = 1;
/// Task 4 is writing it.
const WRITING: u32 = 2;
/// The context running the app is adding to it.
const FILLING: u32 = 3;
static STATE: AtomicU32 = AtomicU32::new(IDLE);
/// Bytes waiting in `spare::CPU_LOG`.
static LEN: AtomicUsize = AtomicUsize::new(0);
/// Writes that reached the card, and the count the app was last told of.
static WRITES: AtomicU32 = AtomicU32::new(0);
static TOLD: AtomicU32 = AtomicU32::new(0);

/// In UTF-16, as the file functions take names; relative to the folder the takes are in.
const NAME: [u16; 13] = x8_patch::wide::widen(b"CPU_TEST.TXT\0");

/// Take a finished run's log from the app, and tell the app when its log is on the card. From
/// the context running the app, once a pass. Not inlined: it has two callers and runs rarely.
#[inline(never)]
pub(super) fn take(app: &mut BatApp<'static>) {
    let writes = WRITES.load(Acquire);
    if writes != TOLD.load(Relaxed) {
        TOLD.store(writes, Relaxed);
        app.cpu_test_log_saved();
    }
    let before = STATE.load(Acquire);
    if !matches!(before, IDLE | READY) || STATE.compare_exchange(before, FILLING, Acquire, Relaxed).is_err() {
        return;
    }
    let len = LEN.load(Relaxed);
    // SAFETY: spare SDRAM no stock code uses (see `spare`); task 4 does not read it while FILLING.
    let room = unsafe { slice::from_raw_parts_mut((spare::CPU_LOG + len) as *mut u8, spare::CPU_LOG_BYTES - len) };
    match app.take_cpu_test_log(room) {
        Some(added) => {
            LEN.store(len + added, Relaxed);
            STATE.store(READY, Release);
        }
        None => STATE.store(before, Release),
    }
}

/// Append the logs waiting to `CPU_TEST.TXT`, from the GUANO hook as the take's file `file`
/// closes. A failure leaves them for the next take.
///
/// # Safety
/// Only from the GUANO hook, on the recorder's task, while it closes a take's files.
pub(super) unsafe fn write_log(file: usize) {
    if STATE.compare_exchange(READY, WRITING, Acquire, Relaxed).is_err() {
        return;
    }
    let len = LEN.load(Relaxed);
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
    // SAFETY: the stock header buffer, which nothing else uses while the take closes; the
    // location log has finished with this part of it.
    let out = unsafe { slice::from_raw_parts_mut(stock::HEADER_BUFFER.add(LINES_AT), LINES_BYTES) };
    // SAFETY: as above: "r+" opens the log if it is there; "w+" makes it.
    let mut handle = unsafe { open(NAME.as_ptr(), c"r+".as_ptr().cast()) };
    if handle == 0 {
        handle = unsafe { open(NAME.as_ptr(), c"w+".as_ptr().cast()) };
    }
    if handle == 0 {
        STATE.store(READY, Release);
        return;
    }
    // The take the log was written with, so it can be found.
    let mut name = [0u8; PATH_UNITS];
    let mut first = Text { out, len: 0 };
    let _ = match path(file, &mut name).and_then(guano::file_name) {
        Some(take) => writeln!(first, "Written as {take} closed."),
        None => Ok(()),
    };
    let mut end = 0u32;
    // SAFETY: the open log, from the recorder's task; each piece is copied into the header
    // buffer and written from there, as the stock recorder writes.
    unsafe { size(handle, &mut end) };
    let mut ok = unsafe { seek(handle, 0, end, 0, 0) } >= 0;
    let prefix = first.len;
    ok = ok && (prefix == 0 || unsafe { write(out.as_ptr(), prefix, handle) } == prefix);
    // SAFETY: spare SDRAM no stock code uses; the context running the app does not change it
    // while WRITING.
    let text = unsafe { slice::from_raw_parts(spare::CPU_LOG as *const u8, len) };
    for piece in text.chunks(LINES_BYTES) {
        if !ok {
            break;
        }
        out[..piece.len()].copy_from_slice(piece);
        ok = unsafe { write(out.as_ptr(), piece.len(), handle) } == piece.len();
    }
    let closed = unsafe { close(handle) } == 0;
    if ok && closed {
        LEN.store(0, Relaxed);
        WRITES.fetch_add(1, Release);
        STATE.store(IDLE, Release);
    } else {
        STATE.store(READY, Release);
    }
}

/// A writer into the header buffer that fails when it is full.
struct Text<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Write for Text<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.out.get_mut(self.len..end).ok_or(core::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
