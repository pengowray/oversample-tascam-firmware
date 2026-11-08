//! Drawing the screen. The whole screen is redrawn each time; on the X8 that
//! is about 150,000 pixel writes per frame.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{
    Circle, CornerRadii, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle,
};
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

use crate::fb::Rgb565Buffer;
use crate::layout::*;
use crate::palette::*;
use crate::{SettingsPage, 
    AutoVolume, BatApp, CallKind, DEFAULT_CUTOFF_HZ, Feed, InputKind, Meter, MixSilence, Mode, MonitorSource, Recorder, Screen, Setting, TeStatus,
    VIEW_HI_HZ, VIEW_LO_HZ, WF_COLUMN_MS,
};
use bat_dsp::heterodyne::DEFAULT_COMB_SPACING_HZ;
use bat_dsp::time_expansion::{DEFAULT_POST_MS, DEFAULT_PRE_MS};

mod chime_screen;
mod cpu_test_screen;
mod pages;
#[cfg(test)]
pub(crate) use chime_screen::{CLOCK_Y, COLUMN_X, FAILURE_Y, ROW_H, ROW_Y, chime_text, columns as chime_columns, lines_with_widths as chime_test_lines};
#[cfg(test)]
pub(crate) use cpu_test_screen::{COLUMN_GAP as CPU_TEST_COLUMN_GAP, column_contents as cpu_test_columns, lines_with_widths as cpu_test_lines};
#[cfg(test)]
pub(crate) use pages::lines_with_widths as pages_lines;

/// The width of the chime test's clock, for tests.
#[cfg(test)]
pub(crate) fn chime_clock_width(text: &str) -> i32 {
    chime_screen::CLOCK.get_rendered_dimensions(text, Point::zero(), VerticalPosition::Baseline).unwrap().advance.x
}

const SMALL: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();
const BODY: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR10_tf>();
const BODY_BOLD: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB10_tf>();
const VALUE: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB14_tf>();
const BIG: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB18_tf>();

type Fb<'a, 'b> = &'a mut Rgb565Buffer<'b>;

/// Draw `content` with its baseline at `y`; returns the x where the next text
/// would start (for left-aligned text).
fn text(
    fb: Fb,
    font: &FontRenderer,
    content: impl u8g2_fonts::Content,
    x: i32,
    y: i32,
    align: HorizontalAlignment,
    color: Rgb565,
) -> i32 {
    let p = Point::new(x, y);
    let color = FontColor::Transparent(color);
    if let HorizontalAlignment::Left = align {
        font.render(content, p, VerticalPosition::Baseline, color, fb).map(|d| x + d.advance.x).unwrap_or(x)
    } else {
        let _ = font.render_aligned(content, p, VerticalPosition::Baseline, align, color, fb);
        x
    }
}

const LEFT: HorizontalAlignment = HorizontalAlignment::Left;
const CENTER: HorizontalAlignment = HorizontalAlignment::Center;
const RIGHT: HorizontalAlignment = HorizontalAlignment::Right;

fn fill(fb: Fb, r: Rectangle, color: Rgb565) {
    let _ = fb.fill_solid(&r, color);
}

fn rounded(fb: Fb, r: Rectangle, radius: u32, color: Rgb565) {
    let _ = RoundedRectangle::new(r, CornerRadii::new(Size::new(radius, radius)))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(fb);
}

fn hline(fb: Fb, x0: i32, x1: i32, y: i32, color: Rgb565) {
    fill(fb, Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1).max(0) as u32, 1)), color);
}

pub fn draw(app: &BatApp, fb: Fb) {
    let _ = fb.clear(BACKGROUND);
    if app.screen == Screen::Settings {
        title_bar(fb, "SETTINGS", false);
        settings(app, fb);
        return;
    }
    if app.screen == Screen::ChimeTest {
        title_bar(fb, chime_screen::TITLE, false);
        chime_screen::draw(app, fb);
        return;
    }
    if app.screen == Screen::CpuTest {
        title_bar(fb, cpu_test_screen::TITLE, false);
        cpu_test_screen::draw(app, fb);
        return;
    }
    let title = match app.input_kind() {
        InputKind::Live => "BAT DETECTOR",
        InputKind::Playback => "BAT PLAYBACK",
    };
    if app.cpu_test.running {
        let (at, count) = (app.cpu_test.at + 1, app.cpu_test.count);
        title_bar(fb, format_args!("{} row {at}/{count}", cpu_test_screen::TITLE), true);
    } else {
        title_bar(fb, title, true);
    }
    if app.remote_on() {
        let colour = if app.remote.listening(app.samples) { ACCENT } else { TEXT_MUTED };
        text(fb, &SMALL, pages::TITLE_LABEL, 284, 16, RIGHT, colour);
    }
    status_bar(app, fb);
    meters(app, fb);
    waterfall(app, fb);
    if let Some(rate) = app.rate_warning() {
        rate_warning(fb, rate, app.input_kind());
    } else if app.inputs()[0].is_none() {
        no_input_notice(fb);
    } else if app.pause_refused_shown() {
        corner_notice_lines(fb, &pages::PAUSE_OFF_LINES);
    } else if let Some(result) = app.remote.result_shown(app.samples) {
        corner_notice_lines(fb, pages::result_lines(result));
    } else if let Some(silence) = app.mix_silence() {
        mix_silence_notice(fb, silence);
    }
    snapshot(app, fb);
    if app.cpu_test.running {
        cpu_test_screen::running_line(app, fb);
    } else {
        value_line(app, fb);
    }
    mode_buttons(app, fb);
}

