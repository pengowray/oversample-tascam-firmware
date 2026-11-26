//! 192 kHz over USB: widens the clock's rate list, moves the packet buffers to the host so 8
//! channels fit at 192 kHz, and at 192 kHz sends interface 2 what the stock audio routine sends at
//! 96 kHz and below, in the same order (the mix left and right, then INPUT 1 to 6), captured each
//! frame from the stock 192 kHz routine. See docs/HOMEBREW.md, "192 kHz stereo over USB", and
//! x8_patch::usb_audio for the decisions.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use x8_patch::usb_audio::{
    BIG_IN_PACKET, STOCK_TOP_RANGE, STRIDE_SITES, StockFrame, WIDE_TOP_RANGE, alt_to_use, big_packet, must_stop,
    rate_to_use, ring_frames, stock_sample, thumb_bl, with_modified_imm,
};

use super::{Handler, read_byte, stock, system_mode};

/// Set once the start-up has replaced all three class callback words and widened the rate list.
/// Until then the stock code runs unchanged.
pub(super) static USB_READY: AtomicBool = AtomicBool::new(false);
/// Set once the packet buffers to the host are 800 bytes, so 8 channels fit at 192 kHz.
static BIG_BUFFERS: AtomicBool = AtomicBool::new(false);
/// Set once the 192 kHz routine's recorder step call goes through the capture stub.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The audio interrupt's write position in the ring, in frames.
static RING_WRITE: AtomicU32 = AtomicU32::new(0);

/// The 4 packet buffers to the host, 800 bytes each, in place of the stock 416 at 0x2000BE60. In
/// DTCM, as the stock ones are, for the USB controller's DMA.
#[repr(C, align(32))]
struct InBuffers([u8; 4 * BIG_IN_PACKET as usize]);
#[unsafe(link_section = ".dtcm")]
static mut IN_BUFFERS: InBuffers = InBuffers([0; 4 * BIG_IN_PACKET as usize]);

/// The block's 4 frames as the capture stub copies them from the stock 192 kHz routine.
#[repr(C, align(4))]
struct Captured([StockFrame; 4]);
#[unsafe(link_section = ".dtcm")]
static mut CAPTURED: Captured = Captured([[0.0; 8]; 4]);

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

fn big() -> bool {
    BIG_BUFFERS.load(Acquire)
}

/// The changes, all or none: the class callback words first, then the rate list, so that 192 kHz
/// is offered only with the guard in place. Then, each on its own, the bigger buffers and the
/// capture; without them 192 kHz works as in B263 to B275, minus the audio.
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
        BIG_BUFFERS.store(move_buffers(), Release);
        CAPTURING.store(install_capture(), Release);
    }
    USB_READY.store(true, Release);
}

/// Point the audio class callback and the SET_INTERFACE handler at 4 buffers of 800 bytes: the
/// literal with the buffers' address, and the 7 instructions that hold the stride between them.
/// Only if every site holds its stock bytes.
///
/// # Safety
/// Only from `prepare`, before the device stack starts: nothing runs this USB code yet.
unsafe fn move_buffers() -> bool {
    let read = |address: u32| -> [u8; 4] {
        // SAFETY: stock ITCM code, readable.
        core::array::from_fn(|k| unsafe { ptr::read_volatile((address as usize + k) as *const u8) })
    };
    // SAFETY: the stock literal in ITCM.
    if unsafe { ptr::read_volatile(stock::USB_IN_BUFFERS_LITERAL) } != stock::USB_STOCK_IN_BUFFERS
        || STRIDE_SITES.iter().any(|&(address, bytes, _)| read(address) != bytes)
    {
        return false;
    }
    let mut new = [[0u8; 4]; STRIDE_SITES.len()];
    for (out, &(_, bytes, multiple)) in new.iter_mut().zip(&STRIDE_SITES) {
        let Some(insn) = with_modified_imm(bytes, BIG_IN_PACKET * multiple) else { return false };
        *out = insn;
    }
    // SAFETY: the patch's own buffers, then the stock sites checked above: halfword writes into
    // ITCM code that does not run until the device stack starts.
    unsafe {
        let buffers = ptr::addr_of_mut!(IN_BUFFERS);
        ptr::write_bytes(buffers, 0, 1);
        for (&(address, _, _), insn) in STRIDE_SITES.iter().zip(&new) {
            let at = address as usize as *mut u16;
            ptr::write_volatile(at, u16::from_le_bytes([insn[0], insn[1]]));
            ptr::write_volatile(at.add(1), u16::from_le_bytes([insn[2], insn[3]]));
        }
        ptr::write_volatile(stock::USB_IN_BUFFERS_LITERAL, buffers as u32);
        core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
    }
    true
}

