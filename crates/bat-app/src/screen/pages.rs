//! The settings screen's Record, Phone and Tests pages, and the phone remote's notices on the main
//! screen.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

use super::{BODY, BODY_BOLD, Fb, LEFT, SMALL, outline_selected, rounded, switch_control, text};
use crate::layout::{
    PHONE_CLOCK, PHONE_REMOTE, QR_MARGIN, QR_MODULE, QR_TEXT_X, RECORD_GUANO, RECORD_MIX_FILE, RECORD_PAUSE,
    TESTS_CPU_TEST, TESTS_DEBUG, panel_switch, phone_time, qr_card,
};
use crate::palette::*;
use crate::{BatApp, Mode, RecorderCommand, RemoteCommand, RemoteResult, Setting, qr};

/// In the title bar of the main screen while the remote's switch is on. Muted while it waits for
/// a tone, and in the accent colour while the chime decoders run.
pub(super) const TITLE_LABEL: &str = "REMOTE";

pub(super) const REMOTE_NAME: &str = "Phone remote (experimental)";
pub(super) const REMOTE_HELP: [&str; 2] =
    ["Phone chimes set the clock, start or stop recording,", "and change the listening mode. BAT DETECTOR only."];
/// The beeps' words match the phone page's help.
pub(super) const REMOTE_BEEPS: [&str; 2] =
    ["Two rising notes: started. Two falling: stopped.", "One high: clock or mode set. One low: failed."];
pub(super) const CHIME_TEST_NAME: &str = "Phone time";
pub(super) const CHIME_TEST_HELP: &str = "Time from a chime";
/// Since B257 the phone's Set X8 clock always sets the clock; this switch is for the other commands.
pub(super) const CLOCK_FROM_REMOTE_NAME: &str = "Set clock from all chimes";
pub(super) const PAUSE_NAME: &str = "Pause with REC";
/// Each switch's line says what its current setting does: [off, on].
pub(super) const PAUSE_STATE: [&str; 2] =
    ["REC is ignored during a take, so takes have no gaps", "REC pauses and resumes a take, with a MARK per pause"];
/// On the main screen when REC was kept from pausing a take.
pub(super) const PAUSE_OFF_LINES: [&str; 2] = ["Pausing is off.", "STOP ends the take."];
pub(super) const CLOCK_FROM_REMOTE_STATE: [&str; 2] =
    ["Only the phone's \"Set X8 clock\" sets the clock.", "Start, Stop and mode chimes also set the clock."];
pub(super) const CPU_TEST_NAME: &str = "CPU test";
pub(super) const CPU_TEST_HELP: [&str; 2] =
    ["Runs every listening mode with Auto volume off and on,", "8 s each, up to 3 min. Lists CPU load and lost input."];
pub(super) const MIX_FILE_NAME: &str = "MIX file";
/// The MIX file switch on follows MANUAL's REC READY for MASTER; off, it overrides it.
pub(super) const MIX_FILE_FOLLOWS: [&str; 2] =
    ["No MIX file: MASTER REC READY is off in MANUAL", "MIX file is saved, as MASTER REC READY is on in MANUAL"];
pub(super) const MIX_FILE_OVERRIDES: &str = "Overrides REC READY: no MIX file for BAT DETECTOR";
pub(super) const DEBUG_NAME: &str = "Debug info";
pub(super) const DEBUG_STATE: [&str; 2] =
    ["CPU and input figures are hidden", "CPU and input figures are shown on the main screen"];
pub(super) const GUANO_NAME: &str = "GUANO metadata";
pub(super) const GUANO_STATE: [&str; 2] =
    ["No GUANO metadata. WAV files are stock X8 files", "Each WAV file gets start time, recorder model, location"];
pub(super) const GUANO_HELP: &str = "Bat analysis apps read GUANO. Most audio apps ignore it.";

