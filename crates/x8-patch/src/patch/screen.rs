use core::fmt::Write as _;
use core::ptr;
use core::sync::atomic::{Ordering::Acquire, Ordering::Relaxed, Ordering::Release};

use bat_app::fb::Rgb565Buffer;
use bat_app::{AutoVolume, BatApp, Feed, InputKind, MixSilence, Mode, MonitorSource, Recorder, Screen};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::{FontRenderer, fonts};

use super::inputs::{PLAYBACK_LEFT, PLAYBACK_RIGHT, on_tracks_muted, selected_inputs, tracks_on};
use super::audio::OUTPUT_GAIN;
use super::itcm::load_itcm;
use super::state::{
    APP, APP_READY, AUDIO_INSTALLED, AUDIO_SLOT_UNEXPECTED, AUDIO_STATE, BACKLOG_DROPPED,
    BAT_ACTIVE, BLOCKS, BUFFERS, CALLS_PER_S, CYCLES_PER_S, DEBUG_SHOWN, DRAW_PERMILLE, DSP_PERMILLE,
    DUMMY_LCDIF, FRAME, HEAR_MIX, HEARD_WORDS, HOOK_CYCLES_PER_S, IN_DROPPED, IN_READ, IN_WRITE,
    LAST_EVENT, LAUNCH_AT, MIX_FILE_OFF, LOAD_WINDOW_START, LAUNCH_FROM, LAUNCHING, MUTE_SEEN, MUTE_SEEN_AT, NO_APP, NO_INPUT_WORDS, OUT_LAST, OUT_READ,
    OUT_WRITE, PENDING, PLAYBACK, POPUP_HIDDEN, PRIMED, SAVED_BUFFER, SAVED_LITERAL, SCREEN_APP, SESSION,
    SOURCE_PEAK, STOCK_CYCLES_PER_S, TASK, TEST_DSP_MAX, TEST_SETTINGS, TEST_START_BLOCKS, TEST_START_DRY,
    TEST_START_BACKLOG, TEST_START_LOST, TEST_START_SKIPPED, UNDERRUNS, WORD_PEAKS, WORK_ENTER,
};
use super::task::{app, start_task, task_running};
use super::{app_byte, blocks_per_second, rate_class, read_byte, spare, stock, system_state};

const SAMPLE_RATE: f32 = 192_000.0;

/// Open the bat screen on task 1's next pass through `on_message`. The only
/// way into `enter_work`, so it never runs with the screen already up. While
/// a pop-up is open, wait for it to close.
pub(super) fn request_enter() {
    if BAT_ACTIVE.load(Relaxed) {
        return;
    }
    if system_state() == stock::STATE_POPUP {
        SCREEN_APP.store(app_byte() as u32, Relaxed);
        POPUP_HIDDEN.store(true, Relaxed);
        return;
    }
    PENDING.store(WORK_ENTER, Relaxed);
}

/// Open the bat screen once a tap on a bat tile has taken effect, close it when
/// the app it was opened in closes, and take it back after a pop-up.
pub(super) fn follow_app() {
    let app = app_byte() as u32;
    if LAUNCHING.load(Acquire) {
        let waited = BLOCKS.load(Relaxed).wrapping_sub(LAUNCH_AT.load(Relaxed));
        // The app byte changes when the C core has switched; if it already
        // said MANUAL, give the switch half a second.
        let switched = app != LAUNCH_FROM.load(Relaxed) && app != 0;
        if switched || (app == stock::C_MANUAL as u32 && waited >= blocks_per_second() / 2) {
            LAUNCHING.store(false, Relaxed);
            request_enter();
        } else if waited >= 5 * blocks_per_second() {
            LAUNCHING.store(false, Relaxed);
        }
    }
    let owner = SCREEN_APP.load(Relaxed);
    if owner != NO_APP && app != owner {
        if BAT_ACTIVE.load(Relaxed) {
            leave_bat_screen();
        }
        SCREEN_APP.store(NO_APP, Relaxed);
        POPUP_HIDDEN.store(false, Relaxed);
    }
    if POPUP_HIDDEN.load(Acquire) && system_state() != stock::STATE_POPUP {
        POPUP_HIDDEN.store(false, Relaxed);
        request_enter();
    }
}

