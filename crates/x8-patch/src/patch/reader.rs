//! BAT PLAYBACK's card reader: a task that lists the WAV files in `C:\SOUND` and its folders,
//! reads each one's header, and reads the file playing into a ring for the DSP task
//! (playback.rs), which owns the app and passes on what the reader finds.
//!
//! The reader touches the card only while the bat screen is open in BAT PLAYBACK. It uses the
//! core file functions with absolute paths (see `stock::CORE_OPEN`), and keeps at most one
//! folder and two files open: the file playing, and one whose header it is reading.
//!
//! **Commands** come from the DSP task only: [`FILE_COMMANDS`] holds the latest Open or Close,
//! and [`SCAN_REQUESTS`] counts requests to list the card. **Results** go back in a ring of
//! [`Event`]s, each tagged with the command or scan it answers, so the DSP task can ignore the
//! answer to a command it has since replaced. **Audio** goes into `spare::READ_RING`, counted in
//! bytes by [`RING_WRITTEN`] (the reader) and [`RING_TAKEN`] (the DSP task); both start again at
//! each Open.
//!
//! **Leaving.** When the bat screen closes, the reader closes everything at its next check and
//! clears [`HOLDING`], and `leave_bat_screen` waits for that (see [`wait_until_closed`]), so the
//! stock firmware does not use the card while the reader is halfway through a read.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{
    AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release, Ordering::SeqCst, fence,
};

use bat_app::player::{FOLDER_NAME_BYTES, FileEntry, FileInfo, NotPlayable};
use bat_dsp::wav::{self, Format, WavError};
use x8_patch::card::{Path, checked, header_info, is_wav_name, read_start};
use x8_patch::handoff::{Mailbox, Ring};

use super::state::{BAT_ACTIVE, BLOCKS, PLAYBACK};
use super::task::{TASK_IDS, sleep_ms};
use super::{Slot, blocks_per_second, spare, stock};

/// Above task 1 (15) and the DSP task (16), so a read starts again as soon as the card has
/// answered the last one, and below the stock tasks that record and play (task 6 at 8, task 20
/// at 10), so they come first if anything of theirs runs. The reader spends nearly all its time
/// waiting for the card, and sleeps when it has nothing to do, so it takes little time from the
/// tasks below it. A loop here must always reach a card call or a sleep, or the UI stops.
const TASK_PRIORITY: u32 = 12;
/// The stock player reads files on task 6, whose stack is 6 KB in all.
pub(super) const STACK_BYTES: usize = 10 * 1024;
/// Written into the stack before the task starts, to measure how much it has used.
const STACK_PAINT: u32 = 0x5afe_5afe;

/// Bytes read from the card at a time, so a seek or close waits at most this long. A multiple
/// of the sector size, so every read after the first goes straight to the ring by DMA.
const CHUNK: u32 = 32 * 1024;
const _: () = assert!(spare::READ_RING_BYTES.is_multiple_of(CHUNK));
/// Reads into the ring before the reader tells the DSP task a file is open (128 KB).
const FIRST_READS: u32 = 4;
/// The first read of a header. The X8's own header is 5,548 bytes; longer ones are read again,
/// up to `spare::HEADER_BYTES` (one Xeno-canto file's audio starts at 23,944).
const FIRST_HEADER: u32 = 8 * 1024;

/// Entries listed between short sleeps: entries often come from the file system's own buffers,
/// without waiting for the card, so a long folder would otherwise keep the tasks below waiting.
const YIELD_EVERY: u32 = 32;
/// Folder levels listed below `C:\SOUND`: the X8's own folders, and one more.
const DEEPEST: u8 = 2;
/// Folders the reader keeps, `C:\SOUND` first. The app names the first 32.
const FOLDERS: usize = 64;
const FOLDER_NAME: usize = 61;
/// Files with longer names are left out.
const NAME: usize = 96;

/// Sleeps: with nothing to do, and while the bat screen is closed or in BAT DETECTOR.
const IDLE_MS: u32 = 4;
const AWAY_MS: u32 = 20;
/// While the event ring is full.
const EVENTS_FULL_MS: u32 = 2;

