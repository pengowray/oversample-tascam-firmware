use core::cell::UnsafeCell;
use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::fb::Rgb565Buffer;
use bat_app::{BatApp, Button, Mode};
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
    /// Touch polling runs only while this word is non-zero (it is 1 once the
    /// touch controller is set up).
    pub const TOUCH_ENABLED: *mut u32 = 0x8198_edbc as *mut u32;

    /// Where `EDMA_SetCallback` stored the audio callback: the channel 9
    /// (SAI1 receive) eDMA handle's first word.
    pub const AUDIO_CALLBACK_SLOT: *mut usize = 0x2001_024c as *mut usize;
    /// The stock audio callback (ITCM, Thumb address).
    pub const AUDIO_CALLBACK: usize = 0x0002_adfd;
    /// Ring index of the input block the callback is about to process.
    pub const AUDIO_INDEX: *const u32 = 0x2001_0268 as *const u32;
    /// Three input blocks of 0x180 bytes: per frame 12 signed 32-bit words;
    /// input 3 is word 0 and input 4 word 6.
    pub const INPUT_BLOCKS: usize = 0x2000_e340;
    /// Three output blocks of 0x40 bytes: per frame left, right.
    pub const OUTPUT_BLOCKS: usize = 0x2001_04c0;
    /// Rate class: 0 for 44.1/48 kHz, 1 for 88.2/96, 2 for 176.4/192.
    pub const RATE_CLASS: *const u8 = 0x0002_a2cb as *const u8;
    /// Current headphone gain (f32): -10^(-volume/20), or 0 when muted.
    pub const OUTPUT_GAIN: *const f32 = 0x0002_9ed8 as *const f32;

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
    pub const REC: i32 = 0x07;
    pub const MARK: i32 = 0x2b;
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

/// What `tools/x8patch.py` reads to find the entry points: it must stay at the
/// start of the patch.
#[repr(C)]
pub struct Header {
    magic: [u8; 4],
    version: u32,
    /// Replaces the stock key handler of the launcher and app screens.
    key_hook: Handler,
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".header")]
pub static X8_PATCH_HEADER: Header = Header { magic: *b"BATX", version: 5, key_hook };

const SAMPLE_RATE: f32 = 192_000.0;
/// Output samples buffered before the headphones start playing the bat app's
/// output (43 ms at 192 kHz), so the DSP task's passes do not run dry.
const PRIME_SAMPLES: u32 = 8_192;
/// Frames processed per call at most when processing in task 1 (only if the
/// DSP task could not be created), so one call cannot hold task 1 for long.
const MAX_FRAMES_PER_CALL: u32 = 24_576;
const CHUNK: usize = 1_024;
const RENDERS_PER_SECOND: u32 = 10;
/// The DSP task stops a pass after this much wall time (8 ms) even with input
/// waiting, then draws and sleeps, so a mode that cannot keep up drops input
/// instead of starving Qt (which also handles powering off).
const PASS_BLOCKS: u32 = 384;
/// Gain on the app's output before the stock headphone gain (+18 dB), with a
/// ceiling of -6 dBFS: bat calls are often 40 to 60 dB below full scale.
const OUTPUT_GAIN: f32 = 8.0;
const OUTPUT_CEILING: f32 = 0.5;
/// MARK presses with the bat screen closed are held this long (0.5 s), in case
/// two more follow, before they are passed on to the stock app.
const MARK_HOLD_DIVISOR: u32 = 2;

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
static MARK_TIMES: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
/// MARK presses held back while the bat screen is closed, and when the last
/// one came (audio block count).
static MARKS_HELD: AtomicU32 = AtomicU32::new(0);
static MARK_HELD_AT: AtomicU32 = AtomicU32::new(0);
static AUDIO_STATE: AtomicU32 = AtomicU32::new(AUDIO_NOT_TRIED);
const AUDIO_NOT_TRIED: u32 = 0;
const AUDIO_INSTALLED: u32 = 1;
const AUDIO_SLOT_UNEXPECTED: u32 = 2;
static APP_READY: AtomicBool = AtomicBool::new(false);
static LAST_RENDER: AtomicU32 = AtomicU32::new(0);
static SAVED_LITERAL: AtomicU32 = AtomicU32::new(0);
static SAVED_BUFFER: AtomicU32 = AtomicU32::new(0);
static SAVED_TOUCH: AtomicU32 = AtomicU32::new(0);
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
const ACTION_REC: u32 = 2;
const ACTION_NEXT_MODE: u32 = 3;
const ACTION_WHEEL: u32 = 4;
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
/// Peak magnitude of each of the 12 words of an input frame since the last
/// redraw: the interrupt raises them, the drawing context reads and clears.
static WORD_PEAKS: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
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
/// DSP task (or, without one, by task 1).
#[unsafe(link_section = ".noinit")]
static APP: Slot<BatApp<'static>> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
/// The off-screen frame: cached, so drawing is fast, then copied to the
/// display buffer in one pass.
#[unsafe(link_section = ".noinit")]
static FRAME: Slot<[u16; stock::WIDTH * stock::HEIGHT]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
/// The input ring: (input 3, input 4) frames from the interrupt.
#[unsafe(link_section = ".noinit")]
static IN_RING: Slot<[[i32; 2]; spare::IN_FRAMES as usize]> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
const DSP_STACK_BYTES: usize = 48 * 1024;
#[repr(C, align(8))]
struct DspStack([u8; DSP_STACK_BYTES]);
#[unsafe(link_section = ".noinit")]
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

