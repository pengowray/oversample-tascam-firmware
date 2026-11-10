//! Where each part of the 320 × 480 screen is, top to bottom.
//!
//! The order follows the stock recording apps: title bar, status bar with the
//! recording time, input meters, then the app's own content, then buttons
//! along the bottom where the stock apps put their tabs.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::{AutoVolume, Mode, Setting, VIEW_HI_HZ, VIEW_LO_HZ};

pub const TITLE_H: i32 = 24;

/// Status bar: recording state, recorded time and clock.
pub const STATUS: Rectangle = Rectangle::new(Point::new(6, 28), Size::new(308, 32));

/// BAT PLAYBACK's player, in the status bar's place: play or pause on the left, then the time
/// played, the seek bar, and the file's length on the right. The seek bar takes touches over the
/// whole height of the bar, 32 px.
pub const PLAY_BUTTON: Rectangle = Rectangle::new(Point::new(6, 28), Size::new(40, 32));
pub const POSITION_RIGHT: i32 = 92;
pub const SEEK_X0: i32 = 100;
pub const SEEK_X1: i32 = 254;
pub const SEEK: Rectangle = Rectangle::new(Point::new(SEEK_X0 - 6, 28), Size::new((SEEK_X1 - SEEK_X0 + 12) as u32, 32));
/// The title bar's middle: in BAT PLAYBACK it names the file, and a tap opens the list.
pub const TITLE_MIDDLE: Rectangle = Rectangle::new(Point::new(44, 0), Size::new(232, TITLE_H as u32));

/// BAT PLAYBACK's list of recordings: a line saying how many, then one row per file, two lines
/// each, and Back at the bottom.
pub const LIST_HEADING_BASELINE: i32 = 42;
pub const LIST_TOP: i32 = 50;
pub const LIST_ROW_H: i32 = 47;
pub const LIST_ROWS: usize = 8;
pub const LIST: Rectangle =
    Rectangle::new(Point::new(0, LIST_TOP), Size::new(320, (LIST_ROW_H * LIST_ROWS as i32) as u32));
/// The folder choice's Back, the width of the screen.
pub const LIST_BACK: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(308, BUTTON_H as u32));
/// The list of recordings' Back, and the button that names the folder listed and opens the
/// folder choice.
pub const FILES_BACK: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(100, BUTTON_H as u32));
pub const FILES_FOLDER: Rectangle = Rectangle::new(Point::new(112, BUTTONS_Y), Size::new(202, BUTTON_H as u32));

/// The title bar's information button, between the title and the battery, shown in BAT PLAYBACK
/// once a file is chosen: it opens that file's information. The title then has the room from
/// the menu to this button, centred.
pub const TITLE_INFO: Rectangle = Rectangle::new(Point::new(250, 0), Size::new(34, TITLE_H as u32));
pub const TITLE_INFO_CENTRE: Point = Point::new(266, 12);

/// The file information view: rows of a label and a value, scrolled by dragging or the wheel,
/// then Back, and Play when the view was opened from the list for a file that can be played.
pub const INFO_TOP: i32 = 30;
pub const INFO_ROW_H: i32 = 20;
pub const INFO_ROWS: usize = 19;
pub const INFO_ROWS_AREA: Rectangle =
    Rectangle::new(Point::new(0, INFO_TOP), Size::new(320, (INFO_ROW_H * INFO_ROWS as i32) as u32));
/// Where values start, after the labels.
pub const INFO_VALUE_X: i32 = 116;
pub const INFO_PLAY: Rectangle = FILES_FOLDER;

/// The file information view's Back: beside Play, or the width of the screen.
pub fn info_back(play: bool) -> Rectangle {
    if play { FILES_BACK } else { LIST_BACK }
}

/// The fraction of the file at screen position `x` on the seek bar.
pub fn seek_fraction(x: i32) -> f32 {
    ((x - SEEK_X0) as f32 / (SEEK_X1 - SEEK_X0) as f32).clamp(0.0, 1.0)
}