/// Copy the capture stub into the free ITCM after the stock code, at the same address mod 4 as
/// its link address (its literals are PC-relative), and point the 192 kHz routine's recorder step
/// call at it. With interrupts off: the audio routine may be running.
///
/// # Safety
/// Only from `prepare`.
unsafe fn install_capture() -> bool {
    // Link-time symbols around the stub. The start is a Thumb function symbol, with bit 0 set.
    let start = ptr::addr_of!(usb_capture_start) as usize & !1;
    let end = ptr::addr_of!(usb_capture_end) as usize;
    let len = end - start;
    let at = stock::ITCM_GAP + (start & 2);
    // SAFETY: the stock call's two halfwords in ITCM.
    let call = unsafe {
        [ptr::read_volatile(stock::RECORDER_STEP_CALL), ptr::read_volatile(stock::RECORDER_STEP_CALL.add(1))]
    };
    if call != stock::RECORDER_STEP_CALL_STOCK || at + len > stock::ITCM_GAP_END {
        return false;
    }
    let Some(bl) = thumb_bl(stock::RECORDER_STEP_CALL as usize, at) else { return false };
    // SAFETY: the stub's bytes in the patch's code, copied into ITCM no stock code uses; then
    // the call, both halfwords with interrupts off, so the audio routine never sees half of it.
    unsafe {
        for k in 0..len {
            ptr::write_volatile((at + k) as *mut u8, ptr::read_volatile((start + k) as *const u8));
        }
        core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
        super::without_interrupts(|| {
            ptr::write_volatile(stock::RECORDER_STEP_CALL, bl[0]);
            ptr::write_volatile(stock::RECORDER_STEP_CALL.add(1), bl[1]);
            core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
        });
    }
    true
}

unsafe extern "C" {
    static usb_capture_start: u8;
    static usb_capture_end: u8;
}

// The capture stub: called in place of the recorder step, once a frame. Copies the mix and INPUT
// 1 to 6 (DSP state + 0xA8, + 0xAC, + 0x50 to + 0x64, r0 = the state) into the frame's row of
// CAPTURED (row = the caller's frame index at [sp + 4]), then goes on to the recorder step with
// r0 to r3 and lr as they came. Copied into ITCM by `install_capture`; it runs only there.
core::arch::global_asm!(
    ".section .text.usb_capture_stub,\"ax\",%progbits",
    ".p2align 2",
    ".thumb_func",
    ".global usb_capture_start",
    "usb_capture_start:",
    "push {{r4, r5}}",
    "ldr r12, [sp, #12]",
    "and r12, r12, #3",
    "ldr r4, 2f",
    "add.w r4, r4, r12, lsl #5",
    "ldr.w r5, [r0, #0xa8]",
    "str r5, [r4, #0]",
    "ldr.w r5, [r0, #0xac]",
    "str r5, [r4, #4]",
    "ldr r5, [r0, #0x50]",
    "str r5, [r4, #8]",
    "ldr r5, [r0, #0x54]",
    "str r5, [r4, #12]",
    "ldr r5, [r0, #0x58]",
    "str r5, [r4, #16]",
    "ldr r5, [r0, #0x5c]",
    "str r5, [r4, #20]",
    "ldr r5, [r0, #0x60]",
    "str r5, [r4, #24]",
    "ldr r5, [r0, #0x64]",
    "str r5, [r4, #28]",
    "pop {{r4, r5}}",
    "ldr pc, 3f",
    ".p2align 2",
    "2: .word {captured}",
    "3: .word {recorder_step}",
    ".global usb_capture_end",
    "usb_capture_end:",
    captured = sym CAPTURED,
    recorder_step = const stock::RECORDER_STEP,
);

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
        alt_to_use(alt as u8, state_word(state, stock::USB_RATE), big()) as usize
    } else {
        alt
    };
    // SAFETY: the stock function this call replaced, with its arguments.
    let handler: Handler = unsafe { core::mem::transmute(stock::USB_SET_ALT_IF2) };
    unsafe { handler(handle, interface, alt, d) }
}