fn title_bar(fb: Fb, title: impl u8g2_fonts::Content, menu: bool) {
    fill(fb, Rectangle::new(Point::zero(), Size::new(320, TITLE_H as u32)), TITLE);
    text(fb, &BODY_BOLD, title, 160, 17, CENTER, TEXT);
    if menu {
        // Three lines, as the stock apps' Shortcut Menu button.
        for y in [7, 11, 15] {
            fill(fb, Rectangle::new(Point::new(12, y), Size::new(16, 2)), TEXT);
        }
    }
    // Battery, as in the stock title bars. The emulator always shows it full.
    let outline = PrimitiveStyleBuilder::new().stroke_color(TEXT).stroke_width(1).build();
    let _ = Rectangle::new(Point::new(288, 7), Size::new(21, 11)).into_styled(outline).draw(fb);
    fill(fb, Rectangle::new(Point::new(309, 10), Size::new(2, 5)), TEXT);
    fill(fb, Rectangle::new(Point::new(290, 9), Size::new(17, 7)), TEXT);
}

/// The recorder's state as the X8's REC LED shows it: red while recording
/// or paused, a blinking red dot in record standby.
fn status_bar(app: &BatApp, fb: Fb) {
    let (state, secs) = app.recorder();
    let red = matches!(state, Recorder::Recording | Recorder::Paused);
    rounded(fb, STATUS, 6, if red { RECORDING } else { PANEL });
    let cy = STATUS.top_left.y + STATUS.size.height as i32 / 2;
    let dot = |fb: Fb, colour| {
        let _ = Circle::with_center(Point::new(24, cy), 14).into_styled(PrimitiveStyle::with_fill(colour)).draw(fb);
    };
    match state {
        Recorder::Stopped => fill(fb, Rectangle::with_center(Point::new(24, cy), Size::new(12, 12)), TEXT),
        Recorder::Standby => {
            // Half a second on, half off, by sample time.
            if (app.samples as f32 / (app.sample_rate * 0.5)) as u64 % 2 == 0 {
                dot(fb, RECORDING);
            }
        }
        Recorder::Recording => dot(fb, TEXT),
        Recorder::Paused => {
            for x in [18, 26] {
                fill(fb, Rectangle::new(Point::new(x, cy - 6), Size::new(4, 12)), TEXT);
            }
        }
    }
    let recording = red;
    // Recorded time in the stock style: large digits, small h m s.
    let (h, m, s) = (secs / 3600 % 100, secs / 60 % 60, secs % 60);
    let baseline = cy + 9;
    let mut x = 42;
    for (value, unit) in [(h, "h"), (m, "m"), (s, "s")] {
        x = text(fb, &BIG, format_args!("{value:02}"), x, baseline, LEFT, TEXT);
        x = text(fb, &BODY, unit, x + 1, baseline, LEFT, TEXT) + 3;
    }
    let c = app.clock_s;
    text(
        fb,
        &BODY,
        format_args!("/{:02}:{:02}:{:02}", c / 3600, c / 60 % 60, c % 60),
        306,
        baseline - 1,
        RIGHT,
        if recording { TEXT } else { TEXT_MUTED },
    );
}

/// Over the waterfall while the input's sample rate is not 192 kHz: the app
/// gets no sound then, so the waterfall would stay empty. For live input the
/// fix is the record rate setting; for playback, a recording made at 192 kHz.
fn rate_warning(fb: Fb, rate_hz: u32, kind: InputKind) {
    let (x, y) = notice_panel(fb);
    let tenths = (rate_hz + 50) / 100;
    if tenths % 10 == 0 {
        text(fb, &BODY_BOLD, format_args!("Wrong sample rate: {}kHz", tenths / 10), x, y, LEFT, TEXT);
    } else {
        text(fb, &BODY_BOLD, format_args!("Wrong sample rate: {}.{}kHz", tenths / 10, tenths % 10), x, y, LEFT, TEXT);
    }
    notice_lines(
        fb,
        x,
        y,
        match kind {
            InputKind::Live => &RATE_FIX_LIVE,
            InputKind::Playback => &RATE_FIX_PLAYBACK,
        },
    );
}

/// Over the waterfall while no input is on: the app hears nothing.
fn no_input_notice(fb: Fb) {
    let (x, y) = notice_panel(fb);
    text(fb, &BODY_BOLD, NO_INPUT_HEADING, x, y, LEFT, TEXT);
    notice_lines(fb, x, y, &NO_INPUT_FIX);
}

