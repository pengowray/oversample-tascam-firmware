//! The chime test screen: what it reads, and what it leaves alone.

use super::*;
use crate::chime_test::FailureReason;
use crate::screen::{CLOCK_Y, COLUMN_X, FAILURE_Y, ROW_H, ROW_Y, chime_test_lines};

use chime::encode as chime_gen;

const SECOND: usize = FS as usize;
/// 2026-10-26 07:33:20 UTC.
const UNIX: u32 = 1_793_000_000;

/// A chime with its melody, as the page renders it.
fn chime_at(unix: u32, tz_minutes: i16) -> Vec<f32> {
    let bytes = chime_gen::time_payload(unix, tz_minutes);
    let render = chime_gen::render(FS as u32, &bytes, chime_gen::tune_for_payload(bytes.len()));
    render.mix().iter().map(|&v| v as f32).collect()
}

fn silence(n: usize) -> Vec<f32> {
    vec![0.0; n]
}

/// Silence, a chime that starts at `lead_in` samples, then silence for `tail` samples.
fn chime_in_silence(lead_in: usize, chime: &[f32], tail: usize) -> Vec<f32> {
    let mut x = silence(lead_in);
    x.extend_from_slice(chime);
    x.extend(silence(tail));
    x
}

fn run_two(app: &mut BatApp, first: &[f32], second: &[f32]) {
    let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
    for (a, b) in first.chunks(1024).zip(second.chunks(1024)) {
        app.process(a, b, &mut l[..a.len()], &mut r[..a.len()]);
    }
}

/// The pixels of a part of the screen, from `y0` to `y1` and from `x0` to `x1`.
fn area(pixels: &[u16], x0: i32, x1: i32, y0: i32, y1: i32) -> Vec<u16> {
    let mut out = Vec::new();
    for y in y0..y1 {
        let row = y as usize * WIDTH;
        out.extend_from_slice(&pixels[row + x0 as usize..row + x1 as usize]);
    }
    out
}

/// The clock's digits.
fn clock_digits(pixels: &[u16]) -> Vec<u16> {
    area(pixels, 40, 280, CLOCK_Y - 52, CLOCK_Y + 2)
}

/// A value in the table: `row` from 0 and `column` from 0.
fn table_value(pixels: &[u16], row: i32, column: usize) -> Vec<u16> {
    let y = ROW_Y + row * ROW_H;
    area(pixels, COLUMN_X[column], COLUMN_X[column] + 90, y - 12, y + 3)
}

fn failure_line(pixels: &[u16]) -> Vec<u16> {
    area(pixels, 16, 304, FAILURE_Y - 12, FAILURE_Y + 3)
}