/// Beside the QR code on the Phone page.
pub(super) const QR_HEADING: &str = "Phone page";
pub(super) const QR_HELP: [&str; 2] = ["Plays the chimes.", "Scan with the phone."];
/// The clock command and the listening modes, on the remote panel's last line.
const LAST_SET_CLOCK: &str = "Set clock";
const LAST_MODE: [&str; 6] = ["HET mode", "COMB mode", "FD mode", "TE mode", "PITCH mode", "1:1 mode"];
pub(super) const CLOCK_SET_LINES: [&str; 1] = ["Phone: clock set"];
pub(super) const CLOCK_AFTER_TAKE_LINES: [&str; 2] = ["Phone: clock will be set", "when recording stops."];
pub(super) const CLOCK_FAILED_LINES: [&str; 2] = ["Could not set the clock.", "Try again from the phone."];
/// For each mode of `Mode::ALL`.
pub(super) const MODE_SET_LINES: [[&str; 1]; 6] = [
    ["Phone: listening mode HET"],
    ["Phone: listening mode COMB"],
    ["Phone: listening mode FD"],
    ["Phone: listening mode TE"],
    ["Phone: listening mode PITCH"],
    ["Phone: listening mode 1:1"],
];
/// Only PITCH is left out on the X8, because it cannot keep up there.
pub(super) const MODE_NOT_OFFERED_LINES: [[&str; 2]; 6] = [
    ["Phone asked for HET.", "Not offered on this X8."],
    ["Phone asked for COMB.", "Not offered on this X8."],
    ["Phone asked for FD.", "Not offered on this X8."],
    ["Phone asked for TE.", "Not offered on this X8."],
    ["Phone asked for PITCH.", "The X8 is too slow for it."],
    ["Phone asked for 1:1.", "Not offered on this X8."],
];