/// Draw the panel over the waterfall; returns where its heading goes.
fn notice_panel(fb: Fb) -> (i32, i32) {
    let panel = Rectangle::new(Point::new(6, WF_Y + 8), Size::new(308, WF_H as u32 - 16));
    rounded(fb, panel, 6, PANEL);
    (panel.top_left.x + 10, panel.top_left.y + 22)
}

fn notice_lines(fb: Fb, x: i32, heading_y: i32, lines: &[&str]) {
    for (line, y) in lines.iter().zip((1..).map(|row| heading_y + row * 24)) {
        text(fb, &BODY, *line, x, y, LEFT, TEXT);
    }
}

/// What to press to hear the mix, in the top left corner of the waterfall, so the
/// waterfall stays in view. That corner shows the oldest columns and the highest
/// frequencies, the least used part of the waterfall, and a MARK press still shows at
/// the right. White on `PANEL` has a contrast of 12:1, whatever the waterfall shows.
/// The waterfall's grey columns are also `PANEL`, so the box has a border.
fn mix_silence_notice(fb: Fb, silence: MixSilence) {
    corner_notice_lines(fb, mix_silence_lines(silence));
}

/// A small notice in the top left corner of the waterfall.
fn corner_notice_lines(fb: Fb, lines: &[&str]) {
    let width = |line: &&str| BODY.get_rendered_dimensions(*line, Point::zero(), VerticalPosition::Baseline).map_or(0, |d| d.advance.x);
    let panel = corner_notice(lines.iter().map(width).max().unwrap_or(0), lines.len());
    rounded(fb, panel, 3, PANEL);
    let border = PrimitiveStyleBuilder::new().stroke_color(LINE).stroke_width(1).build();
    let _ = RoundedRectangle::new(panel, CornerRadii::new(Size::new(3, 3))).into_styled(border).draw(fb);
    let (x, y) = (panel.top_left.x + CORNER_NOTICE_PAD_X, panel.top_left.y + CORNER_NOTICE_BASELINE);
    for (line, y) in lines.iter().zip((0..).map(|row| y + row * CORNER_NOTICE_LINE)) {
        text(fb, &BODY, *line, x, y, LEFT, TEXT);
    }
}

/// What to press to hear the mix.
fn mix_silence_lines(silence: MixSilence) -> &'static [&'static str] {
    match silence {
        MixSilence::RecPause => &["Press REC once to listen"],
        MixSilence::Stopped => &["Press REC to record and listen"],
        MixSilence::PageOpen => &["Headphones muted. Press PLAY", "then close the page in MANUAL"],
    }
}

const NO_INPUT_HEADING: &str = "No inputs turned on in MANUAL";
const NO_INPUT_FIX: [&str; 3] = [
    "1. Press PLAY and open the INPUT screen.",
    "2. Turn on inputs with ultrasonic mics.",
    "3. Press PLAY to return to BAT DETECTOR.",
];

const RATE_FIX_LIVE: [&str; 6] = [
    "BAT DETECTOR needs a 192kHz sample rate.",
    "1. Hold HOME to return to the LAUNCHER.",
    "2. Tap GENERAL SETTINGS (bottom right).",
    "3. Open the REC SETTINGS page.",
    "4. In FILE FORMAT, set SAMPLE to 192kHz.",
    "5. Press HOME, then tap BAT DETECTOR.",
];

const RATE_FIX_PLAYBACK: [&str; 4] = [
    "BAT PLAYBACK needs a 192kHz recording.",
    "1. Press HOME briefly to stop playback.",
    "2. In MANUAL, choose a 192kHz recording.",
    "3. Press PLAY to return to BAT PLAYBACK.",
];

fn meter_x(db: f32) -> i32 {
    let t = ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0);
    METER_X + (t * METER_W as f32) as i32
}

fn meters(app: &BatApp, fb: Fb) {
    const SEGMENT: i32 = 4;
    const GAP: i32 = 1;
    let names = match (app.feed(), app.inputs()) {
        (_, [None, _]) => [None, None],
        (Feed::Mix, _) => [Some("L"), Some("R")],
        (Feed::Inputs, inputs) => inputs.map(|input| input.map(|input| input.short)),
    };
    for ((meter, &y), name) in app.meters.iter().zip(&METER_Y).zip(names) {
        let Some(name) = name else { continue };
        text(fb, &SMALL, name, 8, y + 6, LEFT, TEXT_MUTED);
        let Meter { level_db, hold_db, .. } = *meter;
        let mut x = METER_X;
        while x + SEGMENT <= METER_X + METER_W {
            // The level at this segment's right edge.
            let db = METER_MIN_DB + (x + SEGMENT - METER_X) as f32 / METER_W as f32 * -METER_MIN_DB;
            let lit = db <= level_db || (x == METER_X && level_db > METER_MIN_DB);
            let color = match (lit, db) {
                (false, _) => rgb_dim(),
                (true, d) if d > -3.0 => METER_RED,
                (true, d) if d > -12.0 => METER_YELLOW,
                _ => METER_GREEN,
            };
            fill(fb, Rectangle::new(Point::new(x, y), Size::new(SEGMENT as u32, METER_H as u32)), color);
            x += SEGMENT + GAP;
        }
        if hold_db > METER_MIN_DB {
            let hx = meter_x(hold_db).min(METER_X + METER_W - 2);
            fill(fb, Rectangle::new(Point::new(hx, y), Size::new(2, METER_H as u32)), TEXT);
        }
    }
    for db in [-72.0f32, -48.0, -24.0, -12.0, -6.0, 0.0] {
        let x = meter_x(db);
        let align = if db == METER_MIN_DB { LEFT } else if db == 0.0 { RIGHT } else { CENTER };
        let x = if db == METER_MIN_DB { METER_X } else if db == 0.0 { METER_X + METER_W } else { x };
        text(fb, &SMALL, format_args!("{}", db as i32), x, METER_SCALE_BASELINE, align, TEXT_MUTED);
    }
    text(fb, &SMALL, "dB", 314, METER_SCALE_BASELINE, RIGHT, TEXT_MUTED);
}