/// Build the app (the first time), start feeding it, and show its screen.
pub(super) extern "C" fn enter_work() {
    if !APP_READY.load(Relaxed) {
        load_itcm();
        // SAFETY: spare SDRAM no stock code uses (see `spare`).
        let te = unsafe { core::slice::from_raw_parts_mut(spare::TE_STORAGE as *mut f32, spare::TE_SAMPLES) };
        // SAFETY: zeroing the buffers in place makes them a valid Buffers.
        let buffers = unsafe {
            ptr::write_bytes(BUFFERS.0.get(), 0, 1);
            (*BUFFERS.0.get()).assume_init_mut()
        };
        // SAFETY: init_in_place writes every field; only task 1 uses APP.
        unsafe { BatApp::init_in_place((*APP.0.get()).as_mut_ptr(), SAMPLE_RATE, te, buffers) };
        // SAFETY: just built; no other context uses it yet.
        let app = unsafe { app() };
        app.set_modes(&Mode::WITHOUT_PITCH);
        app.set_recorder(Recorder::Stopped, 0);
        app.set_debug_available(true);
        app.set_auto_level_target(AUTO_LEVEL_OUTPUT / OUTPUT_GAIN);
        app.set_mix_available(true);
        app.set_feed(Feed::Mix);
        APP_READY.store(true, Relaxed);
        start_task();
    }
    SESSION.store(SESSION.load(Relaxed).wrapping_add(1), Release);
    SCREEN_APP.store(app_byte() as u32, Relaxed);
    MUTE_SEEN.store(false, Relaxed);
    // The audio hook leaves the output alone while the bat screen is closed. Empty the
    // output ring and start with the output stopped, so nothing from the last opening
    // plays in the time before the DSP task wakes and runs `new_session` (up to 20 ms).
    OUT_LAST[0].store(0, Relaxed);
    OUT_LAST[1].store(0, Relaxed);
    PRIMED.store(false, Relaxed);
    // Rounded down because the audio hook needs a multiple of 4 here.
    OUT_READ.store(OUT_WRITE.load(Relaxed) & !3, Release);

    // Take over the display: Qt's present function now writes to a dummy,
    // and the display shows the patch's buffer.
    // SAFETY: the literal and registers named in `stock`; task 1 only.
    unsafe {
        SAVED_BUFFER.store(ptr::read_volatile(stock::LCDIF_CUR_BUF) as u32, Relaxed);
        SAVED_LITERAL.store(ptr::read_volatile(stock::PRESENT_LCDIF_LITERAL) as u32, Relaxed);
        (*DUMMY_LCDIF.0.get()).write([0; 32]);
        ptr::write_volatile(stock::PRESENT_LCDIF_LITERAL, DUMMY_LCDIF.0.get() as usize);
    }
    if task_running() {
        // The DSP task may still be finishing a chunk from the last opening,
        // so it draws the first frame; until then the display is blank.
        for i in 0..stock::WIDTH * stock::HEIGHT / 2 {
            // SAFETY: the display buffer (see `spare`).
            unsafe { ptr::write_volatile((spare::DISPLAY as *mut u32).add(i), 0) };
        }
    } else {
        // SAFETY: no other context uses the app without a DSP task.
        render();
    }
    BAT_ACTIVE.store(true, Release);
    // SAFETY: the display controller's next buffer register.
    unsafe { ptr::write_volatile(stock::LCDIF_NEXT_BUF, spare::DISPLAY) };
}

/// Give the display back to Qt and stop feeding the app.
pub(super) fn leave_bat_screen() {
    BAT_ACTIVE.store(false, Release);
    // SAFETY: as in enter_work.
    unsafe {
        let literal = SAVED_LITERAL.load(Relaxed) as usize;
        ptr::write_volatile(
            stock::PRESENT_LCDIF_LITERAL,
            if literal == stock::LCDIF_CTRL_SET { literal } else { stock::LCDIF_CTRL_SET },
        );
        // The buffer Qt last presented while the bat screen was up, or the
        // one it showed before.
        let presented = (*DUMMY_LCDIF.0.get()).assume_init_ref()[0x13] as usize;
        let back = if stock::FRAMEBUFFERS.contains(&presented) {
            presented
        } else {
            SAVED_BUFFER.load(Relaxed) as usize
        };
        if stock::FRAMEBUFFERS.contains(&back) {
            ptr::write_volatile(stock::LCDIF_NEXT_BUF, back);
        }
    }
}

