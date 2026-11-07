//! The CPU test: what it runs, what it keeps, and what it puts back.

use std::boxed::Box;

use super::*;
use crate::screen::{CPU_TEST_COLUMN_GAP, cpu_test_columns, cpu_test_lines};
use crate::{CallKind, HostFigures, Recorder, Screen};

const SECOND: usize = FS as usize;
const ROW_SECONDS: usize = 8;

fn two_inputs(app: &mut BatApp) {
    app.set_inputs([Some(InputName { short: "3", long: "Input 3" }), Some(InputName { short: "4", long: "Input 4" })]);
}

/// Run the app for `seconds`, passing figures five times a second, as the X8 does while
/// recording. `dsp` is each row's CPU share and `lost_each` is added to the lost count each time.
fn run(app: &mut BatApp, seconds: usize, figures: &mut HostFigures, dsp: u32, lost_each: u32) {
    let block = SECOND / 5;
    let (mut first, mut second) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    for _ in 0..seconds * 5 {
        first.fill(0.0);
        second.fill(0.0);
        if app.test_calls_on() {
            app.add_test_calls(&mut first, &mut second);
        }
        app.process(&first, &second, &mut l, &mut r);
        figures.dsp_percent = dsp;
        figures.window = figures.window.wrapping_add(1);
        figures.lost += lost_each;
        app.cpu_test_figures(*figures);
    }
}

fn app_with_storage<'a>(storage: &'a mut [f32], buffers: &'a mut Buffers) -> BatApp<'a> {
    let mut app = BatApp::new(FS, storage, buffers);
    app.set_modes(&Mode::WITHOUT_PITCH);
    app.set_debug_available(true);
    app
}

#[test]
fn it_opens_from_tools_and_goes_back() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    app.open_settings();
    let tools = layout::TOOLS.center();
    app.touch(tools.x, tools.y);
    assert_eq!(app.screen(), Screen::Tools);
    let cpu = layout::TOOLS_CPU_TEST.center();
    app.touch(cpu.x, cpu.y);
    assert_eq!(app.screen(), Screen::CpuTest);
    let back = layout::CPU_TEST_BACK.center();
    app.touch(back.x, back.y);
    assert_eq!(app.screen(), Screen::Tools);
    let back = layout::BACK.center();
    app.touch(back.x, back.y);
    assert_eq!(app.screen(), Screen::Settings);
}

#[test]
fn the_plan_has_a_row_for_each_combination() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.open_cpu_test();
    // HET, COMB and 1:1 twice and FD and TE once, for each Auto volume choice, and FD and COMB
    // stereo with Auto volume on and the decoders.
    assert_eq!(app.cpu_test_rows().len(), (3 * 2 + 2) * 3 + 2);
    // About 3.5 min, as the Tools screen says.
    assert_eq!(app.cpu_test_rows().len() * ROW_SECONDS, 208);
    let start = layout::CPU_TEST_START.center();
    app.touch(start.x, start.y);

    let mut one_input_storage = vec![0.0f32; SECOND];
    let mut one_input_buffers = Box::new(Buffers::new());
    let mut one = app_with_storage(&mut one_input_storage, &mut one_input_buffers);
    one.set_inputs([Some(InputName { short: "3", long: "Input 3" }), None]);
    one.open_cpu_test();
    assert_eq!(one.cpu_test_rows().len(), 5 * 3 + 2);
    assert!(one.cpu_test_rows().iter().all(|row| !row.has_stereo));
}