/// Unlit meter segments.
const fn rgb_dim() -> Rgb565 {
    Rgb565::new(5, 10, 5)
}

/// Screen x of a sample time on the waterfall, if it is on screen.
fn sample_x(app: &BatApp, sample: u64) -> Option<i32> {
    let column_len = (app.sample_rate * WF_COLUMN_MS * 0.001) as u64;
    let col = (sample / column_len.max(1)) as i64;
    // The rightmost pixel column is the last finished display column.
    let newest = app.wf_column as i64 - 1;
    let x = WF_X as i64 + WF_W as i64 - 1 - (newest - col);
    (x >= WF_X as i64 && x < (WF_X + WF_W as i32) as i64).then_some(x as i32)
}

fn waterfall(app: &BatApp, fb: Fb) {
    for r in 0..WF_H {
        let row = fb.row_mut(WF_Y as usize + r);
        let dst = &mut row[WF_X as usize..WF_X as usize + WF_W];
        // The ring's columns from the oldest to the end, then from the start.
        let (older, newer) = dst.split_at_mut(WF_W - app.wf_next);
        let (first, second) = (&app.bufs.wf[app.wf_next * WF_H + r..], &app.bufs.wf[r..]);
        for (px, &value) in older.iter_mut().zip(first.iter().step_by(WF_H)) {
            *px = WATERFALL_565[value as usize];
        }
        for (px, &value) in newer.iter_mut().zip(second.iter().step_by(WF_H)) {
            *px = WATERFALL_565[value as usize];
        }
    }
    let right = WF_X + WF_W as i32 - 1;
    let y_of = |hz: f32| freq_to_y(hz, WF_Y, WF_H);
    let in_view = |hz: f32| (VIEW_LO_HZ..=VIEW_HI_HZ).contains(&hz);

    // Frequency axis: a label and a tick every 10 kHz, the tick in the
    // resistor colour of its tens digit, as Oversample marks frequencies.
    text(fb, &SMALL, "kHz", 2, WF_Y + 7, LEFT, TEXT_MUTED);
    let first = libm::ceilf(VIEW_LO_HZ / 10_000.0) as usize;
    for (k, &color) in RESISTOR.iter().enumerate().skip(first) {
        let y = y_of(k as f32 * 10_000.0);
        text(fb, &SMALL, format_args!("{}", k * 10), 24, y + 4, RIGHT, TEXT_MUTED);
        fill(fb, Rectangle::new(Point::new(25, y), Size::new(3, 1)), color);
    }

    // What the current mode listens to.
    match app.mode {
        Mode::Het => {
            let lo = app.het.lo();
            for edge in [lo - DEFAULT_CUTOFF_HZ, lo + DEFAULT_CUTOFF_HZ] {
                if in_view(edge) {
                    dashed(fb, WF_X, right, y_of(edge), ACCENT);
                }
            }
            hline(fb, WF_X, right, y_of(lo), ACCENT);
        }
        Mode::Comb => {
            for i in 0..app.comb.active() {
                let lo = app.comb_first_hz + i as f32 * DEFAULT_COMB_SPACING_HZ;
                if in_view(lo) {
                    hline(fb, WF_X, right, y_of(lo), ACCENT);
                }
            }
        }
        Mode::Te => {
            // The window the time expander captured or will play.
            if let Some(t) = app.te_trigger {
                let pre = (DEFAULT_PRE_MS * 0.001 * app.sample_rate) as u64;
                let post = (DEFAULT_POST_MS * 0.001 * app.sample_rate) as u64;
                let start = sample_x(app, t.saturating_sub(pre));
                let end = sample_x(app, t + post);
                if start.is_some() || end.is_some() {
                    let (x0, x1) = (start.unwrap_or(WF_X), end.unwrap_or(right));
                    // A bar along the top and a line at each end of the window.
                    fill(fb, Rectangle::new(Point::new(x0, WF_Y), Size::new((x1 - x0 + 1) as u32, 3)), ACCENT);
                    for x in [start, end].into_iter().flatten() {
                        fill(fb, Rectangle::new(Point::new(x, WF_Y), Size::new(1, WF_H as u32)), ACCENT);
                    }
                }
            }
        }
        Mode::Fd | Mode::Pitch | Mode::Thru => {}
    }

    // Marked detections (ticks in the strip under the waterfall, coloured by
    // kind) and marks (along the top).
    for (t, kind) in app.calls.iter() {
        if let Some(x) = sample_x(app, t) {
            fill(fb, Rectangle::new(Point::new(x, TICKS_Y), Size::new(2, TICK_H as u32)), tick_colour(kind));
        }
    }
    for t in app.marks.iter() {
        if let Some(x) = sample_x(app, t) {
            fill(fb, Rectangle::new(Point::new(x, WF_Y), Size::new(2, 8)), TEXT);
        }
    }

    // Time ruler: seconds before now.
    let px_per_s = (1000.0 / WF_COLUMN_MS) as i32;
    for k in 0..=5 {
        let x = right - k * px_per_s / 2;
        if x < WF_X {
            break;
        }
        let tall = k % 2 == 0;
        fill(fb, Rectangle::new(Point::new(x, TICKS_Y + TICK_H), Size::new(1, if tall { 4 } else { 2 })), LINE);
    }
    text(fb, &SMALL, "now", right, RULER_BASELINE, RIGHT, TEXT_MUTED);
    text(fb, &SMALL, "-1 s", right - px_per_s, RULER_BASELINE, CENTER, TEXT_MUTED);
    text(fb, &SMALL, "-2 s", right - 2 * px_per_s, RULER_BASELINE, CENTER, TEXT_MUTED);
}