/// The reader's table of what it has listed, in `spare` memory. A file's id is its place here.
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct CardFile {
    folder: u8,
    name_len: u8,
    name: [u8; NAME],
    bytes: u32,
}
const _: () = assert!(core::mem::size_of::<CardFile>() * spare::LIST_FILES <= spare::CARD_FILES_BYTES);

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct CardFolder {
    /// The folder it is in; `C:\SOUND` is folder 0 and has none.
    parent: u8,
    depth: u8,
    name_len: u8,
    name: [u8; FOLDER_NAME],
}
const _: () = assert!(core::mem::size_of::<CardFolder>() * FOLDERS <= spare::CARD_FOLDERS_BYTES);

/// What the reader tells the DSP task.
#[derive(Clone, Copy)]
pub(super) enum Event {
    /// For the scan counted `scan` in SCAN_REQUESTS: a folder, before any file in it.
    Folder {
        scan: u32,
        index: u8,
        name: [u8; FOLDER_NAME_BYTES],
        len: u8,
    },
    File {
        scan: u32,
        entry: FileEntry,
    },
    Info {
        scan: u32,
        id: u16,
        info: FileInfo,
    },
    ScanDone {
        scan: u32,
        ok: bool,
    },
    /// For file command `command`: the file is open, reading started at the sector holding
    /// `frame`, `skip` bytes before it, and its audio ends `limit` bytes after that sector.
    Opened {
        command: u32,
        id: u16,
        format: Format,
        frame: u32,
        skip: u32,
        limit: u32,
    },
    OpenFailed {
        command: u32,
        id: u16,
        why: NotPlayable,
    },
}
pub(super) const EVENT_SLOTS: u32 = 256;
const _: () = assert!(core::mem::size_of::<Event>() * EVENT_SLOTS as usize <= spare::EVENTS_BYTES);
const _: () = assert!(core::mem::align_of::<Event>() <= 32);

/// File commands: [kind, file id, frame].
pub(super) static FILE_COMMANDS: Mailbox<3> = Mailbox::new();
pub(super) const OPEN: u32 = 1;
pub(super) const CLOSE: u32 = 2;
/// Counts requests to list the card (the DSP task).
pub(super) static SCAN_REQUESTS: AtomicU32 = AtomicU32::new(0);
static EVENTS_WRITTEN: AtomicU32 = AtomicU32::new(0);
static EVENTS_READ: AtomicU32 = AtomicU32::new(0);
/// Bytes of the file playing in the ring, from the sector the reading started at: written by
/// the reader, and taken by the DSP task, which sets its count to the Opened event's `skip`.
pub(super) static RING_WRITTEN: AtomicU32 = AtomicU32::new(0);
pub(super) static RING_TAKEN: AtomicU32 = AtomicU32::new(0);
/// A read of the file playing failed: the reader has stopped filling the ring.
pub(super) static RING_FAILED: AtomicBool = AtomicBool::new(false);
/// The reader may be using the card, or has a file open. Cleared once it has closed everything.
pub(super) static HOLDING: AtomicBool = AtomicBool::new(false);
/// The reader task's id, or the error cre_tsk returned (negative), or 0 before it is created.
pub(super) static READER_TASK: AtomicU32 = AtomicU32::new(0);
/// For the debug figures: what the reader is doing, the card errors since start-up, and the
/// times `leave_bat_screen` stopped waiting for the reader.
pub(super) static STATE: AtomicU32 = AtomicU32::new(AWAY);
pub(super) static ERRORS: AtomicU32 = AtomicU32::new(0);
pub(super) static LEAVE_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub(super) const AWAY: u32 = 0;
pub(super) const IDLE: u32 = 1;
pub(super) const LISTING: u32 = 2;
pub(super) const HEADERS: u32 = 3;
pub(super) const OPENING: u32 = 4;
pub(super) const READING: u32 = 5;

/// In DTCM, like the DSP task's: the patch region has no room left. The reader's reads go to its
/// buffers in `spare`, never to its stack.
#[repr(C, align(8))]
pub(super) struct ReaderStack([u32; STACK_BYTES / 4]);
#[unsafe(link_section = ".dtcm")]
static STACK: Slot<ReaderStack> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