/// Called by task 1 for every message while the launcher or an app is on
/// screen, with the stock handler's arguments; the first is the message.
unsafe extern "C" fn key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        if AUDIO_STATE.load(Relaxed) == AUDIO_NOT_TRIED {
            install_audio_hook();
        }
        count_call();
        replay_held_marks();
        // SAFETY: the message copy on task 1's stack, 0x14 bytes.
        unsafe { handle_message(msg) };
        let work = PENDING.swap(0, Relaxed);
        if work == WORK_ENTER {
            // SAFETY: only task 1 runs on the patch stack.
            unsafe { on_patch_stack(enter_work) };
        } else if BAT_ACTIVE.load(Relaxed) {
            if !task_running() {
                // SAFETY: as above.
                unsafe { on_patch_stack(tick_work) };
            }
        } else {
            draw_status_strip();
        }
    }
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
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

/// Pass held MARK presses on to the stock app once no more have come for
/// 0.5 s, by calling the stock key handler with a MARK message, as task 1
/// would have.
fn replay_held_marks() {
    let held = MARKS_HELD.load(Relaxed);
    if held == 0 || BLOCKS.load(Relaxed).wrapping_sub(MARK_HELD_AT.load(Relaxed)) < blocks_per_second() / MARK_HOLD_DIVISOR {
        return;
    }
    MARKS_HELD.store(0, Relaxed);
    if BAT_ACTIVE.load(Relaxed) {
        return;
    }
    // SAFETY: the stock handler, from task 1, with a message laid out as task
    // 1's copies are (0x14 bytes: category 0, kind 0 at +4, code at +8).
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::KEY_HANDLER) };
    for _ in 0..held {
        let mut message = [0u32; 5];
        message[2] = stock::MARK as u32;
        unsafe { stock_handler(message.as_mut_ptr() as usize, stock::KEY_HANDLER, 0, 0) };
    }
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

/// Three MARK presses within 1.5 s.
fn mark_triple() -> bool {
    let now = BLOCKS.load(Relaxed);
    let first = MARK_TIMES[0].load(Relaxed);
    MARK_TIMES[0].store(MARK_TIMES[1].load(Relaxed), Relaxed);
    MARK_TIMES[1].store(now, Relaxed);
    if first != 0 && now.wrapping_sub(first) < blocks_per_second() * 3 / 2 {
        MARK_TIMES[0].store(0, Relaxed);
        MARK_TIMES[1].store(0, Relaxed);
        return true;
    }
    false
}

