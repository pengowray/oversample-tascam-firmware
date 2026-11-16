//! 192 kHz over USB: widens the clock's rate list, adds interface 2's alternate settings 4 (2
//! channels of 32-bit float) and 5 (4 channels), sends INPUT 3 to 6 to the host at 192 kHz, and
//! keeps the 8-channel settings 1 and 3 at 96 kHz or below. See docs/HOMEBREW.md, "192 kHz stereo
//! over USB", and x8_patch::usb_audio for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use x8_patch::usb_audio::{
    EXTENDED_CONFIG_LEN, Merge, NEW_ALTS, STOCK_CONFIG_LEN, STOCK_TOP_RANGE, WIDE_TOP_RANGE, alt_to_use, before_alt,
    extend_config, float_channels, merged, must_stop, new_alt_packet, new_alt_packet_offsets, put_sample,
    rate_to_use, ring_frames,
};

use super::{Handler, read_byte, stock, system_mode};

/// Set once the start-up has replaced all three class callback words and widened the rate list.
/// Until then the stock code runs unchanged.
pub(super) static USB_READY: AtomicBool = AtomicBool::new(false);
/// Set once the device stack's list has alternate settings 4 and 5, so the descriptor may show them.
static ALTS_READY: AtomicBool = AtomicBool::new(false);
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

/// Interface 2's list of alternate settings with the patch's two: the stock entries for alts 0 to
/// 3 (still pointing at the stock endpoint records, which bus reset updates), then 4 and 5, each
/// with one endpoint record of its own. The device stack reads it on every SET_CONFIGURATION and
/// SET_INTERFACE.
#[repr(C, align(4))]
struct AltList([[u8; 16]; 6]);
#[repr(C, align(4))]
struct EndpointRecords([[u8; 8]; 2]);
static mut ALT_LIST: AltList = AltList([[0; 16]; 6]);
static mut ENDPOINTS: EndpointRecords = EndpointRecords([[0; 8]; 2]);

/// Point the device stack at the longer list. Harmless with the stock descriptor: a host cannot
/// choose settings it was never shown.
///
/// # Safety
/// Only from `prepare`, before the device stack starts.
unsafe fn add_alts() -> bool {
    // SAFETY: the stock interface record and list in `.data`, and the patch's own statics, which
    // nothing else touches until the stack is pointed at them.
    unsafe {
        if ptr::read_volatile(stock::IF2_ALT_LIST) != stock::IF2_STOCK_ALT_LIST
            || ptr::read_volatile(stock::IF2_ALT_COUNT) != 5
        {
            return false;
        }
        let list = &mut *ptr::addr_of_mut!(ALT_LIST);
        let records = &mut *ptr::addr_of_mut!(ENDPOINTS);
        let stock_list = stock::IF2_STOCK_ALT_LIST as *const [u8; 16];
        for (alt, entry) in list.0.iter_mut().enumerate().take(4) {
            *entry = ptr::read_volatile(stock_list.add(alt));
            if entry[0] != alt as u8 {
                return false;
            }
        }
        for (k, &(alt, channels)) in NEW_ALTS.iter().enumerate() {
            let size = new_alt_packet(channels, true).to_le_bytes();
            // {endpoint 0x84, isochronous, size, interval byte 0}, as the stock records.
            records.0[k] = [0x84, 0x01, size[0], size[1], 0, 0, 0, 0];
            let entry = &mut list.0[4 + k];
            *entry = [0; 16];
            entry[0] = alt;
            entry[4] = 1;
            entry[8..12].copy_from_slice(&(records.0[k].as_ptr() as usize as u32).to_le_bytes());
        }
        ptr::write_volatile(stock::IF2_ALT_LIST, list.0.as_ptr() as usize);
        ptr::write_volatile(stock::IF2_ALT_COUNT, 6);
    }
    true
}