#[test]
fn a_whole_run_fills_every_row_and_puts_the_settings_back() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.set_mode(Mode::Fd);
    app.set_auto_volume(AutoVolume::On);
    app.set_source(MonitorSource::Second);
    app.set_recorder(Recorder::Recording, 10);
    app.open_cpu_test();
    app.start_cpu_test();
    assert_eq!(app.screen(), Screen::Main);
    assert_eq!(app.mode(), Mode::Het);
    assert_eq!(app.auto_volume(), AutoVolume::Off);

    let mut figures = HostFigures::default();
    let rows = app.cpu_test_rows().len();
    let mut modes_seen = std::vec::Vec::new();
    for _ in 0..rows {
        modes_seen.push((app.mode(), app.auto_volume(), app.is_stereo()));
        run(&mut app, ROW_SECONDS, &mut figures, 40, 3);
    }
    assert!(!app.cpu_test_running());
    assert_eq!(app.screen(), Screen::CpuTest);
    assert_eq!(app.mode(), Mode::Fd);
    assert_eq!(app.auto_volume(), AutoVolume::On);
    assert_eq!(app.source(), MonitorSource::Second);
    assert!(modes_seen.contains(&(Mode::Te, AutoVolume::BatsOnly, false)));
    let last_two: std::vec::Vec<_> = modes_seen[rows - 2..].iter().map(|&(mode, volume, _)| (mode, volume)).collect();
    assert_eq!(last_two, [(Mode::Fd, AutoVolume::On), (Mode::Comb, AutoVolume::On)]);

    for row in app.cpu_test_rows() {
        let result = row.result.expect("every row ran");
        assert_eq!(result.dsp_percent, 40);
        assert_eq!(result.dsp_max, 40);
        assert_eq!(result.recorder, Some(Recorder::Recording));
        // Only the measured part counts: 6 s at 5 redraws a second and 3 lost each time.
        assert!((85..=95).contains(&result.lost), "{}", result.lost);
    }
}

#[test]
fn a_tap_stops_the_test_and_keeps_the_rows_that_ran() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.set_mode(Mode::Comb);
    let before = app.het_lo_hz();
    app.start_cpu_test();
    let mut figures = HostFigures::default();
    run(&mut app, ROW_SECONDS + 1, &mut figures, 30, 0);
    assert!(app.cpu_test_running());
    // The wheel does not retune during a row.
    app.wheel(3);
    assert_eq!(app.het_lo_hz(), before);
    // Where a mode button is: the tap only stops the test.
    let button = layout::mode_button(3, 5).center();
    app.touch(button.x, button.y);
    assert!(!app.cpu_test_running());
    assert_eq!(app.screen(), Screen::CpuTest);
    assert_eq!(app.mode(), Mode::Comb);
    let ran = app.cpu_test_rows().iter().filter(|row| row.result.is_some()).count();
    assert_eq!(ran, 1);
}

#[test]
fn a_pause_in_the_input_stops_the_test() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.set_mode(Mode::Te);
    app.start_cpu_test();
    app.input_lost(0);
    assert!(!app.cpu_test_running());
    assert_eq!(app.mode(), Mode::Te);
}

#[test]
fn a_change_of_recorder_state_shows_in_the_row() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.set_recorder(Recorder::Standby, 0);
    app.start_cpu_test();
    let mut figures = HostFigures::default();
    run(&mut app, 4, &mut figures, 30, 0);
    app.set_recorder(Recorder::Recording, 0);
    run(&mut app, 4, &mut figures, 30, 0);
    let first = app.cpu_test_rows()[0].result.expect("the row ran");
    assert_eq!(first.recorder, None);
}

#[test]
fn the_test_calls_are_classed_as_bat_calls() {
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.start_cpu_test();
    let mut figures = HostFigures::default();
    // A little noise under the calls, as an input has.
    let block = SECOND / 5;
    let mut seed = 1u32;
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    for _ in 0..3 * 5 {
        let mut first: std::vec::Vec<f32> = (0..block)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2e-4
            })
            .collect();
        let mut second = first.clone();
        app.add_test_calls(&mut first, &mut second);
        app.process(&first, &second, &mut l, &mut r);
        app.cpu_test_figures(figures);
        figures.window += 1;
    }
    assert_eq!(app.last_call_kind(), Some(CallKind::Bat));
    let call = app.last_call().expect("a call was detected");
    assert!((35_000.0..=65_000.0).contains(&call.peak_freq_hz), "{}", call.peak_freq_hz);
}

#[test]
fn the_screen_text_fits() {
    for (line, width, room) in cpu_test_lines() {
        assert!(width <= room, "{line}: {width} px in {room} px");
    }
}

