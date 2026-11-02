use core::cell::UnsafeCell;
use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use bat_app::fb::Rgb565Buffer;
use bat_dsp::heterodyne::Heterodyne;
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
    /// The two framebuffers: 320 x 480, RGB565, mapped uncached by the MPU.
    pub const FRAMEBUFFERS: [usize; 2] = [0x87e0_0000, 0x87e4_b000];
    pub const WIDTH: usize = 320;

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
pub static X8_PATCH_HEADER: Header = Header { magic: *b"BATX", version: 2, key_hook };

const SAMPLE_RATE: f32 = 192_000.0;
const MARK: i32 = 0x2b;
const LO_DEFAULT_HZ: f32 = 45_000.0;
const LO_STEP_HZ: f32 = 1_000.0;
const LO_RANGE_HZ: (f32, f32) = (15_000.0, 95_000.0);
/// Heterodyne output gain before the stock volume, and its ceiling (-6 dBFS).
const HET_GAIN: f32 = 8.0;
const HET_CEILING: f32 = 0.5;

// Task 1 only (key_hook).
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

// Shared with the audio interrupt: plain loads and stores, one writer each.
static HET_ON: AtomicBool = AtomicBool::new(false);
static LO_HZ: AtomicU32 = AtomicU32::new(0);
static BLOCKS: AtomicU32 = AtomicU32::new(0);

/// The heterodyne, used only by the audio interrupt once installed.
struct IrqState {
    het: Heterodyne,
    lo_applied: f32,
}
struct Shared(UnsafeCell<MaybeUninit<IrqState>>);
// SAFETY: written once by task 1 before the interrupt can see it, then only
// by the audio interrupt.
unsafe impl Sync for Shared {}
static IRQ: Shared = Shared(UnsafeCell::new(MaybeUninit::uninit()));

fn system_mode() -> u8 {
    // SAFETY: a byte the stock firmware sets before the RTOS starts.
    unsafe { ptr::read_volatile(stock::SYSTEM_MODE) }
}

/// Called by task 1 with the stock handler's arguments; the first is a copy of
/// the message: +0 category (0 = input), +4 kind (0 button, 2 wheel, 9
/// swallowed), +8 button code or wheel count.
unsafe extern "C" fn key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        if AUDIO_STATE.load(Relaxed) == AUDIO_NOT_TRIED {
            install_audio_hook();
        }
        count_call();
        // SAFETY: the message copy on task 1's stack, 0x14 bytes.
        unsafe { handle_message(msg) };
        draw_status();
    }
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
}

fn install_audio_hook() {
    // SAFETY: the eDMA handle's callback word, set once by the stock setup.
    let current = unsafe { ptr::read_volatile(stock::AUDIO_CALLBACK_SLOT) };
    if current == 0 {
        // The audio is not set up yet (the launcher can come up first): try
        // again on the next call.
        return;
    }
    if current != stock::AUDIO_CALLBACK {
        AUDIO_STATE.store(AUDIO_SLOT_UNEXPECTED, Relaxed);
        return;
    }
    LO_HZ.store(LO_DEFAULT_HZ.to_bits(), Relaxed);
    let state = IrqState { het: Heterodyne::new(SAMPLE_RATE, LO_DEFAULT_HZ), lo_applied: LO_DEFAULT_HZ };
    // SAFETY: the interrupt does not use IRQ until the slot points at audio_hook.
    unsafe { (*IRQ.0.get()).write(state) };
    // SAFETY: a barrier has no other effect.
    unsafe { core::arch::asm!("dsb sy") };
    let hook: Handler = audio_hook;
    // SAFETY: a single aligned word store, which the interrupt reads whole.
    unsafe { ptr::write_volatile(stock::AUDIO_CALLBACK_SLOT, hook as usize) };
    AUDIO_STATE.store(AUDIO_INSTALLED, Relaxed);
}

