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
use crate::{
    BatApp, CallKind, DEFAULT_CUTOFF_HZ, Meter, Mode, MonitorSource, Screen, Setting, TeStatus, VIEW_HI_HZ,
    VIEW_LO_HZ, WF_COLUMN_MS,
};
use bat_dsp::heterodyne::DEFAULT_COMB_SPACING_HZ;
use bat_dsp::time_expansion::{DEFAULT_POST_MS, DEFAULT_PRE_MS};

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
    title_bar(fb, "BAT DETECTOR", true);
    status_bar(app, fb);
    meters(app, fb);
    waterfall(app, fb);
    snapshot(app, fb);
    value_line(app, fb);
    mode_buttons(app, fb);
}

fn title_bar(fb: Fb, title: &str, menu: bool) {
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

fn status_bar(app: &BatApp, fb: Fb) {
    let recording = app.recording;
    rounded(fb, STATUS, 6, if recording { RECORDING } else { PANEL });
    let cy = STATUS.top_left.y + STATUS.size.height as i32 / 2;
    if recording {
        let _ = Circle::with_center(Point::new(24, cy), 14).into_styled(PrimitiveStyle::with_fill(TEXT)).draw(fb);
    } else {
        fill(fb, Rectangle::with_center(Point::new(24, cy), Size::new(12, 12)), TEXT);
    }
    // Recorded time in the stock style: large digits, small h m s.
    let secs = app.recorded_seconds();
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

fn meter_x(db: f32) -> i32 {
    let t = ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0);
    METER_X + (t * METER_W as f32) as i32
}

fn meters(app: &BatApp, fb: Fb) {
    const SEGMENT: i32 = 4;
    const GAP: i32 = 1;
    for (ch, (meter, &y)) in app.meters.iter().zip(&METER_Y).enumerate() {
        text(fb, &SMALL, if ch == 0 { "3" } else { "4" }, 8, y + 6, LEFT, TEXT_MUTED);
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
        for (x, px) in dst.iter_mut().enumerate() {
            let c = (app.wf_next + x) % WF_W;
            *px = INFERNO_565[app.bufs.wf[c * WF_H + r] as usize];
        }
    }
    let right = WF_X + WF_W as i32 - 1;
    let y_of = |hz: f32| freq_to_y(hz, WF_Y, WF_H);
    let in_view = |hz: f32| (VIEW_LO_HZ..=VIEW_HI_HZ).contains(&hz);

    // Frequency axis: a label and a tick every 10 kHz, the tick in the
    // resistor colour of its tens digit, as Oversample marks frequencies.
    text(fb, &SMALL, "kHz", 2, WF_Y + 7, LEFT, TEXT_MUTED);
    for (k, &color) in RESISTOR.iter().enumerate().skip(2) {
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
        Mode::Fd | Mode::Pitch => {}
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
        Mode::Pitch => {
            text(fb, &BODY, "Divide by", label, y, LEFT, TEXT_MUTED);
            text(fb, &VALUE, format_args!("{:.0}", app.pitch.factor()), x, y, LEFT, TEXT);
        }
    }
}

fn mode_buttons(app: &BatApp, fb: Fb) {
    for (i, mode) in Mode::ALL.iter().enumerate() {
        let r = mode_button(i);
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

fn outline_selected(fb: Fb, r: Rectangle) {
    let outline = PrimitiveStyleBuilder::new().stroke_color(ACCENT).stroke_width(2).build();
    let _ = RoundedRectangle::new(r, CornerRadii::new(Size::new(6, 6))).into_styled(outline).draw(fb);
}

fn settings(app: &BatApp, fb: Fb) {
    for (i, setting) in Setting::ALL.iter().enumerate() {
        let selected = app.selected_setting() == *setting;
        if let Setting::Mark(kind) = *setting {
            let r = switch_row(i - Setting::STEPPED);
            rounded(fb, r, 6, PANEL);
            if selected {
                outline_selected(fb, r);
            }
            let (x, y) = (r.top_left.x, r.top_left.y);
            fill(fb, Rectangle::new(Point::new(x + 8, y + 7), Size::new(4, 24)), tick_colour(kind));
            text(fb, &BODY, mark_name(kind), x + 18, y + 16, LEFT, TEXT);
            let (hx, hy) = (x + 18, y + 31);
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
            // On: blue track, knob at the right. Off: grey track, knob at
            // the left. The white knob's position shows the state.
            let on = app.is_marked(kind);
            let sw = switch(i - Setting::STEPPED);
            let h = sw.size.height;
            let _ = RoundedRectangle::new(sw, CornerRadii::new(Size::new(h / 2, h / 2)))
                .into_styled(PrimitiveStyle::with_fill(if on { ACCENT } else { LINE }))
                .draw(fb);
            let knob_x = if on { sw.top_left.x + sw.size.width as i32 - h as i32 / 2 } else { sw.top_left.x + h as i32 / 2 };
            let _ = Circle::with_center(Point::new(knob_x, sw.center().y), h - 6)
                .into_styled(PrimitiveStyle::with_fill(TEXT))
                .draw(fb);
            continue;
        }
        let r = setting_row(i);
        rounded(fb, r, 6, PANEL);
        if selected {
            // The row the wheel changes.
            outline_selected(fb, r);
        }
        let (x, y) = (r.top_left.x + 10, r.top_left.y);
        let (label, help) = match setting {
            Setting::LowestBatCall => ("Lowest bat call", "Sounds peaking below this are not counted as bat calls."),
            _ => ("Listen to", "Heard in the headphones and shown on the waterfall."),
        };
        text(fb, &BODY, label, x, y + 20, LEFT, TEXT_MUTED);
        if *setting == Setting::LowestBatCall {
            text(fb, &VALUE, format_args!("{:.0} kHz", app.lowest_bat_call_hz() / 1000.0), x, y + 44, LEFT, TEXT);
        } else {
            let value = match app.source {
                MonitorSource::Mix => "Inputs 3 and 4",
                MonitorSource::Input3 => "Input 3",
                MonitorSource::Input4 => "Input 4",
            };
            text(fb, &VALUE, value, x, y + 44, LEFT, TEXT);
        }
        text(fb, &SMALL, help, x, y + 66, LEFT, TEXT_MUTED);
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
    text(fb, &SMALL, "Marked on the waterfall and played in TE if switched on.", 16, SWITCHES_HELP, LEFT, TEXT_MUTED);
    text(fb, &SMALL, "These settings reset when the recorder is turned off.", 16, SETTINGS_NOTE, LEFT, TEXT_MUTED);
    rounded(fb, DONE, 4, ACCENT);
    text(fb, &BODY_BOLD, "Done", DONE.center().x, DONE.top_left.y + 28, CENTER, TEXT);
}
