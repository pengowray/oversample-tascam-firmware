use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use bat_app::{BatApp, RemoteCommand, RemoteResult};

use super::state::{BLOCKS, HEAR_MIX};
use super::{blocks_per_second, read_byte, stock};

// The context running the app only.
static GOAL: AtomicU32 = AtomicU32::new(GOAL_NONE);
static GOAL_SINCE: AtomicU32 = AtomicU32::new(0);
static LAST_PRESS: AtomicU32 = AtomicU32::new(0);

const GOAL_NONE: u32 = 0;
const GOAL_RECORDING: u32 = 1;
const GOAL_STOPPED: u32 = 2;
/// After a stop with the mix muted while stopped: press REC once for standby, so the app can
/// hear the next start.
const GOAL_STANDBY: u32 = 3;

/// A command that has not got there by this many seconds is given up, with a beep.
const GIVE_UP_S: u32 = 5;
/// A press the stock code ignored is tried again after this many seconds. Each press is decided
/// from the recorder's state at that moment. If a second REC did reach a take that had just
/// started, it would pause the take, and the next look at the state presses REC to resume.
const PRESS_AGAIN_S: u32 = 2;
/// The stock code ignores REC while its worker task is still closing the last take's files.
const STANDBY_AFTER_S: u32 = 1;

// The recorder's transport byte: see `stock::TRANSPORT`.
const STOPPED: u8 = 0;
const STANDBY: u8 = 2;
const RECORDING: u8 = 3;
const PAUSED: u8 = 4;
const PAUSED_BY_EVENT: u8 = 6;

/// Give task 1 a button press, as `FUN_80d6c058` does after its press and release translation.
/// That translation is left out on purpose: it keeps shared state that task 9 also uses, and for
/// HOME it polls the audio engine. For REC and HOME it passes the code on unchanged at the press.
/// The block allocator and the mailbox are safe from any task, as the Bluetooth remote's task
/// shows.
pub(super) fn post_button(code: i32) {
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

fn hold_on() -> bool {
    // SAFETY: a stock getter that reads one byte.
    let hold: unsafe extern "C" fn() -> u8 = unsafe { core::mem::transmute(stock::HOLD_ON) };
    unsafe { hold() != 0 }
}

fn guide_on() -> bool {
    read_byte(stock::RECORDING_GUIDE) == 1
}

/// Press `code` unless it was pressed in the last `PRESS_AGAIN_S`, or a clock command is under
/// way: task 1 ignores REC while task 4 is busy, and a start chime sets the clock first so that
/// the take's names and `bext` times use the new time.
fn press(code: i32, now: u32) {
    if super::clock::busy() {
        return;
    }
    if now.wrapping_sub(LAST_PRESS.load(Relaxed)) >= PRESS_AGAIN_S * blocks_per_second() {
        LAST_PRESS.store(now, Relaxed);
        post_button(code);
    }
}

fn finish(app: &mut BatApp<'static>, result: RemoteResult, now: u32) {
    GOAL.store(GOAL_NONE, Relaxed);
    GOAL_SINCE.store(now, Relaxed);
    app.remote_result(result);
}

/// Carry on towards what the phone last asked for (the context running the app). Each pass looks
/// at the recorder's state, and presses at most one button.
pub(super) fn run_remote(app: &mut BatApp<'static>) {
    let now = BLOCKS.load(Relaxed);
    let transport = read_byte(stock::TRANSPORT);
    if let Some(command) = app.take_remote_command() {
        let goal = match command {
            RemoteCommand::Start => GOAL_RECORDING,
            RemoteCommand::Stop => GOAL_STOPPED,
        };
        GOAL.store(goal, Relaxed);
        GOAL_SINCE.store(now, Relaxed);
        LAST_PRESS.store(now.wrapping_sub(PRESS_AGAIN_S * blocks_per_second()), Relaxed);
        // These say nothing was done, so they do not wait for the recorder.
        if goal == GOAL_RECORDING && transport == RECORDING {
            finish(app, RemoteResult::AlreadyRecording, now);
        } else if goal == GOAL_STOPPED && !matches!(transport, RECORDING | PAUSED | PAUSED_BY_EVENT | 5) {
            finish(app, RemoteResult::AlreadyStopped, now);
        }
    }
    let waited = now.wrapping_sub(GOAL_SINCE.load(Relaxed));
    match GOAL.load(Relaxed) {
        GOAL_RECORDING => match transport {
            RECORDING => finish(app, RemoteResult::Started, now),
            _ if waited >= GIVE_UP_S * blocks_per_second() => finish(app, RemoteResult::NoResponse, now),
            STOPPED | STANDBY | PAUSED | PAUSED_BY_EVENT if hold_on() => finish(app, RemoteResult::HoldOn(RemoteCommand::Start), now),
            STOPPED if guide_on() => finish(app, RemoteResult::GuideOn, now),
            STOPPED if read_byte(stock::DSP_PAGE) != 0 => finish(app, RemoteResult::PageOpen, now),
            9..=11 => finish(app, RemoteResult::GuideOn, now),
            STOPPED | STANDBY | PAUSED | PAUSED_BY_EVENT => press(stock::REC, now),
            // Entering or leaving standby, closing a file or the self-timer: wait.
            _ => {}
        },
        GOAL_STOPPED => match transport {
            STOPPED => {
                finish(app, RemoteResult::Stopped, now);
                // The mix is muted while stopped with REC PAUSE on, and then the app could not
                // hear the next start.
                let muted_when_stopped = HEAR_MIX.load(Relaxed) && read_byte(stock::MONITOR_WHEN_STOPPED) == 0;
                if muted_when_stopped && !guide_on() {
                    GOAL.store(GOAL_STANDBY, Relaxed);
                }
            }
            _ if waited >= GIVE_UP_S * blocks_per_second() => finish(app, RemoteResult::NoResponse, now),
            RECORDING | PAUSED | PAUSED_BY_EVENT if hold_on() => finish(app, RemoteResult::HoldOn(RemoteCommand::Stop), now),
            RECORDING | PAUSED | PAUSED_BY_EVENT => press(stock::HOME, now),
            // Closing the file.
            _ => {}
        },
        GOAL_STANDBY if waited >= STANDBY_AFTER_S * blocks_per_second() => {
            GOAL.store(GOAL_NONE, Relaxed);
            if transport == STOPPED && read_byte(stock::DSP_PAGE) == 0 && !hold_on() {
                post_button(stock::REC);
            }
        }
        _ => {}
    }
}
