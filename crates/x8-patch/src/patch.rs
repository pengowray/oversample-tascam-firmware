use core::cell::UnsafeCell;
use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::fb::Rgb565Buffer;
use bat_app::{BatApp, Buffers, Button, InputKind, InputName, Mode, Recorder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

/// Stock firmware addresses, V2.03 build 217. See docs/HOMEBREW.md.
mod stock {
    /// The system mode: 0 or 1 for a normal start, 3 in firmware update mode.
    pub const SYSTEM_MODE: *const u8 = 0x8196_9dd9 as *const u8;
    /// The key handler the launcher and app screens share (Thumb address).
    pub const KEY_HANDLER: usize = 0x80da_c0d5;
    /// Qt's two framebuffers: 320 x 480, RGB565, mapped uncached by the MPU.
    pub const FRAMEBUFFERS: [usize; 2] = [0x87e0_0000, 0x87e4_b000];
    pub const WIDTH: usize = 320;
    pub const HEIGHT: usize = 480;

    /// Literal that Qt's present function (0x80e2b574) uses as the LCDIF
    /// register base for its writes, NEXT_BUF among them (base + 0x4c).
    /// Nothing else reads it.
    pub const PRESENT_LCDIF_LITERAL: *mut usize = 0x80e2_b71c as *mut usize;
    pub const LCDIF_CTRL_SET: usize = 0x402b_8004;
    /// The display controller's current and next buffer registers.
    pub const LCDIF_CUR_BUF: *const usize = 0x402b_8040 as *const usize;
    pub const LCDIF_NEXT_BUF: *mut usize = 0x402b_8050 as *mut usize;
    /// `FUN_80e0364a(x, y, pressed)`: posts a touch to Qt's event queue;
    /// the touch poll (task 9) calls it on every change (Thumb address).
    pub const QT_TOUCH: usize = 0x80e0_364b;

    /// Where `EDMA_SetCallback` stored the audio callback: the channel 9
    /// (SAI1 receive) eDMA handle's first word.
    pub const AUDIO_CALLBACK_SLOT: *mut usize = 0x2001_024c as *mut usize;
    /// The stock audio callback (ITCM, Thumb address).
    pub const AUDIO_CALLBACK: usize = 0x0002_adfd;
    /// Ring index of the input block the callback is about to process.
    pub const AUDIO_INDEX: *const u32 = 0x2001_0268 as *const u32;
    /// Three input blocks of 0x180 bytes: per frame 12 signed 32-bit words.
    pub const INPUT_BLOCKS: usize = 0x2000_e340;
    /// The word in an input frame that carries each of inputs 1 to 6 (the
    /// high gain path of its dual-gain pair). EXT IN uses the words of inputs
    /// 5 and 6.
    pub const INPUT_WORDS: [u32; 6] = [2, 5, 0, 6, 8, 11];
    /// DSP state + 0x622: 1 when tracks hear inputs 1 and 2 swapped.
    pub const SWAP_INPUTS_1_2: *const u8 = 0x0002_a2ce as *const u8;
    /// Settings + 0x30, one byte every 0x20 bytes for tracks 1 to 6: the pair
    /// of inputs the track hears (INPUT SELECT), 1 INPUT 1-2, 2 INPUT 3-4,
    /// 3 INPUT 5-6, 4 EXT IN. An odd track hears the left of the pair.
    pub const TRACK_SOURCES: *const u8 = 0x81e8_c8ac as *const u8;
    pub const TRACK_SOURCE_STEP: usize = 0x20;
    pub const TRACKS: usize = 6;
    /// Settings + 0x200 and + 0x204: bit 0 to 5 set in either word when track
    /// 1 to 6 is turned on (red on MANUAL's INPUT screen).
    pub const TRACKS_ON: [*const u32; 2] = [0x81e8_ca7c as *const u32, 0x81e8_ca80 as *const u32];
    /// Three output blocks of 0x40 bytes: per frame left, right.
    pub const OUTPUT_BLOCKS: usize = 0x2001_04c0;
    /// Rate class: 0 for 44.1/48 kHz, 1 for 88.2/96, 2 for 176.4/192.
    pub const RATE_CLASS: *const u8 = 0x0002_a2cb as *const u8;
    /// The output gain (f32), smoothed once per block towards -10^(-v/20)
    /// for the 0 to 10 dB output level setting (settings + 0x253). It falls
    /// towards 0 only while a sample rate change mutes the output.
    pub const OUTPUT_GAIN: *const f32 = 0x0002_9ed8 as *const f32;
    /// DSP state + 0x62d: 1 while the transport plays a file into the mix.
    pub const DSP_PLAYING: *const u8 = 0x0002_a2d9 as *const u8;

    /// Message fields (a copy on task 1's stack): +0 category (0 = input),
    /// +4 kind (0 button, 2 wheel, 9 swallowed), +8 button code or count.
    pub const KIND_BUTTON: u16 = 0;
    pub const KIND_WHEEL: u16 = 2;
    pub const KIND_SWALLOWED: u16 = 9;
    pub const NO_KEY: i32 = 0x4d;
    pub const PLAY: i32 = 0x05;

    /// The RTOS's uITRON calls (ITCM, Thumb addresses). `cre_tsk(id, pk)`
    /// accepts ids 1 to 23 (the maximum is at 0x20004148), returns -41 when
    /// the id is taken, and starts the task at once with attribute TA_ACT.
    /// `dly_tsk(ms)` sleeps the calling task (the kernel tick is 2 ms).
    pub const CRE_TSK: usize = 0x0002_d4a3;
    pub const DLY_TSK: usize = 0x0001_a7f5;
    pub const HOME: i32 = 0x06;
    pub const MARK: i32 = 0x2b;

    /// The recording screen's key handler, installed from record standby to
    /// the end of a take in place of KEY_HANDLER (Thumb address).
    pub const REC_KEY_HANDLER: usize = 0x80dd_3f9d;
    /// Qt's bridge setter `FUN_80d6946c(bridge, property, value, from_c)`;
    /// property 6 opens an app by its Qt number (Thumb address).
    pub const BRIDGE_SET: usize = 0x80d6_946d;
    pub const APP_PROPERTY: usize = 6;
    pub const QT_MANUAL: usize = 1;
    /// `FUN_80d6a1c0(bridge, slot)`: loads a USER SETTINGS preset, slot 0 to 2
    /// (Thumb address).
    pub const LOAD_USER_SETTINGS: usize = 0x80d6_a1c1;
    /// `FUN_80d6a0f8(bridge, slot, app, from_c)`: with from_c = 0 and app 0,
    /// asks the C core to delete a preset (Thumb address).
    pub const DELETE_USER_SETTINGS: usize = 0x80d6_a0f9;
    /// The bridge's USER SETTINGS slots: per slot, the Qt number of the app
    /// whose settings it holds (1 MANUAL, 2 PODCAST, 9 POCKET STUDIO), or 0.
    pub const USER_SETTINGS_SLOTS: *const u32 = 0x81e9_da60 as *const u32;
    /// The launcher's cursor, an int property at root + 0xd360: 1 MANUAL to
    /// 14 POCKET STUDIO, with 11 to 13 for USER SETTINGS 1 to 3.
    pub const CURSOR_PROPERTY: usize = 0xd360;
    pub const CURSOR_USER_SETTINGS_1: i32 = 11;
    /// `FUN_80e02a34(property)`: a pointer to a Qt property's value, noting
    /// the read for the binding being evaluated (Thumb address).
    pub const PROPERTY_VALUE: usize = 0x80e0_2a35;
    /// `FUN_80e3ea10(to, from)` assigns a Qt for MCUs string and
    /// `FUN_80e03154(string)` releases one (Thumb addresses).
    pub const STRING_ASSIGN: usize = 0x80e3_ea11;
    pub const STRING_RELEASE: usize = 0x80e0_3155;
    /// The launcher's text bindings `(out, binding, root)`: the title,
    /// description lines 1 to 6, and the label shown over the title in delete
    /// mode (Thumb addresses).
    pub const LAUNCHER_TEXTS: [usize; 8] =
        [0x80f2_326b, 0x80f2_393d, 0x80f2_3da9, 0x80f2_4225, 0x80f2_4695, 0x80f2_4ad9, 0x80f2_4e59, 0x80f2_9139];
    /// `FUN_80f23220(binding, root)`: the title's y, (H + 1) / 2 - 100, or
    /// - 25 for the USER SETTINGS tiles, which have no description (Thumb).
    pub const TITLE_Y: usize = 0x80f2_3221;
    pub const USER_SETTINGS_TITLE_DROP: usize = 75;
    /// The USER SETTINGS 2 and 3 tiles' delete badges `(binding, root)`:
    /// slot in use and delete mode on (Thumb addresses).
    pub const DELETE_BADGES: [usize; 2] = [0x80f2_a7e5, 0x80f2_b2b9];
    /// The USER SETTINGS 2 and 3 tiles' icon visibility `(binding, root)`:
    /// slot in use and delete mode off (Thumb addresses).
    pub const ICONS_SHOWN: [usize; 2] = [0x80f2_aa41, 0x80f2_b511];
    /// `FUN_80e05870(property, &value)`: sets a Qt property (Thumb address).
    pub const SET_PROPERTY: usize = 0x80e0_5871;
    /// The audio engine: 1 playing, 2 recording, 0 idle or paused.
    pub const ENGINE_STATE: *const u32 = 0x81cb_e780 as *const u32;
    /// The open app in the C core's numbering: 0 the LAUNCHER, 2 MANUAL.
    pub const APP: *const u8 = 0x81e8_ca90 as *const u8;
    pub const C_MANUAL: u8 = 2;
    /// Task 1's system state: 1 app screens, 2 recording screen, 6 a pop-up
    /// (which installs its own key handler).
    pub const SYSTEM_STATE: *const u8 = 0x8196_a3e0 as *const u8;
    pub const STATE_POPUP: u8 = 6;
    /// The recorder: 0 stopped, 1 and 2 standby, 3 recording, 4 paused,
    /// 5 closing the file, 6 paused by an event, 7 leaving standby, 8
    /// self-timer.
    pub const TRANSPORT: *const u8 = 0x8196_a3e5 as *const u8;
    /// Frames in the current take, as `FUN_80dae75c` works it out: the frozen
    /// copy when the flag is set, otherwise offset + count - start.
    pub const TAKE_FROZEN_FLAG: *const u8 = 0x8191_3f70 as *const u8;
    pub const TAKE_FROZEN: *const u32 = 0x8191_448c as *const u32;
    pub const TAKE_OFFSET: *const u32 = 0x8191_40c4 as *const u32;
    pub const TAKE_COUNT: *const u32 = 0x8191_4178 as *const u32;
    pub const TAKE_START: *const u32 = 0x8191_3f60 as *const u32;
    /// The rate the converters run at now (Hz), and the record rate setting.
    pub const CONVERTER_RATE: *const u32 = 0x8191_4120 as *const u32;
    pub const RECORD_RATE: *const u32 = 0x81cb_e678 as *const u32;
}

/// SDRAM after Qt's framebuffers, 0x87ea6004 to 0x87ffffff: zeroed by no init
/// table entry and referred to by no stock code (checked in Ghidra). Mapped
/// uncached, so the display controller sees what is written without cache
/// maintenance.
mod spare {
    pub const DISPLAY: usize = 0x87eb_0000; // 320 x 480 RGB565, 0x4b000 bytes
    pub const TE_STORAGE: usize = 0x87f0_0000; // f32 x TE_SAMPLES
    pub const TE_SAMPLES: usize = 96_000; // 0.5 s at 192 kHz
    /// SDRAM between two zeroed ranges, 0x865e1d88 to 0x865fffff, which no
    /// stock code refers to; the MPU maps it cached (region 9). The interrupt
    /// and the tasks run on one core, so cached rings need no maintenance.
    pub const OUT_RING: usize = 0x865e_2000; // f32 x OUT_SAMPLES, 64 KB
    pub const OUT_SAMPLES: u32 = 16_384;
    /// The input ring is in the patch region (`IN_RING`), also cached.
    pub const IN_FRAMES: u32 = 8_192;
}

type Handler = unsafe extern "C" fn(usize, usize, usize, usize) -> usize;

/// What `tools/x8patch.py` reads to find the entry points: it
/// must stay at the start of the patch.
#[repr(C)]
pub struct Header {
    magic: [u8; 4],
    version: u32,
    /// Replaces the stock key handler of the launcher and app screens.
    key_hook: Handler,
    /// Replaces the recording screen's key handler.
    rec_key_hook: Handler,
    /// Replaces the touch poll's call that passes a touch to Qt.
    touch_hook: Handler,
    /// Replaces the USER SETTINGS 2 and 3 tiles' call that loads a preset.
    tile_hook: Handler,
    /// Replace the bridge's slot getter and first free slot (whole functions).
    slot_hook: Handler,
    free_slot_hook: Handler,
    /// Wrap the launcher's title and description line bindings.
    title_hook: Handler,
    line_hooks: [Handler; 6],
    /// Wraps the delete mode label binding.
    label_hook: Handler,
    /// Wraps the title's y binding.
    title_y_hook: Handler,
    /// Replaces the USER SETTINGS 2 and 3 tiles' tap handlers' read of the
    /// delete mode property.
    delete_mode_hook: Handler,
    /// Replaces the delete listener's tail call that deletes a preset.
    delete_hook: Handler,
    /// Wrap the USER SETTINGS 2 and 3 tiles' delete badge bindings.
    badge_hooks: [Handler; 2],
    /// Wrap the USER SETTINGS 2 and 3 tiles' icon visibility bindings.
    icon_hooks: [Handler; 2],
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".header")]
pub static X8_PATCH_HEADER: Header = Header {
    magic: *b"BATX",
    version: 13,
    key_hook,
    rec_key_hook,
    touch_hook,
    tile_hook,
    slot_hook,
    free_slot_hook,
    title_hook: text_hook::<0>,
    line_hooks: [text_hook::<1>, text_hook::<2>, text_hook::<3>, text_hook::<4>, text_hook::<5>, text_hook::<6>],
    label_hook: text_hook::<7>,
    title_y_hook,
    delete_mode_hook,
    delete_hook,
    badge_hooks: [badge_hook::<0>, badge_hook::<1>],
    icon_hooks: [icon_hook::<0>, icon_hook::<1>],
};