fn clock_text(app: &BatApp) -> Option<std::string::String> {
    use std::format;
    let clock = app.chime.clock(app.samples)?;
    let t = clock.local;
    Some(format!("{}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second))
}

#[test]
fn the_screen_opens_from_settings_and_goes_back() {
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_settings();
    let tests = layout::tab(2, 3).center();
    app.touch(tests.x, tests.y);
    assert_eq!(app.settings_page(), SettingsPage::Tests);
    let chime_test = layout::TESTS_CHIME_TEST.center();
    assert!(app.is_tappable(chime_test.x, chime_test.y));
    app.touch(chime_test.x, chime_test.y);
    assert_eq!(app.screen(), Screen::ChimeTest);
    assert!(draw(&app).iter().any(|&p| p != 0));
    // Nothing else on this screen responds.
    let before = app.het.lo();
    app.wheel(3);
    assert_eq!(app.het.lo(), before);
    assert!(!app.is_tappable(160, 100));
    app.touch(160, 100);
    assert_eq!(app.screen(), Screen::ChimeTest);
    // Set X8 clock does nothing before a chime has given a time.
    let set = layout::CLOCK_SET.center();
    assert!(!app.is_tappable(set.x, set.y));
    app.touch(set.x, set.y);
    assert_eq!(app.take_phone_time(), None);
    let back = layout::CLOCK_BACK.center();
    assert!(app.is_tappable(back.x, back.y));
    app.touch(back.x, back.y);
    assert_eq!(app.screen(), Screen::Settings);
    assert_eq!(app.settings_page(), SettingsPage::Tests);
    // Done still closes settings, and HOME closes the test.
    let done = layout::DONE.center();
    app.touch(done.x, done.y);
    assert_eq!(app.screen(), Screen::Main);
    app.open_chime_test();
    app.button(Button::Home);
    assert_eq!(app.screen(), Screen::Main);
}

#[test]
fn a_chime_on_one_input_shows_the_time_and_the_clock_ticks() {
    let chime = chime_at(UNIX, 660);
    // The chime starts 0.5 s in, and the time in it is the time of its start.
    let first = chime_in_silence(SECOND / 2, &chime, SECOND);
    let second = silence(first.len());
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    assert!(clock_text(&app).is_none());
    run_two(&mut app, &first, &second);

    assert_eq!(app.chime.channels[0].read, 1);
    assert_eq!(app.chime.channels[1].read, 0);
    assert_eq!(app.chime.channels[1].failed, 0);
    assert!(app.chime.failure.is_none());
    let carrier = app.chime.channels[0].carrier_dbfs.expect("the level of the chime");
    assert!((-20.0..-10.0).contains(&carrier), "{carrier}");
    // Since the chime started: the rest of the input after 0.5 s.
    let elapsed = (first.len() - SECOND / 2) / SECOND;
    let expected = chime::format::LocalTime::from_epoch_seconds(UNIX as i64 + 660 * 60 + elapsed as i64);
    let clock = app.chime.clock(app.samples).unwrap();
    assert_eq!(clock.local, expected);
    assert_eq!(clock.timezone_minutes, 660);

    // 3 s more of sound is 3 s more on the clock.
    run_two(&mut app, &silence(3 * SECOND), &silence(3 * SECOND));
    let later = app.chime.clock(app.samples).unwrap();
    assert_eq!(later.local.to_epoch_seconds() - clock.local.to_epoch_seconds(), 3);
    assert_eq!(later.read_s_ago, clock.read_s_ago + 3);
    // The decoder finished about the end of the chime, and 4 s of silence have followed.
    assert_eq!(later.read_s_ago, 4);
    assert!(draw(&app).iter().any(|&p| p != 0));
}

#[test]
fn the_clock_keeps_time_over_a_long_run() {
    let chime = chime_at(UNIX, 0);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &chime, &silence(chime.len()));
    let quiet = silence(SECOND);
    for _ in 0..200 {
        run_two(&mut app, &quiet, &quiet);
    }
    let seconds = (app.samples / SECOND as u64) as i64;
    assert_eq!(app.chime.clock(app.samples).unwrap().local.to_epoch_seconds(), UNIX as i64 + seconds);
}

#[test]
fn the_screen_shows_the_clock_and_each_inputs_results() {
    let chime = chime_at(UNIX, 0);
    let x = chime_in_silence(0, &chime, SECOND / 4);
    let quiet = silence(x.len());
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    let empty = draw(&app);
    run_two(&mut app, &x, &quiet);
    let read = draw(&app);
    assert_ne!(clock_digits(&empty), clock_digits(&read), "the digits replace the dashes");
    // Chimes read: 1 on the first input, 0 on the second. So the two columns differ in
    // that row, and the second column has not changed since the screen opened.
    assert_ne!(table_value(&read, 0, 0), table_value(&read, 0, 1));
    assert_ne!(table_value(&read, 0, 0), table_value(&empty, 0, 0));
    assert_eq!(table_value(&read, 0, 1), table_value(&empty, 0, 1));
    // The same for the carrier level.
    assert_ne!(table_value(&read, 2, 0), table_value(&empty, 2, 0));
    assert_eq!(table_value(&read, 2, 1), table_value(&empty, 2, 1));
    // The clock ticks on the screen.
    run_two(&mut app, &silence(SECOND), &silence(SECOND));
    assert_ne!(clock_digits(&read), clock_digits(&draw(&app)));

    // A failure names its own input.
    let cut = chime[..chime.len() / 2].iter().copied().chain(silence(SECOND)).collect::<Vec<f32>>();
    let quiet = silence(cut.len());
    let failure_on = |first: &[f32], second: &[f32]| {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.open_chime_test();
        let before = failure_line(&draw(&app));
        assert!(before.iter().all(|&p| p == before[0]), "nothing shows before a failure");
        run_two(&mut app, first, second);
        failure_line(&draw(&app))
    };
    let (on_first, on_second) = (failure_on(&cut, &quiet), failure_on(&quiet, &cut));
    assert!(on_first.iter().any(|&p| p != on_first[0]));
    assert_ne!(on_first, on_second);
}

#[test]
fn the_levels_follow_the_sound() {
    let chime = chime_at(UNIX, 0);
    let mut rng = 1u32;
    let noisy: Vec<f32> = (0..chime.len() + SECOND / 2)
        .map(|i| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((rng >> 8) as f32 / 8_388_608.0 - 1.0) * 0.001;
            let at = i.wrapping_sub(SECOND / 2);
            noise + chime.get(at).copied().unwrap_or(0.0)
        })
        .collect();
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    assert_eq!(app.chime.channels[0].band_dbfs, -120.0);
    run_two(&mut app, &noisy, &silence(noisy.len()));
    let channel = app.chime.channels[0];
    assert_eq!(channel.read, 1);
    let snr = channel.snr_db.expect("a chime was read");
    assert!(snr > 10.0, "the noise was measured: {snr}");
    assert!(channel.band_dbfs > -60.0, "{}", channel.band_dbfs);
    let during = channel.band_dbfs;
    // It falls after the chime, with a time constant of 1 s.
    run_two(&mut app, &silence(3 * SECOND), &silence(3 * SECOND));
    assert!(app.chime.channels[0].band_dbfs < during - 6.0, "{} after {during}", app.chime.channels[0].band_dbfs);
}

