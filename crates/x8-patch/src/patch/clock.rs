//! Sets the X8's clock from the phone's chimes, with the stock recorder task's own clock commands,
//! and keeps the phone's time for a take's GUANO metadata (docs/HOMEBREW.md, "Setting the clock").
//!
//! Only the context running the app calls `run_clock`. It gives task 4 a command only when the
//! recorder is stopped and task 4 is idle, and sets the clock only between takes: a chime during a
//! take is kept, the GUANO hook uses its time for that take, and the clock is set once the take's
//! files are closed.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::{BatApp, ClockState, PhoneTime, Screen};
use x8_patch::clock_record::{self, ClockReading, RECORD_BYTES};

use super::state::BLOCKS;
use super::{Slot, blocks_per_second, read_byte, stock, system_mode};

// The phone's time, written by the context running the app and read by the GUANO hook (task 4).
// `PHONE_SEQ` is odd while the others change, so a reader can tell it read a mix.
static PHONE_SEQ: AtomicU32 = AtomicU32::new(0);
static PHONE_LOCAL_S: AtomicU32 = AtomicU32::new(0);
static PHONE_MS: AtomicU32 = AtomicU32::new(0);
static PHONE_BLOCKS: AtomicU32 = AtomicU32::new(0);
static PHONE_TZ: AtomicI32 = AtomicI32::new(0);
/// The clock is to be set from the phone's time: as soon as the recorder is stopped.
static SET_WANTED: AtomicBool = AtomicBool::new(false);
/// The clock has been set from a chime since the X8 started, so it keeps the phone's local time
/// in the zone `PHONE_TZ` had then.
static SET_FROM_PHONE: AtomicBool = AtomicBool::new(false);
static SET_TZ: AtomicI32 = AtomicI32::new(0);

// The context running the app only.
static STEP: AtomicU32 = AtomicU32::new(IDLE);
static STEP_SINCE: AtomicU32 = AtomicU32::new(0);
static LAST_READ: AtomicU32 = AtomicU32::new(0);
static TAKE_ENDED: AtomicU32 = AtomicU32::new(0);
static WAS_STOPPED: AtomicBool = AtomicBool::new(true);
/// The latest reading of the clock and the block count it was taken at.
static X8: Slot<Option<(ClockReading, u32)>> = Slot(core::cell::UnsafeCell::new(core::mem::MaybeUninit::new(None)));
/// The record command 76 copies from. It must stay put until task 4 has run the command.
static RECORD: Slot<[u8; RECORD_BYTES]> = Slot(core::cell::UnsafeCell::new(core::mem::MaybeUninit::new([0; RECORD_BYTES])));

const IDLE: u32 = 0;
/// Waiting for command 75; then, if `SET_WANTED`, giving command 76.
const READING: u32 = 1;
const SETTING: u32 = 2;
/// Waiting for command 75 after a set, to check it.
const CHECKING: u32 = 3;

/// A command task 4 has not finished in this many seconds has failed.
const COMMAND_TIMEOUT_S: u32 = 2;
/// The clock screen reads the clock this often; in between it counts on from the last reading.
const READ_EVERY_S: u32 = 5;
/// After a take's files close, task 4 is left alone this long before the clock is set.
const AFTER_TAKE_S: u32 = 1;
/// A set is good when the clock reads within this of the phone's time.
const SET_WITHIN_S: i64 = 2;

fn task_4_busy() -> bool {
    // SAFETY: a stock function that reads task 4's state.
    let busy: unsafe extern "C" fn() -> i32 = unsafe { core::mem::transmute(stock::TASK_4_BUSY) };
    unsafe { busy() != 0 }
}

fn give_task_4(command: i32, parameter: usize) {
    // SAFETY: the stock function the stock code gives task 4 commands with, called only when
    // task 4 is idle and the recorder is stopped.
    let give: unsafe extern "C" fn(i32, usize) -> i32 = unsafe { core::mem::transmute(stock::TASK_4_COMMAND) };
    unsafe { give(command, parameter) };
}

/// The phone's local time at block count `now`, in ms since 1970, once a chime has been read.
fn phone_ms_at(now: u32) -> Option<i64> {
    loop {
        let seq = PHONE_SEQ.load(Acquire);
        if seq == 0 {
            return None;
        }
        let (s, ms, at) = (PHONE_LOCAL_S.load(Relaxed), PHONE_MS.load(Relaxed), PHONE_BLOCKS.load(Relaxed));
        if seq.is_multiple_of(2) && PHONE_SEQ.load(Acquire) == seq {
            let elapsed_ms = now.wrapping_sub(at) as i64 * 1000 / blocks_per_second() as i64;
            return Some(s as i64 * 1000 + ms as i64 + elapsed_ms);
        }
    }
}

fn keep_phone_time(time: &PhoneTime, now: u32) {
    let local_ms = time.local_ms.max(0);
    PHONE_SEQ.fetch_add(1, Release);
    PHONE_LOCAL_S.store((local_ms / 1000) as u32, Relaxed);
    PHONE_MS.store((local_ms % 1000) as u32, Relaxed);
    PHONE_BLOCKS.store(now, Relaxed);
    PHONE_TZ.store(time.timezone_minutes.into(), Relaxed);
    let seq = PHONE_SEQ.fetch_add(1, Release) + 1;
    // 0 means no reading, and an odd count means a write in progress.
    if seq == 0 {
        PHONE_SEQ.store(2, Release);
    }
}