/// The bat tiles, in USER SETTINGS slots 2 and 3 (slots 0 to 2 in the
/// firmware's numbering). Slot 1 stays a USER SETTINGS preset.
const RECORDER_SLOT: usize = 1;
const PLAYBACK_SLOT: usize = 2;

/// Each bat tile's launcher texts: its title, then description lines 1 to 6
/// (ASCII; empty for an unused line).
const TILE_TEXTS: [[&[u8]; 7]; 2] = [
    [
        b"BAT DETECTOR",
        b"Hear and record bat calls",
        b"live, and see them on a",
        b"spectrogram. In MANUAL,",
        b"turn off inputs without",
        b"ultrasonic mics. Set the",
        b"sample rate to 192kHz.",
    ],
    [
        b"BAT PLAYBACK",
        b"Hear bat calls in 192kHz",
        b"recordings. PLAY plays or",
        b"pauses the current file in",
        b"MANUAL. To choose another",
        b"file, press HOME briefly.",
        b"Experimental and untested.",
    ],
];

fn bat_tile(slot: usize) -> Option<usize> {
    match slot {
        RECORDER_SLOT => Some(0),
        PLAYBACK_SLOT => Some(1),
        _ => None,
    }
}

const SAMPLE_RATE: f32 = 192_000.0;
/// Output samples buffered before the headphones start playing the bat app's
/// output (43 ms at 192 kHz), so the DSP task's passes do not run dry.
const PRIME_SAMPLES: u32 = 8_192;
/// Frames processed per call at most when processing in task 1 (only if the
/// DSP task could not be created), so one call cannot hold task 1 for long.
const MAX_FRAMES_PER_CALL: u32 = 24_576;
const CHUNK: usize = 1_024;
const RENDERS_PER_SECOND: u32 = 10;
/// The DSP task stops a pass after this much wall time (30 ms) even with input
/// waiting, then draws and sleeps, so a mode that cannot keep up cannot starve
/// Qt (which also handles powering off). Input left beyond BACKLOG_FRAMES
/// after a pass is dropped, so the sound stays current.
const PASS_BLOCKS: u32 = 1_440;
const BACKLOG_FRAMES: u32 = 4_096;
const SLEEP_MS: u32 = 2;
/// Gain on the app's output before the stock headphone gain (+18 dB), with a
/// ceiling of -6 dBFS: bat calls are often 40 to 60 dB below full scale.
const OUTPUT_GAIN: f32 = 8.0;
const OUTPUT_CEILING: f32 = 0.5;

/// The DSP task: unused ids at start-up (the table at 0x8131dac4 uses 1, 2,
/// 4 to 7, 9 to 12 and 15 to 22), tried in turn. Priority 16 is below task 1
/// (15) and every audio and recording task, equal to the settings save task,
/// and above task 21 (17), which has a 1 MB stack and is probably Qt.
const TASK_IDS: [i32; 5] = [23, 14, 13, 8, 3];
const TASK_PRIORITY: u32 = 16;
const TA_ACT: u32 = 0x02;
const E_OBJ: i32 = -41;

#[repr(C)]
struct CreateTask {
    attributes: u32,
    extended_info: isize,
    entry: extern "C" fn(isize),
    priority: u32,
    stack_size: usize,
    stack: *mut u8,
}
const SCALE: f32 = 1.0 / 2_147_483_648.0;

// Task 1 only.
static CALLS: AtomicU32 = AtomicU32::new(0);
static CALLS_PER_S: AtomicU32 = AtomicU32::new(0);
static LAST_SECOND: AtomicU32 = AtomicU32::new(0);
static CALLS_AT_SECOND: AtomicU32 = AtomicU32::new(0);
static LAST_EVENT: AtomicU32 = AtomicU32::new(0);
static AUDIO_STATE: AtomicU32 = AtomicU32::new(AUDIO_NOT_TRIED);
const AUDIO_NOT_TRIED: u32 = 0;
const AUDIO_INSTALLED: u32 = 1;
const AUDIO_SLOT_UNEXPECTED: u32 = 2;
static APP_READY: AtomicBool = AtomicBool::new(false);
static LAST_RENDER: AtomicU32 = AtomicU32::new(0);
static SAVED_LITERAL: AtomicU32 = AtomicU32::new(0);
static SAVED_BUFFER: AtomicU32 = AtomicU32::new(0);
/// Set from task 1 for the work to run on the patch stack.
static PENDING: AtomicU32 = AtomicU32::new(0);
const WORK_ENTER: u32 = 1;
/// The DSP task's id once created, or the error cre_tsk returned (negative).
static TASK: AtomicU32 = AtomicU32::new(0);
/// Counts openings of the bat screen; the context running the app empties
/// the rings when it sees a new one.
static SESSION: AtomicU32 = AtomicU32::new(0);
static SESSION_SEEN: AtomicU32 = AtomicU32::new(0);
/// Button and wheel actions from task 1 for the app, applied by whichever
/// context runs it: (action << 24) | (value & 0xffffff).
static ACTIONS: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
static ACTIONS_WRITE: AtomicU32 = AtomicU32::new(0);
static ACTIONS_READ: AtomicU32 = AtomicU32::new(0);
const ACTION_MARK: u32 = 1;
const ACTION_WHEEL: u32 = 4;
/// Set when a bat tile was tapped: the bat screen opens once the C core has
/// switched to the app the tile asked for.
static LAUNCHING: AtomicBool = AtomicBool::new(false);
/// The last bat tile tapped was BAT PLAYBACK: the app hears the X8's
/// playback (the stock output) instead of the inputs turned on in MANUAL, and
/// PLAY and a short HOME press go on to MANUAL's player.
static PLAYBACK: AtomicBool = AtomicBool::new(false);
static LAUNCH_FROM: AtomicU32 = AtomicU32::new(0);
static LAUNCH_AT: AtomicU32 = AtomicU32::new(0);
/// The app (C numbering) the bat screen was opened in, or NO_APP. It stays
/// set while PLAY shows the app's own screen, and is cleared when the app
/// changes.
static SCREEN_APP: AtomicU32 = AtomicU32::new(NO_APP);
const NO_APP: u32 = 0x100;
/// The DSP task gave the display back for a pop-up; task 1 takes it again
/// once the pop-up has closed.
static POPUP_HIDDEN: AtomicBool = AtomicBool::new(false);

