//! 192 kHz stereo over USB: widens the clock's rate list, sends INPUT 3 and INPUT 4 to the host at
//! 192 kHz (3-byte samples on interface 2's alternate setting 2, 32-bit float after the dual-gain
//! merge on alternate setting 3), and keeps alternate setting 1 at 96 kHz or below. See
//! docs/HOMEBREW.md, "192 kHz stereo over USB", and x8_patch::usb_audio for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use x8_patch::usb_audio::{
    FOUR_BYTE_FORMAT, Merge, STOCK_TOP_RANGE, WIDE_TOP_RANGE, after_alt, alt_to_use, float_channels, merged,
    must_stop, put_sample, rate_to_use, ring_frames, sends_float,
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

fn set_state_byte(state: usize, offset: usize, value: u8) {
    // SAFETY: as in state_byte; the stock code writes these bytes the same way.
    unsafe { ptr::write_volatile((state + offset) as *mut u8, value) }
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
    let ready = USB_READY.load(Acquire) && state != 0;
    let rate = if ready { state_word(state, stock::USB_RATE) } else { 0 };
    let stock_alt = if ready { alt_to_use(alt as u8, rate) as usize } else { alt };
    // SAFETY: the stock function this call replaced, with its arguments.
    let handler: Handler = unsafe { core::mem::transmute(stock::USB_SET_ALT_IF2) };
    let result = unsafe { handler(handle, interface, stock_alt, d) };
    if ready && let Some((channels, bytes)) = after_alt(alt as u8, rate) {
        // Float at 192 kHz: the stock handler set up 2 channels of 3 bytes and queued its first
        // packets; from the next packet each sample is 4 bytes.
        set_state_byte(state, stock::USB_BYTES, bytes);
        set_state_byte(state, stock::USB_CHANNELS, channels);
        set_state_byte(state, stock::USB_FORMAT, FOUR_BYTE_FORMAT);
    }
    result
}

/// Wraps the audio class callback once `prepare` has put it in the class config list.
unsafe extern "C" fn class_callback(handle: usize, event: usize, param: usize, d: usize) -> usize {
    let state = state();
    // Channels to write after the stock code has taken a new rate.
    let mut channels_after = None;
    if state != 0 && param != 0 && stock::USB_EVENT_SET_RATE.contains(&event) {
        // SAFETY: the stock request block: the data pointer, its length and the setup stage flag.
        unsafe {
            let data = ptr::read_volatile((param + 4) as *const usize) as *mut u8;
            let len = ptr::read_volatile((param + 8) as *const u32);
            if ptr::read_volatile((param + 0xc) as *const u8) != 1 && len == 4 && !data.is_null() {
                let rate = u32::from_le_bytes([0, 1, 2, 3].map(|k| ptr::read_volatile(data.add(k))));
                let alt = read_byte(stock::USB_ALT_IF2);
                let allowed = rate_to_use(rate, alt);
                if allowed != rate {
                    for (k, b) in allowed.to_le_bytes().into_iter().enumerate() {
                        ptr::write_volatile(data.add(k), b);
                    }
                }
                match float_channels(alt, allowed) {
                    Some((channels, true)) => set_state_byte(state, stock::USB_CHANNELS, channels),
                    Some((channels, false)) => channels_after = Some(channels),
                    None => {}
                }
            }
        }
    }
    if let Some(channels) = channels_after {
        // SAFETY: the stock callback, with its arguments.
        let stock_callback: Handler = unsafe { core::mem::transmute(stock::USB_AUDIO_CALLBACK) };
        let result = unsafe { stock_callback(handle, event, param, d) };
        set_state_byte(state, stock::USB_CHANNELS, channels);
        return result;
    }
    if state != 0
        && event == stock::USB_EVENT_SENT
        && must_stop(
            state_byte(state, stock::USB_CHANNELS),
            state_byte(state, stock::USB_BYTES),
            state_word(state, stock::USB_RATE),
            high_speed(),
        )
    {
        // Above 96 kHz the stock code would build a packet bigger than its buffers: stop the stream.
        return 1;
    }
    // SAFETY: the stock callback, with its arguments.
    let stock_callback: Handler = unsafe { core::mem::transmute(stock::USB_AUDIO_CALLBACK) };
    unsafe { stock_callback(handle, event, param, d) }
}