/// The list row at screen height `y`, counting from the first row shown.
pub fn list_row_at(y: i32) -> Option<usize> {
    (LIST_TOP..LIST_TOP + LIST_ROW_H * LIST_ROWS as i32).contains(&y).then(|| ((y - LIST_TOP) / LIST_ROW_H) as usize)
}

/// Meters for the two inputs, from -72 to 0 dBFS. The stock meters stop at
/// -48 dBFS, but 32-bit float bat recordings often peak below that.
pub const METER_X: i32 = 22;
pub const METER_W: i32 = 268;
pub const METER_Y: [i32; 2] = [66, 76];
pub const METER_H: i32 = 6;
pub const METER_SCALE_BASELINE: i32 = 92;
pub const METER_MIN_DB: f32 = -72.0;

/// Waterfall: time runs right to left, newest at the right edge, one pixel
/// column per 10 ms (2.9 s across). Frequency runs up, 15 to 96 kHz, about
/// one 375 Hz FFT bin (384 Hz) per pixel row.
pub const WF_X: i32 = 28;
pub const WF_Y: i32 = 98;
pub const WF_W: usize = 288;
pub const WF_H: usize = 211;
/// Strip under the waterfall for the ticks of marked sounds, on black, so
/// every tick colour shows whatever the waterfall's colours are.
pub const TICKS_Y: i32 = WF_Y + WF_H as i32 + 1;
pub const TICK_H: i32 = 6;
/// Time labels under the waterfall.
pub const RULER_BASELINE: i32 = 328;
/// A small notice in the top left corner of the waterfall: text in the BODY font, on a
/// panel 2 px inside the waterfall's edges, so it stays clear of the frequency labels at
/// the left and the time labels below.
pub const CORNER_NOTICE_PAD_X: i32 = 5;
pub const CORNER_NOTICE_BASELINE: i32 = 13;
pub const CORNER_NOTICE_LINE: i32 = 15;

/// The panel of a corner notice with this text width and number of lines.
pub fn corner_notice(text_width: i32, lines: usize) -> Rectangle {
    Rectangle::new(
        Point::new(WF_X + 2, WF_Y + 2),
        Size::new((text_width + 2 * CORNER_NOTICE_PAD_X) as u32, (lines as i32 * CORNER_NOTICE_LINE + 3) as u32),
    )
}

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

/// Mode buttons along the bottom, 46 px high with 3 px gaps, centred in the
/// same 6..314 px as the panels above them: 59 px wide for five, 74 for four.
pub const BUTTONS_Y: i32 = 430;
pub const BUTTON_H: i32 = 46;
const BUTTONS_X: i32 = 6;
const BUTTONS_W: i32 = 308;
const BUTTON_GAP: i32 = 3;

/// Button `i` of `count`.
pub fn mode_button(i: usize, count: usize) -> Rectangle {
    let n = count.max(1) as i32;
    let w = (BUTTONS_W - BUTTON_GAP * (n - 1)) / n;
    let x0 = BUTTONS_X + (BUTTONS_W - (n * w + (n - 1) * BUTTON_GAP)) / 2;
    Rectangle::new(Point::new(x0 + i as i32 * (w + BUTTON_GAP), BUTTONS_Y), Size::new(w as u32, BUTTON_H as u32))
}

pub fn mode_button_at(x: i32, y: i32, modes: &[Mode]) -> Option<Mode> {
    let p = Point::new(x, y);
    (0..modes.len()).find(|&i| mode_button(i, modes.len()).contains(p)).map(|i| modes[i])
}

/// Menu button at the left of the title bar, as on the stock apps: opens the
/// settings screen.
pub const MENU: Rectangle = Rectangle::new(Point::new(0, 0), Size::new(40, TITLE_H as u32));

