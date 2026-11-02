use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

/// Stock firmware addresses, V2.03 build 217. See docs/HOMEBREW.md.
mod stock {
    /// The system mode: 0 or 1 for a normal start, 3 in firmware update mode.
    pub const SYSTEM_MODE: *const u8 = 0x8196_9dd9 as *const u8;
    /// The key handler the app screens share (Thumb address).
    pub const APP_KEY_HANDLER: usize = 0x80da_c0d5;
    /// The two framebuffers: 320 x 480, RGB565. The MPU maps them uncached
    /// (only its region 4 covers them), so pixels written here reach SDRAM,
    /// and the display controller, without a cache clean.
    pub const FRAMEBUFFERS: [usize; 2] = [0x87e0_0000, 0x87e4_b000];
    pub const WIDTH: usize = 320;
}

type KeyHandler = unsafe extern "C" fn(usize, usize, usize, usize) -> usize;

/// What `tools/x8patch.py` reads to find the entry points: it must stay at the
/// start of the patch.
#[repr(C)]
pub struct Header {
    magic: [u8; 4],
    version: u32,
    /// Replaces the stock app screen key handler.
    key_hook: KeyHandler,
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".header")]
pub static X8_PATCH_HEADER: Header = Header { magic: *b"BATX", version: 1, key_hook };

static PRESSES: AtomicU32 = AtomicU32::new(0);

/// Called with the stock handler's arguments (the event message is the first).
unsafe extern "C" fn key_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: a byte the stock firmware sets before the RTOS starts.
    let mode = unsafe { ptr::read_volatile(stock::SYSTEM_MODE) };
    if mode <= 1 {
        // A plain load and store, not fetch_add: the hook runs on one task
        // only, and exclusive access (ldrex/strex) to SDRAM may never succeed,
        // depending on how the MPU marks it.
        let n = PRESSES.load(Ordering::Relaxed);
        PRESSES.store(n.wrapping_add(1), Ordering::Relaxed);
        draw_test_bar(n);
    }
    // SAFETY: the address of the stock handler this hook replaces.
    let stock_handler: KeyHandler = unsafe { core::mem::transmute(stock::APP_KEY_HANDLER) };
    unsafe { stock_handler(a, b, c, d) }
}

/// A 16-pixel bar across both framebuffers at mid-height, its colour stepping
/// round six colours with each call.
fn draw_test_bar(n: u32) {
    const COLOURS: [u16; 6] = [0xf800, 0xffe0, 0x07e0, 0x07ff, 0x001f, 0xf81f];
    const TOP: usize = 232;
    const ROWS: usize = 16;
    let colour = COLOURS[n as usize % COLOURS.len()] as u32;
    let pair = colour | colour << 16;
    for &fb in &stock::FRAMEBUFFERS {
        let start = fb + TOP * stock::WIDTH * 2;
        let bytes = ROWS * stock::WIDTH * 2;
        let words = start as *mut u32;
        for i in 0..bytes / 4 {
            // SAFETY: inside a framebuffer the stock firmware allocated.
            unsafe { ptr::write_volatile(words.add(i), pair) };
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