/// From the DSP task, when a pop-up opens under the bat screen: pop-ups
/// install their own key handler, so the key hooks cannot see them. Give the
/// display (and touch) back so the pop-up can be read and answered; task 1
/// takes it again once the pop-up closes (`follow_app`). BAT_ACTIVE is cleared
/// first and POPUP_HIDDEN set last, so task 1 cannot reopen the screen halfway.
pub(super) fn hide_for_popup() {
    leave_bat_screen();
    POPUP_HIDDEN.store(true, Release);
}

/// Where automatic headphone level aims, as the output after `OUTPUT_GAIN`:
/// half of its ceiling, -12 dBFS at the stock headphone gain.
const AUTO_LEVEL_OUTPUT: f32 = 0.25;

/// Draw the app off-screen and show it.
pub(super) fn render() {
    draw_frame();
    show_frame();
}

/// Draw the app off-screen, with the debug figures if they are on.
pub(super) fn draw_frame() {
    // SAFETY: the off-screen frame, used only by the context running the app.
    let pixels = unsafe { &mut *(*FRAME.0.get()).as_mut_ptr() };
    let mut fb = Rgb565Buffer::new(pixels, stock::WIDTH, stock::HEIGHT);
    // SAFETY: built by enter_work.
    let app = unsafe { app() };
    update_status(app);
    app.render(&mut fb);
    DEBUG_SHOWN.store(app.debug_info(), Relaxed);
    if app.debug_info() && app.screen() != Screen::Settings {
        draw_debug(&mut fb, app);
    }
}

/// Copy the off-screen frame to the display buffer, in one block copy, so the
/// writes to the uncached display buffer go out in bursts.
pub(super) fn show_frame() {
    // SAFETY: both buffers hold 320 x 480 pixels and do not overlap; the
    // off-screen frame is used only by the context running the app.
    unsafe { ptr::copy_nonoverlapping((*FRAME.0.get()).as_ptr().cast::<u16>(), spare::DISPLAY as *mut u16, stock::WIDTH * stock::HEIGHT) };
}

/// A peak word as dB below full scale, for the debug figures.
fn db_below_full_scale(peak: u32) -> u32 {
    if peak == 0 { 99 } else { ((-20.0 * libm::log10f(peak as f32 / 2_147_483_648.0)) as u32).min(99) }
}

