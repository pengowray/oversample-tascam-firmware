//! The X8's built-in speaker on the bat screens.
//!
//! The stock firmware plays through the speaker only while it plays a file: GENERAL SETTINGS >
//! SPEAKER on, no headphones, and the DSP state's playback byte set (`FUN_80da0024`). While it
//! monitors the inputs the speaker stays off, and during record standby and a take with nothing
//! plugged in it also turns the output stage off (`FUN_80d9ff82`). Two hooks change the answers
//! those two functions get while the bat app's sound is the output, so the stock code switches
//! the speaker itself, with its own order and delays. The SPEAKER setting and the headphone jack
//! work as before.
//!
//! 1:1, and HET and COMB tuned low, would play the speaker's sound back through the microphones.
//! So the app is told about the speaker first (`BatApp::set_speaker`) and is silent in those
//! cases with live input (`BatApp::speaker_muted`). The speaker amplifier is also off then, and
//! during a take with Mute while recording on.

use core::sync::atomic::{AtomicBool, Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::BatApp;

use super::state::BAT_ACTIVE;
use super::{rate_class, read_byte, stock};

/// The DSP task: the app has been told that its sound goes to the speaker, and has sound for it.
static WANTED: AtomicBool = AtomicBool::new(false);
/// The bat screen is about to close, so the speaker goes off before the stock monitoring
/// returns. Cleared when the screen opens.
static CLOSING: AtomicBool = AtomicBool::new(false);
/// What the stock functions were last run for.
static APPLIED: AtomicBool = AtomicBool::new(false);

/// Whether the speaker should play now. The audio hook writes the app's sound to the output only
/// on an open bat screen at 176.4 or 192 kHz; otherwise the output is the stock monitoring.
fn playing() -> bool {
    BAT_ACTIVE.load(Acquire) && rate_class() == 2 && WANTED.load(Acquire) && !CLOSING.load(Relaxed)
}

/// From the context running the app, each pass. The CPU test plays 1:1 from the inputs whatever
/// the app's settings say, so the speaker is off for all of it.
pub(super) fn update(app: &mut BatApp<'static>) {
    let free = read_byte(stock::SPEAKER_SETTING) != 0
        && read_byte(stock::HEADPHONES_PLUGGED) == 0
        && !app.cpu_test_running();
    app.set_speaker(free);
    WANTED.store(free && !app.speaker_muted() && !app.muted_for_take(), Release);
}

/// Task 1, as the bat screen opens.
pub(super) fn opening() {
    CLOSING.store(false, Relaxed);
}

/// Before the bat screen closes, from task 1 or, for a pop-up, from the DSP task: turn the
/// speaker off first. The stock firmware also calls these functions from more than one task
/// (`FUN_80dbe324` runs in the system task), and `FUN_80da0024` counts calls that arrive while
/// it runs and repeats itself for them.
pub(super) fn close() {
    CLOSING.store(true, Relaxed);
    follow();
}

/// Task 1, for each message, and `close`: when the answer the hooks give has changed, run the stock functions
/// that read it. They sleep (100 ms as the speaker comes on, 200 ms for the output stage), as
/// they do when the stock code calls them from this task.
pub(super) fn follow() {
    let want = playing();
    if APPLIED.swap(want, Relaxed) == want {
        return;
    }
    // SAFETY: stock functions with no arguments, called from a task as the stock code does.
    let (output_power, route): (unsafe extern "C" fn(), unsafe extern "C" fn()) =
        unsafe { (core::mem::transmute(stock::SET_OUTPUT_POWER), core::mem::transmute(stock::ROUTE_OUTPUT)) };
    // The output stage's state differs from the stock one only from record standby to a take's end.
    let in_take = read_byte(stock::TRANSPORT) != 0;
    unsafe {
        if want && in_take {
            output_power();
        }
        route();
        if !want && in_take {
            output_power();
        }
    }
}

/// Replaces `FUN_80da0024`'s read of the DSP state's playback byte, which lets the speaker play.
pub(super) unsafe extern "C" fn speaker_hook(_: usize, _: usize, _: usize, _: usize) -> usize {
    if playing() {
        return 1;
    }
    // SAFETY: the stock getter this call went to, with no arguments.
    let stock_answer: unsafe extern "C" fn() -> usize = unsafe { core::mem::transmute(stock::PLAYBACK_MONITORED) };
    unsafe { stock_answer() & 0xff }
}

/// Replaces `FUN_80d9ff82`'s read of the transport byte, which turns the output stage off from
/// record standby to a take's end when nothing is plugged in.
pub(super) unsafe extern "C" fn output_power_hook(_: usize, _: usize, _: usize, _: usize) -> usize {
    if playing() { 0 } else { read_byte(stock::TRANSPORT) as usize }
}