// CPU share, in 1/1000 of the blocks in the last second (DSP task only).
static DSP_BLOCKS: AtomicU32 = AtomicU32::new(0);
static DRAW_BLOCKS: AtomicU32 = AtomicU32::new(0);
static DSP_PERMILLE: AtomicU32 = AtomicU32::new(0);
static DRAW_PERMILLE: AtomicU32 = AtomicU32::new(0);
static LOAD_SECOND: AtomicU32 = AtomicU32::new(0);

// Shared with the audio interrupt.
static BAT_ACTIVE: AtomicBool = AtomicBool::new(false);
static BLOCKS: AtomicU32 = AtomicU32::new(0);
static IN_WRITE: AtomicU32 = AtomicU32::new(0); // interrupt
static IN_READ: AtomicU32 = AtomicU32::new(0); // task 1
static IN_DROPPED: AtomicU32 = AtomicU32::new(0); // interrupt
static OUT_WRITE: AtomicU32 = AtomicU32::new(0); // task 1
static OUT_READ: AtomicU32 = AtomicU32::new(0); // interrupt
static PRIMED: AtomicBool = AtomicBool::new(false); // interrupt
static UNDERRUNS: AtomicU32 = AtomicU32::new(0); // interrupt
static BACKLOG_DROPPED: AtomicU32 = AtomicU32::new(0); // DSP task
/// Peak magnitude of each of the 12 words of an input frame since the last
/// redraw: the interrupt raises them, the drawing context reads and clears.
static WORD_PEAKS: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
/// The same for the left channel of the stock output, unscaled, in BAT
/// PLAYBACK.
static SOURCE_PEAK: AtomicU32 = AtomicU32::new(0);
/// The words of an input frame the interrupt passes to the app, first | second
/// << 8, or `NO_INPUT_WORDS` to pass silence. Inputs 3 and 4 until
/// `update_status` first runs.
static HEARD_WORDS: AtomicU32 = AtomicU32::new(stock::INPUT_WORDS[2] | stock::INPUT_WORDS[3] << 8);
const NO_INPUT_WORDS: u32 = u32::MAX;
/// Core cycles in the last second: in all, in the stock audio callback, and in
/// the rest of the audio hook (interrupt; read by the drawing context).
static CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
static STOCK_CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
static HOOK_CYCLES_PER_S: AtomicU32 = AtomicU32::new(0);
static CYCLES_AT_SECOND: AtomicU32 = AtomicU32::new(0);
static BLOCKS_AT_SECOND: AtomicU32 = AtomicU32::new(0);
static STOCK_CYCLES: AtomicU32 = AtomicU32::new(0);
static HOOK_CYCLES: AtomicU32 = AtomicU32::new(0);
static CYCLE_COUNTER_ON: AtomicBool = AtomicBool::new(false);

/// Arm's debug registers for the cycle counter (privileged access only, so
/// used from the audio interrupt).
const DEMCR: *mut u32 = 0xe000_edfc as *mut u32;
const DWT_CTRL: *mut u32 = 0xe000_1000 as *mut u32;
const DWT_CYCCNT: *const u32 = 0xe000_1004 as *const u32;
const DWT_LAR: *mut u32 = 0xe000_1fb0 as *mut u32;

fn cycles() -> u32 {
    // SAFETY: reading the cycle counter, from the interrupt.
    unsafe { ptr::read_volatile(DWT_CYCCNT) }
}

/// Statics written by one context at a time, as the comments on each use say.
struct Slot<T>(UnsafeCell<MaybeUninit<T>>);
// SAFETY: see each use.
unsafe impl<T> Sync for Slot<T> {}

/// The bat app: built in task 1 on the patch stack, then used only by the
/// DSP task (or, without one, by task 1). Its state is in DTCM; its large
/// buffers are in SDRAM.
#[unsafe(link_section = ".dtcm")]
static APP: Slot<BatApp<'static>> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
#[unsafe(link_section = ".noinit")]
static BUFFERS: Slot<Buffers> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

unsafe extern "C" {
    /// The hot code's place in ITCM and its copy in the patch (x8patch.ld).
    static __itcm_start: u8;
    static __itcm_end: u8;
    static __itcm_load: u8;
}
/// Checksum of the code copied into ITCM, to stop processing if anything
/// changes it (no stock code is known to use that part of ITCM).
static ITCM_SUM: AtomicU32 = AtomicU32::new(0);
static ITCM_OK: AtomicBool = AtomicBool::new(false);
static ITCM_CHECKED_AT: AtomicU32 = AtomicU32::new(0);

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
fn load_itcm() {
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
/// The off-screen frame: cached, so drawing is fast, then copied to the
/// display buffer in one pass.
#[unsafe(link_section = ".noinit")]
static FRAME: Slot<[u16; stock::WIDTH * stock::HEIGHT]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
/// The input ring: frames of the two inputs the app hears, from the interrupt.
#[unsafe(link_section = ".dtcm")]
static IN_RING: Slot<[[i32; 2]; spare::IN_FRAMES as usize]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
const DSP_STACK_BYTES: usize = 32 * 1024;
#[repr(C, align(8))]
struct DspStack([u8; DSP_STACK_BYTES]);
#[unsafe(link_section = ".dtcm")]
static DSP_STACK: Slot<DspStack> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
/// Where Qt's present function writes while the bat screen is up (it writes
/// at offsets 0x0, 0x10 and 0x4c of what it takes for the LCDIF registers).
static DUMMY_LCDIF: Slot<[u32; 32]> = Slot(UnsafeCell::new(MaybeUninit::zeroed()));

/// The stack task 1 runs the patch's heavy work on (its own is 6 KB): building
/// the app, and processing when there is no DSP task.
const STACK_WORDS: usize = 10 * 1024;
#[repr(C, align(8))]
struct Stack([u32; STACK_WORDS]);
#[unsafe(link_section = ".noinit")]
static STACK: Slot<Stack> = Slot(UnsafeCell::new(MaybeUninit::uninit()));

fn system_mode() -> u8 {
    // SAFETY: a byte the stock firmware sets before the RTOS starts.
    unsafe { ptr::read_volatile(stock::SYSTEM_MODE) }
}

fn rate_class() -> u8 {
    // SAFETY: a byte in the stock DSP state.
    unsafe { ptr::read_volatile(stock::RATE_CLASS) }
}

fn blocks_per_second() -> u32 {
    if rate_class() == 0 { 24_000 } else { 48_000 }
}

/// Bytes the stock firmware keeps in RAM; single byte reads.
fn read_byte(at: *const u8) -> u8 {
    // SAFETY: addresses named in `stock`.
    unsafe { ptr::read_volatile(at) }
}

fn app_byte() -> u8 {
    read_byte(stock::APP)
}

fn system_state() -> u8 {
    read_byte(stock::SYSTEM_STATE)
}

/// Which stock key handler a message goes on to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    /// The launcher and app screens' handler.
    App,
    /// The recording screen's handler (record standby and takes).
    Rec,
}

/// Called by task 1 for every message while the launcher or an app is on
/// screen, with the stock handler's arguments; the first is the message.
unsafe extern "C" fn key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as the stock handler is called.
    unsafe { on_message(msg, Via::App) };
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
}

/// The same, while the recording screen's handler is installed.
unsafe extern "C" fn rec_key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as the stock handler is called.
    unsafe { on_message(msg, Via::Rec) };
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::REC_KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
}

/// Replaces the USER SETTINGS 2 and 3 tiles' tail call that loads a preset
/// (on the Qt task, with r0 = bridge and r1 = the slot): opens MANUAL, as
/// its own tile does, so the C core runs a recording app, and has task 1 put
/// the bat screen over it.
unsafe extern "C" fn tile_hook(bridge: usize, slot: usize, c: usize, d: usize) -> usize {
    let Some(tile) = bat_tile(slot).filter(|_| system_mode() <= 1) else {
        // SAFETY: the stock function the tile called, with its arguments.
        let load: Handler = unsafe { core::mem::transmute(stock::LOAD_USER_SETTINGS) };
        return unsafe { load(bridge, slot, c, d) };
    };
    PLAYBACK.store(tile == 1, Relaxed);
    LAUNCH_FROM.store(app_byte() as u32, Relaxed);
    LAUNCH_AT.store(BLOCKS.load(Relaxed), Relaxed);
    LAUNCHING.store(true, Release);
    // SAFETY: the bridge setter, called as the MANUAL tile calls it.
    let set: Handler = unsafe { core::mem::transmute(stock::BRIDGE_SET) };
    unsafe { set(bridge, stock::APP_PROPERTY, stock::QT_MANUAL, 0) }
}