fn blocks_per_second() -> u32 {
    // SAFETY: a byte in the stock DSP state.
    match unsafe { ptr::read_volatile(stock::RATE_CLASS) } {
        0 => 24_000,
        _ => 48_000,
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

unsafe fn handle_message(msg: usize) {
    // SAFETY: fields of the message copy (see key_hook).
    let (category, kind, value) = unsafe {
        (ptr::read_volatile(msg as *const u16), ptr::read_volatile((msg + 4) as *const u16), ptr::read_volatile((msg + 8) as *const i32))
    };
    if category != 0 || kind == 9 {
        return;
    }
    LAST_EVENT.store((kind as u32) << 16 | (value as u32 & 0xffff), Relaxed);
    let installed = AUDIO_STATE.load(Relaxed) == AUDIO_INSTALLED;
    if kind == 0 && value == MARK && installed {
        // Three presses within 1.5 s toggle the heterodyne.
        let now = BLOCKS.load(Relaxed);
        let first = MARK_TIMES[0].load(Relaxed);
        MARK_TIMES[0].store(MARK_TIMES[1].load(Relaxed), Relaxed);
        MARK_TIMES[1].store(now, Relaxed);
        if first != 0 && now.wrapping_sub(first) < blocks_per_second() * 3 / 2 {
            HET_ON.store(!HET_ON.load(Relaxed), Relaxed);
            MARK_TIMES[0].store(0, Relaxed);
            MARK_TIMES[1].store(0, Relaxed);
        }
    } else if kind == 2 && HET_ON.load(Relaxed) {
        let lo = f32::from_bits(LO_HZ.load(Relaxed)) + value as f32 * LO_STEP_HZ;
        LO_HZ.store(lo.clamp(LO_RANGE_HZ.0, LO_RANGE_HZ.1).to_bits(), Relaxed);
        // Swallow the turn, as the stock HOLD check does.
        // SAFETY: the kind field of the message copy.
        unsafe { ptr::write_volatile((msg + 4) as *mut u16, 9) };
    }
}

/// The audio DMA callback: runs the stock one, then, at 192 kHz with the
/// heterodyne on, replaces the headphone samples of the block it just wrote.
unsafe extern "C" fn audio_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: read before the stock callback moves the ring on.
    let index = unsafe { ptr::read_volatile(stock::AUDIO_INDEX) } as usize % 3;
    // SAFETY: the stock callback, with its own arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::AUDIO_CALLBACK) };
    let result = unsafe { stock_callback(a, b, c, d) };
    BLOCKS.store(BLOCKS.load(Relaxed).wrapping_add(1), Relaxed);
    // SAFETY: a byte in the stock DSP state.
    let rate_class = unsafe { ptr::read_volatile(stock::RATE_CLASS) };
    if HET_ON.load(Relaxed) && rate_class == 2 {
        // SAFETY: IRQ was written before this callback was installed.
        let state = unsafe { (*IRQ.0.get()).assume_init_mut() };
        let lo = f32::from_bits(LO_HZ.load(Relaxed));
        if lo != state.lo_applied {
            state.het.set_lo(lo);
            state.lo_applied = lo;
        }
        // SAFETY: the stock headphone gain, an f32 in ITCM.
        let gain = unsafe { ptr::read_volatile(stock::OUTPUT_GAIN) } * 2_147_483_647.0;
        let input = (stock::INPUT_BLOCKS + index * 0x180) as *const i32;
        let output = (stock::OUTPUT_BLOCKS + (index + 2) % 3 * 0x40) as *mut i32;
        for k in 0..4 {
            // SAFETY: words inside the input and output blocks of this frame.
            let (in3, in4) = unsafe { (ptr::read_volatile(input.add(12 * k)), ptr::read_volatile(input.add(12 * k + 6))) };
            let x = (in3 as f32 + in4 as f32) * (0.5 / 2_147_483_648.0);
            let y = (state.het.process_sample(x) * HET_GAIN).clamp(-HET_CEILING, HET_CEILING);
            let sample = (y * gain) as i32;
            unsafe {
                ptr::write_volatile(output.add(2 * k), sample);
                ptr::write_volatile(output.add(2 * k + 1), sample);
            }
        }
    }
    result
}

/// Text in a fixed buffer, for formatting without an allocator.
struct Line {
    buf: [u8; 96],
    len: usize,
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

impl Line {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

const STRIP_ROWS: usize = 13;
const FONT: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();

/// A 13-pixel strip across the top of both framebuffers.
fn draw_status() {
    let mut line = Line { buf: [0; 96], len: 0 };
    let event = LAST_EVENT.load(Relaxed);
    // SAFETY: a byte in the stock DSP state.
    let rate_class = unsafe { ptr::read_volatile(stock::RATE_CLASS) };
    let audio = match AUDIO_STATE.load(Relaxed) {
        AUDIO_INSTALLED => "audio hooked",
        AUDIO_SLOT_UNEXPECTED => "audio NOT hooked",
        _ => "audio ?",
    };
    let het = if HET_ON.load(Relaxed) {
        if rate_class == 2 { "HET ON" } else { "HET ON (needs 192k)" }
    } else {
        "HET off"
    };
    let _ = write!(
        line,
        "{het} {:.0} kHz | {audio} | rate {rate_class} | key {}/s | ev {} {:x}",
        f32::from_bits(LO_HZ.load(Relaxed)) / 1000.0,
        CALLS_PER_S.load(Relaxed),
        event >> 16,
        event & 0xffff,
    );
    let background = if HET_ON.load(Relaxed) { Rgb565::new(0, 18, 12) } else { Rgb565::new(4, 8, 4) };
    for &fb in &stock::FRAMEBUFFERS {
        // SAFETY: the top rows of a framebuffer the stock firmware allocated.
        let pixels = unsafe { core::slice::from_raw_parts_mut(fb as *mut u16, stock::WIDTH * STRIP_ROWS) };
        let mut target = Rgb565Buffer::new(pixels, stock::WIDTH, STRIP_ROWS);
        let _ = target.fill_solid(&Rectangle::new(Point::zero(), Size::new(stock::WIDTH as u32, STRIP_ROWS as u32)), background);
        let _ = FONT.render(line.as_str(), Point::new(2, 10), VerticalPosition::Baseline, FontColor::Transparent(Rgb565::WHITE), &mut target);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