/// The line for a switch's current setting.
fn state(lines: &[&'static str; 2], on: bool) -> &'static str {
    lines[on as usize]
}

/// A panel with a name, a switch at the right of the name, and under it a line saying what the
/// current setting does, then any fixed help.
fn switch_panel(fb: Fb, panel: Rectangle, name: &str, lines: &[&str], on: bool, selected: bool) {
    rounded(fb, panel, 6, PANEL);
    if selected {
        outline_selected(fb, panel);
    }
    let (x, y) = (panel.top_left.x + 10, panel.top_left.y);
    text(fb, &BODY, name, x, y + 20, LEFT, TEXT);
    switch_control(fb, panel_switch(panel), on);
    for (line, y) in lines.iter().zip([y + 40, y + 54]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
}

/// A button that opens a test, with its help inside.
fn test_button(fb: Fb, button: Rectangle, name: &str, help: &[&str]) {
    rounded(fb, button, 4, BUTTON);
    let (x, y) = (button.top_left.x + 10, button.top_left.y);
    text(fb, &BODY_BOLD, name, x, y + 19, LEFT, TEXT);
    for (line, y) in help.iter().zip([y + 37, y + 51]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
}

/// What a take's files get, and the phone remote. Each panel is shown only on a host that offers
/// it, in a fixed place.
pub(super) fn record(app: &BatApp, fb: Fb) {
    if app.mix_file_available() {
        let selected = app.selected_setting() == Setting::MixFile;
        let line = if app.mix_file() { state(&MIX_FILE_FOLLOWS, app.manual_mix_file()) } else { MIX_FILE_OVERRIDES };
        switch_panel(fb, RECORD_MIX_FILE, MIX_FILE_NAME, &[line], app.mix_file(), selected);
    }
    if app.guano_available() {
        switch_panel(fb, RECORD_GUANO, GUANO_NAME, &[state(&GUANO_STATE, app.guano()), GUANO_HELP], app.guano(), false);
    }
    if app.pause_option_available() {
        let selected = app.selected_setting() == Setting::PauseWithRec;
        switch_panel(
            fb,
            RECORD_PAUSE,
            PAUSE_NAME,
            &[state(&PAUSE_STATE, app.pause_with_rec())],
            app.pause_with_rec(),
            selected,
        );
    }
}

/// The phone remote and its clock switch, the phone page's QR code, and the chime test.
pub(super) fn phone(app: &BatApp, fb: Fb) {
    if app.remote_available() {
        remote_panel(app, fb);
        let selected = app.selected_setting() == Setting::ClockFromRemote;
        let line = state(&CLOCK_FROM_REMOTE_STATE, app.clock_from_remote());
        switch_panel(fb, PHONE_CLOCK, CLOCK_FROM_REMOTE_NAME, &[line], app.clock_from_remote(), selected);
    }
    let card = qr_card(app.remote_available());
    qr_code(fb, card);
    let y = card.top_left.y;
    text(fb, &BODY_BOLD, QR_HEADING, QR_TEXT_X, y + 14, LEFT, TEXT);
    text(fb, &SMALL, qr::ADDRESS, QR_TEXT_X, y + 32, LEFT, TEXT);
    for (line, y) in QR_HELP.iter().zip([y + 50, y + 64]) {
        text(fb, &SMALL, *line, QR_TEXT_X, y, LEFT, TEXT_MUTED);
    }
    let button = phone_time(app.remote_available());
    rounded(fb, button, 4, BUTTON);
    let (x, y) = (button.top_left.x + 10, button.top_left.y);
    text(fb, &BODY_BOLD, CHIME_TEST_NAME, x, y + 19, LEFT, TEXT);
    text(fb, &SMALL, CHIME_TEST_HELP, x, y + 37, LEFT, TEXT_MUTED);
}

/// Black modules on a white card. Square corners, so the card's edge never cuts into the quiet zone.
fn qr_code(fb: Fb, card: Rectangle) {
    let _ = card.into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE)).draw(fb);
    let origin = card.top_left + Point::new(QR_MARGIN * QR_MODULE, QR_MARGIN * QR_MODULE);
    let module = Size::new(QR_MODULE as u32, QR_MODULE as u32);
    for y in 0..qr::SIZE {
        for x in 0..qr::SIZE {
            if qr::dark(x, y) {
                let at = origin + Point::new(x as i32 * QR_MODULE, y as i32 * QR_MODULE);
                let _ = Rectangle::new(at, module).into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(fb);
            }
        }
    }
}

/// The debug figures' switch and the CPU test.
pub(super) fn tests(app: &BatApp, fb: Fb) {
    if app.debug_available() {
        let selected = app.selected_setting() == Setting::DebugInfo;
        switch_panel(fb, TESTS_DEBUG, DEBUG_NAME, &[state(&DEBUG_STATE, app.debug_info())], app.debug_info(), selected);
        test_button(fb, TESTS_CPU_TEST, CPU_TEST_NAME, &CPU_TEST_HELP);
    }
}

/// The name of a phone command on the remote panel's last line.
fn command_name(command: Option<RemoteCommand>) -> &'static str {
    match command {
        Some(RemoteCommand::Start) => "Start",
        Some(RemoteCommand::Stop) => "Stop",
        Some(RemoteCommand::SetClock) => LAST_SET_CLOCK,
        Some(RemoteCommand::Listen(mode)) => LAST_MODE[mode_index(Mode::from(mode))],
        None => "Unknown command",
    }
}

fn remote_panel(app: &BatApp, fb: Fb) {
    rounded(fb, PHONE_REMOTE, 6, PANEL);
    let (x, y) = (PHONE_REMOTE.top_left.x + 10, PHONE_REMOTE.top_left.y);
    text(fb, &BODY, REMOTE_NAME, x, y + 20, LEFT, TEXT);
    switch_control(fb, panel_switch(PHONE_REMOTE), app.remote_on());
    for (line, y) in REMOTE_HELP.iter().zip([y + 40, y + 54]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
    for (line, y) in REMOTE_BEEPS.iter().zip([y + 68, y + 82]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
    let remote = &app.remote;
    text(
        fb,
        &SMALL,
        format_args!(
            "Tones heard {}. Chimes read {}, failed {}.",
            remote.tones, remote.chimes_read, remote.chimes_failed
        ),
        x,
        y + 100,
        LEFT,
        TEXT,
    );
    match remote.last {
        Some(last) => {
            let name = command_name(last.message.known());
            let seconds = app.samples.saturating_sub(last.read_at) / app.sample_rate as u64;
            let line = format_args!("Last: {name}, {seconds} s ago, carrier {:.0} dBFS", last.level_dbfs);
            text(fb, &SMALL, line, x, y + 114, LEFT, TEXT);
        }
        None => {
            text(fb, &SMALL, "No command read yet", x, y + 114, LEFT, TEXT_MUTED);
        }
    }
}

/// A phone remote message from a newer phone page than this build reads.
pub(super) const UNKNOWN_LINES: [&str; 2] = ["Unknown phone command.", "Update the bat firmware."];

/// A command's outcome, in the corner of the waterfall.
pub(super) fn result_lines(result: RemoteResult) -> &'static [&'static str] {
    match result {
        RemoteResult::Started => &["Phone: recording started"],
        RemoteResult::AlreadyRecording => &["Phone asked to start.", "Already recording."],
        RemoteResult::Stopped => &["Phone: recording stopped"],
        RemoteResult::AlreadyStopped => &["Phone asked to stop.", "Already stopped."],
        RemoteResult::HoldOn(RecorderCommand::Start) => &["Cannot start: HOLD is on.", "Slide HOLD off."],
        RemoteResult::HoldOn(RecorderCommand::Stop) => &["Cannot stop: HOLD is on.", "Slide HOLD off."],
        RemoteResult::GuideOn => &["Cannot start: recording guide", "is on. Turn it off in MANUAL."],
        RemoteResult::PageOpen => &["Cannot start: MANUAL settings", "page open. Press PLAY, close it."],
        RemoteResult::NoResponse => &["Recorder did not respond to", "the phone's command within 5 s."],
        RemoteResult::Unknown => &UNKNOWN_LINES,
        RemoteResult::ClockSet => &CLOCK_SET_LINES,
        RemoteResult::ClockAfterTake => &CLOCK_AFTER_TAKE_LINES,
        RemoteResult::ClockFailed => &CLOCK_FAILED_LINES,
        RemoteResult::ModeSet(mode) => &MODE_SET_LINES[mode_index(mode)],
        RemoteResult::ModeNotOffered(mode) => &MODE_NOT_OFFERED_LINES[mode_index(mode)],
    }
}

fn mode_index(mode: Mode) -> usize {
    Mode::ALL.iter().position(|&m| m == mode).unwrap_or(0)
}

/// Test helpers: each line that has a limit, with its width and the room for it.
#[cfg(test)]
pub(crate) fn lines_with_widths() -> std::vec::Vec<(std::string::String, i32, i32)> {
    use std::string::ToString;
    use std::vec::Vec;

    use embedded_graphics::prelude::Point;
    use u8g2_fonts::FontRenderer;
    use u8g2_fonts::types::VerticalPosition;

    let width = |font: &FontRenderer, line: &str| {
        font.get_rendered_dimensions(line, Point::zero(), VerticalPosition::Baseline).map_or(0, |d| d.advance.x)
    };
    let mut lines = Vec::new();
    let mut add = |font: &FontRenderer, line: &str, room: i32| lines.push((line.to_string(), width(font, line), room));
    let inside = PHONE_REMOTE.size.width as i32 - 20;
    for line in REMOTE_HELP.iter().chain(&CPU_TEST_HELP).chain(&REMOTE_BEEPS) {
        add(&SMALL, line, inside);
    }
    // Right of the QR code.
    let beside_qr = 314 - QR_TEXT_X;
    add(&BODY_BOLD, QR_HEADING, beside_qr);
    for line in QR_HELP.iter().chain([&qr::ADDRESS]) {
        add(&SMALL, line, beside_qr);
    }
    add(&SMALL, CHIME_TEST_HELP, phone_time(true).size.width as i32 - 20);
    add(&BODY_BOLD, CHIME_TEST_NAME, phone_time(true).size.width as i32 - 20);
    let states = [MIX_FILE_FOLLOWS, DEBUG_STATE, GUANO_STATE, PAUSE_STATE, CLOCK_FROM_REMOTE_STATE];
    for line in states.iter().flatten().chain([&MIX_FILE_OVERRIDES, &GUANO_HELP]) {
        add(&SMALL, line, inside);
    }
    // The name's line ends at the switch.
    for name in [REMOTE_NAME, GUANO_NAME, MIX_FILE_NAME, DEBUG_NAME, CLOCK_FROM_REMOTE_NAME, PAUSE_NAME] {
        add(&BODY, name, 240);
    }
    add(&SMALL, "Tones heard 100. Chimes read 100, failed 100.", inside);
    add(&SMALL, "Last: Start, 3600 s ago, carrier -100 dBFS", inside);
    add(&SMALL, "Last: Unknown command, 3600 s ago, carrier -100 dBFS", inside);
    for name in LAST_MODE.iter().chain([&LAST_SET_CLOCK]) {
        add(&SMALL, &std::format!("Last: {name}, 3600 s ago, carrier -100 dBFS"), inside);
    }
    for result in [
        RemoteResult::Started,
        RemoteResult::AlreadyRecording,
        RemoteResult::Stopped,
        RemoteResult::AlreadyStopped,
        RemoteResult::HoldOn(RecorderCommand::Start),
        RemoteResult::HoldOn(RecorderCommand::Stop),
        RemoteResult::GuideOn,
        RemoteResult::PageOpen,
        RemoteResult::NoResponse,
        RemoteResult::Unknown,
        RemoteResult::ClockSet,
        RemoteResult::ClockAfterTake,
        RemoteResult::ClockFailed,
    ]
    .into_iter()
    .chain(Mode::ALL.map(RemoteResult::ModeSet))
    .chain(Mode::ALL.map(RemoteResult::ModeNotOffered))
    {
        for line in result_lines(result) {
            add(&BODY, line, crate::layout::WF_W as i32 - 20);
        }
    }
    for line in PAUSE_OFF_LINES {
        add(&BODY, line, crate::layout::WF_W as i32 - 20);
    }
    lines
}