/// What a USER SETTINGS slot holds, as the launcher sees it: the bat tiles'
/// slots hold MANUAL settings, so the launcher shows them (with MANUAL's
/// icon); slot 0 is the stock slot.
fn slot_value(slot: usize) -> u32 {
    if bat_tile(slot).is_some() && system_mode() <= 1 {
        return stock::QT_MANUAL as u32;
    }
    // SAFETY: the bridge's slot words (stock callers pass slots 0 to 2).
    unsafe { ptr::read_volatile(stock::USER_SETTINGS_SLOTS.add(slot)) }
}

/// Replaces the bridge's slot getter `FUN_80d6a238(bridge, slot)`, whose only
/// callers are the launcher's slot properties: slot 3 asks whether any slot
/// is in use.
unsafe extern "C" fn slot_hook(_bridge: usize, slot: usize, _: usize, _: usize) -> usize {
    if slot == 3 {
        ((slot_value(0) | slot_value(1) | slot_value(2)) & 0xff) as usize
    } else {
        slot_value(slot) as usize
    }
}

/// Replaces the bridge's first free slot `FUN_80d6a252()`, which the apps'
/// save screens read: 1 to 3, or 0 when every slot is in use. Only slot 0 is
/// offered, so a save never goes to a slot hidden behind a bat tile.
unsafe extern "C" fn free_slot_hook(_: usize, _: usize, _: usize, _: usize) -> usize {
    (1..=3).find(|&n| slot_value(n - 1) == 0).unwrap_or(0)
}

/// A Qt for MCUs string as the launcher's bindings build one from a literal:
/// the text at +0, and length << 8 | kind at +0x18 (kind 0: static text,
/// which releasing leaves alone).
#[repr(C)]
struct QtString {
    text: *const u8,
    unused: [u32; 5],
    length_kind: u32,
}

/// The bat tile the launcher's cursor is on, read as the launcher's
/// bindings read it (from a binding, this notes the read).
fn tile_at_cursor(root: usize) -> Option<usize> {
    // SAFETY: the cursor property of the launcher's root.
    let value: unsafe extern "C" fn(usize) -> *const i32 = unsafe { core::mem::transmute(stock::PROPERTY_VALUE) };
    let cursor = unsafe { ptr::read_volatile(value(root + stock::CURSOR_PROPERTY)) };
    usize::try_from(cursor - stock::CURSOR_USER_SETTINGS_1).ok().and_then(bat_tile)
}

/// A zero byte, the text of an empty string.
static EMPTY_TEXT: [u8; 1] = [0];

/// Wraps one of the launcher's text bindings (`LINE` 0 is the title, 1 to 6
/// the description lines, 7 the delete mode label): with the cursor on a bat
/// tile, the text is the tile's instead of "USER SETTINGS 2" or "3" and its
/// description. The delete mode label is drawn where a USER SETTINGS tile's
/// title is, so a bat tile leaves it empty, as the app tiles do.
unsafe extern "C" fn text_hook<const LINE: usize>(out: usize, binding: usize, root: usize, d: usize) -> usize {
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::LAUNCHER_TEXTS[LINE]) };
    let result = unsafe { binding_fn(out, binding, root, d) };
    if system_mode() > 1 {
        return result;
    }
    if let Some(tile) = tile_at_cursor(root) {
        let text = if LINE == 7 { &[][..] } else { TILE_TEXTS[tile][LINE] };
        let pointer = if text.is_empty() { EMPTY_TEXT.as_ptr() } else { text.as_ptr() };
        let from = QtString { text: pointer, unused: [0; 5], length_kind: (text.len() as u32) << 8 };
        // SAFETY: as the bindings assign a literal to their result and then
        // release the temporary.
        unsafe {
            let assign: unsafe extern "C" fn(usize, *const QtString) = core::mem::transmute(stock::STRING_ASSIGN);
            let release: unsafe extern "C" fn(*const QtString) = core::mem::transmute(stock::STRING_RELEASE);
            assign(out, &from);
            release(&from);
        }
    }
    result
}

/// Wraps the title's y binding: on a bat tile, which has a description, the
/// title (and the description lines placed under it) sits where it does on
/// the app tiles.
unsafe extern "C" fn title_y_hook(binding: usize, root: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::TITLE_Y) };
    let y = unsafe { binding_fn(binding, root, c, d) };
    if system_mode() <= 1 && tile_at_cursor(root).is_some() {
        y.wrapping_sub(stock::USER_SETTINGS_TITLE_DROP)
    } else {
        y
    }
}

/// An int property value of 0, for `delete_mode_hook`.
static NOT_DELETE_MODE: i32 = 0;

/// Replaces the read of the delete mode property (root + 0xd370) in the
/// USER SETTINGS 2 and 3 tiles' tap handlers: a bat tile is never deleted,
/// so a tap in delete mode ends delete mode, as a tap on an app tile does,
/// and opens the tile.
unsafe extern "C" fn delete_mode_hook(property: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        // SAFETY: the stock property setter, called as the app tiles' tap
        // handlers call it on this property.
        let set: unsafe extern "C" fn(usize, *const i32) = unsafe { core::mem::transmute(stock::SET_PROPERTY) };
        unsafe { set(property, &NOT_DELETE_MODE) };
        return &raw const NOT_DELETE_MODE as usize;
    }
    // SAFETY: the stock function the handler called, with its arguments.
    let value: Handler = unsafe { core::mem::transmute(stock::PROPERTY_VALUE) };
    unsafe { value(property, b, c, d) }
}

/// Replaces the delete listener's tail call `FUN_80d6a0f8(bridge, slot, 0, 0)`
/// (after YES in the delete pop-up): a bat tile's slot is never deleted, as
/// that would delete a preset hidden behind the tile.
unsafe extern "C" fn delete_hook(bridge: usize, slot: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 && bat_tile(slot).is_some() {
        return 0;
    }
    // SAFETY: the stock function the listener called, with its arguments.
    let delete: Handler = unsafe { core::mem::transmute(stock::DELETE_USER_SETTINGS) };
    unsafe { delete(bridge, slot, c, d) }
}

/// Wraps the delete badge binding of USER SETTINGS 2 (`TILE` 0) or 3 (1):
/// the bat tiles never show it.
unsafe extern "C" fn badge_hook<const TILE: usize>(binding: usize, root: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        return 0;
    }
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::DELETE_BADGES[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}

/// Wraps the icon visibility binding of USER SETTINGS 2 (`TILE` 0) or 3 (1),
/// which hides the icon in delete mode for the badge: the bat tiles always
/// show their icon.
unsafe extern "C" fn icon_hook<const TILE: usize>(binding: usize, root: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        return 1;
    }
    // SAFETY: the stock binding the launcher called, with its arguments.
    let binding_fn: Handler = unsafe { core::mem::transmute(stock::ICONS_SHOWN[TILE]) };
    unsafe { binding_fn(binding, root, c, d) }
}

/// Touches for the app, from task 9: pressed << 31 | x << 16 | y.
static TOUCHES: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
static TOUCH_WRITE: AtomicU32 = AtomicU32::new(0);
static TOUCH_READ: AtomicU32 = AtomicU32::new(0);
/// The last touch seen, for the debug strip.
static LAST_TOUCH: AtomicU32 = AtomicU32::new(0);
/// Qt has been sent a press without its release (task 9).
static QT_PRESSED: AtomicBool = AtomicBool::new(false);
/// The context running the app: a finger is down, and it went down where a
/// drag tunes.
static TOUCH_DOWN: AtomicBool = AtomicBool::new(false);
static DRAGGING: AtomicBool = AtomicBool::new(false);

/// Replaces the touch poll's call to Qt (task 9, on every change of x, y or
/// pressed). While the bat screen is up, touches go to the bat app instead,
/// except the release of a touch Qt saw start, so Qt never keeps a press.
unsafe extern "C" fn touch_hook(x: usize, y: usize, pressed: usize, d: usize) -> usize {
    // SAFETY: the stock function the poll called, with its arguments.
    let qt: Handler = unsafe { core::mem::transmute(stock::QT_TOUCH) };
    let event = ((pressed != 0) as u32) << 31 | (x as u32 & 0x7fff) << 16 | (y as u32 & 0xffff);
    LAST_TOUCH.store(event, Relaxed);
    if system_mode() > 1 || !BAT_ACTIVE.load(Acquire) {
        QT_PRESSED.store(pressed != 0, Relaxed);
        return unsafe { qt(x, y, pressed, d) };
    }
    if pressed == 0 && QT_PRESSED.load(Relaxed) {
        QT_PRESSED.store(false, Relaxed);
        unsafe { qt(x, y, 0, d) };
    }
    let write = TOUCH_WRITE.load(Relaxed);
    if write.wrapping_sub(TOUCH_READ.load(Acquire)) < TOUCHES.len() as u32 {
        TOUCHES[write as usize % TOUCHES.len()].store(event, Relaxed);
        TOUCH_WRITE.store(write.wrapping_add(1), Release);
    }
    0
}

/// Apply the queued touches (the context running the app): a touch down is a
/// tap; moves while down on the waterfall tune HET and COMB.
fn apply_touches(app: &mut BatApp<'static>) {
    let mut read = TOUCH_READ.load(Relaxed);
    while read != TOUCH_WRITE.load(Acquire) {
        let event = TOUCHES[read as usize % TOUCHES.len()].load(Relaxed);
        let (x, y) = ((event >> 16 & 0x7fff) as i32, (event & 0xffff) as i32);
        if event >> 31 == 0 {
            TOUCH_DOWN.store(false, Relaxed);
        } else if !TOUCH_DOWN.load(Relaxed) {
            TOUCH_DOWN.store(true, Relaxed);
            DRAGGING.store(app.is_draggable(x, y), Relaxed);
            app.touch(x, y);
        } else if DRAGGING.load(Relaxed) {
            app.drag(x, y);
        }
        read = read.wrapping_add(1);
        TOUCH_READ.store(read, Release);
    }
}