unsafe fn handle_message(msg: usize) {
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
        (stock::KIND_BUTTON, stock::MARK) => {
            // Held back from the stock app: in MANUAL without a file, MARK opens
            // a pop-up that takes the next presses (with the bat screen up, a
            // hidden one, with touch off). With the screen closed, presses that
            // do not make a triple are passed on after 0.5 s.
            swallow_button();
            let triple = mark_triple();
            if active {
                if triple { leave_bat_screen() } else { queue_action(ACTION_MARK, 0) }
            } else if triple {
                MARKS_HELD.store(0, Relaxed);
                PENDING.store(WORK_ENTER, Relaxed);
            } else {
                MARKS_HELD.store(MARKS_HELD.load(Relaxed) + 1, Relaxed);
                MARK_HELD_AT.store(BLOCKS.load(Relaxed), Relaxed);
            }
        }
        (stock::KIND_BUTTON, stock::HOME) if active => {
            swallow_button();
            leave_bat_screen();
        }
        (stock::KIND_BUTTON, stock::PLAY) if active => {
            swallow_button();
            queue_action(ACTION_NEXT_MODE, 0);
        }
        (stock::KIND_BUTTON, stock::REC) if active => {
            // REC still reaches the app, which records as usual; the bat
            // screen's recording indicator follows the presses.
            queue_action(ACTION_REC, 0);
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
            ACTION_REC => app.button(Button::Rec),
            ACTION_NEXT_MODE => {
                let i = Mode::ALL.iter().position(|&m| m == app.mode()).unwrap_or(0);
                app.set_mode(Mode::ALL[(i + 1) % Mode::ALL.len()]);
            }
            ACTION_WHEEL => app.wheel(value),
            _ => {}
        }
        read = read.wrapping_add(1);
        ACTIONS_READ.store(read, Release);
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
        // SAFETY: built before BAT_ACTIVE is set; only this task uses it.
        let app = unsafe { app() };
        new_session();
        apply_actions(app);
        let start = BLOCKS.load(Relaxed);
        process_input(app, u32::MAX, PASS_BLOCKS);
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
        // 21, priority 17) still runs the hidden app, REC included. 4 ms is 2
        // or 3 kernel ticks, about 1,000 frames a pass.
        sleep_ms(4);
    }
}

/// Build the app (the first time), start feeding it, and show its screen.
extern "C" fn enter_work() {
    if !APP_READY.load(Relaxed) {
        // SAFETY: spare SDRAM no stock code uses (see `spare`).
        let te = unsafe { core::slice::from_raw_parts_mut(spare::TE_STORAGE as *mut f32, spare::TE_SAMPLES) };
        // SAFETY: init_in_place writes every field; only task 1 uses APP.
        unsafe { BatApp::init_in_place((*APP.0.get()).as_mut_ptr(), SAMPLE_RATE, te) };
        APP_READY.store(true, Relaxed);
        start_task();
    }
    SESSION.store(SESSION.load(Relaxed).wrapping_add(1), Release);

    // Take over the display: Qt's present function now writes to a dummy,
    // and the display shows the patch's buffer.
    // SAFETY: the literal and registers named in `stock`; task 1 only.
    unsafe {
        SAVED_BUFFER.store(ptr::read_volatile(stock::LCDIF_CUR_BUF) as u32, Relaxed);
        SAVED_LITERAL.store(ptr::read_volatile(stock::PRESENT_LCDIF_LITERAL) as u32, Relaxed);
        (*DUMMY_LCDIF.0.get()).write([0; 32]);
        ptr::write_volatile(stock::PRESENT_LCDIF_LITERAL, DUMMY_LCDIF.0.get() as usize);
        SAVED_TOUCH.store(ptr::read_volatile(stock::TOUCH_ENABLED), Relaxed);
        ptr::write_volatile(stock::TOUCH_ENABLED, 0);
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
        ptr::write_volatile(stock::TOUCH_ENABLED, SAVED_TOUCH.load(Relaxed));
    }
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
    process_input(app, MAX_FRAMES_PER_CALL, PASS_BLOCKS);
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
        LAST_RENDER.store(now, Relaxed);
        render();
    }
}