/// Figures for performance tests, over the last-call panel: what is running,
/// the CPU load, and the dropouts since any of those settings last changed.
/// The second debug line's max and counts restart at each change, so one mode's
/// figures can be read without another's in them.
fn draw_debug(fb: &mut Rgb565Buffer<'_>, app: &BatApp<'static>) {
    let transport = read_byte(stock::TRANSPORT);
    let words = HEARD_WORDS.load(Relaxed);
    let inputs_heard = if app.feed() == Feed::Inputs { (words & 0xf | (words >> 8 & 0xf) << 4) << 24 } else { 0 };
    let settings = app.mode() as u32
        | (app.auto_volume() as u32) << 4
        | ((app.screen() == Screen::ChimeTest) as u32) << 6
        | (app.mix_file() as u32) << 7
        | (app.feed() as u32) << 8
        | (app.source() as u32) << 10
        | (PLAYBACK.load(Relaxed) as u32) << 13
        | (SESSION.load(Relaxed) & 3) << 14
        | (transport as u32) << 16
        | inputs_heard;
    let now = BLOCKS.load(Relaxed);
    let dry = UNDERRUNS.load(Relaxed);
    let lost = IN_DROPPED.load(Relaxed);
    let dropped = BACKLOG_DROPPED.load(Relaxed);
    let dsp = DSP_PERMILLE.load(Relaxed) / 10;
    if TEST_SETTINGS.swap(settings, Relaxed) != settings {
        TEST_START_BLOCKS.store(now, Relaxed);
        TEST_START_DRY.store(dry, Relaxed);
        TEST_START_LOST.store(lost, Relaxed);
        TEST_START_BACKLOG.store(dropped, Relaxed);
        TEST_START_SKIPPED.store(app.skipped_chunks(), Relaxed);
        TEST_DSP_MAX.store(0, Relaxed);
    }
    // Only a window that began after the change counts towards the highest.
    let window_is_current =
        LOAD_WINDOW_START.load(Relaxed).wrapping_sub(TEST_START_BLOCKS.load(Relaxed)) as i32 >= 0;
    let dsp_max = TEST_DSP_MAX.load(Relaxed).max(if window_is_current { dsp } else { 0 });
    TEST_DSP_MAX.store(dsp_max, Relaxed);
    let percent = |part: u32, all: u32| if all == 0 { 0 } else { (part as u64 * 100 / all as u64) as u32 };
    let all = CYCLES_PER_S.load(Relaxed);
    // SAFETY: a word in the stock recorder block.
    let rate = unsafe { ptr::read_volatile(stock::CONVERTER_RATE) };
    let (top, height) = app.last_call_area();
    let background = Rgb565::new(4, 8, 4);
    // The lines replace the whole panel.
    let _ = fb.fill_solid(&Rectangle::new(Point::new(0, top), Size::new(stock::WIDTH as u32, height)), background);

    let mut line = Line::new();
    let _ = write!(line, "{} | ", app.mode().label());
    let [first, second] = app.inputs().map(|input| input.map_or("-", |input| input.short));
    let _ = match (app.feed(), app.source(), app.inputs()[1].is_some()) {
        (Feed::Mix, MonitorSource::Both, _) => write!(line, "mix"),
        (Feed::Mix, MonitorSource::First, _) => write!(line, "mix L"),
        (Feed::Mix, MonitorSource::Second, _) => write!(line, "mix R"),
        (Feed::Inputs, MonitorSource::Both, true) => write!(line, "in {first}+{second}"),
        (Feed::Inputs, MonitorSource::Second, true) => write!(line, "in {second}"),
        (Feed::Inputs, ..) => write!(line, "in {first}"),
    };
    let state = match transport {
        0 => "stopped",
        1 | 2 | 8 => "standby",
        3 => "RECORDING",
        4 | 6 => "paused",
        5 => "closing file",
        7 => "leaving standby",
        9..=11 => "guide",
        _ => "transport ?",
    };
    let _ = write!(line, " | {state} | {} kHz", rate / 1000);
    let _ = match app.auto_volume() {
        AutoVolume::Off => Ok(()),
        AutoVolume::On => write!(line, " | auto on"),
        AutoVolume::BatsOnly => write!(line, " | auto bats only"),
    };
    draw_strip(fb, top, &line, background);

    let mut line = Line::new();
    let _ = write!(
        line,
        "CPU % irq {} hook {} dsp {} (max {}) draw {}",
        percent(STOCK_CYCLES_PER_S.load(Relaxed), all),
        percent(HOOK_CYCLES_PER_S.load(Relaxed), all),
        dsp,
        dsp_max,
        DRAW_PERMILLE.load(Relaxed) / 10,
    );
    draw_strip(fb, top + STRIP_ROWS as i32, &line, background);

    let mut line = Line::new();
    let task = TASK.load(Relaxed) as i32;
    if task <= 0 {
        let _ = write!(line, "no task {task} | ");
    }
    let _ = write!(line, "dry {} lost {} ", dry.wrapping_sub(TEST_START_DRY.load(Relaxed)), lost.wrapping_sub(TEST_START_LOST.load(Relaxed)));
    // Input dropped by the task, as against input that did not fit in the ring. This
    // should almost never happen, so it takes no room on the line until it does.
    let dropped = dropped.wrapping_sub(TEST_START_BACKLOG.load(Relaxed));
    if dropped != 0 {
        let _ = write!(line, "drop {dropped} ");
    }
    let _ = write!(
        line,
        "skip {} in {} s | ring in {} out {}",
        app.skipped_chunks().wrapping_sub(TEST_START_SKIPPED.load(Relaxed)),
        now.wrapping_sub(TEST_START_BLOCKS.load(Relaxed)) / blocks_per_second(),
        IN_WRITE.load(Relaxed).wrapping_sub(IN_READ.load(Relaxed)),
        OUT_WRITE.load(Relaxed).wrapping_sub(OUT_READ.load(Relaxed)),
    );
    draw_strip(fb, top + 2 * STRIP_ROWS as i32, &line, background);

    // Peak level of each input word since the last redraw, in dB below full
    // scale, to check which word carries which input, and of what the app gets.
    let mut line = Line::new();
    let source = db_below_full_scale(SOURCE_PEAK.swap(0, Relaxed));
    if PLAYBACK.load(Relaxed) {
        // SAFETY: words in the stock audio engine and DSP blocks.
        let (engine, playing) = unsafe { (ptr::read_volatile(stock::ENGINE_STATE), ptr::read_volatile(stock::DSP_PLAYING)) };
        let _ = write!(line, "engine {engine} play {playing} | file -{source} dB");
    } else {
        if app.feed() == Feed::Mix {
            let _ = write!(line, "on {:02x} mix -{source} dB:", tracks_on());
        } else {
            let _ = write!(line, "on {:02x} w{},{} dB:", tracks_on(), words & 0xff, words >> 8 & 0xff);
        }
        for peak in &WORD_PEAKS {
            let _ = write!(line, " {}", db_below_full_scale(peak.swap(0, Relaxed)));
        }
    }
    draw_strip(fb, top + 3 * STRIP_ROWS as i32, &line, background);
}