/// Task 1, for each message, from either key hook.
unsafe fn on_message(msg: usize, via: Via) {
    if system_mode() > 1 {
        return;
    }
    if AUDIO_STATE.load(Relaxed) == AUDIO_NOT_TRIED {
        install_audio_hook();
    }
    count_call();
    if AUDIO_STATE.load(Relaxed) == AUDIO_INSTALLED {
        follow_app();
    }
    // SAFETY: the message copy on task 1's stack, 0x14 bytes.
    unsafe { handle_message(msg, via) };
    let work = PENDING.swap(0, Relaxed);
    if work == WORK_ENTER && !BAT_ACTIVE.load(Relaxed) {
        // SAFETY: only task 1 runs on the patch stack.
        unsafe { on_patch_stack(enter_work) };
    } else if BAT_ACTIVE.load(Relaxed) {
        if !task_running() {
            // SAFETY: as above.
            unsafe { on_patch_stack(tick_work) };
        }
    } else if via == Via::App {
        draw_status_strip();
    }
}

/// Open the bat screen on task 1's next pass through `on_message`. The only
/// way into `enter_work`, so it never runs with the screen already up. While
/// a pop-up is open, wait for it to close.
fn request_enter() {
    if BAT_ACTIVE.load(Relaxed) {
        return;
    }
    if system_state() == stock::STATE_POPUP {
        SCREEN_APP.store(app_byte() as u32, Relaxed);
        POPUP_HIDDEN.store(true, Relaxed);
        return;
    }
    PENDING.store(WORK_ENTER, Relaxed);
}

/// Open the bat screen once a tap on a bat tile has taken effect, close it when
/// the app it was opened in closes, and take it back after a pop-up.
fn follow_app() {
    let app = app_byte() as u32;
    if LAUNCHING.load(Acquire) {
        let waited = BLOCKS.load(Relaxed).wrapping_sub(LAUNCH_AT.load(Relaxed));
        // The app byte changes when the C core has switched; if it already
        // said MANUAL, give the switch half a second.
        let switched = app != LAUNCH_FROM.load(Relaxed) && app != 0;
        if switched || (app == stock::C_MANUAL as u32 && waited >= blocks_per_second() / 2) {
            LAUNCHING.store(false, Relaxed);
            request_enter();
        } else if waited >= 5 * blocks_per_second() {
            LAUNCHING.store(false, Relaxed);
        }
    }
    let owner = SCREEN_APP.load(Relaxed);
    if owner != NO_APP && app != owner {
        if BAT_ACTIVE.load(Relaxed) {
            leave_bat_screen();
        }
        SCREEN_APP.store(NO_APP, Relaxed);
        POPUP_HIDDEN.store(false, Relaxed);
    }
    if POPUP_HIDDEN.load(Acquire) && system_state() != stock::STATE_POPUP {
        POPUP_HIDDEN.store(false, Relaxed);
        request_enter();
    }
}

/// Run `work` on the patch's own stack. The RTOS task switch (PendSV at
/// 0x3023c) only saves and restores the stack pointer, so a task may run on
/// any stack.
unsafe fn on_patch_stack(work: extern "C" fn()) {
    let top = (STACK.0.get() as usize + core::mem::size_of::<Stack>()) & !7;
    // SAFETY: r4 keeps the task's stack pointer across the call; the work
    // function follows the C ABI, so it may change r0 to r3, r12, lr and
    // s0 to s15 (this FPU has no d16 to d31) and keeps the rest.
    unsafe {
        core::arch::asm!(
            "mov r4, sp",
            "mov sp, {top}",
            "blx {work}",
            "mov sp, r4",
            top = in(reg) top,
            work = in(reg) work,
            out("r4") _,
            out("r0") _, out("r1") _, out("r2") _, out("r3") _, out("r12") _, out("lr") _,
            out("s0") _, out("s1") _, out("s2") _, out("s3") _, out("s4") _, out("s5") _, out("s6") _, out("s7") _,
            out("s8") _, out("s9") _, out("s10") _, out("s11") _, out("s12") _, out("s13") _, out("s14") _, out("s15") _,
        );
    }
}

fn install_audio_hook() {
    // SAFETY: the eDMA handle's callback word, set once by the stock setup.
    let current = unsafe { ptr::read_volatile(stock::AUDIO_CALLBACK_SLOT) };
    if current == 0 {
        // The audio is not set up yet: try again on the next call.
        return;
    }
    if current != stock::AUDIO_CALLBACK {
        AUDIO_STATE.store(AUDIO_SLOT_UNEXPECTED, Relaxed);
        return;
    }
    let hook: Handler = audio_hook;
    // SAFETY: a single aligned word store, which the interrupt reads whole.
    unsafe { ptr::write_volatile(stock::AUDIO_CALLBACK_SLOT, hook as usize) };
    AUDIO_STATE.store(AUDIO_INSTALLED, Relaxed);
}

fn count_call() {
    let calls = CALLS.load(Relaxed).wrapping_add(1);
    CALLS.store(calls, Relaxed);
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(LAST_SECOND.load(Relaxed)) >= blocks_per_second() {
        CALLS_PER_S.store(calls.wrapping_sub(CALLS_AT_SECOND.load(Relaxed)), Relaxed);
        CALLS_AT_SECOND.store(calls, Relaxed);
        LAST_SECOND.store(now, Relaxed);
    }
}

unsafe fn handle_message(msg: usize, via: Via) {
    let kind_ptr = (msg + 4) as *mut u16;
    let value_ptr = (msg + 8) as *mut i32;
    // SAFETY: fields of the message copy (see key_hook).
    let (category, kind, value) =
        unsafe { (ptr::read_volatile(msg as *const u16), ptr::read_volatile(kind_ptr), ptr::read_volatile(value_ptr)) };
    if category != 0 || kind == stock::KIND_SWALLOWED {
        return;
    }
    LAST_EVENT.store((kind as u32) << 16 | (value as u32 & 0xffff), Relaxed);
    if AUDIO_STATE.load(Relaxed) != AUDIO_INSTALLED {
        return;
    }
    let active = BAT_ACTIVE.load(Relaxed);
    let swallow_button = || unsafe { ptr::write_volatile(value_ptr, stock::NO_KEY) };
    match (kind, value) {
        (stock::KIND_BUTTON, stock::MARK) if active => {
            // A mark in the bat app (in TE, a capture). During a take MARK also
            // goes on to the recording screen's handler, which marks the file.
            // Otherwise it is kept from the app: in MANUAL without a file,
            // MARK opens a pop-up.
            queue_action(ACTION_MARK, 0);
            if !(via == Via::Rec && matches!(read_byte(stock::TRANSPORT), 3 | 4)) {
                swallow_button();
            }
        }
        (stock::KIND_BUTTON, stock::PLAY) if SCREEN_APP.load(Relaxed) == app_byte() as u32 => {
            if PLAYBACK.load(Relaxed) {
                // BAT PLAYBACK: PLAY goes on to MANUAL's player (play or pause
                // the current file) and brings the bat screen back if MANUAL's
                // own screen is showing.
                if !active {
                    request_enter();
                }
            } else {
                // BAT DETECTOR: PLAY swaps between the bat screen and the
                // app's own screen; the app's own playback is not used there.
                swallow_button();
                if active { leave_bat_screen() } else { request_enter() }
            }
        }
        (stock::KIND_BUTTON, stock::HOME) if active && PLAYBACK.load(Relaxed) && app_byte() != 0 => {
            // BAT PLAYBACK: a short press goes on (it stops playback) and shows
            // MANUAL's own screen, where another file can be chosen.
            leave_bat_screen();
        }
        (stock::KIND_BUTTON, stock::HOME) if active && app_byte() == 0 => {
            // On the LAUNCHER, where holding HOME does nothing. In an app a
            // short press goes on (it stops a take or playback, as in the
            // stock apps) and holding HOME returns to the LAUNCHER.
            swallow_button();
            leave_bat_screen();
        }
        (stock::KIND_WHEEL, count) if active => {
            // SAFETY: the kind field of the message copy.
            unsafe { ptr::write_volatile(kind_ptr, stock::KIND_SWALLOWED) };
            queue_action(ACTION_WHEEL, count as u32);
        }
        _ => {}
    }
}

/// Queue a button or wheel action for the app (task 1).
fn queue_action(action: u32, value: u32) {
    let write = ACTIONS_WRITE.load(Relaxed);
    if write.wrapping_sub(ACTIONS_READ.load(Acquire)) < ACTIONS.len() as u32 {
        ACTIONS[write as usize % ACTIONS.len()].store(action << 24 | (value & 0xff_ffff), Relaxed);
        ACTIONS_WRITE.store(write.wrapping_add(1), Release);
    }
}

/// Apply the queued actions (the context running the app).
fn apply_actions(app: &mut BatApp<'static>) {
    let mut read = ACTIONS_READ.load(Relaxed);
    while read != ACTIONS_WRITE.load(Acquire) {
        let a = ACTIONS[read as usize % ACTIONS.len()].load(Relaxed);
        let value = ((a << 8) as i32) >> 8;
        match a >> 24 {
            ACTION_MARK => app.button(Button::Mark),
            ACTION_WHEEL => app.wheel(value),
            _ => {}
        }
        read = read.wrapping_add(1);
        ACTIONS_READ.store(read, Release);
    }
}