/// Settings screen: three pages, picked with tabs under the title bar. Listen has what changes
/// the sound and the marks; Record has what a take's files get and the phone remote; Tests has the
/// debug figures and the two tests. Below the tabs, Listen has its two stepped settings, one panel
/// each, with the name and value on the left, - and + buttons on the right, and a line saying what
/// the setting does; then the Auto volume row, with its three choices; then, under a heading, the
/// sounds to mark, one compact row each, with a colour swatch, a name and a line of help on the left
/// and the switch on the right. Record and Tests have panels with a name, a switch or a button, and
/// up to three lines of help. Every page ends with the note that the settings reset at power-off
/// and the Done button.
pub const TABS_TOP: i32 = 29;
pub const TAB_H: i32 = 32;
const TAB_GAP: i32 = 4;
/// Where the pages start, under the tabs.
pub const PAGE_TOP: i32 = 67;
pub const SETTING_H: i32 = 66;
const SETTING_GAP: i32 = 4;
const STEP_W: u32 = 48;
const STEP_H: u32 = 40;
/// The Auto volume row, under the two stepped settings. Its choices need more height than a name
/// and a help line.
const AUTO_VOLUME_ROW: Rectangle = Rectangle::new(Point::new(6, 208), Size::new(308, 38));
/// Baselines of the marks' heading and its help line.
pub const SWITCHES_HEADING: i32 = 260;
pub const SWITCHES_HELP: i32 = 272;
const SWITCHES_TOP: i32 = 276;
pub const SWITCH_ROW_H: i32 = 33;
const SWITCH_ROW_GAP: i32 = 2;
const SWITCH_W: u32 = 44;
const SWITCH_H: u32 = 24;
/// Baseline of the note above Done.
pub const SETTINGS_NOTE: i32 = 426;
/// Closes the settings screen, where the mode buttons are on the main screen.
pub const DONE: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(308, BUTTON_H as u32));
/// The clock screen's buttons: Back, and setting the X8's clock from the latest chime.
pub const CLOCK_BACK: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(90, BUTTON_H as u32));
pub const CLOCK_SET: Rectangle = Rectangle::new(Point::new(102, BUTTONS_Y), Size::new(212, BUTTON_H as u32));

/// Tab `i` of `count`, left to right.
pub fn tab(i: usize, count: usize) -> Rectangle {
    let n = count.max(1) as i32;
    let w = (308 - TAB_GAP * (n - 1)) / n;
    Rectangle::new(Point::new(6 + i as i32 * (w + TAB_GAP), TABS_TOP), Size::new(w as u32, TAB_H as u32))
}

/// A panel on the Record or Tests page: the name on its first line with a switch at the right,
/// then help lines. The top 40 px are what a tap on a switch's panel hits, so the switch flips
/// from anywhere on its name's line.
const fn panel(top: i32, height: u32) -> Rectangle {
    Rectangle::new(Point::new(6, top), Size::new(308, height))
}

/// The top of a panel, where a tap flips its switch.
pub const fn panel_switch_row(panel: Rectangle) -> Rectangle {
    Rectangle::new(panel.top_left, Size::new(panel.size.width, 40))
}

/// The switch at the right of a panel's first line.
pub const fn panel_switch(panel: Rectangle) -> Rectangle {
    Rectangle::new(Point::new(260, panel.top_left.y + 8), Size::new(SWITCH_W, SWITCH_H))
}

