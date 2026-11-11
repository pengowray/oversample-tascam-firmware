//! 192 kHz stereo over USB: widens the clock's rate list, sends INPUT 3 and INPUT 4 to the host at
//! 192 kHz, and keeps interface 2's 8-channel alternate settings at 96 kHz or below. See
//! docs/HOMEBREW.md, "192 kHz stereo over USB", and x8_patch::usb_audio for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use x8_patch::usb_audio::{
    STOCK_TOP_RANGE, WIDE_TOP_RANGE, alt_to_use, packet_fits, put_sample, rate_to_use, ring_frames,
};

use super::{Handler, read_byte, stock, system_mode};

/// Set once the start-up has replaced all three class callback words and widened the rate list.
/// Until then the stock code runs unchanged.
pub(super) static USB_READY: AtomicBool = AtomicBool::new(false);
/// The audio interrupt's write position in the ring, in frames.
static RING_WRITE: AtomicU32 = AtomicU32::new(0);

fn state() -> usize {
    // SAFETY: a word in the stock DTCM data.
    unsafe { ptr::read_volatile(stock::USB_STATE) }
}

fn state_byte(state: usize, offset: usize) -> u8 {
    // SAFETY: a byte of the stock USB audio state, which `state` found set up.
    unsafe { ptr::read_volatile((state + offset) as *const u8) }
}

fn state_word(state: usize, offset: usize) -> u32 {
    // SAFETY: as in state_byte.
    unsafe { ptr::read_volatile((state + offset) as *const u32) }
}

fn high_speed() -> bool {
    read_byte(stock::USB_SPEED) >= 2
}

/// The changes, all or none: the class callback words first, then the rate list, so that 192 kHz
/// is offered only with the guard in place.
fn prepare() {
    if USB_READY.load(Relaxed) {
        return;
    }
    let ours: Handler = class_callback;
    // SAFETY: words of the stock class config list in `.data`; the device stack has not started.
    unsafe {
        let words = stock::USB_CALLBACK_WORDS;
        if !words.iter().all(|&w| matches!(ptr::read_volatile(w), stock::USB_AUDIO_CALLBACK)) {
            return;
        }
        let range = core::slice::from_raw_parts(stock::USB_TOP_RANGE, 12);
        if range != STOCK_TOP_RANGE {
            return;
        }
        for &w in &words {
            ptr::write_volatile(w, ours as usize);
        }
        for (k, &b) in WIDE_TOP_RANGE.iter().enumerate() {
            ptr::write_volatile(stock::USB_TOP_RANGE.add(k), b);
        }
    }
    USB_READY.store(true, Release);
}

/// Replaces the USB task's call to the device stack's class init, in `FUN_80dc7064`.
pub(super) unsafe extern "C" fn usb_init_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    if system_mode() <= 1 {
        prepare();
    }
    // SAFETY: the stock function this call replaced, with its arguments.
    let init: Handler = unsafe { core::mem::transmute(stock::USB_CLASS_INIT) };
    unsafe { init(a, b, c, d) }
}

/// Replaces the device callback's call to interface 2's SET_INTERFACE handler, with r2 = the
/// alternate setting the host chose.
pub(super) unsafe extern "C" fn usb_alt_hook(handle: usize, interface: usize, alt: usize, d: usize) -> usize {
    let state = state();
    let alt = if USB_READY.load(Acquire) && state != 0 {
        alt_to_use(alt as u8, state_word(state, stock::USB_RATE)) as usize
    } else {
        alt
    };
    // SAFETY: the stock function this call replaced, with its arguments.
    let handler: Handler = unsafe { core::mem::transmute(stock::USB_SET_ALT_IF2) };
    unsafe { handler(handle, interface, alt, d) }
}

/// Wraps the audio class callback once `prepare` has put it in the class config list.
unsafe extern "C" fn class_callback(handle: usize, event: usize, param: usize, d: usize) -> usize {
    let state = state();
    if state != 0 && param != 0 && stock::USB_EVENT_SET_RATE.contains(&event) {
        // SAFETY: the stock request block: the data pointer, its length and the setup stage flag.
        unsafe {
            let data = ptr::read_volatile((param + 4) as *const usize) as *mut u8;
            let len = ptr::read_volatile((param + 8) as *const u32);
            if ptr::read_volatile((param + 0xc) as *const u8) != 1 && len == 4 && !data.is_null() {
                let rate = u32::from_le_bytes([0, 1, 2, 3].map(|k| ptr::read_volatile(data.add(k))));
                let allowed = rate_to_use(rate, read_byte(stock::USB_ALT_IF2));
                if allowed != rate {
                    for (k, b) in allowed.to_le_bytes().into_iter().enumerate() {
                        ptr::write_volatile(data.add(k), b);
                    }
                }
            }
        }
    }
    if state != 0
        && event == stock::USB_EVENT_SENT
        && !packet_fits(
            state_byte(state, stock::USB_CHANNELS),
            state_byte(state, stock::USB_BYTES),
            state_word(state, stock::USB_RATE),
            high_speed(),
        )
    {
        // The stock code would build a packet bigger than its buffers: stop the stream instead.
        return 1;
    }
    // SAFETY: the stock callback, with its arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::USB_AUDIO_CALLBACK) };
    unsafe { stock_callback(handle, event, param, d) }
}

/// From the audio interrupt at 192 kHz, after the stock callback: counts the block's 4 frames for
/// the rate feedback, and with interface 2 at alternate setting 2 (2 channels), writes INPUT 3 and
/// INPUT 4 into the ring as the stock 44.1 to 96 kHz routine writes its mix. `input` is the block's
/// first input frame. Not inlined, so it stays in SDRAM and out of both audio hook copies.
#[inline(never)]
pub(super) fn feed(input: *const i32) {
    let state = state();
    if !USB_READY.load(Acquire) || state == 0 {
        return;
    }
    // SAFETY: the stock function, called from the audio interrupt as the stock routine calls it.
    let count: Handler = unsafe { core::mem::transmute(stock::USB_COUNT_FRAMES) };
    unsafe { count(4, 0, 0, 0) };
    let (channels, bytes) = (state_byte(state, stock::USB_CHANNELS), state_byte(state, stock::USB_BYTES));
    if read_byte(stock::USB_ALT_IF2) != 2 || channels != 2 || !matches!(bytes, 3 | 4) {
        return;
    }
    let frame_bytes = 2 * bytes as u32;
    let frames = ring_frames(frame_bytes);
    let mut write = RING_WRITE.load(Relaxed) % frames;
    let [left, right] = [stock::INPUT_WORDS[2] as usize, stock::INPUT_WORDS[3] as usize];
    let mut frame = [0u8; 8];
    for k in 0..4 {
        // SAFETY: words of this block's input frames, and a whole frame inside the ring.
        unsafe {
            let n = put_sample(ptr::read_volatile(input.add(12 * k + left)), bytes, &mut frame);
            put_sample(ptr::read_volatile(input.add(12 * k + right)), bytes, &mut frame[n..]);
            let at = stock::USB_RING.add((write * frame_bytes) as usize);
            for (j, &b) in frame[..frame_bytes as usize].iter().enumerate() {
                ptr::write_volatile(at.add(j), b);
            }
        }
        write = (write + 1) % frames;
    }
    RING_WRITE.store(write, Relaxed);
    // SAFETY: the write offset word of the stock USB audio state, as `FUN_00014b84` stores it.
    unsafe { ptr::write_volatile((state + stock::USB_WRITE_OFFSET) as *mut u32, write * frame_bytes) };
}
