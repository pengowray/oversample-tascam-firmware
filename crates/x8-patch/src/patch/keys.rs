use core::ptr;
use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::{BatApp, Button};

use super::audio::install_audio_hook;
use super::screen::{draw_status_strip, enter_work, follow_app, leave_bat_screen, request_enter};
use super::state::{
    ACTION_HOME, ACTION_MARK, ACTION_PLAY, ACTION_WHEEL, ACTIONS, ACTIONS_READ, ACTIONS_WRITE, AUDIO_INSTALLED,
    AUDIO_NOT_TRIED, AUDIO_STATE, BAT_ACTIVE, BLOCKS, CALLS, CALLS_AT_SECOND, CALLS_PER_S, DRAGGING, LAST_EVENT,
    LAST_SECOND, PENDING, PLAYBACK, QT_PRESSED, SCREEN_APP, TOUCH_DOWN, TOUCH_DOWN_AT, TOUCH_HELD, TOUCH_READ,
    TOUCH_WRITE, TOUCHED_SINCE_RENDER, TOUCHES, WORK_ENTER,
};
use super::task::{on_patch_stack, task_running, tick_work};
use super::{Handler, app_byte, blocks_per_second, read_byte, stock, system_mode};

/// Which stock key handler a message goes on to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Via {
    /// The launcher and app screens' handler.
    App,
    /// The recording screen's handler (record standby and takes).
    Rec,
}

/// Called by task 1 for every message while the launcher or an app is on
/// screen, with the stock handler's arguments; the first is the message.
pub(super) unsafe extern "C" fn key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as the stock handler is called.
    unsafe { on_message(msg, Via::App) };
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
}

/// The same, while the recording screen's handler is installed.
pub(super) unsafe extern "C" fn rec_key_hook(msg: usize, b: usize, c: usize, d: usize) -> usize {
    // SAFETY: as the stock handler is called.
    unsafe { on_message(msg, Via::Rec) };
    // SAFETY: the stock handler this hook replaces.
    let stock_handler: Handler = unsafe { core::mem::transmute(stock::REC_KEY_HANDLER) };
    unsafe { stock_handler(msg, b, c, d) }
}

/// Replaces the touch poll's call to Qt (task 9, on every change of x, y or
/// pressed). While the bat screen is up, touches go to the bat app instead,
/// except the release of a touch Qt saw start, so Qt never keeps a press.
pub(super) unsafe extern "C" fn touch_hook(x: usize, y: usize, pressed: usize, d: usize) -> usize {
    // SAFETY: the stock function the poll called, with its arguments.
    let qt: Handler = unsafe { core::mem::transmute(stock::QT_TOUCH) };
    let event = ((pressed != 0) as u32) << 31 | (x as u32 & 0x7fff) << 16 | (y as u32 & 0xffff);
    if system_mode() > 1 || !BAT_ACTIVE.load(Acquire) {
        QT_PRESSED.store(pressed != 0, Relaxed);
        return unsafe { qt(x, y, pressed, d) };
    }
    if pressed == 0 && QT_PRESSED.load(Relaxed) {
        QT_PRESSED.store(false, Relaxed);
        unsafe { qt(x, y, 0, d) };
    }
    let write = TOUCH_WRITE.load(Relaxed);
    if write.wrapping_sub(TOUCH_READ.load(Acquire)) < TOUCHES.len() as u32 {
        TOUCHES[write as usize % TOUCHES.len()].store(event, Relaxed);
        TOUCH_WRITE.store(write.wrapping_add(1), Release);
    }
    0
}