fn dashed(fb: Fb, x0: i32, x1: i32, y: i32, color: Rgb565) {
    let mut x = x0;
    while x <= x1 {
        hline(fb, x, (x + 3).min(x1), y, color);
        x += 8;
    }
}

fn snapshot(app: &BatApp, fb: Fb) {
    rounded(fb, SNAP_PANEL, 6, PANEL);
    let top = SNAP_PANEL.top_left.y;
    let Some((call, kind)) = app.last_call else {
        let cx = SNAP_PANEL.center().x;
        if app.marked == 0 {
            text(fb, &BODY, "No sounds set to mark", cx, top + 30, CENTER, TEXT_MUTED);
            text(fb, &SMALL, "Open the menu at the top left to switch them on.", cx, top + 46, CENTER, TEXT_MUTED);
        } else {
            text(fb, &BODY, "No marked sounds yet", cx, top + 36, CENTER, TEXT_MUTED);
        }
        return;
    };
    // The kind of detection, beside a swatch in its tick colour.
    fill(fb, Rectangle::new(Point::new(SNAP_TEXT_X, top + 5), Size::new(3, 8)), tick_colour(kind));
    text(fb, &SMALL, last_heading(kind), SNAP_TEXT_X + 6, top + 12, LEFT, TEXT_MUTED);
    for y in 0..SNAP_H {
        let row = fb.row_mut(SNAP_Y as usize + y);
        let dst = &mut row[SNAP_X as usize..SNAP_X as usize + SNAP_W];
        for (x, px) in dst.iter_mut().enumerate() {
            *px = INFERNO_565[app.bufs.snapshot[x * SNAP_H + y] as usize];
        }
    }
    // The frequencies at the top and bottom of the snapshot.
    let (lo, hi) = app.snapshot_hz;
    text(fb, &SMALL, format_args!("{:.0} kHz", hi / 1000.0), SNAP_X + 2, SNAP_Y + 8, LEFT, TEXT);
    text(fb, &SMALL, format_args!("{:.0} kHz", lo / 1000.0), SNAP_X + 2, SNAP_Y + SNAP_H as i32 - 2, LEFT, TEXT);
    let fs = app.sample_rate;
    let ago_s = app.samples.saturating_sub(call.end_sample) as f32 / fs;
    // A steady tone's length is the whole tone so far, not its latest piece.
    let length_ms = (call.end_sample - app.last_call_since) as f32 / fs * 1000.0;
    if ago_s < 60.0 {
        text(fb, &SMALL, format_args!("{:.0} s ago", libm::floorf(ago_s)), 308, top + 12, RIGHT, TEXT_MUTED);
    } else {
        text(fb, &SMALL, format_args!("{:.0} min ago", libm::floorf(ago_s / 60.0)), 308, top + 12, RIGHT, TEXT_MUTED);
    }
    let (y1, y2, y3) = (top + 28, top + 42, top + 56);
    for (label, y) in [("Peak", y1), ("Range", y2), ("Length", y3)] {
        text(fb, &BODY, label, SNAP_TEXT_X, y, LEFT, TEXT_MUTED);
    }
    // Every number has one decimal, with its decimal point in a fixed
    // column, so the digits stay in place as the values change: Peak, the
    // range's low end and Length share one column, the range's high end has
    // its own, and the units follow at fixed places.
    let width = |s: &str| {
        BODY.get_rendered_dimensions(s, Point::zero(), VerticalPosition::Baseline).map_or(0, |d| d.advance.x)
    };
    let (two_digits, point_digit, space) = (width("88"), width(".8"), width(" "));
    let point1 = SNAP_VALUE_X + two_digits;
    let unit1 = point1 + point_digit + space;
    // 1 px in, to sit midway between the two numbers.
    let dash = point1 + point_digit + 1;
    let point2 = dash + width("-") + two_digits;
    let unit2 = point2 + point_digit + space;

    number_at_point(fb, &BODY, call.peak_freq_hz / 1000.0, point1, y1, TEXT);
    text(fb, &BODY, "kHz", unit1, y1, LEFT, TEXT);
    number_at_point(fb, &BODY, call.min_freq_hz / 1000.0, point1, y2, TEXT);
    text(fb, &BODY, "-", dash, y2, LEFT, TEXT);
    number_at_point(fb, &BODY, call.max_freq_hz / 1000.0, point2, y2, TEXT);
    text(fb, &BODY, "kHz", unit2, y2, LEFT, TEXT);
    let (length, unit) = if length_ms >= 1000.0 { (length_ms / 1000.0, "s") } else { (length_ms, "ms") };
    number_at_point(fb, &BODY, length, point1, y3, TEXT);
    text(fb, &BODY, unit, unit1, y3, LEFT, TEXT);
}