#[test]
fn a_chime_with_lost_frames_in_it_gives_no_time() {
    let chime = chime_at(UNIX, 0);
    // Short enough for the chime to pass its CRC, as the review of this code found.
    for lost in [32, 256] {
        let at = chime.len() / 2;
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.open_chime_test();
        run_two(&mut app, &chime[..at], &silence(at));
        app.input_lost(lost as u32);
        let rest = chime_in_silence(0, &chime[at + lost..], SECOND / 4);
        run_two(&mut app, &rest, &silence(rest.len()));
        assert_eq!(app.chime.channels[0].read, 0, "{lost}");
        assert!(clock_text(&app).is_none(), "{lost}");
        assert_eq!(app.chime.channels[0].failed, 1, "{lost}");
        // The next chime reads normally, and its time is right.
        let next = chime_in_silence(0, &chime, 0);
        let start = app.samples;
        run_two(&mut app, &next, &silence(next.len()));
        assert_eq!(app.chime.channels[0].read, 1, "{lost}");
        let since = ((app.samples - start) / SECOND as u64) as i64;
        assert_eq!(app.chime.clock(app.samples).unwrap().local.to_epoch_seconds(), UNIX as i64 + since, "{lost}");
    }
}

#[test]
fn the_time_is_right_when_frames_were_lost_before_the_chime() {
    // The app's count includes lost frames, and the decoder's does not.
    let chime = chime_at(UNIX, 0);
    let first = chime_in_silence(0, &chime, SECOND / 4);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &silence(4096), &silence(4096));
    app.input_lost(10 * SECOND as u32);
    let start = app.samples;
    run_two(&mut app, &first, &silence(first.len()));
    assert_eq!(app.chime.channels[0].read, 1);
    let seconds = ((app.samples - start) / SECOND as u64) as i64;
    assert_eq!(app.chime.clock(app.samples).unwrap().local.to_epoch_seconds(), UNIX as i64 + seconds);
}

#[test]
fn each_input_has_its_own_results_and_a_later_time_replaces_an_earlier_one() {
    let (a, b) = (chime_at(UNIX, 60), chime_at(UNIX + 100, -300));
    let first = chime_in_silence(0, &a, 2 * SECOND);
    let second = chime_in_silence(SECOND + SECOND / 2, &b, SECOND / 2);
    let n = first.len().min(second.len());
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &first[..n], &second[..n]);
    assert_eq!(app.chime.channels[0].read, 1);
    assert_eq!(app.chime.channels[1].read, 1);
    let clock = app.chime.clock(app.samples).unwrap();
    assert_eq!(clock.timezone_minutes, -300);
    let since_b = ((n - (SECOND + SECOND / 2)) / SECOND) as i64;
    assert_eq!(clock.local.to_epoch_seconds(), (UNIX + 100) as i64 - 300 * 60 + since_b);
}