/// The event ring, for either end.
pub(super) fn events() -> Ring<'static, Event> {
    // SAFETY: the event range in `spare`, which the asserts above show is large enough, and the
    // two counters above; only the reader pushes and only the DSP task pops.
    unsafe { Ring::new(spare::EVENTS as *mut Event, EVENT_SLOTS, &EVENTS_WRITTEN, &EVENTS_READ) }
}

pub(super) fn running() -> bool {
    (READER_TASK.load(Relaxed) as i32) > 0
}

fn active() -> bool {
    BAT_ACTIVE.load(SeqCst) && PLAYBACK.load(Relaxed)
}

/// Create the reader task (task 1, once, after the DSP task), trying each free id in turn.
pub(super) fn start_reader() {
    // SAFETY: the reader's stack, which nothing uses until the task starts.
    unsafe {
        let words = STACK.0.get().cast::<u32>();
        for i in 0..STACK_BYTES / 4 {
            ptr::write_volatile(words.add(i), STACK_PAINT);
        }
    }
    let result = super::task::create_task(reader_task, TASK_PRIORITY, STACK.0.get().cast(), STACK_BYTES, &TASK_IDS);
    READER_TASK.store(result as u32, Relaxed);
}

/// The most of the reader's stack it has used, in bytes, for the debug figures.
pub(super) fn stack_used() -> usize {
    if !running() {
        return 0;
    }
    let words = STACK.0.get().cast::<u32>();
    // SAFETY: reading the reader's stack, from the bottom, as words.
    let unused = (0..STACK_BYTES / 4).take_while(|&i| unsafe { ptr::read_volatile(words.add(i)) } == STACK_PAINT);
    STACK_BYTES - unused.count() * 4
}

/// After the bat screen has closed (task 1, or the DSP task for a pop-up): wait until the reader
/// has closed its files, polling every 2 ms for up to 200 ms. A read the card is retrying can
/// take seconds; then the wait gives up and counts it, rather than hang the X8.
pub(super) fn wait_until_closed() {
    if !running() || !PLAYBACK.load(Relaxed) {
        return;
    }
    fence(SeqCst);
    let start = BLOCKS.load(Relaxed);
    // Counted as well as timed, in case the sleep returns at once.
    for _ in 0..100 {
        if !HOLDING.load(SeqCst) {
            return;
        }
        if BLOCKS.load(Relaxed).wrapping_sub(start) >= blocks_per_second() / 5 {
            break;
        }
        sleep_ms(2);
    }
    if HOLDING.load(SeqCst) {
        LEAVE_TIMEOUTS.store(LEAVE_TIMEOUTS.load(Relaxed).wrapping_add(1), Relaxed);
    }
}

type CoreOpen = unsafe extern "C" fn(*const u16, *const u8) -> usize;
type FileRead = unsafe extern "C" fn(*mut u8, usize, usize) -> usize;
type FileSeek = unsafe extern "C" fn(usize, usize, u32, u32, i32) -> i32;
type FileSize = unsafe extern "C" fn(usize, *mut u32) -> usize;
type FileClose = unsafe extern "C" fn(usize) -> i32;
type FolderOpen = unsafe extern "C" fn(*const u16) -> usize;
type FolderNext = unsafe extern "C" fn(usize) -> usize;
type FolderClose = unsafe extern "C" fn(usize) -> i32;

// The stock file system's calls. SAFETY for each: called from the reader task as the stock code
// calls them, with paths ending in a 0 unit and buffers as long as the count.
fn open_file(path: &Path) -> usize {
    let open: CoreOpen = unsafe { core::mem::transmute(stock::CORE_OPEN) };
    unsafe { open(path.as_units().as_ptr(), c"r".as_ptr().cast()) }
}
fn read(to: usize, bytes: u32, file: usize) -> bool {
    let read: FileRead = unsafe { core::mem::transmute(stock::FILE_READ) };
    unsafe { read(to as *mut u8, bytes as usize, file) == bytes as usize }
}
fn seek(file: usize, offset: u32) -> bool {
    let seek: FileSeek = unsafe { core::mem::transmute(stock::FILE_SEEK) };
    unsafe { seek(file, 0, offset, 0, 0) >= 0 }
}
fn file_size(file: usize) -> u32 {
    let size: FileSize = unsafe { core::mem::transmute(stock::FILE_SIZE) };
    let mut bytes = 0;
    unsafe { size(file, &mut bytes) };
    bytes
}
fn close_file(file: usize) {
    let close: FileClose = unsafe { core::mem::transmute(stock::FILE_CLOSE) };
    unsafe { close(file) };
}
fn open_folder(path: &Path) -> usize {
    let open: FolderOpen = unsafe { core::mem::transmute(stock::FOLDER_OPEN) };
    unsafe { open(path.as_units().as_ptr()) }
}
fn next_entry(folder: usize) -> usize {
    let next: FolderNext = unsafe { core::mem::transmute(stock::FOLDER_NEXT) };
    unsafe { next(folder) }
}
fn close_folder(folder: usize) {
    let close: FolderClose = unsafe { core::mem::transmute(stock::FOLDER_CLOSE) };
    unsafe { close(folder) };
}