/// Replaces the device callback's call to `FUN_80df4a24(handle, speed)` at bus reset, which sets
/// the stock packet sizes. With the bigger buffers, then gives the 8-channel settings room for
/// 192 kHz: 600 and 800 bytes at high speed, in the descriptor and in their endpoint records.
pub(super) unsafe extern "C" fn usb_reset_hook(handle: usize, speed: usize, c: usize, d: usize) -> usize {
    // SAFETY: the stock function this call replaced, with its arguments.
    let reset: Handler = unsafe { core::mem::transmute(stock::USB_BUS_RESET) };
    let result = unsafe { reset(handle, speed, c, d) };
    if !big() || speed != 2 {
        return result;
    }
    for (alt, endpoint, record) in [
        (1, stock::USB_CONFIG_ALT1_ENDPOINT, stock::USB_ALT1_RECORD),
        (3, stock::USB_CONFIG_ALT3_ENDPOINT, stock::USB_ALT3_RECORD),
    ] {
        let Some(size) = big_packet(alt) else { continue };
        let size = size.to_le_bytes();
        // SAFETY: the descriptor copy the stock function has just walked, and the stack's
        // endpoint record for this setting; checked to be endpoint 0x84 first.
        unsafe {
            let at = stock::USB_CONFIG.add(endpoint);
            let descriptor = [ptr::read_volatile(at), ptr::read_volatile(at.add(1)), ptr::read_volatile(at.add(2))];
            if descriptor == [7, 5, 0x84] && ptr::read_volatile(record) == 0x84 {
                ptr::write_volatile(at.add(4), size[0]);
                ptr::write_volatile(at.add(5), size[1]);
                ptr::write_volatile(record.add(2), size[0]);
                ptr::write_volatile(record.add(3), size[1]);
            }
        }
    }
    result
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
                let allowed = rate_to_use(rate, read_byte(stock::USB_ALT_IF2), big());
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
            big(),
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
/// the rate feedback, as the stock 44.1 to 96 kHz routine does every block, and with interface 2
/// streaming calls `feed`. Inlined into both audio hook copies, so the usual case costs no call
/// into SDRAM.
#[inline(always)]
pub(super) fn on_block(_input: *const i32) {
    if !USB_READY.load(Relaxed) || state() == 0 {
        return;
    }
    // SAFETY: the stock function (ITCM), called from the audio interrupt as the stock routine
    // calls it.
    let count: Handler = unsafe { core::mem::transmute(stock::USB_COUNT_FRAMES) };
    unsafe { count(4, 0, 0, 0) };
    if read_byte(stock::USB_ALT_IF2) != 0 {
        feed();
    }
}

/// Writes the block's 4 frames into the ring as the stock 44.1 to 96 kHz routine would: the
/// first channels of the mix and INPUT 1 to 6, in the setting's sample size, or zeros while the
/// transport plays or without the capture. Not inlined, so it stays in SDRAM.
#[inline(never)]
fn feed() {
    let state = state();
    let (channels, bytes) = (state_byte(state, stock::USB_CHANNELS), state_byte(state, stock::USB_BYTES));
    if !matches!(channels, 1..=8) || !matches!(bytes, 3 | 4) {
        return;
    }
    let silent = !CAPTURING.load(Relaxed) || read_byte(stock::TRANSPORT_PLAYING) == 1;
    let frame_bytes = channels as usize * bytes as usize;
    let frames = ring_frames(frame_bytes as u32) as usize;
    let mut write = RING_WRITE.load(Relaxed) as usize % frames;
    // SAFETY: the stub filled this block's rows before the audio hook ran, in the same interrupt.
    let rows = unsafe { &*ptr::addr_of!(CAPTURED.0) };
    for row in rows {
        let at = write * frame_bytes;
        if bytes == 4 {
            // Float, as is: whole words, since the ring and 4-byte frames are word-aligned.
            let words = stock::USB_RING.wrapping_add(at) as *mut u32;
            for (j, &value) in row[..channels as usize].iter().enumerate() {
                let bits = if silent { 0 } else { value.to_bits() };
                // SAFETY: a whole frame inside the ring.
                unsafe { ptr::write_volatile(words.add(j), bits) };
            }
        } else {
            put_frame_24(at, &row[..channels as usize], silent);
        }
        write = (write + 1) % frames;
    }
    RING_WRITE.store(write as u32, Relaxed);
    // SAFETY: the write offset word of the stock USB audio state, as `FUN_00014b84` stores it.
    unsafe { ptr::write_volatile((state + stock::USB_WRITE_OFFSET) as *mut u32, (write * frame_bytes) as u32) };
}

/// One frame of 3-byte samples into the ring at byte `at`, packed as the stock routine packs
/// them. Not inlined: one call a frame keeps `feed` small.
#[inline(never)]
fn put_frame_24(at: usize, row: &[f32], silent: bool) {
    for (j, &value) in row.iter().enumerate() {
        let mut sample = [0u8; 4];
        stock_sample(if silent { 0.0 } else { value }, 3, &mut sample);
        for (k, &b) in sample[..3].iter().enumerate() {
            // SAFETY: inside the ring: `feed` writes whole frames below its whole-frame size.
            unsafe { ptr::write_volatile(stock::USB_RING.add(at + 3 * j + k), b) };
        }
    }
}