/// Once a second, check that the code in ITCM is unchanged; if anything has
/// written over it, stop running it for good.
fn itcm_still_ok() -> bool {
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

/// Drop input beyond BACKLOG_FRAMES, so a mode that cannot keep up plays
/// current sound with gaps rather than ever later sound.
fn drop_backlog() {
    let read = IN_READ.load(Relaxed);
    let waiting = IN_WRITE.load(Acquire).wrapping_sub(read);
    if waiting > BACKLOG_FRAMES {
        let skip = waiting - BACKLOG_FRAMES;
        IN_READ.store(read.wrapping_add(skip), Release);
        BACKLOG_DROPPED.store(BACKLOG_DROPPED.load(Relaxed).wrapping_add(skip), Relaxed);
    }
}

/// At the start of each opening of the bat screen: empty the rings, from the
/// consumer's side of the input one and the producer's side of the output one,
/// and drop actions queued before.
fn new_session() {
    let session = SESSION.load(Acquire);
    if session != SESSION_SEEN.load(Relaxed) {
        SESSION_SEEN.store(session, Relaxed);
        IN_READ.store(IN_WRITE.load(Acquire), Release);
        OUT_WRITE.store(OUT_READ.load(Acquire), Release);
        ACTIONS_READ.store(ACTIONS_WRITE.load(Acquire), Release);
        TOUCH_READ.store(TOUCH_WRITE.load(Acquire), Release);
        TOUCH_DOWN.store(false, Relaxed);
    }
}

fn task_running() -> bool {
    (TASK.load(Relaxed) as i32) > 0
}

/// The bat app, once `enter_work` has built it. Only one context uses it at a
/// time: the DSP task, or task 1 when there is no DSP task.
unsafe fn app() -> &'static mut BatApp<'static> {
    // SAFETY: built by enter_work.
    unsafe { (*APP.0.get()).assume_init_mut() }
}

/// Create the DSP task, trying each free id in turn.
fn start_task() {
    // SAFETY: the RTOS's cre_tsk, from task 1.
    let create: unsafe extern "C" fn(i32, *const CreateTask) -> i32 = unsafe { core::mem::transmute(stock::CRE_TSK) };
    let pk = CreateTask {
        attributes: TA_ACT,
        extended_info: 0,
        entry: dsp_task,
        priority: TASK_PRIORITY,
        stack_size: DSP_STACK_BYTES,
        stack: DSP_STACK.0.get().cast(),
    };
    let mut result = E_OBJ;
    for id in TASK_IDS {
        // SAFETY: as above; pk lives until cre_tsk returns, and the stack for
        // as long as the patch.
        result = unsafe { create(id, &pk) };
        if result != E_OBJ {
            if result == 0 {
                result = id;
            }
            break;
        }
    }
    TASK.store(result as u32, Relaxed);
}

fn sleep_ms(ms: u32) {
    // SAFETY: the RTOS's dly_tsk, from the DSP task.
    let delay: unsafe extern "C" fn(u32) -> i32 = unsafe { core::mem::transmute(stock::DLY_TSK) };
    unsafe { delay(ms) };
}

/// The DSP task: runs the app while the bat screen is up, sleeps otherwise.
extern "C" fn dsp_task(_: isize) {
    loop {
        if !BAT_ACTIVE.load(Acquire) {
            sleep_ms(20);
            continue;
        }
        if system_state() == stock::STATE_POPUP {
            hide_for_popup();
            continue;
        }
        if !itcm_still_ok() {
            sleep_ms(20);
            continue;
        }
        // SAFETY: built before BAT_ACTIVE is set; only this task uses it.
        let app = unsafe { app() };
        new_session();
        apply_actions(app);
        apply_touches(app);
        let start = BLOCKS.load(Relaxed);
        process_input(app, u32::MAX, PASS_BLOCKS);
        drop_backlog();
        let processed = BLOCKS.load(Relaxed);
        DSP_BLOCKS.store(DSP_BLOCKS.load(Relaxed).wrapping_add(processed.wrapping_sub(start)), Relaxed);
        if processed.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
            LAST_RENDER.store(processed, Relaxed);
            render();
            DRAW_BLOCKS.store(DRAW_BLOCKS.load(Relaxed).wrapping_add(BLOCKS.load(Relaxed).wrapping_sub(processed)), Relaxed);
        }
        let now = BLOCKS.load(Relaxed);
        let second = now.wrapping_sub(LOAD_SECOND.load(Relaxed));
        if second >= blocks_per_second() {
            DSP_PERMILLE.store(DSP_BLOCKS.swap(0, Relaxed) * 1000 / second, Relaxed);
            DRAW_PERMILLE.store(DRAW_BLOCKS.swap(0, Relaxed) * 1000 / second, Relaxed);
            LOAD_SECOND.store(now, Relaxed);
        }
        // Sleep after every pass, even with input waiting: the audio interrupt
        // adds 4 frames every 21 us, so the ring is never empty, and Qt (task
        // 21, priority 17) still runs the hidden app, REC included.
        sleep_ms(SLEEP_MS);
    }
}

/// Build the app (the first time), start feeding it, and show its screen.
extern "C" fn enter_work() {
    if !APP_READY.load(Relaxed) {
        load_itcm();
        // SAFETY: spare SDRAM no stock code uses (see `spare`).
        let te = unsafe { core::slice::from_raw_parts_mut(spare::TE_STORAGE as *mut f32, spare::TE_SAMPLES) };
        // SAFETY: zeroing the buffers in place makes them a valid Buffers.
        let buffers = unsafe {
            ptr::write_bytes(BUFFERS.0.get(), 0, 1);
            (*BUFFERS.0.get()).assume_init_mut()
        };
        // SAFETY: init_in_place writes every field; only task 1 uses APP.
        unsafe { BatApp::init_in_place((*APP.0.get()).as_mut_ptr(), SAMPLE_RATE, te, buffers) };
        // SAFETY: just built; no other context uses it yet.
        let app = unsafe { app() };
        app.set_modes(&Mode::WITHOUT_PITCH);
        app.set_recorder(Recorder::Stopped, 0);
        APP_READY.store(true, Relaxed);
        start_task();
    }
    SESSION.store(SESSION.load(Relaxed).wrapping_add(1), Release);
    SCREEN_APP.store(app_byte() as u32, Relaxed);

    // Take over the display: Qt's present function now writes to a dummy,
    // and the display shows the patch's buffer.
    // SAFETY: the literal and registers named in `stock`; task 1 only.
    unsafe {
        SAVED_BUFFER.store(ptr::read_volatile(stock::LCDIF_CUR_BUF) as u32, Relaxed);
        SAVED_LITERAL.store(ptr::read_volatile(stock::PRESENT_LCDIF_LITERAL) as u32, Relaxed);
        (*DUMMY_LCDIF.0.get()).write([0; 32]);
        ptr::write_volatile(stock::PRESENT_LCDIF_LITERAL, DUMMY_LCDIF.0.get() as usize);
    }
    if task_running() {
        // The DSP task may still be finishing a chunk from the last opening,
        // so it draws the first frame; until then the display is blank.
        for i in 0..stock::WIDTH * stock::HEIGHT / 2 {
            // SAFETY: the display buffer (see `spare`).
            unsafe { ptr::write_volatile((spare::DISPLAY as *mut u32).add(i), 0) };
        }
    } else {
        // SAFETY: no other context uses the app without a DSP task.
        render();
    }
    BAT_ACTIVE.store(true, Release);
    // SAFETY: the display controller's next buffer register.
    unsafe { ptr::write_volatile(stock::LCDIF_NEXT_BUF, spare::DISPLAY) };
}

/// Give the display back to Qt and stop feeding the app.
fn leave_bat_screen() {
    BAT_ACTIVE.store(false, Release);
    // SAFETY: as in enter_work.
    unsafe {
        let literal = SAVED_LITERAL.load(Relaxed) as usize;
        ptr::write_volatile(
            stock::PRESENT_LCDIF_LITERAL,
            if literal == stock::LCDIF_CTRL_SET { literal } else { stock::LCDIF_CTRL_SET },
        );
        // The buffer Qt last presented while the bat screen was up, or the
        // one it showed before.
        let presented = (*DUMMY_LCDIF.0.get()).assume_init_ref()[0x13] as usize;
        let back = if stock::FRAMEBUFFERS.contains(&presented) {
            presented
        } else {
            SAVED_BUFFER.load(Relaxed) as usize
        };
        if stock::FRAMEBUFFERS.contains(&back) {
            ptr::write_volatile(stock::LCDIF_NEXT_BUF, back);
        }
    }
}

/// From the DSP task, when a pop-up opens under the bat screen: pop-ups
/// install their own key handler, so the key hooks cannot see them. Give the
/// display (and touch) back so the pop-up can be read and answered; task 1
/// takes it again once the pop-up closes (`follow_app`). BAT_ACTIVE is cleared
/// first and POPUP_HIDDEN set last, so task 1 cannot reopen the screen halfway.
fn hide_for_popup() {
    leave_bat_screen();
    POPUP_HIDDEN.store(true, Release);
}

/// Without a DSP task: process in task 1 and redraw now and then.
extern "C" fn tick_work() {
    if !BAT_ACTIVE.load(Relaxed) {
        return;
    }
    // SAFETY: built by enter_work; with no DSP task only task 1 uses it.
    let app = unsafe { app() };
    new_session();
    apply_actions(app);
    apply_touches(app);
    process_input(app, MAX_FRAMES_PER_CALL, PASS_BLOCKS);
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
        LAST_RENDER.store(now, Relaxed);
        render();
    }
}

