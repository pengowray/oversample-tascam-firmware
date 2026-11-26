//! Button presses the patch gives task 1: the phone remote's REC and HOME, and the MARK at a
//! pause's end. They act with HOLD on.

use core::ptr;
use core::sync::atomic::{
    AtomicBool, AtomicU32,
    Ordering::{Acquire, Relaxed, Release},
};

use super::state::BLOCKS;
use super::{blocks_per_second, stock};

/// When the patch last gave task 1 a button press, from any task, once it has given one. Read by
/// `hold_hook` in task 1.
static POSTED: AtomicBool = AtomicBool::new(false);
static POSTED_AT: AtomicU32 = AtomicU32::new(0);

/// For this long after the patch posts a button press, task 1 is told that HOLD is off, so the press
/// acts instead of opening the HOLD pop-up. Task 1 reads the press within a few ms; a button pressed
/// by hand in this second also gets past HOLD.
const PAST_HOLD_S: u32 = 1;

/// Give task 1 a button press, as `FUN_80d6c058` does after its press and release translation.
/// That translation is left out on purpose: it keeps shared state that task 9 also uses, and for
/// HOME it polls the audio engine. For REC and HOME it passes the code on unchanged at the press.
/// The block allocator and the mailbox are safe from any task, as the Bluetooth remote's task
/// shows. The press gets past HOLD: see `hold_hook`.
pub(super) fn post_button(code: i32) {
    POSTED_AT.store(BLOCKS.load(Relaxed), Relaxed);
    POSTED.store(true, Release);
    // SAFETY: the stock functions named in `stock`, called from a task as the stock code does.
    unsafe {
        let new_block: unsafe extern "C" fn(*mut usize) -> i32 = core::mem::transmute(stock::NEW_MESSAGE);
        let send: unsafe extern "C" fn(i32, usize) -> i32 = core::mem::transmute(stock::SND_MBX);
        let mut block = 0usize;
        if new_block(&mut block) != 0 {
            return;
        }
        ptr::write_volatile((block + 4) as *mut u32, 1);
        // Category 0 and kind 0 mean a button press.
        let message = (block + 8) as *mut u8;
        ptr::write_bytes(message, 0, stock::MESSAGE_BYTES);
        ptr::write_volatile(message.add(stock::MESSAGE_CODE_OFFSET).cast::<i32>(), code);
        send(stock::TASK_1_MAILBOX, block);
    }
}

/// Replaces task 1's call to the HOLD getter, the check that turns a button press into the HOLD
/// pop-up while HOLD is on. Says HOLD is off just after the patch posted a press, so the phone
/// remote can start and stop takes, and a pause's end can add its mark, with HOLD on.
pub(super) unsafe extern "C" fn hold_hook(_: usize, _: usize, _: usize, _: usize) -> usize {
    if POSTED.load(Acquire)
        && BLOCKS.load(Relaxed).wrapping_sub(POSTED_AT.load(Relaxed)) < PAST_HOLD_S * blocks_per_second()
    {
        return 0;
    }
    // SAFETY: the stock getter this call went to, with no arguments; it reads one byte.
    let hold: unsafe extern "C" fn() -> usize = unsafe { core::mem::transmute(stock::HOLD_ON) };
    unsafe { hold() & 0xff }
}