/// For a take's GUANO metadata, at the moment its files close: the phone's local time now when a
/// clock set is waiting for the take to end (the X8's clock is then wrong by however much the set
/// will move it), and the UTC offset, in minutes, of the time the take's clock keeps, when known.
pub(super) fn take_times() -> (Option<i64>, Option<i16>) {
    if SET_WANTED.load(Relaxed) {
        let tz = PHONE_TZ.load(Relaxed) as i16;
        return (phone_ms_at(BLOCKS.load(Relaxed)).map(|ms| ms.div_euclid(1000)), Some(tz));
    }
    (None, SET_FROM_PHONE.load(Relaxed).then(|| SET_TZ.load(Relaxed) as i16))
}

/// Whether a clock command is under way, so the phone remote holds back its button presses: task 1
/// ignores REC while task 4 is busy.
pub(super) fn busy() -> bool {
    STEP.load(Relaxed) != IDLE
}

fn x8() -> &'static mut Option<(ClockReading, u32)> {
    // SAFETY: the context running the app only.
    unsafe { (*X8.0.get()).assume_init_mut() }
}

/// The record task 4 left, as a reading at block count `now`.
fn read_record(now: u32) -> Option<(ClockReading, u32)> {
    // SAFETY: the stock clock record, which task 4 has finished writing (it is idle).
    let record = unsafe { ptr::read_volatile(stock::CLOCK_RECORD) };
    clock_record::read(&record).map(|reading| (reading, now))
}

/// The X8's clock at block count `now`, from the latest reading.
fn x8_seconds_at(now: u32) -> Option<i64> {
    x8().map(|(reading, at)| reading.local_seconds + (now.wrapping_sub(at) / blocks_per_second()) as i64)
}

fn go(step: u32, now: u32) {
    STEP.store(step, Relaxed);
    STEP_SINCE.store(now, Relaxed);
}

fn fail(app: &mut BatApp<'static>, now: u32) {
    SET_WANTED.store(false, Relaxed);
    app.set_clock_state(ClockState::Failed);
    go(IDLE, now);
}

/// One pass: take the app's phone time, and move the clock commands on by at most one step.
pub(super) fn run_clock(app: &mut BatApp<'static>) {
    let now = BLOCKS.load(Relaxed);
    let bps = blocks_per_second();
    let transport = read_byte(stock::TRANSPORT);
    let stopped = transport == 0;
    if stopped && !WAS_STOPPED.load(Relaxed) {
        TAKE_ENDED.store(now, Relaxed);
    }
    WAS_STOPPED.store(stopped, Relaxed);
    if let Some(time) = app.take_phone_time() {
        keep_phone_time(&time, now);
        super::location::keep(&time);
        if time.set_clock {
            SET_WANTED.store(true, Relaxed);
            app.set_clock_state(if stopped { ClockState::Setting } else { ClockState::AfterTake });
        }
    }
    let free = system_mode() <= 1 && stopped && !task_4_busy() && now.wrapping_sub(TAKE_ENDED.load(Relaxed)) >= AFTER_TAKE_S * bps;
    let waited = now.wrapping_sub(STEP_SINCE.load(Relaxed));
    match STEP.load(Relaxed) {
        IDLE if free && SET_WANTED.load(Relaxed) => {
            app.set_clock_state(ClockState::Setting);
            give_task_4(stock::COMMAND_READ_CLOCK, 0);
            go(READING, now);
        }
        IDLE if free && app.screen() == Screen::ChimeTest && now.wrapping_sub(LAST_READ.load(Relaxed)) >= READ_EVERY_S * bps => {
            give_task_4(stock::COMMAND_READ_CLOCK, 0);
            go(READING, now);
        }
        IDLE if SET_WANTED.load(Relaxed) && !stopped => app.set_clock_state(ClockState::AfterTake),
        READING | SETTING | CHECKING if waited >= COMMAND_TIMEOUT_S * bps => {
            if SET_WANTED.load(Relaxed) {
                fail(app, now);
            } else {
                go(IDLE, now);
            }
        }
        // Wait a block at least, so task 4 has started before it counts as finished.
        READING if waited > 0 && !task_4_busy() => {
            LAST_READ.store(now, Relaxed);
            *x8() = read_record(now);
            if !SET_WANTED.load(Relaxed) {
                go(IDLE, now);
                return;
            }
            let Some((conventions, _)) = *x8() else { return fail(app, now) };
            let Some(target) = phone_ms_at(now) else { return fail(app, now) };
            // The chip counts whole seconds: round to the nearest.
            let Some(record) = clock_record::write((target + 500).div_euclid(1000), &conventions) else { return fail(app, now) };
            // SAFETY: the context running the app only; task 4 reads it once it runs the command.
            let record_ptr = unsafe { (*RECORD.0.get()).as_mut_ptr() };
            unsafe { ptr::write_volatile(record_ptr, record) };
            give_task_4(stock::COMMAND_SET_CLOCK, record_ptr as usize);
            go(SETTING, now);
        }
        SETTING if waited > 0 && !task_4_busy() => {
            give_task_4(stock::COMMAND_READ_CLOCK, 0);
            go(CHECKING, now);
        }
        CHECKING if waited > 0 && !task_4_busy() => {
            LAST_READ.store(now, Relaxed);
            *x8() = read_record(now);
            let (Some(x8), Some(phone)) = (x8_seconds_at(now), phone_ms_at(now)) else { return fail(app, now) };
            if (x8 - phone.div_euclid(1000)).abs() > SET_WITHIN_S {
                return fail(app, now);
            }
            SET_WANTED.store(false, Relaxed);
            SET_TZ.store(PHONE_TZ.load(Relaxed), Relaxed);
            SET_FROM_PHONE.store(true, Relaxed);
            app.set_clock_state(ClockState::Set(x8));
            go(IDLE, now);
        }
        _ => {}
    }
    app.set_host_clock(x8_seconds_at(now));
}