#[test]
fn the_table_columns_keep_apart() {
    use embedded_graphics::prelude::Point;
    use u8g2_fonts::FontRenderer;
    use u8g2_fonts::fonts::u8g2_font_helvR08_tf;
    use u8g2_fonts::types::VerticalPosition;

    let small = FontRenderer::new::<u8g2_font_helvR08_tf>();
    let width = |s: &str| small.get_rendered_dimensions(s, Point::zero(), VerticalPosition::Baseline).unwrap().advance.x;
    // The widest row name, with the `*` drawn 1 px after it, ends here. The `*` shows only when
    // the recorder's state changed, and its glyph has room to its right.
    let name_end = 4 + 6 + 1 + width("COMB stereo*");
    for columns in cpu_test_columns() {
        let mut previous_right = None;
        for (right, contents) in columns {
            let widest = contents.iter().map(|s| width(s)).max().unwrap();
            let (before, gap) = previous_right.map_or((name_end, 2), |x| (x, CPU_TEST_COLUMN_GAP));
            assert!(right - widest >= before + gap, "{contents:?}: {} px from the column before", right - widest - before);
            previous_right = Some(right);
        }
        assert!(previous_right.unwrap() <= 314);
    }
}

#[test]
fn the_longest_table_stays_above_the_switch() {
    let last_baseline = layout::CPU_TEST_FIRST_ROW + (26 + 4 - 1) * layout::CPU_TEST_ROW_H;
    assert!(last_baseline < layout::CPU_TEST_CALLS.top_left.y);
}

#[test]
fn a_stage_clock_adds_each_stage_and_rows_keep_their_shares() {
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
    use crate::stages::{Stage, StageClock};

    static TICKS: AtomicU32 = AtomicU32::new(u32::MAX - 5);
    let mut clock = StageClock::new();
    clock.set(&TICKS, 48_000);
    clock.start();
    // Across the counter's wrap.
    TICKS.store(4, Relaxed);
    clock.lap(Stage::Analysis);
    TICKS.store(7, Relaxed);
    clock.lap(Stage::Mode);
    assert_eq!(clock.totals[Stage::Analysis as usize], 10);
    assert_eq!(clock.totals[Stage::Mode as usize], 3);

    // A row of 6 s measured, with a quarter of it in the analysis and 5% in the mode.
    let mut test = crate::cpu_test::CpuTest::new();
    test.plan(&Mode::WITHOUT_PITCH, true);
    test.begin_row(0);
    let rate = FS as u64;
    let figures = HostFigures { dsp_percent: 40, draw_percent: 18, copy_percent: 8, ..Default::default() };
    let settle = crate::cpu_test::SETTLE_S * rate;
    assert!(!test.figures(figures, Recorder::Stopped, 0, &clock, settle, rate));
    let start = clock.totals;
    clock.totals[Stage::Analysis as usize] = start[Stage::Analysis as usize] + 48_000 * 6 / 4;
    clock.totals[Stage::Mode as usize] = start[Stage::Mode as usize] + 48_000 * 6 / 20;
    let end = (crate::cpu_test::SETTLE_S + crate::cpu_test::MEASURE_S) * rate;
    let next = HostFigures { window: 1, ..figures };
    assert!(test.figures(next, Recorder::Stopped, 0, &clock, end, rate));
    let result = test.rows[0].result.expect("the row finished");
    let stages = result.stages.expect("the clock was running");
    assert_eq!(stages[Stage::Analysis as usize], 25);
    assert_eq!(stages[Stage::Mode as usize], 5);
    assert_eq!(stages[Stage::Input as usize], 0);
    assert_eq!(result.copy_percent, 8);
}

#[test]
fn the_view_button_shows_only_with_a_stage_clock_and_switches_tables() {
    use core::sync::atomic::AtomicU32;
    use crate::cpu_test::View;

    static TICKS: AtomicU32 = AtomicU32::new(0);
    let mut storage = vec![0.0f32; SECOND];
    let mut buffers = Box::new(Buffers::new());
    let mut app = app_with_storage(&mut storage, &mut buffers);
    two_inputs(&mut app);
    app.set_debug_available(true);
    app.open_cpu_test();
    let view = layout::CPU_TEST_VIEW.center();
    app.touch(view.x, view.y);
    assert_eq!(app.cpu_test.view, View::Totals, "no clock, so no button");
    app.set_stage_clock(&TICKS, 48_000);
    app.touch(view.x, view.y);
    assert_eq!(app.cpu_test.view, View::Stages);
    let mut pixels = vec![0u16; WIDTH * HEIGHT];
    app.render(&mut Rgb565Buffer::new(&mut pixels, WIDTH, HEIGHT));
    app.touch(view.x, view.y);
    assert_eq!(app.cpu_test.view, View::Totals);
}