/// Process up to `budget` frames from the input ring, stopping after
/// `time_blocks` audio blocks of wall time, and queue the output. Returns the
/// frames processed.
fn process_input(app: &mut BatApp<'static>, budget: u32, time_blocks: u32) -> u32 {
    let start = BLOCKS.load(Relaxed);
    let mut in3 = [0.0f32; CHUNK];
    let mut in4 = [0.0f32; CHUNK];
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
            in3[i] = a as f32 * SCALE;
            in4[i] = b as f32 * SCALE;
        }
        IN_READ.store(read.wrapping_add(n as u32), Release);
        app.process(&in3[..n], &in4[..n], &mut left[..n], &mut right[..n]);
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
    unsafe { app() }.render(&mut fb);
    let task = TASK.load(Relaxed) as i32;
    let percent = |part: u32, all: u32| if all == 0 { 0 } else { (part as u64 * 100 / all as u64) as u32 };
    let all = CYCLES_PER_S.load(Relaxed);
    let mut line = Line::new();
    let _ = write!(
        line,
        "{} | key {}/s | dsp {}% draw {}% | core {} MHz",
        if task > 0 { "task" } else { "no task" },
        CALLS_PER_S.load(Relaxed),
        DSP_PERMILLE.load(Relaxed) / 10,
        DRAW_PERMILLE.load(Relaxed) / 10,
        all / 1_000_000,
    );
    if task <= 0 {
        let _ = write!(line, " | cre_tsk {task}");
    }
    draw_strip(&mut fb, 0, &line, Rgb565::new(4, 8, 4));
    let mut line = Line::new();
    let _ = write!(
        line,
        "audio irq: stock {}% hook {}% | in {} out {} | dry {} lost {}",
        percent(STOCK_CYCLES_PER_S.load(Relaxed), all),
        percent(HOOK_CYCLES_PER_S.load(Relaxed), all),
        IN_WRITE.load(Relaxed).wrapping_sub(IN_READ.load(Relaxed)),
        OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Relaxed)),
        UNDERRUNS.load(Relaxed),
        IN_DROPPED.load(Relaxed),
    );
    draw_strip(&mut fb, STRIP_ROWS as i32, &line, Rgb565::new(4, 8, 4));
    // Peak level of each input word since the last redraw, in dB below full
    // scale, to check which word carries which input.
    let mut peaks = Line::new();
    let _ = write!(peaks, "word -dBFS");
    for (i, peak) in WORD_PEAKS.iter().enumerate() {
        let p = peak.swap(0, Relaxed);
        let db = if p == 0 { 99 } else { (-20.0 * libm::log10f(p as f32 / 2_147_483_648.0)) as u32 };
        let _ = write!(peaks, " {i}:{}", db.min(99));
    }
    draw_strip(&mut fb, 2 * STRIP_ROWS as i32, &peaks, Rgb565::new(8, 4, 4));
    // One block copy, so the writes to the uncached display buffer go out in
    // bursts.
    // SAFETY: both buffers hold 320 x 480 pixels and do not overlap.
    unsafe { ptr::copy_nonoverlapping(pixels.as_ptr(), spare::DISPLAY as *mut u16, stock::WIDTH * stock::HEIGHT) };
}

/// The audio DMA callback: runs the stock one, then, with the bat screen up at
/// 192 kHz, passes inputs 3 and 4 to the app and plays the app's output.
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

    // Inputs 3 and 4 of the block's 4 frames into the input ring.
    let write = IN_WRITE.load(Relaxed);
    if spare::IN_FRAMES - write.wrapping_sub(IN_READ.load(Acquire)) >= 4 {
        let ring = IN_RING.0.get().cast::<[i32; 2]>();
        for k in 0..4 {
            // SAFETY: words of this frame in the input block, and the ring.
            unsafe {
                let pair = [ptr::read_volatile(input.add(12 * k)), ptr::read_volatile(input.add(12 * k + 6))];
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
    // SAFETY: the stock headphone gain, an f32 in ITCM.
    let gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) } * 2_147_483_647.0;
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
        AUDIO_INSTALLED => "MARK x3: bat screen",
        AUDIO_SLOT_UNEXPECTED => "audio NOT hooked",
        _ => "waiting for audio",
    };
    let event = LAST_EVENT.load(Relaxed);
    let _ = write!(
        line,
        "{audio} | rate {} | key {}/s | ev {} {:x}",
        rate_class(),
        CALLS_PER_S.load(Relaxed),
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