/// Process up to `budget` frames from the input ring, stopping after
/// `time_blocks` audio blocks of wall time, and queue the output. Returns the
/// frames processed. Not inlined, so x8patch.ld can put it in ITCM.
#[inline(never)]
fn process_input(app: &mut BatApp<'static>, budget: u32, time_blocks: u32) -> u32 {
    let start = BLOCKS.load(Relaxed);
    let mut first = [0.0f32; CHUNK];
    let mut second = [0.0f32; CHUNK];
    let mut left = [0.0f32; CHUNK];
    let mut right = [0.0f32; CHUNK];
    let ring = IN_RING.0.get().cast::<[i32; 2]>();
    let out = spare::OUT_RING as *mut f32;
    let mut done = 0;
    while done < budget && BLOCKS.load(Relaxed).wrapping_sub(start) < time_blocks {
        let read = IN_READ.load(Relaxed);
        let available = IN_WRITE.load(Acquire).wrapping_sub(read);
        let n = available.min(CHUNK as u32).min(budget - done) as usize;
        if n == 0 {
            break;
        }
        for i in 0..n {
            let at = read.wrapping_add(i as u32) % spare::IN_FRAMES;
            // SAFETY: inside the input ring.
            let [a, b] = unsafe { ptr::read_volatile(ring.add(at as usize)) };
            first[i] = a as f32 * SCALE;
            second[i] = b as f32 * SCALE;
        }
        IN_READ.store(read.wrapping_add(n as u32), Release);
        app.process(&first[..n], &second[..n], &mut left[..n], &mut right[..n]);
        let write = OUT_WRITE.load(Relaxed);
        let space = spare::OUT_SAMPLES - write.wrapping_sub(OUT_READ.load(Acquire));
        for (i, &y) in left[..n.min(space as usize)].iter().enumerate() {
            let at = write.wrapping_add(i as u32) % spare::OUT_SAMPLES;
            // SAFETY: inside the output ring.
            unsafe { ptr::write_volatile(out.add(at as usize), y) };
        }
        OUT_WRITE.store(write.wrapping_add(n.min(space as usize) as u32), Release);
        done += n as u32;
    }
    done
}

/// Draw the app off-screen, with a line of figures across the top for
/// testing, then copy the frame to the display buffer.
fn render() {
    // SAFETY: the off-screen frame, used only by the context running the app.
    let pixels = unsafe { &mut *(*FRAME.0.get()).as_mut_ptr() };
    let mut fb = Rgb565Buffer::new(pixels, stock::WIDTH, stock::HEIGHT);
    // SAFETY: built by enter_work.
    let app = unsafe { app() };
    update_status(app);
    app.render(&mut fb);
    let task = TASK.load(Relaxed) as i32;
    let percent = |part: u32, all: u32| if all == 0 { 0 } else { (part as u64 * 100 / all as u64) as u32 };
    let all = CYCLES_PER_S.load(Relaxed);
    let mut line = Line::new();
    let _ = write!(
        line,
        "{}{} | key {}/s | app {} st {} rec {} | dsp {}% draw {}%",
        if task > 0 { "task" } else { "no task" },
        if ITCM_OK.load(Relaxed) { "" } else { " ITCM CHANGED" },
        CALLS_PER_S.load(Relaxed),
        app_byte(),
        system_state(),
        read_byte(stock::TRANSPORT),
        DSP_PERMILLE.load(Relaxed) / 10,
        DRAW_PERMILLE.load(Relaxed) / 10,
    );
    if task <= 0 {
        let _ = write!(line, " | cre_tsk {task}");
    }
    draw_strip(&mut fb, 0, &line, Rgb565::new(4, 8, 4));
    let mut line = Line::new();
    let _ = write!(
        line,
        "irq {}% hook {}% | in {} out {} | dry {} lost {} | touch {} {},{}",
        percent(STOCK_CYCLES_PER_S.load(Relaxed), all),
        percent(HOOK_CYCLES_PER_S.load(Relaxed), all),
        IN_WRITE.load(Relaxed).wrapping_sub(IN_READ.load(Relaxed)),
        OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Relaxed)),
        UNDERRUNS.load(Relaxed),
        IN_DROPPED.load(Relaxed).wrapping_add(BACKLOG_DROPPED.load(Relaxed)),
        if LAST_TOUCH.load(Relaxed) >> 31 != 0 { "down" } else { "up" },
        LAST_TOUCH.load(Relaxed) >> 16 & 0x7fff,
        LAST_TOUCH.load(Relaxed) & 0xffff,
    );
    draw_strip(&mut fb, STRIP_ROWS as i32, &line, Rgb565::new(4, 8, 4));
    // Peak level of each input word since the last redraw, in dB below full
    // scale, to check which word carries which input.
    let mut peaks = Line::new();
    let words = HEARD_WORDS.load(Relaxed);
    let _ = write!(peaks, "on {:02x} w{},{} dB", tracks_on(), words & 0xff, words >> 8 & 0xff);
    for peak in &WORD_PEAKS {
        let p = peak.swap(0, Relaxed);
        let db = if p == 0 { 99 } else { (-20.0 * libm::log10f(p as f32 / 2_147_483_648.0)) as u32 };
        let _ = write!(peaks, " {}", db.min(99));
    }
    if PLAYBACK.load(Relaxed) {
        // BAT PLAYBACK: the player's state and rate, and the level the app
        // hears, instead of the input words.
        let p = SOURCE_PEAK.swap(0, Relaxed);
        let db = if p == 0 { 99 } else { (-20.0 * libm::log10f(p as f32 / 2_147_483_648.0)) as u32 };
        // SAFETY: words in the stock audio engine, recorder and DSP blocks.
        let (engine, playing, rate) = unsafe {
            (
                ptr::read_volatile(stock::ENGINE_STATE),
                ptr::read_volatile(stock::DSP_PLAYING),
                ptr::read_volatile(stock::CONVERTER_RATE),
            )
        };
        peaks = Line::new();
        let _ = write!(peaks, "playback | engine {engine} play {playing} | rate {rate} | source -{}dBFS", db.min(99));
    }
    draw_strip(&mut fb, 2 * STRIP_ROWS as i32, &peaks, Rgb565::new(8, 4, 4));
    // One block copy, so the writes to the uncached display buffer go out in
    // bursts.
    // SAFETY: both buffers hold 320 x 480 pixels and do not overlap.
    unsafe { ptr::copy_nonoverlapping(pixels.as_ptr(), spare::DISPLAY as *mut u16, stock::WIDTH * stock::HEIGHT) };
}

/// An input the app can hear: its word in an input frame and its name.
#[derive(Clone, Copy, PartialEq)]
struct Input {
    word: u32,
    name: InputName,
}

/// Names for inputs 1 to 6, EXT IN and the X8's playback.
const INPUT_NAMES: [InputName; 6] = [
    InputName { short: "1", long: "Input 1" },
    InputName { short: "2", long: "Input 2" },
    InputName { short: "3", long: "Input 3" },
    InputName { short: "4", long: "Input 4" },
    InputName { short: "5", long: "Input 5" },
    InputName { short: "6", long: "Input 6" },
];
const EXT_IN_NAMES: [InputName; 2] =
    [InputName { short: "EL", long: "EXT IN left" }, InputName { short: "ER", long: "EXT IN right" }];
const PLAYBACK_LEFT: InputName = InputName { short: "L", long: "Left" };
const PLAYBACK_RIGHT: InputName = InputName { short: "R", long: "Right" };

/// The input a track hears, from the pair chosen for it in INPUT SELECT, or
/// `None` for a source this firmware does not name.
fn track_input(track: usize) -> Option<Input> {
    let side = track % 2;
    // SAFETY: a byte in the settings record, inside the track's record.
    let source = read_byte(unsafe { stock::TRACK_SOURCES.add(track * stock::TRACK_SOURCE_STEP) });
    let (input, name) = match source {
        1 | 2 | 3 => {
            let input = (source as usize - 1) * 2 + side;
            (input, INPUT_NAMES[input])
        }
        4 => (4 + side, EXT_IN_NAMES[side]),
        _ => return None,
    };
    let swapped = input < 2 && read_byte(stock::SWAP_INPUTS_1_2) == 1;
    Some(Input { word: stock::INPUT_WORDS[if swapped { 1 - input } else { input }], name })
}

/// Tracks turned on in the app, as a bit for each of tracks 1 to 6.
fn tracks_on() -> u32 {
    // SAFETY: words in the settings record.
    let [a, b] = stock::TRACKS_ON.map(|at| unsafe { ptr::read_volatile(at) });
    (a | b) & ((1 << stock::TRACKS) - 1)
}

/// The inputs the app hears: the first two different inputs among the tracks
/// turned on in MANUAL, in track order. All six tracks are on by default, so
/// with the default assignment that means inputs 1 and 2. With none on, the X8
/// powers the inputs down.
fn selected_inputs() -> [Option<Input>; 2] {
    let on = tracks_on();
    let mut picked: [Option<Input>; 2] = [None; 2];
    for track in 0..stock::TRACKS {
        let Some(input) = track_input(track).filter(|_| on >> track & 1 != 0) else { continue };
        if picked.contains(&Some(input)) {
            continue;
        }
        if let Some(free) = picked.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(input);
        }
    }
    picked
}