/// The X8 recorder's state and take length, and the record rate, for the
/// status bar and the wrong-rate warning. Reads only; a value may be one
/// update old.
fn update_status(app: &mut BatApp<'static>) {
    let transport = read_byte(stock::TRANSPORT);
    let state = match transport {
        3 => Recorder::Recording,
        4 | 6 => Recorder::Paused,
        // 9 to 11 are MANUAL's recording guide, with the REC LED blinking.
        1 | 2 | 8..=11 => Recorder::Standby,
        _ => Recorder::Stopped,
    };
    // SAFETY: words in the stock recorder block (see `stock`).
    let seconds = unsafe {
        let frames = if read_byte(stock::TAKE_FROZEN_FLAG) != 0 {
            ptr::read_volatile(stock::TAKE_FROZEN)
        } else {
            ptr::read_volatile(stock::TAKE_OFFSET)
                .wrapping_add(ptr::read_volatile(stock::TAKE_COUNT))
                .wrapping_sub(ptr::read_volatile(stock::TAKE_START))
        };
        let rate = ptr::read_volatile(stock::CONVERTER_RATE);
        if matches!(transport, 3..=6) && rate != 0 { (frames / rate) as u64 } else { 0 }
    };
    app.set_recorder(state, seconds);
    // SAFETY: as above.
    let rate = unsafe {
        if !PLAYBACK.load(Relaxed) {
            ptr::read_volatile(stock::RECORD_RATE)
        } else if ptr::read_volatile(stock::DSP_PLAYING) == 1 {
            // The converters run at the file's rate while a file plays or is
            // paused (the DSP play byte stays 1 then), so BAT PLAYBACK checks
            // that rate.
            ptr::read_volatile(stock::CONVERTER_RATE)
        } else {
            // No file is playing, so there is no rate to warn about.
            SAMPLE_RATE as u32
        }
    };
    app.set_input_rate(rate);
    if PLAYBACK.load(Relaxed) {
        app.set_input_kind(InputKind::Playback);
        app.set_mix_available(false);
        app.set_mix_file_available(false);
        MIX_FILE_OFF.store(false, Relaxed);
        app.set_inputs([Some(PLAYBACK_LEFT), Some(PLAYBACK_RIGHT)]);
    } else {
        app.set_input_kind(InputKind::Live);
        app.set_mix_available(true);
        app.set_mix_file_available(true);
        MIX_FILE_OFF.store(!app.mix_file(), Relaxed);
        let [first, second] = selected_inputs();
        let words = first.map_or(NO_INPUT_WORDS, |first| first.word | second.unwrap_or(first).word << 8);
        HEARD_WORDS.store(words, Relaxed);
        app.set_inputs([first, second].map(|input| input.map(|input| input.name)));
    }
    let hear_mix = !PLAYBACK.load(Relaxed) && app.feed() == Feed::Mix;
    HEAR_MIX.store(hear_mix, Relaxed);
    // A page comes first: it mutes whatever REC PAUSE does, and REC does nothing
    // until it is closed.
    let page = read_byte(stock::DSP_PAGE);
    let rec_pause = transport <= 1 && read_byte(stock::MONITOR_WHEN_STOPPED) == 0;
    let muted = hear_mix && on_tracks_muted();
    // The firmware analysis says the inputs are not muted while stopped with REC
    // PAUSE off, except for a few seconds at most while an app opens, which needs no
    // notice. This has not been checked on a unit, so a mute that lasts gets a notice.
    let stopped_mute_lasted = mute_has_lasted(muted && transport == 0 && page == 0 && !rec_pause);
    let silence = if !muted {
        None
    } else if !matches!(page, 0 | 7 | 0xb) || (rec_pause && page != 0) {
        Some(MixSilence::PageOpen)
    } else if rec_pause {
        Some(MixSilence::RecPause)
    } else if stopped_mute_lasted {
        Some(MixSilence::Stopped)
    } else {
        None
    };
    app.set_mix_silence(silence);
}

