//! SETTINGS > Tests > Backlight off when dimmed: on the bat screens, the patch writes PWM value 0
//! to the backlight after the stock BACKLIGHT timer has dimmed it.
//!
//! The stock timer dims the light with `FUN_80da95b4(0)`, which writes PWM value 1 and stores level
//! 0 (work/notes/backlight.md). `FUN_80da95b4` writes only when a new level differs from the stored
//! one. The patch writes PWM 0 through the FPGA driver function that `FUN_80da95b4` calls, and
//! leaves the stored level at 0. A stock wake (touch, button, wheel turn) asks for the BRIGHTNESS
//! level, 1 to 3, so `FUN_80da95b4` writes it and the patch has nothing to undo. Whether the FPGA
//! turns the LEDs fully off at 0 is not known; the switch stays on the Tests page until a unit
//! shows whether it does.
//!
//! The patch writes 0 only while BACKLIGHT is set to 5 to 30 s. With OFF a stock wake leaves the
//! light at the dim level, so a 0 would stay dark; with ALWAYS the timer never dims. When the bat
//! screen closes or a pop-up opens over it, the PWM value goes back to 1 (the stock dim), so the
//! recorder's own screens do not stay dark.

use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering::Acquire, Ordering::Relaxed};

use super::state::BAT_ACTIVE;
use super::{read_byte, stock};

/// The switch, from the DSP task's copy of the app's setting.
pub(super) static SWITCH_ON: AtomicBool = AtomicBool::new(false);

/// Write `pwm` to the backlight (FPGA register 0x12) through the driver entry that
/// `FUN_80da95b4` calls, leaving its remembered level alone. From a task: the driver takes the
/// SPI bus semaphore.
fn write_pwm(pwm: u32) {
    // SAFETY: the FPGA driver table's backlight entry, read as `FUN_80da95b4` reads it, and
    // called with one byte as it calls it.
    let write: unsafe extern "C" fn(u32) = unsafe {
        let entry = ptr::read_volatile((stock::FPGA_DRIVER + stock::FPGA_BACKLIGHT_ENTRY) as *const usize);
        core::mem::transmute(entry)
    };
    unsafe { write(pwm) };
}

/// The BRIGHTNESS level, through `FUN_80da95b4` with its force flag set, so it is written even if
/// the remembered level already matches. As a stock wake does when BACKLIGHT is a time.
fn relight() {
    // SAFETY: the force flag beside the remembered level, then the stock function from a task.
    unsafe { ptr::write_volatile(stock::BACKLIGHT_FORCE, 1) };
    let set: unsafe extern "C" fn(u32) = unsafe { core::mem::transmute(stock::SET_BACKLIGHT_LEVEL) };
    unsafe { set(read_byte(stock::BRIGHTNESS) as u32) };
}

fn dimmed() -> bool {
    read_byte(stock::BACKLIGHT_DIMMED) != 0
}

/// Task 1, for each message while the launcher or an app is on screen. Task 1 also runs the
/// button and wheel wakes, after this, so only a touch (task 9) can wake between the check and
/// the write; the check after the write catches that.
pub(super) fn follow() {
    // SAFETY: the BACKLIGHT setting, a halfword in the settings record.
    let timed = unsafe { ptr::read_volatile(stock::BACKLIGHT_TIME) } > 0;
    let dark = read_byte(stock::BACKLIGHT_PWM) == 0;
    if BAT_ACTIVE.load(Acquire) && SWITCH_ON.load(Relaxed) && timed && dimmed() {
        if !dark {
            write_pwm(0);
            if !dimmed() {
                relight();
            }
        }
    } else if dark {
        restore();
    }
}

/// Back to what the stock firmware would show: the stock dim while its timer has the light
/// dimmed, otherwise the BRIGHTNESS level. From task 1 or the DSP task (a pop-up).
pub(super) fn restore() {
    if read_byte(stock::BACKLIGHT_PWM) != 0 {
        return;
    }
    if dimmed() { write_pwm(stock::PWM_DIM) } else { relight() }
}