/// The X8 recorder's state and take length, and the record rate, for the
/// status bar and the wrong-rate warning. Reads only; a value may be one
/// update old.
fn update_status(app: &mut BatApp<'static>) {
    let transport = read_byte(stock::TRANSPORT);
    let state = match transport {
        3 => Recorder::Recording,
        4 | 6 => Recorder::Paused,
        1 | 2 | 8 => Recorder::Standby,
        _ => Recorder::Stopped,
    };
    // SAFETY: words in the stock recorder block (see `stock`).
    let seconds = unsafe {
        let frames = if read_byte(stock::TAKE_FROZEN_FLAG) != 0 {
            ptr::read_volatile(stock::TAKE_FROZEN)
        } else {
            ptr::read_volatile(stock::TAKE_OFFSET)
                .wrapping_add(ptr::read_volatile(stock::TAKE_COUNT))
                .wrapping_sub(ptr::read_volatile(stock::TAKE_START))
        };
        let rate = ptr::read_volatile(stock::CONVERTER_RATE);
        if matches!(transport, 3..=6) && rate != 0 { (frames / rate) as u64 } else { 0 }
    };
    app.set_recorder(state, seconds);
    // SAFETY: as above.
    let rate = unsafe {
        if !PLAYBACK.load(Relaxed) {
            ptr::read_volatile(stock::RECORD_RATE)
        } else if ptr::read_volatile(stock::DSP_PLAYING) == 1 {
            // The converters run at the file's rate while a file plays or is
            // paused (the DSP play byte stays 1 then), so BAT PLAYBACK checks
            // that rate.
            ptr::read_volatile(stock::CONVERTER_RATE)
        } else {
            // No file is playing, so there is no rate to warn about.
            SAMPLE_RATE as u32
        }
    };
    app.set_input_rate(rate);
    if PLAYBACK.load(Relaxed) {
        app.set_input_kind(InputKind::Playback);
        app.set_inputs([Some(PLAYBACK_LEFT), Some(PLAYBACK_RIGHT)]);
    } else {
        app.set_input_kind(InputKind::Live);
        let [first, second] = selected_inputs();
        let words = first.map_or(NO_INPUT_WORDS, |first| first.word | second.unwrap_or(first).word << 8);
        HEARD_WORDS.store(words, Relaxed);
        app.set_inputs([first, second].map(|input| input.map(|input| input.name)));
    }
}

/// The audio DMA callback: runs the stock one, then, with the bat screen up at
/// 192 kHz, passes the inputs chosen by `selected_inputs` to the app and plays
/// the app's output.
unsafe extern "C" fn audio_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    if !CYCLE_COUNTER_ON.load(Relaxed) {
        // SAFETY: turning on the Cortex-M7 cycle counter (trace enable, then
        // the DWT's lock and counter enable); this interrupt is privileged.
        unsafe {
            ptr::write_volatile(DEMCR, ptr::read_volatile(DEMCR) | 1 << 24);
            ptr::write_volatile(DWT_LAR, 0xc5ac_ce55);
            ptr::write_volatile(DWT_CTRL, ptr::read_volatile(DWT_CTRL) | 1);
        }
        CYCLE_COUNTER_ON.store(true, Relaxed);
    }
    let t0 = cycles();
    // SAFETY: read before the stock callback moves the ring on.
    let index = unsafe { ptr::read_volatile(stock::AUDIO_INDEX) } as usize % 3;
    // The stock routine scales frame 0's output by the gain before it
    // updates the gain (once per block), and frames 1 to 3 by the new gain.
    // SAFETY: the stock output gain, an f32 in ITCM.
    let frame0_gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) };
    // SAFETY: the stock callback, with its own arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::AUDIO_CALLBACK) };
    let result = unsafe { stock_callback(a, b, c, d) };
    let t1 = cycles();
    STOCK_CYCLES.store(STOCK_CYCLES.load(Relaxed).wrapping_add(t1.wrapping_sub(t0)), Relaxed);
    let blocks = BLOCKS.load(Relaxed).wrapping_add(1);
    BLOCKS.store(blocks, Relaxed);
    if blocks.wrapping_sub(BLOCKS_AT_SECOND.load(Relaxed)) >= blocks_per_second() {
        BLOCKS_AT_SECOND.store(blocks, Relaxed);
        CYCLES_PER_S.store(t1.wrapping_sub(CYCLES_AT_SECOND.load(Relaxed)), Relaxed);
        CYCLES_AT_SECOND.store(t1, Relaxed);
        STOCK_CYCLES_PER_S.store(STOCK_CYCLES.swap(0, Relaxed), Relaxed);
        HOOK_CYCLES_PER_S.store(HOOK_CYCLES.swap(0, Relaxed), Relaxed);
    }
    if !BAT_ACTIVE.load(Acquire) || rate_class() != 2 {
        return result;
    }
    let input = (stock::INPUT_BLOCKS + index * 0x180) as *const i32;
    let output = (stock::OUTPUT_BLOCKS + (index + 2) % 3 * 0x40) as *mut i32;

    for (w, peak) in WORD_PEAKS.iter().enumerate() {
        let mut p = peak.load(Relaxed);
        for k in 0..4 {
            // SAFETY: a word of this frame in the input block.
            p = p.max(unsafe { ptr::read_volatile(input.add(12 * k + w)) }.unsigned_abs());
        }
        peak.store(p, Relaxed);
    }

    // SAFETY: the stock output gain, an f32 in ITCM.
    let stock_gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) };
    let playback = PLAYBACK.load(Relaxed);
    // The block's 4 frames into the input ring: the two inputs chosen by
    // `selected_inputs`, or for BAT PLAYBACK the stock output (left, right),
    // which the stock callback has just written as clamp(mix) x (2^31 - 1) x
    // the output gain. Until a file plays, MANUAL's output is the live input
    // mix, so BAT PLAYBACK hears silence then.
    // SAFETY: a byte in the stock DSP state.
    let file_playing = playback && unsafe { ptr::read_volatile(stock::DSP_PLAYING) } == 1;
    let words = HEARD_WORDS.load(Relaxed);
    let (first_word, second_word) = ((words & 0xff) as usize, (words >> 8 & 0xff) as usize);
    let no_input = words == NO_INPUT_WORDS;
    let write = IN_WRITE.load(Relaxed);
    if spare::IN_FRAMES - write.wrapping_sub(IN_READ.load(Acquire)) >= 4 {
        let ring = IN_RING.0.get().cast::<[i32; 2]>();
        for k in 0..4 {
            let gain = if k == 0 { frame0_gain } else { stock_gain };
            // SAFETY: words of this frame in the input and output blocks, and
            // the ring.
            unsafe {
                let pair = if !playback && !no_input {
                    [ptr::read_volatile(input.add(12 * k + first_word)), ptr::read_volatile(input.add(12 * k + second_word))]
                } else if file_playing && gain != 0.0 {
                    let unscale = |word: i32| (word as f32 / gain) as i32;
                    [unscale(ptr::read_volatile(output.add(2 * k))), unscale(ptr::read_volatile(output.add(2 * k + 1)))]
                } else {
                    [0, 0]
                };
                if playback {
                    SOURCE_PEAK.store(SOURCE_PEAK.load(Relaxed).max(pair[0].unsigned_abs()), Relaxed);
                }
                ptr::write_volatile(ring.add((write.wrapping_add(k as u32) % spare::IN_FRAMES) as usize), pair);
            }
        }
        IN_WRITE.store(write.wrapping_add(4), Release);
    } else {
        IN_DROPPED.store(IN_DROPPED.load(Relaxed).wrapping_add(4), Relaxed);
    }

    // The app's output, once enough is buffered, at the stock headphone gain.
    let read = OUT_READ.load(Relaxed);
    let available = OUT_WRITE.load(Acquire).wrapping_sub(read);
    let mut primed = PRIMED.load(Relaxed);
    if !primed && available >= PRIME_SAMPLES {
        primed = true;
    } else if primed && available < 4 {
        primed = false;
        UNDERRUNS.store(UNDERRUNS.load(Relaxed).wrapping_add(1), Relaxed);
    }
    PRIMED.store(primed, Relaxed);
    let gain = stock_gain * 2_147_483_647.0;
    let ring = spare::OUT_RING as *const f32;
    for k in 0..4 {
        let y = if primed {
            // SAFETY: inside the output ring.
            unsafe { ptr::read_volatile(ring.add((read.wrapping_add(k as u32) % spare::OUT_SAMPLES) as usize)) }
        } else {
            0.0
        };
        let sample = ((y * OUTPUT_GAIN).clamp(-OUTPUT_CEILING, OUTPUT_CEILING) * gain) as i32;
        // SAFETY: the words of this frame in the output block.
        unsafe {
            ptr::write_volatile(output.add(2 * k), sample);
            ptr::write_volatile(output.add(2 * k + 1), sample);
        }
    }
    if primed {
        OUT_READ.store(read.wrapping_add(4), Release);
    }
    HOOK_CYCLES.store(HOOK_CYCLES.load(Relaxed).wrapping_add(cycles().wrapping_sub(t1)), Relaxed);
    result
}

/// Text in a fixed buffer, for formatting without an allocator.
struct Line {
    buf: [u8; 96],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self { buf: [0; 96], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

const STRIP_ROWS: usize = 13;
const FONT: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();

fn draw_strip(target: &mut Rgb565Buffer<'_>, top: i32, line: &Line, background: Rgb565) {
    let _ = target.fill_solid(&Rectangle::new(Point::new(0, top), Size::new(stock::WIDTH as u32, STRIP_ROWS as u32)), background);
    let _ =
        FONT.render(line.as_str(), Point::new(2, top + 10), VerticalPosition::Baseline, FontColor::Transparent(Rgb565::WHITE), target);
}

/// While the bat screen is closed: a strip across the top of Qt's framebuffers.
fn draw_status_strip() {
    let mut line = Line::new();
    let audio = match AUDIO_STATE.load(Relaxed) {
        AUDIO_INSTALLED => "audio hooked",
        AUDIO_SLOT_UNEXPECTED => "audio NOT hooked",
        _ => "waiting for audio",
    };
    let event = LAST_EVENT.load(Relaxed);
    let _ = write!(
        line,
        "{audio} | rate {} | key {}/s | app {} st {} rec {} | ev {} {:x}",
        rate_class(),
        CALLS_PER_S.load(Relaxed),
        app_byte(),
        system_state(),
        read_byte(stock::TRANSPORT),
        event >> 16,
        event & 0xffff
    );
    for &fb in &stock::FRAMEBUFFERS {
        // SAFETY: the top rows of a framebuffer the stock firmware allocated.
        let pixels = unsafe { core::slice::from_raw_parts_mut(fb as *mut u16, stock::WIDTH * STRIP_ROWS) };
        draw_strip(&mut Rgb565Buffer::new(pixels, stock::WIDTH, STRIP_ROWS), 0, &line, Rgb565::new(4, 8, 4));
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