/// From the audio interrupt at 192 kHz, after the stock callback: counts the block's 4 frames for
/// the rate feedback, as the stock 44.1 to 96 kHz routine does every block, and with interface 2 at
/// alternate setting 2 calls `feed`. `input` is the block's first input frame. Inlined into both
/// audio hook copies, so the usual case costs no call into SDRAM.
#[inline(always)]
pub(super) fn on_block(input: *const i32) {
    if !USB_READY.load(Relaxed) || state() == 0 {
        return;
    }
    // SAFETY: the stock function (ITCM), called from the audio interrupt as the stock routine
    // calls it.
    let count: Handler = unsafe { core::mem::transmute(stock::USB_COUNT_FRAMES) };
    unsafe { count(4, 0, 0, 0) };
    let alt = read_byte(stock::USB_ALT_IF2);
    if alt == 2 {
        feed(input);
    } else if alt == 3 {
        feed_float(input);
    }
}

/// Writes the block's 4 frames of INPUT 3 and INPUT 4 into the ring, as the stock 44.1 to 96 kHz
/// routine writes its mix. Not inlined, so it stays in SDRAM.
#[inline(never)]
fn feed(input: *const i32) {
    let state = state();
    let (channels, bytes) = (state_byte(state, stock::USB_CHANNELS), state_byte(state, stock::USB_BYTES));
    if channels != 2 || !matches!(bytes, 3 | 4) {
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

/// Writes the block's 4 frames of INPUT 3 and INPUT 4 into the ring as 32-bit floats, after the
/// dual-gain merge, with the stock merge's state at the end of this block (it moves once a block
/// at 192 kHz). Not inlined, so it stays in SDRAM.
#[inline(never)]
fn feed_float(input: *const i32) {
    let state = state();
    if !sends_float(read_byte(stock::USB_ALT_IF2), state_byte(state, stock::USB_CHANNELS), state_byte(state, stock::USB_BYTES)) {
        return;
    }
    // INPUT 3 and 4 are merge channels 2 and 3.
    let read = |at: usize| -> f32 {
        // SAFETY: words of the stock DSP state in ITCM, which the audio routine has just updated.
        unsafe { ptr::read_volatile(at as *const f32) }
    };
    let side = |channel: usize| {
        let at = stock::MERGE_STATE + stock::MERGE_STEP * channel;
        let merge = Merge { g: read(at + stock::MERGE_G), c: read(at + stock::MERGE_C), w: read(at + stock::MERGE_W) };
        (merge, read(stock::INPUT_GAIN + stock::INPUT_GAIN_STEP * channel))
    };
    let sides = [side(2), side(3)];
    let high = [stock::INPUT_WORDS[2] as usize, stock::INPUT_WORDS[3] as usize];
    let low = stock::LOW_GAIN_WORDS_3_4.map(|w| w as usize);
    let frames = ring_frames(8);
    let mut write = RING_WRITE.load(Relaxed) % frames;
    for k in 0..4 {
        for (ear, &(merge, gain)) in sides.iter().enumerate() {
            // SAFETY: words of this block's input frames, and a whole frame inside the ring.
            unsafe {
                let a = ptr::read_volatile(input.add(12 * k + high[ear]));
                let b = ptr::read_volatile(input.add(12 * k + low[ear]));
                let at = stock::USB_RING.add((write * 8) as usize + 4 * ear);
                for (j, byte) in merged(a, b, gain, merge).to_le_bytes().into_iter().enumerate() {
                    ptr::write_volatile(at.add(j), byte);
                }
            }
        }
        write = (write + 1) % frames;
    }
    RING_WRITE.store(write, Relaxed);
    // SAFETY: the write offset word of the stock USB audio state, as `FUN_00014b84` stores it.
    unsafe { ptr::write_volatile((state + stock::USB_WRITE_OFFSET) as *mut u32, write * 8) };
}