#[test]
fn it_listens_only_to_blocks_that_can_differ() {
    let name = |short, long| Some(InputName { short, long });
    let chime = chime_at(UNIX, 0);
    let x = chime_in_silence(0, &chime, SECOND / 4);
    let quiet = silence(x.len());
    let ext_in = [name("EL", "EXT IN left"), name("ER", "EXT IN right")];
    let cases = [
        // One input on: both blocks carry it, so one decoder runs and the second block is ignored.
        (Feed::Inputs, [name("3", "Input 3"), None], &x, &x, [1, 0], ["Input 3", ""]),
        (Feed::Inputs, [name("EL", "EXT IN left"), None], &x, &x, [1, 0], ["EXT IN left", ""]),
        // Two inputs: a decoder for each.
        (Feed::Inputs, [name("3", "Input 3"), name("4", "Input 4")], &x, &x, [1, 1], ["Input 3", "Input 4"]),
        (Feed::Inputs, [name("3", "Input 3"), name("4", "Input 4")], &quiet, &x, [0, 1], ["Input 3", "Input 4"]),
        (Feed::Inputs, ext_in, &quiet, &x, [0, 1], ["EXT IN left", "EXT IN right"]),
        // The mix's sides differ, even with one input on, which may be panned to the right.
        (Feed::Mix, [name("3", "Input 3"), None], &quiet, &x, [0, 1], ["Mix, left side", "Mix, right side"]),
        (Feed::Mix, [name("3", "Input 3"), None], &x, &x, [1, 1], ["Mix, left side", "Mix, right side"]),
        (
            Feed::Mix,
            [name("3", "Input 3"), name("4", "Input 4")],
            &quiet,
            &x,
            [0, 1],
            ["Mix, left side", "Mix, right side"],
        ),
        // No input on: nothing to listen to.
        (Feed::Inputs, [None, None], &x, &x, [0, 0], ["", ""]),
        (Feed::Mix, [None, None], &x, &x, [0, 0], ["", ""]),
    ];
    for (feed, inputs, first, second, read, names) in cases {
        let mut storage = vec![0.0f32; 1024];
        let mut app = BatApp::new(FS, &mut storage, buffers());
        app.set_mix_available(feed == Feed::Mix);
        app.set_feed(feed);
        app.set_inputs(inputs);
        app.open_chime_test();
        run_two(&mut app, first, second);
        assert_eq!(app.chime.channels.map(|c| c.read), read, "{feed:?} {inputs:?}");
        assert_eq!(screen::chime_columns(&app).map(|n| n.unwrap_or("")), names, "{feed:?} {inputs:?}");
        assert!(draw(&app).iter().any(|&p| p != 0));
    }
    // BAT PLAYBACK's left and right.
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_input_kind(InputKind::Playback);
    app.set_inputs([name("L", "Left"), name("R", "Right")]);
    app.open_chime_test();
    run_two(&mut app, &quiet, &x);
    assert_eq!(app.chime.channels.map(|c| c.read), [0, 1]);
    assert_eq!(screen::chime_columns(&app), [Some("Left"), Some("Right")]);
}

#[test]
fn a_chime_cut_off_is_a_failure_until_a_chime_is_read() {
    let chime = chime_at(UNIX, 0);
    let mut first = chime[..chime.len() / 2].to_vec();
    first.extend(silence(SECOND));
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &first, &silence(first.len()));
    assert_eq!(app.chime.channels[0].failed, 1);
    let failure = app.chime.failure.expect("the failure");
    assert_eq!(failure.channel, 0);
    assert_eq!(failure.reason, FailureReason::Decoder(chime::UnreadableReason::SignalLost));
    assert!(clock_text(&app).is_none());
    assert!(draw(&app).iter().any(|&p| p != 0));

    let whole = chime_in_silence(0, &chime, SECOND / 4);
    run_two(&mut app, &whole, &silence(whole.len()));
    assert_eq!(app.chime.channels[0].read, 1);
    assert!(app.chime.failure.is_none());
    assert!(clock_text(&app).is_some());
}

