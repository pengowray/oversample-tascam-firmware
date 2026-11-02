//! Where each part of the 320 × 480 screen is, top to bottom.
//!
//! The order follows the stock recording apps: title bar, status bar with the
//! recording time, input meters, then the app's own content, then buttons
//! along the bottom where the stock apps put their tabs.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::{Mode, VIEW_HI_HZ, VIEW_LO_HZ};

pub const TITLE_H: i32 = 24;

/// Status bar: recording state, recorded time and clock.
pub const STATUS: Rectangle = Rectangle::new(Point::new(6, 28), Size::new(308, 32));

/// Meters for inputs 3 and 4, from -72 to 0 dBFS. The stock meters stop at
/// -48 dBFS, but 32-bit float bat recordings often peak below that.
pub const METER_X: i32 = 22;
pub const METER_W: i32 = 268;
pub const METER_Y: [i32; 2] = [66, 76];
pub const METER_H: i32 = 6;
pub const METER_SCALE_BASELINE: i32 = 92;
pub const METER_MIN_DB: f32 = -72.0;

/// Waterfall: time runs right to left, newest at the right edge, one pixel
/// column per 10 ms (2.9 s across). Frequency runs up, 15 to 96 kHz, about
/// one 375 Hz FFT bin per pixel row.
pub const WF_X: i32 = 28;
pub const WF_Y: i32 = 98;
pub const WF_W: usize = 288;
pub const WF_H: usize = 218;
/// Time labels under the waterfall.
pub const RULER_BASELINE: i32 = 328;

/// Last-call panel: a zoomed spectrogram of the call and its measurements.
pub const SNAP_PANEL: Rectangle = Rectangle::new(Point::new(6, 334), Size::new(308, 64));
pub const SNAP_X: i32 = 8;
pub const SNAP_Y: i32 = 336;
pub const SNAP_W: usize = 150;
pub const SNAP_H: usize = 60;
pub const SNAP_TEXT_X: i32 = 168;
pub const SNAP_VALUE_X: i32 = 222;

/// The current mode's setting.
pub const VALUE_BASELINE: i32 = 420;

/// Mode buttons along the bottom: 59 × 46 px each, spanning the same
/// 6..314 px as the panels above them.
pub const BUTTONS_Y: i32 = 430;
pub const BUTTON_W: i32 = 59;
pub const BUTTON_H: i32 = 46;
const BUTTON_X0: i32 = 6;
const BUTTON_PITCH: i32 = 62;

pub fn mode_button(i: usize) -> Rectangle {
    Rectangle::new(
        Point::new(BUTTON_X0 + i as i32 * BUTTON_PITCH, BUTTONS_Y),
        Size::new(BUTTON_W as u32, BUTTON_H as u32),
    )
}

pub fn mode_button_at(x: i32, y: i32) -> Option<Mode> {
    let p = Point::new(x, y);
    (0..Mode::ALL.len()).find(|&i| mode_button(i).contains(p)).map(|i| Mode::ALL[i])
}

/// Menu button at the left of the title bar, as on the stock apps: opens the
/// settings screen.
pub const MENU: Rectangle = Rectangle::new(Point::new(0, 0), Size::new(40, TITLE_H as u32));

/// Settings screen: one panel per setting, with its name and value on the
/// left, - and + buttons on the right, and a line saying what it does.
pub const SETTINGS_TOP: i32 = 34;
pub const SETTING_H: i32 = 76;
const SETTING_GAP: i32 = 8;
const STEP_W: u32 = 48;
const STEP_H: u32 = 40;
/// Closes the settings screen, where the mode buttons are on the main screen.
pub const DONE: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(308, BUTTON_H as u32));

pub fn setting_row(i: usize) -> Rectangle {
    Rectangle::new(Point::new(6, SETTINGS_TOP + i as i32 * (SETTING_H + SETTING_GAP)), Size::new(308, SETTING_H as u32))
}

pub fn setting_minus(i: usize) -> Rectangle {
    let r = setting_row(i);
    Rectangle::new(Point::new(204, r.top_left.y + 10), Size::new(STEP_W, STEP_H))
}

pub fn setting_plus(i: usize) -> Rectangle {
    let r = setting_row(i);
    Rectangle::new(Point::new(258, r.top_left.y + 10), Size::new(STEP_W, STEP_H))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsHit {
    Row(usize),
    Minus(usize),
    Plus(usize),
    Done,
}

pub fn settings_hit(x: i32, y: i32, rows: usize) -> Option<SettingsHit> {
    let p = Point::new(x, y);
    if DONE.contains(p) {
        return Some(SettingsHit::Done);
    }
    (0..rows).find_map(|i| {
        if setting_minus(i).contains(p) {
            Some(SettingsHit::Minus(i))
        } else if setting_plus(i).contains(p) {
            Some(SettingsHit::Plus(i))
        } else if setting_row(i).contains(p) {
            Some(SettingsHit::Row(i))
        } else {
            None
        }
    })
}

pub fn waterfall() -> Rectangle {
    Rectangle::new(Point::new(WF_X, WF_Y), Size::new(WF_W as u32, WF_H as u32))
}

/// Pixel row of a frequency in a view `top` .. `top + height`, high at the top.
pub fn freq_to_y(hz: f32, top: i32, height: usize) -> i32 {
    let t = (VIEW_HI_HZ - hz) / (VIEW_HI_HZ - VIEW_LO_HZ);
    top + libm::roundf(t * height as f32) as i32
}

/// Frequency under a tap on the waterfall, if the tap is on it.
pub fn waterfall_freq_at(x: i32, y: i32) -> Option<f32> {
    if !waterfall().contains(Point::new(x, y)) {
        return None;
    }
    let t = (y - WF_Y) as f32 / WF_H as f32;
    Some(VIEW_HI_HZ - t * (VIEW_HI_HZ - VIEW_LO_HZ))
}