fn count_error() {
    ERRORS.store(ERRORS.load(Relaxed).wrapping_add(1), Relaxed);
}

fn card_file(id: usize) -> *mut CardFile {
    // SAFETY: inside the table (callers keep `id` below LIST_FILES).
    unsafe { (spare::CARD_FILES as *mut CardFile).add(id) }
}

fn card_folder(index: usize) -> *mut CardFolder {
    // SAFETY: inside the table (callers keep `index` below FOLDERS).
    unsafe { (spare::CARD_FOLDERS as *mut CardFolder).add(index) }
}

/// The header buffer's first `bytes`.
fn header(bytes: u32) -> &'static [u8] {
    // SAFETY: the header range in `spare`, used only by the reader.
    unsafe { core::slice::from_raw_parts(spare::HEADER as *const u8, bytes as usize) }
}

/// Read and check a header, from the start of `file`, whose size is `size`. A read that fails
/// gives `HeaderTooLong`, which the list shows as unreadable.
fn read_header(file: usize, size: u32) -> Result<Format, WavError> {
    let first = size.min(FIRST_HEADER);
    if !read(spare::HEADER, first, file) {
        count_error();
        return Err(WavError::HeaderTooLong);
    }
    let parsed = wav::parse(header(first), size);
    if parsed != Err(WavError::HeaderTooLong) || size <= first {
        return checked(parsed);
    }
    let all = size.min(spare::HEADER_BYTES as u32);
    if !read(spare::HEADER + first as usize, all - first, file) {
        count_error();
        return Err(WavError::HeaderTooLong);
    }
    checked(wav::parse(header(all), size))
}

/// The name in a folder entry, as ASCII, or None if it is longer than `out` or not printable
/// ASCII. Copied at once: on exFAT the next call changes the entry.
fn entry_name(entry: usize, out: &mut [u8; NAME]) -> Option<usize> {
    let units = (entry + stock::ENTRY_NAME) as *const u16;
    // SAFETY: the long name in the entry, which ends in a 0 unit within 256 units.
    let unit = |i: usize| unsafe { ptr::read_volatile(units.add(i)) };
    for (i, byte) in out.iter_mut().enumerate() {
        match unit(i) {
            0 => return Some(i),
            u @ 0x20..0x7f => *byte = u as u8,
            _ => return None,
        }
    }
    (unit(NAME) == 0).then_some(NAME)
}