#[test]
fn opening_again_or_a_new_session_starts_a_new_test() {
    let chime = chime_at(UNIX, 0);
    let x = chime_in_silence(0, &chime, SECOND / 4);
    let quiet = silence(x.len());
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &x, &quiet);
    assert_eq!(app.chime.channels[0].read, 1);
    app.open_chime_test();
    assert_eq!(app.chime.channels[0].read, 0);
    assert!(clock_text(&app).is_none());

    run_two(&mut app, &x, &quiet);
    assert_eq!(app.chime.channels[0].read, 1);
    // Lost frames carry on, because the clock can still count them.
    app.input_lost(517);
    assert_eq!(app.chime.channels[0].read, 1);
    assert!(clock_text(&app).is_some());
    // A return to the bat screen: the time away was not counted.
    app.input_lost(0);
    assert_eq!(app.chime.channels[0].read, 0);
    assert!(clock_text(&app).is_none());
    // And it still reads.
    run_two(&mut app, &x, &quiet);
    assert_eq!(app.chime.channels[0].read, 1);
}

#[test]
fn with_the_screen_closed_nothing_listens() {
    let chime = chime_at(UNIX, 0);
    let x = chime_in_silence(0, &chime, SECOND / 4);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    run_two(&mut app, &x, &x);
    assert_eq!(app.chime.channels[0].read, 0);
    // Back to settings stops it too, keeping what it read until the next opening.
    app.open_chime_test();
    run_two(&mut app, &x, &x);
    assert_eq!(app.chime.channels[0].read, 1);
    app.open_settings();
    run_two(&mut app, &x, &x);
    assert_eq!(app.chime.channels[0].read, 1);
}

#[test]
fn the_headphones_carry_on_and_the_analysis_stops() {
    let chime = chime_at(UNIX, 0);
    let x = chime_in_silence(0, &chime, SECOND / 4);
    for mode in [Mode::Het, Mode::Comb, Mode::Thru, Mode::Fd] {
        let outputs = [false, true].map(|open| {
            let mut storage = vec![0.0f32; 1024];
            let mut app = BatApp::new(FS, &mut storage, buffers());
            app.set_mode(mode);
            if open {
                app.open_chime_test();
            }
            let (mut left, mut right) = (vec![0.0; x.len()], vec![0.0; x.len()]);
            for ((a, l), r) in x.chunks(1024).zip(left.chunks_mut(1024)).zip(right.chunks_mut(1024)) {
                app.process(a, a, l, r);
            }
            (left, right)
        });
        assert!(outputs[0].0.iter().zip(&outputs[1].0).all(|(a, b)| a.to_bits() == b.to_bits()), "{mode:?}");
        assert!(outputs[0].1.iter().zip(&outputs[1].1).all(|(a, b)| a.to_bits() == b.to_bits()), "{mode:?}");
    }

    let x = signal();
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    mark_everything(&mut app);
    app.open_chime_test();
    run_two(&mut app, &x, &x);
    assert!(app.last_call().is_none(), "no call is detected on this screen");
    assert_eq!(app.skipped_chunks(), 0, "the debug figure counts only the host's skips");
    assert!(app.bufs.wf.chunks(WF_H).any(|column| column.iter().all(|&v| v == WF_NO_DATA)), "grey columns");
    // Back on the main screen, the analysis restarts.
    app.button(Button::Home);
    run_two(&mut app, &x, &x);
    assert!(app.last_call().is_some());
}

#[test]
fn a_call_in_progress_ends_when_the_screen_opens() {
    let mut storage = vec![0.0f32; FS as usize];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_mode(Mode::Te);
    let calls = train(1.5, 0.5, 8);
    run_two(&mut app, &calls, &calls);
    let sounding: Vec<f32> =
        (0..SECOND / 100).map(|i| 0.3 * (core::f32::consts::TAU * 45_000.0 * i as f32 / FS).sin()).collect();
    run_two(&mut app, &sounding, &sounding);
    app.open_chime_test();
    run_two(&mut app, &silence(1024), &silence(1024));
    let at_opening = app.last_call();
    // A long time on the screen, then back. No call appears then.
    run_two(&mut app, &silence(5 * SECOND), &silence(5 * SECOND));
    app.button(Button::Home);
    run_two(&mut app, &silence(SECOND), &silence(SECOND));
    assert_eq!(app.last_call(), at_opening);
    assert!(matches!(app.te_status(), TeStatus::Waiting), "no capture of silence on the way back");
}

