pub const DISPLAY: usize = 0x87eb_0000; // 320 x 480 RGB565, 0x4b000 bytes
pub const TE_STORAGE: usize = 0x87f0_0000; // f32 x TE_SAMPLES
pub const TE_SAMPLES: usize = 96_000; // 0.5 s at 192 kHz
/// SDRAM between two zeroed ranges, 0x865e1d88 to 0x865fffff, which no
/// stock code refers to; the MPU maps it cached (region 9, 0x86400000 to
/// 0x865fffff). The reset handler points the stack at the top of this range for
/// one call to a function with no stack use, then moves it to DTCM, so nothing is
/// stored here. The interrupt and the tasks run on one core, so cached rings need
/// no maintenance.
pub const OUT_RING: usize = 0x865e_2000; // [f32; 2] x OUT_FRAMES, left then right, 120 KB
/// As many frames as fit in that range. Not a power of two, so an index into the ring
/// comes from `%`, and the frame counts jump in the ring when they wrap; see
/// `OUT_WRITE` in state.rs for what that does.
pub const OUT_FRAMES: u32 = 15_360;
const _: () = assert!(OUT_RING + OUT_FRAMES as usize * 8 == 0x8660_0000);
// The audio hook reads 4 frames a block and relies on a block not crossing the end.
const _: () = assert!(OUT_FRAMES.is_multiple_of(4));
/// The input ring is `IN_RING`, in DTCM (see state.rs).
pub const IN_FRAMES: u32 = 8_192;

// From here to the end of SDRAM (0x87f60000 to 0x87ffffff), after the time expander's storage:
// uncached like the display buffer. Each range below starts 32-byte aligned (a cache line, and
// what the card's DMA wants), and the asserts check that each ends before the next starts.

/// The stack task 1 runs the patch's heavy work on (its own is 6 KB): building the app the first
/// time the bat screen opens, and processing when there is no DSP task. It was in the patch
/// region until B254, which needed that room for the resampler's tables. Uncached makes it
/// slower, but it is used little: the DSP task does the processing.
pub const PATCH_STACK: usize = 0x87f6_0000;
pub const PATCH_STACK_BYTES: usize = 40 * 1024;
const _: () = assert!(PATCH_STACK >= TE_STORAGE + TE_SAMPLES * 4);
/// BAT PLAYBACK's list as the app keeps it: `[bat_app::FileEntry; LIST_FILES]`.
pub const FILE_LIST: usize = 0x87f6_a000;
pub const FILE_LIST_BYTES: usize = 0xc000;
/// Files the list holds, and the reader's own table of them (see reader.rs).
pub const LIST_FILES: usize = 512;
const _: () = assert!(FILE_LIST >= PATCH_STACK + PATCH_STACK_BYTES);
/// The reader's table of the files it has listed: [`super::reader::CardFile`] for each.
pub const CARD_FILES: usize = 0x87f7_6000;
pub const CARD_FILES_BYTES: usize = 0xd000;
const _: () = assert!(CARD_FILES >= FILE_LIST + FILE_LIST_BYTES);
/// The reader's table of folders: [`super::reader::CardFolder`] for each.
pub const CARD_FOLDERS: usize = 0x87f8_3000;
pub const CARD_FOLDERS_BYTES: usize = 0x1000;
const _: () = assert!(CARD_FOLDERS >= CARD_FILES + CARD_FILES_BYTES);
/// What the reader has found, for the DSP task: [`super::reader::Event`] for each.
pub const EVENTS: usize = 0x87f8_4000;
pub const EVENTS_BYTES: usize = 0x6000;
const _: () = assert!(EVENTS >= CARD_FOLDERS + CARD_FOLDERS_BYTES);
/// A file's first bytes, for its header.
pub const HEADER: usize = 0x87f8_a000;
pub const HEADER_BYTES: usize = 32 * 1024;
const _: () = assert!(HEADER >= EVENTS + EVENTS_BYTES);
/// The audio of the file playing, as read from the card, for the DSP task: 13 reads of 32 KB,
/// 0.37 s of 192 kHz 24-bit stereo, to the end of SDRAM. 0x87f92000 to 0x87f97fff is free.
pub const READ_RING: usize = 0x87f9_8000;
pub const READ_RING_BYTES: u32 = 13 * 32 * 1024;
const _: () = assert!(READ_RING >= HEADER + HEADER_BYTES);
const _: () = assert!(READ_RING + READ_RING_BYTES as usize == 0x8800_0000);
const _: () = assert!(READ_RING.is_multiple_of(32) && HEADER.is_multiple_of(32));