/// The changes, all or none: the class callback words first, then the rate list, so that 192 kHz
/// is offered only with the guard in place, then the list of alternate settings.
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
        ALTS_READY.store(add_alts(), Release);
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
    if ready && let Some((channels, bytes, format)) = before_alt(alt as u8) {
        // The stock handler sets these for alternate settings 1 to 3 only. Written first, so its
        // first packets are this format's size. Interrupts off: the audio interrupt reads them.
        without_interrupts(|| {
            set_state_byte(state, stock::USB_BYTES, bytes);
            set_state_byte(state, stock::USB_CHANNELS, channels);
            set_state_byte(state, stock::USB_FORMAT, format);
        });
    }
    let alt = if ready { alt_to_use(alt as u8, state_word(state, stock::USB_RATE)) as usize } else { alt };
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
    match read_byte(stock::USB_ALT_IF2) {
        2 => feed(input),
        4 | 5 => feed_float(input),
        _ => {}
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

/// Writes the block's 4 frames of INPUT 3 to 4 (setting 4) or 3 to 6 (setting 5) into the ring
/// as 32-bit floats, after the dual-gain merge, with the stock merge's state at the end of this
/// block (it moves once a block at 192 kHz). Not inlined, so it stays in SDRAM.
#[inline(never)]
fn feed_float(input: *const i32) {
    let state = state();
    let alt = read_byte(stock::USB_ALT_IF2);
    let Some(channels) = float_channels(alt, state_byte(state, stock::USB_CHANNELS), state_byte(state, stock::USB_BYTES))
    else {
        return;
    };
    let channels = channels as usize;
    let read = |at: usize| -> f32 {
        // SAFETY: words of the stock DSP state in ITCM, which the audio routine has just updated.
        unsafe { ptr::read_volatile(at as *const f32) }
    };
    // INPUT 3 to 6 are merge channels 2 to 5. For INPUT 5 and 6 the stock routine skips the merge
    // when the byte at 0x2A2CF is 1 (EXT IN), and uses the high gain path alone.
    let ext_in = read_byte(stock::EXT_IN_ON_5_6) == 1;
    let mut sides = [(Merge { g: 1.0, c: 1.0, w: 0.0 }, 0.0f32); 4];
    for (k, side) in sides.iter_mut().enumerate().take(channels) {
        let channel = 2 + k;
        let at = stock::MERGE_STATE + stock::MERGE_STEP * channel;
        let merge = if ext_in && channel >= 4 {
            Merge { g: 1.0, c: 1.0, w: 0.0 }
        } else {
            Merge { g: read(at + stock::MERGE_G), c: read(at + stock::MERGE_C), w: read(at + stock::MERGE_W) }
        };
        *side = (merge, read(stock::INPUT_GAIN + stock::INPUT_GAIN_STEP * channel));
    }
    let frame_bytes = 4 * channels as u32;
    let frames = ring_frames(frame_bytes);
    let mut write = RING_WRITE.load(Relaxed) % frames;
    for k in 0..4 {
        for (ear, &(merge, gain)) in sides.iter().enumerate().take(channels) {
            // SAFETY: words of this block's input frames, and a whole frame inside the ring.
            unsafe {
                let a = ptr::read_volatile(input.add(12 * k + stock::INPUT_WORDS[2 + ear] as usize));
                let b = ptr::read_volatile(input.add(12 * k + stock::LOW_GAIN_WORDS_3_6[ear] as usize));
                let at = stock::USB_RING.add((write * frame_bytes) as usize + 4 * ear);
                for (j, byte) in merged(a, b, gain, merge).to_le_bytes().into_iter().enumerate() {
                    ptr::write_volatile(at.add(j), byte);
                }
            }
        }
        write = (write + 1) % frames;
    }
    RING_WRITE.store(write, Relaxed);
    // SAFETY: the write offset word of the stock USB audio state, as `FUN_00014b84` stores it.
    unsafe { ptr::write_volatile((state + stock::USB_WRITE_OFFSET) as *mut u32, write * frame_bytes) };
}

/// Run `f` with interrupts off, putting PRIMASK back as it was.
fn without_interrupts(f: impl FnOnce()) {
    let primask: u32;
    // SAFETY: reads PRIMASK and masks interrupts; restored below.
    unsafe { core::arch::asm!("mrs {}, PRIMASK", "cpsid i", out(reg) primask, options(nomem, nostack, preserves_flags)) };
    f();
    if primask & 1 == 0 {
        // SAFETY: interrupts were on before.
        unsafe { core::arch::asm!("cpsie i", options(nomem, nostack, preserves_flags)) };
    }
}

/// Replaces `FUN_80df48d0`'s call to `FUN_80df471c(n)`, which builds configuration n at each start
/// and plug-in. For configuration 0 (mass storage and UAC2 audio), adds alternate settings 4 and 5
/// to the stock descriptor, once the device stack's list has them.
pub(super) unsafe extern "C" fn usb_config_hook(n: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock function this call replaced, with its arguments.
    let build: Handler = unsafe { core::mem::transmute(stock::USB_BUILD_CONFIG) };
    let result = unsafe { build(n, b, c, d) };
    if n == 0 && system_mode() <= 1 && ALTS_READY.load(Acquire) {
        // SAFETY: the descriptor buffer the build has just filled, which the device stack reads
        // only after the run call that follows, and its stored length.
        unsafe {
            let len = ptr::read_volatile(stock::USB_CONFIG_LEN) as usize;
            let config = core::slice::from_raw_parts_mut(stock::USB_CONFIG, stock::USB_CONFIG_ROOM);
            if len == STOCK_CONFIG_LEN
                && let Some(new_len) = extend_config(config, len)
            {
                ptr::write_volatile(stock::USB_CONFIG_LEN, new_len as u32);
            }
        }
    }
    result
}

/// Replaces the device callback's call to `FUN_80df4a24(handle, speed)` at bus reset, which sets
/// every stock setting's packet size and gives the ones it does not know 0. Then sets settings 4
/// and 5's, in the descriptor and in their endpoint records.
pub(super) unsafe extern "C" fn usb_reset_hook(handle: usize, speed: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock function this call replaced, with its arguments.
    let reset: Handler = unsafe { core::mem::transmute(stock::USB_BUS_RESET) };
    let result = unsafe { reset(handle, speed, c, d) };
    // SAFETY: as in usb_config_hook; the endpoint records are the patch's own.
    unsafe {
        if ptr::read_volatile(stock::USB_CONFIG_LEN) as usize == EXTENDED_CONFIG_LEN {
            let records = &mut *ptr::addr_of_mut!(ENDPOINTS);
            for (k, (&(_, channels), &at)) in NEW_ALTS.iter().zip(&new_alt_packet_offsets()).enumerate() {
                let size = new_alt_packet(channels, speed == 2).to_le_bytes();
                ptr::write_volatile(stock::USB_CONFIG.add(at), size[0]);
                ptr::write_volatile(stock::USB_CONFIG.add(at + 1), size[1]);
                records.0[k][2..4].copy_from_slice(&size);
            }
        }
    }
    result
}
