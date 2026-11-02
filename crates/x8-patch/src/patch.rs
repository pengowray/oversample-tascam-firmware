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
    pub const IN_RING: usize = 0x87f6_0000; // (i32, i32) x IN_FRAMES
    pub const IN_FRAMES: u32 = 65_536;
    pub const OUT_RING: usize = 0x87fe_0000; // f32 x OUT_SAMPLES, to 0x88000000
    pub const OUT_SAMPLES: u32 = 32_768;
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
pub static X8_PATCH_HEADER: Header = Header { magic: *b"BATX", version: 3, key_hook };

const SAMPLE_RATE: f32 = 192_000.0;
/// Output samples buffered before the headphones start playing the bat app's
/// output (85 ms at 192 kHz), so the bursts of processing in task 1 do not run
/// dry.
const PRIME_SAMPLES: u32 = 16_384;
/// Frames processed per call at most, so one call cannot hold task 1 for long.
const MAX_FRAMES_PER_CALL: u32 = 24_576;
const CHUNK: usize = 1_024;
const RENDERS_PER_SECOND: u32 = 20;
const SCALE: f32 = 1.0 / 2_147_483_648.0;

// Task 1 only.
static CALLS: AtomicU32 = AtomicU32::new(0);
static CALLS_PER_S: AtomicU32 = AtomicU32::new(0);
static LAST_SECOND: AtomicU32 = AtomicU32::new(0);
static CALLS_AT_SECOND: AtomicU32 = AtomicU32::new(0);
static LAST_EVENT: AtomicU32 = AtomicU32::new(0);
static MARK_TIMES: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
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

/// Statics written by one context at a time, as the comments on each use say.
struct Slot<T>(UnsafeCell<MaybeUninit<T>>);
// SAFETY: see each use.
unsafe impl<T> Sync for Slot<T> {}

/// The bat app, built and used only on the patch stack in task 1.
static APP: Slot<BatApp<'static>> = Slot(UnsafeCell::new(MaybeUninit::uninit()));
/// Where Qt's present function writes while the bat screen is up (it writes
/// at offsets 0x0, 0x10 and 0x4c of what it takes for the LCDIF registers).
static DUMMY_LCDIF: Slot<[u32; 32]> = Slot(UnsafeCell::new(MaybeUninit::zeroed()));

/// The stack the patch's heavy work runs on (task 1's own is 6 KB).
const STACK_WORDS: usize = 16 * 1024;
#[repr(C, align(8))]
struct Stack([u32; STACK_WORDS]);
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
        // SAFETY: the message copy on task 1's stack, 0x14 bytes.
        unsafe { handle_message(msg) };
        let work = PENDING.swap(0, Relaxed);
        if work != 0 || BAT_ACTIVE.load(Relaxed) {
            // SAFETY: only task 1 runs on the patch stack.
            unsafe { on_patch_stack(if work == WORK_ENTER { enter_work } else { tick_work }) };
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
            // MARK still reaches the app (a mark while recording).
            if mark_triple() {
                if active { leave_bat_screen() } else { PENDING.store(WORK_ENTER, Relaxed) }
            } else if active {
                // SAFETY: APP is built before the screen opens.
                unsafe { app().button(Button::Mark) };
            }
        }
        (stock::KIND_BUTTON, stock::HOME) if active => {
            swallow_button();
            leave_bat_screen();
        }
        (stock::KIND_BUTTON, stock::PLAY) if active => {
            swallow_button();
            // SAFETY: as above.
            let app = unsafe { app() };
            let i = Mode::ALL.iter().position(|&m| m == app.mode()).unwrap_or(0);
            app.set_mode(Mode::ALL[(i + 1) % Mode::ALL.len()]);
        }
        (stock::KIND_BUTTON, stock::REC) if active => {
            // REC still reaches the app, which records as usual; the bat
            // screen's recording indicator follows the presses.
            // SAFETY: as above.
            unsafe { app().button(Button::Rec) };
        }
        (stock::KIND_WHEEL, count) if active => {
            // SAFETY: the kind field of the message copy.
            unsafe { ptr::write_volatile(kind_ptr, stock::KIND_SWALLOWED) };
            // SAFETY: as above.
            unsafe { app().wheel(count) };
        }
        _ => {}
    }
}

/// The bat app. Callers must be in task 1, after `enter_work` has built it.
unsafe fn app() -> &'static mut BatApp<'static> {
    // SAFETY: built by enter_work; used only from task 1.
    unsafe { (*APP.0.get()).assume_init_mut() }
}