/// Draw `value` to one decimal place with its decimal point at `point_x`:
/// the whole part ends there and the point and tenth start there.
fn number_at_point(fb: Fb, font: &FontRenderer, value: f32, point_x: i32, y: i32, color: Rgb565) {
    let tenths = libm::roundf(value.max(0.0) * 10.0) as u32;
    text(fb, font, format_args!("{}", tenths / 10), point_x, y, RIGHT, color);
    text(fb, font, format_args!(".{}", tenths % 10), point_x, y, LEFT, color);
}

fn value_line(app: &BatApp, fb: Fb) {
    let (label, x) = (8, 92);
    let y = VALUE_BASELINE;
    match app.mode {
        Mode::Het => {
            text(fb, &BODY, "Tuned to", label, y, LEFT, TEXT_MUTED);
            text(fb, &VALUE, format_args!("{:.1} kHz", app.het.lo() / 1000.0), x, y, LEFT, TEXT);
        }
        Mode::Comb => {
            text(fb, &BODY, "Tuned to", label, y, LEFT, TEXT_MUTED);
            let f = app.comb_first_hz / 1000.0;
            let s = DEFAULT_COMB_SPACING_HZ / 1000.0;
            text(fb, &VALUE, format_args!("{:.0}, {:.0}, {:.0} kHz", f, f + s, f + 2.0 * s), x, y, LEFT, TEXT);
        }
        Mode::Fd => {
            text(fb, &BODY, "Divide by", label, y, LEFT, TEXT_MUTED);
            text(fb, &VALUE, format_args!("{}", app.fd.division()), x, y, LEFT, TEXT);
        }
        Mode::Te => {
            text(fb, &BODY, "Slower by", label, y, LEFT, TEXT_MUTED);
            text(fb, &VALUE, format_args!("{:.0}\u{d7}", app.te.factor()), x, y, LEFT, TEXT);
            match app.te_status() {
                TeStatus::Waiting => text(fb, &BODY, "Waiting for a call", 312, y, RIGHT, TEXT_MUTED),
                TeStatus::Capturing => text(fb, &BODY, "Capturing", 312, y, RIGHT, TEXT),
                TeStatus::Playing { .. } => {
                    text(fb, &BODY, format_args!("Playing, {:.1} s left", app.te_seconds_left()), 312, y, RIGHT, TEXT)
                }
            };
        }
        Mode::Thru => {
            text(fb, &BODY, "Plays the input unchanged", label, y, LEFT, TEXT_MUTED);
        }
        Mode::Pitch => {
            text(fb, &BODY, "Divide by", label, y, LEFT, TEXT_MUTED);
            text(fb, &VALUE, format_args!("{:.0}", app.pitch.factor()), x, y, LEFT, TEXT);
        }
    }
}

fn mode_buttons(app: &BatApp, fb: Fb) {
    let modes = app.modes();
    for (i, mode) in modes.iter().enumerate() {
        let r = mode_button(i, modes.len());
        let selected = *mode == app.mode;
        rounded(fb, r, 4, if selected { ACCENT } else { BUTTON });
        text(fb, &BODY_BOLD, mode.label(), r.center().x, r.top_left.y + 28, CENTER, TEXT);
    }
}

/// Name of each switch on the settings screen. The help line under it is
/// drawn in `settings`, as the clicks' line includes the detection band's
/// bottom.
fn mark_name(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Bat => "Bat calls",
        CallKind::Tone => "Steady tones",
        CallKind::Audible => "Audible sounds",
        _ => "Clicks and rustles",
    }
}