/// Record page: the MIX file, GUANO metadata, then whether REC pauses takes. A panel is 48 px with
/// one help line, 14 px more for each line after; the gaps are 6 px, as small as keeps the panels
/// apart.
pub const RECORD_MIX_FILE: Rectangle = panel(PAGE_TOP, 48);
pub const RECORD_GUANO: Rectangle = panel(PAGE_TOP + 54, 62);
pub const RECORD_PAUSE: Rectangle = panel(PAGE_TOP + 122, 48);
/// Phone page: the phone remote with its status lines, its clock switch, then the phone page's QR
/// code with the address and the Phone time button beside it. The remote's switch is what people
/// change most, so it is at the top; the QR code is needed once for each phone.
pub const PHONE_REMOTE: Rectangle = panel(PAGE_TOP, 124);
/// Under the phone remote, because only its chimes are heard on the main screen.
pub const PHONE_CLOCK: Rectangle = panel(PAGE_TOP + 130, 48);
/// Size of one module of the QR code, in pixels. At 5 px the code is 125 px, about 19 mm on the
/// X8's screen, which a phone reads from 20 cm.
pub const QR_MODULE: i32 = 5;
/// The white margin round the code, in modules. The QR standard asks for 4; 3 leaves a gap above
/// the settings note, and phone cameras read codes with less. It is white because readers need a
/// light margin, which the dark background is not.
pub const QR_MARGIN: i32 = 3;
const QR_CARD_SIDE: u32 = ((crate::qr::SIZE as i32 + 2 * QR_MARGIN) * QR_MODULE) as u32;
/// The white card of the QR code: under the remote's panels, or at the top of the page on a host
/// without the phone remote (BAT PLAYBACK), so the page has no gap.
pub const fn qr_card(remote_shown: bool) -> Rectangle {
    let top = if remote_shown { PAGE_TOP + 184 } else { PAGE_TOP };
    Rectangle::new(Point::new(6, top), Size::new(QR_CARD_SIDE, QR_CARD_SIDE))
}
/// The address and its help, right of the code.
pub const QR_TEXT_X: i32 = 6 + QR_CARD_SIDE as i32 + 10;
/// Opens the chime test, right of the code and level with its bottom.
pub const fn phone_time(remote_shown: bool) -> Rectangle {
    let card = qr_card(remote_shown);
    Rectangle::new(
        Point::new(QR_TEXT_X - 4, card.top_left.y + QR_CARD_SIDE as i32 - 48),
        Size::new((314 - (QR_TEXT_X - 4)) as u32, 48),
    )
}
/// Tests page: the debug figures' switch, then the CPU test's button, with its help inside.
pub const TESTS_DEBUG: Rectangle = panel(PAGE_TOP, 48);
pub const TESTS_CPU_TEST: Rectangle = panel(PAGE_TOP + 56, 62);

/// Panel of stepped setting `i`.
pub fn setting_row(i: usize) -> Rectangle {
    Rectangle::new(Point::new(6, PAGE_TOP + i as i32 * (SETTING_H + SETTING_GAP)), Size::new(308, SETTING_H as u32))
}

pub fn setting_minus(i: usize) -> Rectangle {
    let r = setting_row(i);
    Rectangle::new(Point::new(204, r.top_left.y + 7), Size::new(STEP_W, STEP_H))
}

pub fn setting_plus(i: usize) -> Rectangle {
    let r = setting_row(i);
    Rectangle::new(Point::new(258, r.top_left.y + 7), Size::new(STEP_W, STEP_H))
}

/// Row of a setting with a control at the right: Auto volume, a sound to mark (`mark` counts
/// them from 0, top to bottom), the MIX file or the debug figures. A tap anywhere in it acts.
pub fn control_row(setting: Setting) -> Rectangle {
    match setting {
        Setting::AutoLevel => AUTO_VOLUME_ROW,
        Setting::MixFile => panel_switch_row(RECORD_MIX_FILE),
        Setting::ClockFromRemote => panel_switch_row(PHONE_CLOCK),
        Setting::PauseWithRec => panel_switch_row(RECORD_PAUSE),
        Setting::DebugInfo => panel_switch_row(TESTS_DEBUG),
        Setting::Mark(kind) => {
            let j = Setting::MARKS.iter().position(|&k| k == kind).unwrap_or(0) as i32;
            Rectangle::new(
                Point::new(6, SWITCHES_TOP + j * (SWITCH_ROW_H + SWITCH_ROW_GAP)),
                Size::new(308, SWITCH_ROW_H as u32),
            )
        }
        Setting::LowestBatCall => setting_row(0),
        Setting::ListenTo => setting_row(1),
    }
}

/// The three choices of the Auto volume row, side by side under its name, in the order of
/// `AutoVolume::ALL`, across the row's width. A tap anywhere in the row's height picks the choice
/// at that x, so each target is 38 px high, and any choice is one tap away.
const AUTO_VOLUME_Y: i32 = 18;
const AUTO_VOLUME_H: u32 = 16;
const AUTO_VOLUME_GAP: i32 = 2;
const AUTO_VOLUME_W: [i32; 3] = [88, 88, 116];

