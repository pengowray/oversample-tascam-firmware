use core::ptr;
use core::slice;
use core::sync::atomic::Ordering::Relaxed;

use x8_patch::take_guano::{self, Recorder};

use super::state::TAKE_GUANO;
use super::{Handler, patch_build, read_byte, stock, system_mode};

type FileSize = unsafe extern "C" fn(usize, *mut u32) -> usize;
/// The offset goes in r2 and r3, so r1 is not used.
type FileSeek = unsafe extern "C" fn(usize, usize, u32, u32, i32) -> i32;
type FileWrite = unsafe extern "C" fn(*const u8, usize, usize) -> usize;

/// How much of the header buffer to read: the stock header is 5,548 bytes.
const HEADER_READ: usize = 0x2000;
/// Where in the header buffer the chunk is built, after anything the stock code puts there.
const CHUNK_AT: usize = 0x8000;
const CHUNK_BYTES: usize = 0x400;
const _: () = assert!(HEADER_READ <= CHUNK_AT && CHUNK_AT + CHUNK_BYTES <= stock::HEADER_BUFFER_BYTES);
/// Free stack the hook requires before it adds a chunk. It uses about 0.7 KB (its frame and the
/// text formatting); the rest is a margin.
const STACK_NEEDED: usize = 2048;
/// Free stack the location log needs: opening a file goes through the C library and the file
/// system, whose use of the stack has not been measured. The end of a take, on task 4, has 17 KB.
const LOG_STACK_NEEDED: usize = 8192;

/// Replaces `FUN_80db216c`'s call to the file size function, straight after it has rewritten a WAV
/// file's header from the file's size, at the end of a take and when a take splits. The file is
/// still open and nothing more is written to its audio. For a take from a bat tile, adds the GUANO
/// chunk after the audio first, so the size the stock code then reads, for its free space count,
/// includes it.
pub(super) unsafe extern "C" fn guano_hook(file: usize, size_out: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1
        && TAKE_GUANO.load(Relaxed)
        && read_byte(stock::RENDER_KIND) == 0
        && read_byte(stock::OVERDUB) == 0
        && stack_left().is_some_and(|left| left >= STACK_NEEDED)
    {
        // SAFETY: the open file the stock code has just written the header of, from its own task.
        unsafe { add_guano(file) };
    }
    if system_mode() <= 1 && stack_left().is_some_and(|left| left >= LOG_STACK_NEEDED) {
        // SAFETY: the recorder's task, closing a take's files.
        unsafe { super::location::write_log() };
    }
    // SAFETY: the stock function this call replaced, with its arguments.
    let size: Handler = unsafe { core::mem::transmute(stock::FILE_SIZE) };
    unsafe { size(file, size_out, c, d) }
}

/// Appends the chunk and corrects the RIFF size, or leaves the file as it is when anything doesn't
/// match the stock layout. A failed write can leave bytes after the RIFF size, which readers skip.
///
/// # Safety
/// `file` must be a WAV file the stock recorder has open, and the header buffer must hold the header
/// it has just written to that file. The writes go from the header buffer, as the stock code's do.
unsafe fn add_guano(file: usize) -> Option<()> {
    // SAFETY: stock functions, called as the stock recorder calls them, from its task.
    let file_size: FileSize = unsafe { core::mem::transmute(stock::FILE_SIZE) };
    let seek: FileSeek = unsafe { core::mem::transmute(stock::FILE_SEEK) };
    let write: FileWrite = unsafe { core::mem::transmute(stock::FILE_WRITE) };
    let mut size = 0u32;
    unsafe { file_size(file, &mut size) };
    // SAFETY: the stock header buffer, which nothing else uses while the take closes.
    let header = guano::read_header(unsafe { slice::from_raw_parts(stock::HEADER_BUFFER, HEADER_READ) })?;
    // The header must describe this file with the audio running to its end: no chunk follows.
    if header.riff_size.checked_add(8)? != size || header.data_end()? != size {
        return None;
    }
    let (phone_now, utc_offset_minutes) = super::clock::take_times();
    let recorder = Recorder {
        firmware: stock::FIRMWARE_VERSION,
        patch_build: patch_build(),
        file_name: path(file).and_then(guano::file_name),
        phone_now,
        utc_offset_minutes,
        location_e6: super::location::for_take(),
        pauses: super::pauses::for_take(),
    };
    // SAFETY: as above; CHUNK_AT + CHUNK_BYTES is within the buffer and after the header.
    let out = unsafe { slice::from_raw_parts_mut(stock::HEADER_BUFFER.add(CHUNK_AT), CHUNK_BYTES) };
    let added = take_guano::write_chunk(&header, &recorder, out)?;
    let riff_size = header.riff_size.checked_add(u32::try_from(added).ok()?)?;
    riff_size.checked_add(8)?;
    unsafe {
        if seek(file, 0, size, 0, 0) < 0 || write(out.as_ptr(), added, file) != added {
            return None;
        }
        out[..4].copy_from_slice(&riff_size.to_le_bytes());
        if seek(file, 0, 4, 0, 0) < 0 {
            return None;
        }
        write(out.as_ptr(), 4, file);
    }
    Some(())
}

/// Bytes of stack below the caller's, on a task from the stock task table. None on any other stack,
/// so the hook adds nothing where it can't tell how much room there is.
fn stack_left() -> Option<usize> {
    let sp: usize;
    // SAFETY: reads the stack pointer.
    unsafe { core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    (0..stock::TASK_TABLE_ENTRIES).find_map(|i| {
        // SAFETY: the stock task table, constant after start-up.
        let [_, _, _, _, _, size, low] = unsafe { ptr::read_volatile(stock::TASK_TABLE.add(i)) };
        let (low, size) = (low as usize, size as usize);
        (sp >= low && sp < low + size).then(|| sp - low)
    })
}

/// The path the take's file slot holding `file` was opened with.
fn path(file: usize) -> Option<&'static [u8]> {
    (0..stock::FILE_SLOTS).find_map(|slot| {
        let entry = stock::TAKE_FILES + slot * stock::TAKE_FILE_STEP;
        // SAFETY: the stock table of a take's open files, which only the recorder's task changes.
        unsafe {
            (ptr::read_volatile(entry as *const usize) == file)
                .then(|| slice::from_raw_parts((entry + 4) as *const u8, stock::TAKE_FILE_PATH_BYTES))
        }
    })
}