/// A folder entry's 32-byte directory entry: attributes, time, date and size.
fn entry_details(entry: usize) -> (u8, u16, u16, u32) {
    // SAFETY: the entry's first word points to its 32-byte directory entry. Read a byte at a
    // time, since its alignment is not known.
    unsafe {
        let dir = ptr::read_volatile(entry as *const usize) as *const u8;
        let byte = |at: usize| ptr::read_volatile(dir.add(at));
        let half = |at: usize| u16::from_le_bytes([byte(at), byte(at + 1)]);
        let size = u32::from_le_bytes([
            byte(stock::DIR_SIZE),
            byte(stock::DIR_SIZE + 1),
            byte(stock::DIR_SIZE + 2),
            byte(stock::DIR_SIZE + 3),
        ]);
        (byte(stock::DIR_ATTRIBUTES), half(stock::DIR_TIME), half(stock::DIR_DATE), size)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scanning {
    No,
    /// Listing the folder with this index next. Folders found are added to the table as each
    /// one is listed, so this goes through every level.
    Folder(usize),
    /// Reading headers, from the last file listed back to the first: the newest files, at the
    /// top of the list, are usually listed last.
    Headers(usize),
}

struct Reader {
    /// The latest file command and scan request handled.
    command_seen: u32,
    scan_seen: u32,
    /// The scan in progress, and how far it has got.
    scan: u32,
    scanning: Scanning,
    folders: usize,
    files: usize,
    /// The file playing: its handle (0 for none), path and format, the offset of the next read,
    /// and the end of its audio.
    file: usize,
    path: Path,
    format: Option<Format>,
    next: u32,
    end: u32,
}

impl Reader {
    fn new() -> Self {
        Self {
            command_seen: 0,
            scan_seen: 0,
            scan: 0,
            scanning: Scanning::No,
            folders: 0,
            files: 0,
            file: 0,
            path: Path::sound(),
            format: None,
            next: 0,
            end: 0,
        }
    }

    /// Give an event to the DSP task, waiting while the ring is full. False if the bat screen
    /// closed first.
    fn post(&self, event: Event) -> bool {
        let ring = events();
        loop {
            if ring.push(event) {
                return true;
            }
            if !active() {
                return false;
            }
            sleep_ms(EVENTS_FULL_MS);
        }
    }

    /// The bat screen has closed, or is in BAT DETECTOR: close everything and forget the scan and
    /// any command not handled yet.
    fn stop(&mut self) {
        self.close();
        self.scanning = Scanning::No;
        self.command_seen = FILE_COMMANDS.latest();
        self.scan_seen = SCAN_REQUESTS.load(Acquire);
        STATE.store(AWAY, Relaxed);
    }

    fn close(&mut self) {
        if self.file != 0 {
            close_file(self.file);
            self.file = 0;
        }
        self.format = None;
    }

    /// Act on a new file command or scan request. False if there is none.
    fn serve_commands(&mut self) -> bool {
        if let Some((number, [kind, id, frame])) = FILE_COMMANDS.read(self.command_seen) {
            self.command_seen = number;
            match kind {
                OPEN => self.open(number, id as u16, frame),
                _ => self.close(),
            }
            return true;
        }
        let scan = SCAN_REQUESTS.load(Acquire);
        if scan != self.scan_seen {
            self.scan_seen = scan;
            self.start_scan(scan);
            return true;
        }
        false
    }

    fn folder_path(&self, index: usize) -> Option<Path> {
        // The folder and its parents, from the folder up; folder 0 is C:\SOUND itself.
        let mut chain = [0usize; DEEPEST as usize];
        let mut depth = 0;
        let mut at = index;
        while at != 0 {
            *chain.get_mut(depth)? = at;
            depth += 1;
            // SAFETY: an entry of the folder table that this scan wrote.
            at = unsafe { (*card_folder(at)).parent } as usize;
        }
        let mut path = Path::sound();
        for &folder in chain[..depth].iter().rev() {
            // SAFETY: as above.
            let folder = unsafe { &*card_folder(folder) };
            if !path.push(&folder.name[..folder.name_len as usize]) {
                return None;
            }
        }
        Some(path)
    }

    fn file_path(&self, id: usize) -> Option<Path> {
        if id >= self.files {
            return None;
        }
        // SAFETY: an entry of the file table that this scan wrote.
        let file = unsafe { &*card_file(id) };
        let mut path = self.folder_path(file.folder as usize)?;
        path.push(&file.name[..file.name_len as usize]).then_some(path)
    }

    /// Open file `id` (or keep it open), go to `frame`, read the first of it into the ring, and
    /// tell the DSP task.
    fn open(&mut self, command: u32, id: u16, frame: u32) {
        STATE.store(OPENING, Relaxed);
        let event = match self.open_at(id, frame) {
            Ok((format, frame, skip, limit)) => {
                for _ in 0..FIRST_READS {
                    if !active() || FILE_COMMANDS.latest() != command || !self.fill_ring() {
                        break;
                    }
                }
                Event::Opened { command, id, format, frame, skip, limit }
            }
            Err(why) => {
                self.close();
                Event::OpenFailed { command, id, why }
            }
        };
        self.post(event);
    }

    /// Returns the format, the frame reading starts at, the bytes before it in the ring, and where
    /// the audio ends, in bytes from the start of the ring.
    fn open_at(&mut self, id: u16, frame: u32) -> Result<(Format, u32, u32, u32), NotPlayable> {
        let path = self.file_path(id as usize).ok_or(NotPlayable::Unreadable)?;
        if self.file == 0 || self.path != path {
            self.close();
            let file = open_file(&path);
            if file == 0 {
                count_error();
                return Err(NotPlayable::Unreadable);
            }
            self.file = file;
            self.path = path;
            match read_header(file, file_size(file)) {
                Ok(format) => self.format = Some(format),
                Err(error) => {
                    return Err(match header_info(Err(error)) {
                        FileInfo::NotPlayable(why) => why,
                        _ => NotPlayable::Unreadable,
                    });
                }
            }
        }
        let format = self.format.ok_or(NotPlayable::Unreadable)?;
        let (base, skip) = read_start(&format, frame);
        if !seek(self.file, base) {
            count_error();
            return Err(NotPlayable::Unreadable);
        }
        // The DSP task does not take from the ring until it has the Opened event.
        RING_FAILED.store(false, Relaxed);
        RING_TAKEN.store(0, Relaxed);
        RING_WRITTEN.store(0, Release);
        self.next = base;
        self.end = format.data_start + format.data_bytes;
        let frame = frame.min(format.frames().saturating_sub(1));
        Ok((format, frame, skip, self.end - base))
    }

    /// Read the next part of the file playing into the ring, if there is room. False if there
    /// was nothing to read.
    fn fill_ring(&mut self) -> bool {
        if self.file == 0 || self.next >= self.end || RING_FAILED.load(Relaxed) {
            return false;
        }
        let written = RING_WRITTEN.load(Relaxed);
        let held = written.wrapping_sub(RING_TAKEN.load(Acquire));
        if spare::READ_RING_BYTES.saturating_sub(held) < CHUNK {
            return false;
        }
        STATE.store(READING, Relaxed);
        // Every read but the last is a whole chunk, so each starts on a chunk in the ring and
        // none crosses its end.
        let bytes = CHUNK.min(self.end - self.next);
        let at = spare::READ_RING + (written % spare::READ_RING_BYTES) as usize;
        if !read(at, bytes, self.file) {
            count_error();
            RING_FAILED.store(true, Release);
            return true;
        }
        self.next += bytes;
        RING_WRITTEN.store(written.wrapping_add(bytes), Release);
        true
    }

    fn start_scan(&mut self, scan: u32) {
        self.scan = scan;
        self.files = 0;
        self.folders = 1;
        // SAFETY: folder 0 of the table.
        unsafe { card_folder(0).write(CardFolder { parent: 0, depth: 0, name_len: 0, name: [0; FOLDER_NAME] }) };
        let mut name = [0; FOLDER_NAME_BYTES];
        name[..5].copy_from_slice(b"SOUND");
        self.scanning = Scanning::Folder(0);
        self.post(Event::Folder { scan, index: 0, name, len: 5 });
    }

    /// List one folder, or read one header. False if there is no scan in progress.
    fn scan_step(&mut self) -> bool {
        match self.scanning {
            Scanning::No => false,
            Scanning::Folder(index) if index >= self.folders => {
                self.scanning = Scanning::Headers(self.files);
                true
            }
            Scanning::Folder(index) => {
                self.scanning = Scanning::Folder(index + 1);
                if !self.list_folder(index) && index == 0 {
                    // No card, or no SOUND folder.
                    self.scanning = Scanning::No;
                    self.post(Event::ScanDone { scan: self.scan, ok: false });
                }
                true
            }
            Scanning::Headers(0) => {
                self.scanning = Scanning::No;
                self.post(Event::ScanDone { scan: self.scan, ok: true });
                true
            }
            Scanning::Headers(next) => {
                self.scanning = Scanning::Headers(next - 1);
                let info = self.header_info(next - 1);
                self.post(Event::Info { scan: self.scan, id: (next - 1) as u16, info });
                true
            }
        }
    }

    /// Add folder `index`'s WAV files and folders to the tables, and tell the DSP task. False if
    /// the folder could not be opened.
    fn list_folder(&mut self, index: usize) -> bool {
        let Some(path) = self.folder_path(index) else { return false };
        STATE.store(LISTING, Relaxed);
        let folder = open_folder(&path);
        if folder == 0 {
            return false;
        }
        // SAFETY: an entry of the folder table that this scan wrote.
        let depth = unsafe { (*card_folder(index)).depth };
        let mut name = [0u8; NAME];
        let mut entries = 0u32;
        while active() {
            let entry = next_entry(folder);
            if entry == 0 {
                break;
            }
            entries += 1;
            if entries.is_multiple_of(YIELD_EVERY) {
                sleep_ms(1);
            }
            let Some(len) = entry_name(entry, &mut name) else { continue };
            let (attributes, time, date, bytes) = entry_details(entry);
            let name = &name[..len];
            if attributes & (stock::ATTR_HIDDEN | stock::ATTR_VOLUME) != 0 {
                continue;
            }
            if attributes & stock::ATTR_FOLDER != 0 {
                if name != b"." && name != b".." && depth < DEEPEST && self.folders < FOLDERS && len <= FOLDER_NAME {
                    self.add_folder(index, depth + 1, name);
                }
            } else if is_wav_name(name) && self.files < spare::LIST_FILES {
                let id = self.files;
                let mut stored = [0; NAME];
                stored[..len].copy_from_slice(name);
                // SAFETY: a new entry of the file table.
                unsafe {
                    card_file(id).write(CardFile { folder: index as u8, name_len: len as u8, name: stored, bytes })
                };
                self.files += 1;
                let entry = FileEntry::new(id as u16, index as u8, name, date, time, bytes);
                self.post(Event::File { scan: self.scan, entry });
            }
        }
        close_folder(folder);
        true
    }

    fn add_folder(&mut self, parent: usize, depth: u8, name: &[u8]) {
        let index = self.folders;
        let mut stored = [0; FOLDER_NAME];
        stored[..name.len()].copy_from_slice(name);
        // SAFETY: a new entry of the folder table.
        unsafe {
            card_folder(index).write(CardFolder {
                parent: parent as u8,
                depth,
                name_len: name.len() as u8,
                name: stored,
            })
        };
        self.folders += 1;
        // The app shows a folder below the X8's own folders as `FOLDER_0001/below`.
        let mut shown = [0; FOLDER_NAME_BYTES];
        let mut len = 0;
        let mut add = |part: &[u8]| {
            for &b in part {
                if len < FOLDER_NAME_BYTES {
                    shown[len] = b;
                    len += 1;
                }
            }
        };
        if depth > 1 {
            // SAFETY: an entry of the folder table that this scan wrote.
            let parent = unsafe { &*card_folder(parent) };
            add(&parent.name[..parent.name_len as usize]);
            add(b"/");
        }
        add(name);
        self.post(Event::Folder { scan: self.scan, index: index as u8, name: shown, len: len as u8 });
    }

    /// What file `id`'s header says.
    fn header_info(&mut self, id: usize) -> FileInfo {
        STATE.store(HEADERS, Relaxed);
        let Some(path) = self.file_path(id) else { return FileInfo::NotPlayable(NotPlayable::Unreadable) };
        if self.file != 0 && path == self.path {
            return header_info(self.format.ok_or(WavError::HeaderTooLong));
        }
        let file = open_file(&path);
        if file == 0 {
            count_error();
            return FileInfo::NotPlayable(NotPlayable::Unreadable);
        }
        // SAFETY: an entry of the file table that this scan wrote.
        let bytes = unsafe { (*card_file(id)).bytes };
        let info = header_info(read_header(file, bytes));
        close_file(file);
        info
    }
}

/// The reader task: serves commands first, then keeps the ring full, then lists the card and
/// reads headers, one step at a time, and sleeps when there is nothing to do.
extern "C" fn reader_task(_: isize) {
    let mut reader = Reader::new();
    loop {
        HOLDING.store(true, SeqCst);
        if !active() {
            reader.stop();
            HOLDING.store(false, SeqCst);
            sleep_ms(AWAY_MS);
            continue;
        }
        if reader.serve_commands() || reader.fill_ring() || reader.scan_step() {
            continue;
        }
        STATE.store(IDLE, Relaxed);
        // A file left open, while paused, still counts: the bat screen's closing waits for it.
        HOLDING.store(reader.file != 0, SeqCst);
        sleep_ms(IDLE_MS);
    }
}