/// Apply the queued touches (the context running the app): a touch down is a
/// tap; moves while down on the waterfall tune HET and COMB; a finger still down
/// after `bat_app::LONG_PRESS_MS` is a long press.
pub(super) fn apply_touches(app: &mut BatApp<'static>) {
    let mut read = TOUCH_READ.load(Relaxed);
    while read != TOUCH_WRITE.load(Acquire) {
        let event = TOUCHES[read as usize % TOUCHES.len()].load(Relaxed);
        let (x, y) = ((event >> 16 & 0x7fff) as i32, (event & 0xffff) as i32);
        if event >> 31 == 0 {
            // The finger lifted: a list row is chosen, or the seek bar moves, only now.
            if TOUCH_DOWN.swap(false, Relaxed) {
                app.release();
            }
        } else if !TOUCH_DOWN.load(Relaxed) {
            TOUCH_DOWN.store(true, Relaxed);
            TOUCH_DOWN_AT.store(BLOCKS.load(Relaxed), Relaxed);
            TOUCH_HELD.store(false, Relaxed);
            DRAGGING.store(app.is_draggable(x, y), Relaxed);
            app.touch(x, y);
        } else if DRAGGING.load(Relaxed) {
            app.drag(x, y);
        }
        read = read.wrapping_add(1);
        TOUCH_READ.store(read, Release);
        TOUCHED_SINCE_RENDER.store(true, Relaxed);
    }
    // The touch poll reports only changes, so the time held is checked on every pass.
    let held = BLOCKS.load(Relaxed).wrapping_sub(TOUCH_DOWN_AT.load(Relaxed));
    if TOUCH_DOWN.load(Relaxed)
        && !TOUCH_HELD.load(Relaxed)
        && held >= blocks_per_second() / 1000 * bat_app::LONG_PRESS_MS
    {
        TOUCH_HELD.store(true, Relaxed);
        app.hold();
    }
}