/// Build the app (the first time), start feeding it, and show its screen.
extern "C" fn enter_work() {
    if !APP_READY.load(Relaxed) {
        // SAFETY: spare SDRAM no stock code uses (see `spare`).
        let te = unsafe { core::slice::from_raw_parts_mut(spare::TE_STORAGE as *mut f32, spare::TE_SAMPLES) };
        // SAFETY: init_in_place writes every field; only task 1 uses APP.
        unsafe { BatApp::init_in_place((*APP.0.get()).as_mut_ptr(), SAMPLE_RATE, te) };
        APP_READY.store(true, Relaxed);
    }
    // Empty both rings, then let the interrupt fill the input one.
    IN_READ.store(IN_WRITE.load(Acquire), Release);
    OUT_WRITE.store(OUT_READ.load(Acquire), Release);
    BAT_ACTIVE.store(true, Release);

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
    render();
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

/// Process what the interrupt has gathered, and redraw now and then.
extern "C" fn tick_work() {
    if !BAT_ACTIVE.load(Relaxed) {
        return;
    }
    // SAFETY: built by enter_work.
    let app = unsafe { app() };
    let mut in3 = [0.0f32; CHUNK];
    let mut in4 = [0.0f32; CHUNK];
    let mut left = [0.0f32; CHUNK];
    let mut right = [0.0f32; CHUNK];
    let ring = spare::IN_RING as *const [i32; 2];
    let out = spare::OUT_RING as *mut f32;
    let mut budget = MAX_FRAMES_PER_CALL;
    while budget > 0 {
        let read = IN_READ.load(Relaxed);
        let available = IN_WRITE.load(Acquire).wrapping_sub(read);
        let n = available.min(CHUNK as u32).min(budget) as usize;
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
        budget -= n as u32;
    }
    let now = BLOCKS.load(Relaxed);
    if now.wrapping_sub(LAST_RENDER.load(Relaxed)) >= blocks_per_second() / RENDERS_PER_SECOND {
        LAST_RENDER.store(now, Relaxed);
        render();
    }
}

/// Draw the app into the patch's display buffer, with a line of figures
/// across the top for testing.
fn render() {
    // SAFETY: spare SDRAM (see `spare`), task 1 only.
    let pixels = unsafe { core::slice::from_raw_parts_mut(spare::DISPLAY as *mut u16, stock::WIDTH * stock::HEIGHT) };
    let mut fb = Rgb565Buffer::new(pixels, stock::WIDTH, stock::HEIGHT);
    // SAFETY: built by enter_work.
    unsafe { app() }.render(&mut fb);
    let mut line = Line::new();
    let _ = write!(
        line,
        "rate {} | key {}/s | in {} | out {} | dry {} | lost {}",
        rate_class(),
        CALLS_PER_S.load(Relaxed),
        IN_WRITE.load(Relaxed).wrapping_sub(IN_READ.load(Relaxed)),
        OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Relaxed)),
        UNDERRUNS.load(Relaxed),
        IN_DROPPED.load(Relaxed),
    );
    draw_strip(&mut fb, &line, Rgb565::new(4, 8, 4));
}

/// The audio DMA callback: runs the stock one, then, with the bat screen up at
/// 192 kHz, passes inputs 3 and 4 to the app and plays the app's output.
unsafe extern "C" fn audio_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: read before the stock callback moves the ring on.
    let index = unsafe { ptr::read_volatile(stock::AUDIO_INDEX) } as usize % 3;
    // SAFETY: the stock callback, with its own arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::AUDIO_CALLBACK) };
    let result = unsafe { stock_callback(a, b, c, d) };
    BLOCKS.store(BLOCKS.load(Relaxed).wrapping_add(1), Relaxed);
    if !BAT_ACTIVE.load(Acquire) || rate_class() != 2 {
        return result;
    }
    let input = (stock::INPUT_BLOCKS + index * 0x180) as *const i32;
    let output = (stock::OUTPUT_BLOCKS + (index + 2) % 3 * 0x40) as *mut i32;

    // Inputs 3 and 4 of the block's 4 frames into the input ring.
    let write = IN_WRITE.load(Relaxed);
    if spare::IN_FRAMES - write.wrapping_sub(IN_READ.load(Acquire)) >= 4 {
        let ring = spare::IN_RING as *mut [i32; 2];
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
        let sample = (y.clamp(-1.0, 1.0) * gain) as i32;
        // SAFETY: the words of this frame in the output block.
        unsafe {
            ptr::write_volatile(output.add(2 * k), sample);
            ptr::write_volatile(output.add(2 * k + 1), sample);
        }
    }
    if primed {
        OUT_READ.store(read.wrapping_add(4), Release);
    }
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

fn draw_strip(target: &mut Rgb565Buffer<'_>, line: &Line, background: Rgb565) {
    let _ = target.fill_solid(&Rectangle::new(Point::zero(), Size::new(stock::WIDTH as u32, STRIP_ROWS as u32)), background);
    let _ = FONT.render(line.as_str(), Point::new(2, 10), VerticalPosition::Baseline, FontColor::Transparent(Rgb565::WHITE), target);
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
        draw_strip(&mut Rgb565Buffer::new(pixels, stock::WIDTH, STRIP_ROWS), &line, Rgb565::new(4, 8, 4));
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