/// Heading of the last-call panel: the kind of the detection it shows, in
/// the singular of its switch's name.
fn last_heading(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Bat | CallKind::Single => "BAT CALL",
        CallKind::Tone => "STEADY TONE",
        CallKind::Audible => "AUDIBLE SOUND",
        _ => "CLICK OR RUSTLE",
    }
}

/// The line under Listen to: what each ear hears.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ListenHelp {
    /// A pair of inputs with numbers for names.
    Ears(&'static str, &'static str),
    /// Two sides without numbers for names: the mix, playback or EXT IN.
    Stereo,
    /// One signal, the same in both ears.
    OneSignal,
    /// No input is on, so nothing is heard.
    Nothing,
}

impl core::fmt::Display for ListenHelp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ListenHelp::Ears(left, right) => write!(f, "Left ear: input {left}. Right ear: input {right}. Mono in FD and TE."),
            ListenHelp::Stereo => f.write_str("Stereo in HET, COMB and 1:1. Mono in FD and TE."),
            ListenHelp::OneSignal => f.write_str("Heard in both ears and shown on the waterfall."),
            ListenHelp::Nothing => f.write_str(NO_INPUT_HEADING),
        }
    }
}

/// Whether an input's short name is its number.
fn is_number(short: &str) -> bool {
    !short.is_empty() && short.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn listen_help(app: &BatApp) -> ListenHelp {
    match (app.feed(), app.inputs(), app.source()) {
        (_, [None, _], _) => ListenHelp::Nothing,
        (Feed::Mix, [_, Some(_)], MonitorSource::Both) => ListenHelp::Stereo,
        (Feed::Mix, ..) => ListenHelp::OneSignal,
        (Feed::Inputs, [Some(first), Some(second)], MonitorSource::Both) => match app.input_kind() {
            InputKind::Live if is_number(first.short) && is_number(second.short) => ListenHelp::Ears(first.short, second.short),
            _ => ListenHelp::Stereo,
        },
        (Feed::Inputs, ..) => ListenHelp::OneSignal,
    }
}

fn outline_selected(fb: Fb, r: Rectangle) {
    let outline = PrimitiveStyleBuilder::new().stroke_color(ACCENT).stroke_width(2).build();
    let _ = RoundedRectangle::new(r, CornerRadii::new(Size::new(6, 6))).into_styled(outline).draw(fb);
}

/// The Auto volume row: three choices, all in view, with the current one filled. A switch could
/// not show three states.
fn auto_volume_row(app: &BatApp, fb: Fb) {
    let r = control_row(Setting::AutoLevel);
    let (x, y) = (r.top_left.x, r.top_left.y);
    let after = text(fb, &BODY, "Auto volume", x + 10, y + 14, LEFT, TEXT);
    text(fb, &SMALL, "headphones", after + 5, y + 14, LEFT, TEXT_MUTED);
    for (i, &volume) in AutoVolume::ALL.iter().enumerate() {
        let c = auto_volume_choice(i);
        let selected = volume == app.auto_volume();
        rounded(fb, c, 3, if selected { ACCENT } else { BUTTON });
        text(fb, &BODY, volume.label(), c.center().x, c.top_left.y + 13, CENTER, TEXT);
    }
}

/// A switch. On: blue track, knob at the right. Off: grey track, knob at the left.
fn switch_control(fb: Fb, sw: Rectangle, on: bool) {
    let h = sw.size.height;
    let _ = RoundedRectangle::new(sw, CornerRadii::new(Size::new(h / 2, h / 2)))
        .into_styled(PrimitiveStyle::with_fill(if on { ACCENT } else { LINE }))
        .draw(fb);
    let knob_x = if on { sw.top_left.x + sw.size.width as i32 - h as i32 / 2 } else { sw.top_left.x + h as i32 / 2 };
    let _ = Circle::with_center(Point::new(knob_x, sw.center().y), h - 6).into_styled(PrimitiveStyle::with_fill(TEXT)).draw(fb);
}

/// The tabs, the page shown, the note and Done.
fn settings(app: &BatApp, fb: Fb) {
    let count = app.settings_pages().count();
    for (i, page) in app.settings_pages().enumerate() {
        let r = tab(i, count);
        // The page shown has the accent fill, as the chosen Auto volume choice does.
        rounded(fb, r, 4, if page == app.settings_page() { ACCENT } else { BUTTON });
        text(fb, &BODY, page.label(), r.center().x, r.top_left.y + 21, CENTER, TEXT);
    }
    match app.settings_page() {
        SettingsPage::Listen => listen_page(app, fb),
        SettingsPage::Record => pages::record(app, fb),
        SettingsPage::Tests => pages::tests(app, fb),
    }
    text(fb, &SMALL, "These settings reset when the recorder is turned off.", 16, SETTINGS_NOTE, LEFT, TEXT_MUTED);
    rounded(fb, DONE, 4, ACCENT);
    text(fb, &BODY_BOLD, "Done", DONE.center().x, DONE.top_left.y + 28, CENTER, TEXT);
}

/// The two stepped settings, Auto volume, and the sounds to mark.
fn listen_page(app: &BatApp, fb: Fb) {
    for (i, setting) in Setting::ALL.iter().enumerate() {
        if setting.page() != SettingsPage::Listen {
            continue;
        }
        let selected = app.selected_setting() == *setting;
        if !matches!(setting, Setting::LowestBatCall | Setting::ListenTo) {
            let r = control_row(*setting);
            rounded(fb, r, 6, PANEL);
            if selected {
                outline_selected(fb, r);
            }
            let (x, y) = (r.top_left.x, r.top_left.y);
            let on = match *setting {
                Setting::Mark(kind) => {
                    fill(fb, Rectangle::new(Point::new(x + 8, y + 5), Size::new(4, 23)), tick_colour(kind));
                    text(fb, &BODY, mark_name(kind), x + 18, y + 14, LEFT, TEXT);
                    let (hx, hy) = (x + 18, y + 28);
                    match kind {
                        CallKind::Bat => text(fb, &SMALL, "Similar calls repeated several times a second.", hx, hy, LEFT, TEXT_MUTED),
                        CallKind::Tone => text(fb, &SMALL, "150 ms or longer, like a rat's 22 kHz alarm call.", hx, hy, LEFT, TEXT_MUTED),
                        CallKind::Audible => text(fb, &SMALL, "Also loud at 2 to 12 kHz, like birds and voices.", hx, hy, LEFT, TEXT_MUTED),
                        _ => text(
                            fb,
                            &SMALL,
                            format_args!("Reaching down to {:.0} kHz, like handling noise.", app.detector.band().0 / 1000.0),
                            hx,
                            hy,
                            LEFT,
                            TEXT_MUTED,
                        ),
                    };
                    app.is_marked(kind)
                }
                _ => {
                    auto_volume_row(app, fb);
                    continue;
                }
            };
            switch_control(fb, switch(*setting), on);
            continue;
        }
        let r = setting_row(i);
        rounded(fb, r, 6, PANEL);
        if selected {
            // The row the wheel changes.
            outline_selected(fb, r);
        }
        let (x, y) = (r.top_left.x + 10, r.top_left.y);
        let label = match setting {
            Setting::LowestBatCall => "Lowest bat call",
            _ => "Listen to",
        };
        text(fb, &BODY, label, x, y + 18, LEFT, TEXT_MUTED);
        if *setting == Setting::LowestBatCall {
            text(fb, &VALUE, format_args!("{:.0} kHz", app.lowest_bat_call_hz() / 1000.0), x, y + 38, LEFT, TEXT);
        } else {
            let value = match (app.feed(), app.inputs(), app.source()) {
                (Feed::Mix, _, MonitorSource::Both) => "MANUAL's mix",
                (Feed::Mix, _, MonitorSource::First) => "Mix, left side",
                (Feed::Mix, _, MonitorSource::Second) => "Mix, right side",
                (Feed::Inputs, [None, _], _) => "No input",
                (Feed::Inputs, [Some(first), None], _) | (Feed::Inputs, [Some(first), _], MonitorSource::First) => first.long,
                (Feed::Inputs, [_, Some(second)], MonitorSource::Second) => second.long,
                (Feed::Inputs, [Some(first), Some(second)], MonitorSource::Both) => match app.input_kind() {
                    InputKind::Playback => "Left and right",
                    // EXT IN's short names are not numbers.
                    InputKind::Live if is_number(first.short) && is_number(second.short) => "",
                    InputKind::Live => "Both inputs",
                },
            };
            if value.is_empty() {
                let [first, second] = app.inputs().map(|input| input.map_or("", |input| input.short));
                text(fb, &VALUE, format_args!("Inputs {first} and {second}"), x, y + 38, LEFT, TEXT);
            } else {
                text(fb, &VALUE, value, x, y + 38, LEFT, TEXT);
            }
        }
        if *setting == Setting::LowestBatCall {
            text(fb, &SMALL, "Sounds peaking below this are not counted as bat calls.", x, y + 58, LEFT, TEXT_MUTED);
        } else {
            text(fb, &SMALL, format_args!("{}", listen_help(app)), x, y + 58, LEFT, TEXT_MUTED);
        }
        // With one choice, Listen to has nothing to change.
        if *setting == Setting::ListenTo && !app.listen_has_choices() {
            continue;
        }
        for (b, plus) in [(setting_minus(i), false), (setting_plus(i), true)] {
            rounded(fb, b, 4, BUTTON);
            let c = b.center();
            fill(fb, Rectangle::with_center(c, Size::new(14, 2)), TEXT);
            if plus {
                fill(fb, Rectangle::with_center(c, Size::new(2, 14)), TEXT);
            }
        }
    }
    // Lined up with the text in the panels above.
    text(fb, &BODY_BOLD, "Sounds to mark", 16, SWITCHES_HEADING, LEFT, TEXT);
    text(fb, &SMALL, "Marked on waterfall, played in TE and Auto volume Bats only.", 16, SWITCHES_HELP, LEFT, TEXT_MUTED);
}