/// Task 1, for each message, from either key hook.
unsafe fn on_message(msg: usize, via: Via) {
    if system_mode() > 1 {
        return;
    }
    if AUDIO_STATE.load(Relaxed) == AUDIO_NOT_TRIED {
        install_audio_hook();
    }
    count_call();
    if AUDIO_STATE.load(Relaxed) == AUDIO_INSTALLED {
        follow_app();
    }
    // SAFETY: the message copy on task 1's stack, 0x14 bytes.
    unsafe { handle_message(msg, via) };
    let work = PENDING.swap(0, Relaxed);
    if work == WORK_ENTER && !BAT_ACTIVE.load(Relaxed) {
        // SAFETY: only task 1 runs on the patch stack.
        unsafe { on_patch_stack(enter_work) };
    } else if BAT_ACTIVE.load(Relaxed) {
        if !task_running() {
            // SAFETY: as above.
            unsafe { on_patch_stack(tick_work) };
        }
    } else if via == Via::App {
        draw_status_strip();
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

unsafe fn handle_message(msg: usize, via: Via) {
    let kind_ptr = (msg + 4) as *mut u16;
    let value_ptr = (msg + 8) as *mut i32;
    // SAFETY: fields of the message copy (see key_hook).
    let (category, kind, value) =
        unsafe { (ptr::read_volatile(msg as *const u16), ptr::read_volatile(kind_ptr), ptr::read_volatile(value_ptr)) };
    if category != 0 || kind == stock::KIND_SWALLOWED {
        return;
    }
    LAST_EVENT.store((kind as u32) << 16 | (value as u32 & 0xffff), Relaxed);
    super::power::on_event(kind_ptr, kind, value);
    if AUDIO_STATE.load(Relaxed) != AUDIO_INSTALLED {
        return;
    }
    let active = BAT_ACTIVE.load(Relaxed);
    let swallow_button = || unsafe { ptr::write_volatile(value_ptr, stock::NO_KEY) };
    match (kind, value) {
        // A mark the patch posted where a pause ended: for the recorder only.
        (stock::KIND_BUTTON, stock::MARK) if super::pauses::OWN_MARKS.load(Relaxed) > 0 => {
            super::pauses::OWN_MARKS.fetch_sub(1, Relaxed);
        }
        (stock::KIND_BUTTON, stock::REC) if via == Via::Rec && super::pauses::keep_rec_from_pausing() => {
            swallow_button();
            super::pauses::REFUSED.store(true, Relaxed);
        }
        (stock::KIND_BUTTON, stock::MARK) if active => {
            // A mark in the bat app (in TE, a capture). During a take MARK also
            // goes on to the recording screen's handler, which marks the file.
            // Otherwise it is kept from the app: in MANUAL without a file,
            // MARK opens a pop-up.
            queue_action(ACTION_MARK, 0);
            if !(via == Via::Rec && matches!(read_byte(stock::TRANSPORT), 3 | 4)) {
                swallow_button();
            }
        }
        // BAT PLAYBACK plays files itself, and reads the card while it does, so MANUAL must
        // not record: REC is kept from it.
        (stock::KIND_BUTTON, stock::REC) if PLAYBACK.load(Relaxed) && SCREEN_APP.load(Relaxed) == app_byte() as u32 => {
            swallow_button();
        }
        (stock::KIND_BUTTON, stock::PLAY) if SCREEN_APP.load(Relaxed) == app_byte() as u32 => {
            swallow_button();
            if PLAYBACK.load(Relaxed) && active {
                // BAT PLAYBACK: plays and pauses, or plays the file chosen on the list.
                queue_action(ACTION_PLAY, 0);
            } else if PLAYBACK.load(Relaxed) {
                request_enter();
            } else {
                // BAT DETECTOR: PLAY swaps between the bat screen and the
                // app's own screen; the app's own playback is not used there.
                if active { leave_bat_screen() } else { request_enter() }
            }
        }
        (stock::KIND_BUTTON, stock::HOME) if active && PLAYBACK.load(Relaxed) && app_byte() != 0 => {
            // BAT PLAYBACK: a short press opens the list of files. Holding HOME
            // sends another code, which still returns to the LAUNCHER.
            swallow_button();
            queue_action(ACTION_HOME, 0);
        }
        (stock::KIND_BUTTON, stock::HOME) if active && app_byte() == 0 => {
            // On the LAUNCHER, where holding HOME does nothing. In an app a
            // short press goes on (it stops a take or playback, as in the
            // stock apps) and holding HOME returns to the LAUNCHER.
            swallow_button();
            leave_bat_screen();
        }
        (stock::KIND_WHEEL, count) if active => {
            // SAFETY: the kind field of the message copy.
            unsafe { ptr::write_volatile(kind_ptr, stock::KIND_SWALLOWED) };
            queue_action(ACTION_WHEEL, count as u32);
        }
        _ => {}
    }
}

/// Queue a button or wheel action for the app (task 1).
fn queue_action(action: u32, value: u32) {
    let write = ACTIONS_WRITE.load(Relaxed);
    if write.wrapping_sub(ACTIONS_READ.load(Acquire)) < ACTIONS.len() as u32 {
        ACTIONS[write as usize % ACTIONS.len()].store(action << 24 | (value & 0xff_ffff), Relaxed);
        ACTIONS_WRITE.store(write.wrapping_add(1), Release);
    }
}

/// Apply the queued actions (the context running the app).
pub(super) fn apply_actions(app: &mut BatApp<'static>) {
    let mut read = ACTIONS_READ.load(Relaxed);
    while read != ACTIONS_WRITE.load(Acquire) {
        let a = ACTIONS[read as usize % ACTIONS.len()].load(Relaxed);
        let value = ((a << 8) as i32) >> 8;
        match a >> 24 {
            ACTION_MARK => app.button(Button::Mark),
            ACTION_PLAY => app.button(Button::Play),
            ACTION_HOME => app.button(Button::Home),
            ACTION_WHEEL => app.wheel(value),
            _ => {}
        }
        read = read.wrapping_add(1);
        ACTIONS_READ.store(read, Release);
        TOUCHED_SINCE_RENDER.store(true, Relaxed);
    }
}