#[test]
fn bats_only_stays_as_it_was_while_the_screen_is_open() {
    let calls = train(1.6, 1.0, 5);
    // Open: a call has just opened the gate, and its 1 s hold would run out on this screen.
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_auto_volume(AutoVolume::BatsOnly);
    run_two(&mut app, &calls, &calls);
    let open = app.auto_volume_gate();
    assert!(open > 0.5, "{open}");
    app.open_chime_test();
    run_two(&mut app, &silence(3 * SECOND), &silence(3 * SECOND));
    assert_eq!(app.auto_volume_gate(), open);

    // Closed: calls on this screen do not open it.
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.set_auto_volume(AutoVolume::BatsOnly);
    run_two(&mut app, &silence(3 * SECOND), &silence(3 * SECOND));
    assert_eq!(app.auto_volume_gate(), 0.0);
    app.open_chime_test();
    run_two(&mut app, &calls, &calls);
    assert_eq!(app.auto_volume_gate(), 0.0);
}

#[test]
fn the_clock_digits_are_all_the_same_width() {
    let width = |text: &str| screen::chime_clock_width(text);
    let digits = width("00:00:00");
    for digit in '1'..='9' {
        let text: std::string::String = [digit, digit, ':', digit, digit, ':', digit, digit].iter().collect();
        assert_eq!(width(&text), digits, "{text}");
    }
    assert!((100..=300).contains(&digits), "{digits} px");
    assert!((100..=300).contains(&width("--:--:--")));
}

#[test]
fn the_text_fits() {
    for (line, width, room) in chime_test_lines() {
        std::println!("{width} px of {room}: {line}");
        assert!(width <= room, "{width} px, against {room} px: {line}");
    }
}

#[test]
fn the_age_and_the_date_read_as_they_should() {
    use crate::screen::chime_text;
    assert_eq!(chime_text::age(0), "X8 clock time not known yet. Chime read 0 s ago.");
    assert_eq!(chime_text::age(252), "X8 clock time not known yet. Chime read 4 min ago.");
    assert_eq!(chime_text::compared(-12, 3), "X8 clock 12 s behind phone. Chime read 3 s ago.");
    assert_eq!(chime_text::compared(12, 59), "X8 clock 12 s ahead of phone. Chime read 59 s ago.");
    assert_eq!(chime_text::compared(0, 4320), "X8 clock same as phone. Chime read 1 h ago.");
    assert_eq!(chime_text::compared(-3 * 86_400, 3), "X8 clock 3 days behind phone. Chime read 3 s ago.");
    assert_eq!(chime_text::date(UNIX as i64 + 330 * 60, 330), "2026-10-26  UTC+05:30");
    assert_eq!(chime_text::date(UNIX as i64 - 210 * 60, -210), "2026-10-26  UTC-03:30");
}

/// A chime read on the phone time screen goes to the host once, with the phone's local time now and
/// no clock set; Set X8 clock then asks the host to set its clock from it.
#[test]
fn a_chime_gives_the_host_the_phone_time_and_the_button_asks_for_a_set() {
    let chime = chime_at(UNIX, 660);
    let first = chime_in_silence(SECOND / 2, &chime, SECOND);
    let mut storage = vec![0.0f32; 1024];
    let mut app = BatApp::new(FS, &mut storage, buffers());
    app.open_chime_test();
    run_two(&mut app, &first, &first);
    let time = app.take_phone_time().expect("the chime's time");
    assert!(!time.set_clock);
    assert_eq!(time.timezone_minutes, 660);
    assert_eq!(time.location, None);
    // The time now: the chime's time plus what has been heard since it started.
    let since_start_ms = ((app.samples - SECOND as u64 / 2) * 1000 / SECOND as u64) as i64;
    let expected = (UNIX as i64 + 660 * 60) * 1000 + since_start_ms;
    assert!((time.local_ms - expected).abs() <= 1, "{} {expected}", time.local_ms);
    // Both inputs heard the same chime: it goes to the host once.
    assert_eq!(app.take_phone_time(), None);

    let set = layout::CLOCK_SET.center();
    assert!(app.is_tappable(set.x, set.y));
    app.touch(set.x, set.y);
    let time = app.take_phone_time().expect("the set request");
    assert!(time.set_clock);
    // The host's clock, once known, is compared with the phone's.
    app.set_host_clock(Some(time.local_ms.div_euclid(1000) - 12));
    let (phone, host) = app.clocks_now().unwrap();
    assert_eq!(host - phone, -12);
    app.set_clock_state(ClockState::AfterTake);
    assert!(draw(&app).iter().any(|&p| p != 0));
}