/// Choice `i` of the Auto volume row.
pub fn auto_volume_choice(i: usize) -> Rectangle {
    let r = AUTO_VOLUME_ROW;
    let x = r.top_left.x + 6 + AUTO_VOLUME_W[..i].iter().map(|w| w + AUTO_VOLUME_GAP).sum::<i32>();
    Rectangle::new(Point::new(x, r.top_left.y + AUTO_VOLUME_Y), Size::new(AUTO_VOLUME_W[i] as u32, AUTO_VOLUME_H))
}

/// The choice a tap at `x` picks in the Auto volume row. The row's margins belong to the nearest
/// choice.
pub fn auto_volume_at(x: i32) -> AutoVolume {
    AutoVolume::ALL
        .iter()
        .enumerate()
        .find_map(|(i, &volume)| {
            let c = auto_volume_choice(i);
            (x < c.top_left.x + c.size.width as i32).then_some(volume)
        })
        .unwrap_or(AutoVolume::BatsOnly)
}

/// The switch of a setting's row, at its right.
pub fn switch(setting: Setting) -> Rectangle {
    match setting {
        Setting::MixFile => panel_switch(RECORD_MIX_FILE),
        Setting::ClockFromRemote => panel_switch(PHONE_CLOCK),
        Setting::PauseWithRec => panel_switch(RECORD_PAUSE),
        Setting::DebugInfo => panel_switch(TESTS_DEBUG),
        _ => {
            let r = control_row(setting);
            let y = r.top_left.y + (r.size.height as i32 - SWITCH_H as i32) / 2;
            Rectangle::new(
                Point::new(r.top_left.x + r.size.width as i32 - 10 - SWITCH_W as i32, y),
                Size::new(SWITCH_W, SWITCH_H),
            )
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsHit {
    /// A stepped setting's panel, away from its buttons.
    Row(Setting),
    Minus(Setting),
    Plus(Setting),
    /// Anywhere on a control's row: flips a switch, or picks an Auto volume choice.
    Control(Setting),
    Done,
}

/// What a tap at (x, y) hits among `settings`, the settings on the page shown, and Done. Tabs and
/// the page's own buttons are not settings and are checked by the caller.
pub fn settings_hit(x: i32, y: i32, settings: impl Iterator<Item = Setting>) -> Option<SettingsHit> {
    let p = Point::new(x, y);
    if DONE.contains(p) {
        return Some(SettingsHit::Done);
    }
    for setting in settings {
        let hit = match setting {
            Setting::LowestBatCall | Setting::ListenTo => {
                let i = if setting == Setting::LowestBatCall { 0 } else { 1 };
                if setting_minus(i).contains(p) {
                    Some(SettingsHit::Minus(setting))
                } else if setting_plus(i).contains(p) {
                    Some(SettingsHit::Plus(setting))
                } else {
                    setting_row(i).contains(p).then_some(SettingsHit::Row(setting))
                }
            }
            _ => control_row(setting).contains(p).then_some(SettingsHit::Control(setting)),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
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

/// CPU test screen. Row pitch and baselines are as small as the text allows, because with two
/// inputs on the table has 26 rows and 4 headings.
pub const CPU_TEST_SUMMARY: i32 = 37;
pub const CPU_TEST_GROUPS: i32 = 49;
pub const CPU_TEST_COLUMNS: i32 = 60;
pub const CPU_TEST_FIRST_ROW: i32 = 72;
pub const CPU_TEST_ROW_H: i32 = 11;
/// The test calls' switch, under the table. The whole row is the target.
pub const CPU_TEST_CALLS: Rectangle = Rectangle::new(Point::new(6, 398), Size::new(308, 30));
pub const CPU_TEST_CALLS_SWITCH: Rectangle = Rectangle::new(Point::new(264, 403), Size::new(40, 20));
pub const CPU_TEST_BACK: Rectangle = Rectangle::new(Point::new(6, BUTTONS_Y), Size::new(90, BUTTON_H as u32));
/// Switches between the Totals and Stages tables, when the host lends the app a clock.
pub const CPU_TEST_VIEW: Rectangle = Rectangle::new(Point::new(102, BUTTONS_Y), Size::new(96, BUTTON_H as u32));
pub const CPU_TEST_START: Rectangle = Rectangle::new(Point::new(204, BUTTONS_Y), Size::new(110, BUTTON_H as u32));