/// How long the mix stays muted, while stopped with REC PAUSE off, before the notice
/// shows. The firmware mutes every track while it powers up an input rail, which it
/// does while an app opens if the last app left that rail off, as VOICE does with
/// its default tracks. `FUN_80da6f98` holds the mute for 900 ms for a rail and adds the
/// time left from the rail before, up to about 2.9 s. Those figures are from reading
/// the firmware, assuming the timer counts milliseconds. This mute has not been timed
/// on a unit.
const MUTE_NOTICE_AFTER_S: u32 = 4;

/// Whether `muted` has been true for MUTE_NOTICE_AFTER_S, from the first redraw that
/// saw it (a redraw comes at most 100 ms later while stopped).
fn mute_has_lasted(muted: bool) -> bool {
    if !muted {
        MUTE_SEEN.store(false, Relaxed);
        return false;
    }
    let now = BLOCKS.load(Relaxed);
    if !MUTE_SEEN.swap(true, Relaxed) {
        MUTE_SEEN_AT.store(now, Relaxed);
    }
    now.wrapping_sub(MUTE_SEEN_AT.load(Relaxed)) >= MUTE_NOTICE_AFTER_S * blocks_per_second()
}

/// Text in a fixed buffer, for formatting without an allocator.
struct Line {
    buf: [u8; 96],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self { buf: [0; 96], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

const STRIP_ROWS: usize = 13;
const FONT: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR08_tf>();

fn draw_strip(target: &mut Rgb565Buffer<'_>, top: i32, line: &Line, background: Rgb565) {
    let _ = target.fill_solid(&Rectangle::new(Point::new(0, top), Size::new(stock::WIDTH as u32, STRIP_ROWS as u32)), background);
    let _ =
        FONT.render(line.as_str(), Point::new(2, top + 10), VerticalPosition::Baseline, FontColor::Transparent(Rgb565::WHITE), target);
}

/// While the bat screen is closed: a strip across the top of Qt's framebuffers.
pub(super) fn draw_status_strip() {
    if !DEBUG_SHOWN.load(Relaxed) {
        return;
    }
    let mut line = Line::new();
    let audio = match AUDIO_STATE.load(Relaxed) {
        AUDIO_INSTALLED => "audio hooked",
        AUDIO_SLOT_UNEXPECTED => "audio NOT hooked",
        _ => "waiting for audio",
    };
    let event = LAST_EVENT.load(Relaxed);
    let _ = write!(
        line,
        "{audio} | rate {} | key {}/s | app {} st {} rec {} | ev {} {:x}",
        rate_class(),
        CALLS_PER_S.load(Relaxed),
        app_byte(),
        system_state(),
        read_byte(stock::TRANSPORT),
        event >> 16,
        event & 0xffff
    );
    for &fb in &stock::FRAMEBUFFERS {
        // SAFETY: the top rows of a framebuffer the stock firmware allocated.
        let pixels = unsafe { core::slice::from_raw_parts_mut(fb as *mut u16, stock::WIDTH * STRIP_ROWS) };
        draw_strip(&mut Rgb565Buffer::new(pixels, stock::WIDTH, STRIP_ROWS), 0, &line, Rgb565::new(4, 8, 4));
    }
}
